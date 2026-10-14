// src/backend.rs
//
// Transports : USB HID (boîtier réel), fichier de capture et simulateur TCP.
// Les trois utilisent le même assembleur de trames que le logiciel constructeur.

use crate::config::*;
pub use crate::ct220s_vendor::Command;
use crate::ct220s_vendor::{AssemblerEvent, FrameAssembler};
use crate::curve::{CurveData, DualCurveData};

use hidapi::{HidApi, HidDevice};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub type SharedCurves = Arc<Mutex<DualCurveData>>;
pub type SharedStatus = Arc<Mutex<Option<String>>>;

/// Construit le rapport HID de 65 octets envoyé au boîtier (et au simulateur).
pub fn encode_command_report(cmd: Command) -> [u8; READ_SIZE] {
    cmd.encode()
}

fn set_status(status: &SharedStatus, msg: Option<String>) {
    if let Ok(mut s) = status.lock() {
        *s = msg;
    }
}

/// Applique un évènement de l'assembleur à l'état partagé.
fn apply_event(event: AssemblerEvent, curves: &SharedCurves) -> bool {
    match event {
        AssemblerEvent::Config(cfg) => {
            if let Ok(mut c) = curves.lock() {
                c.store_config(cfg);
            }
            false
        }
        AssemblerEvent::Frame(frame) => {
            match CurveData::from_raw(frame.channel, frame.voltage, frame.current) {
                Ok(curve) => {
                    if let Ok(mut c) = curves.lock() {
                        c.store_curve(curve);
                    }
                    true
                }
                Err(_) => false,
            }
        }
        AssemblerEvent::Pending | AssemblerEvent::Ignored => false,
    }
}

// ---------------------------------------------------------------------------
// USB HID
// ---------------------------------------------------------------------------

/// Accès HID partagé. Le périphérique peut être absent : le thread de lecture
/// tente alors de le rouvrir toutes les secondes (reconnexion à chaud, comme le
/// timer du logiciel constructeur).
#[derive(Clone)]
pub struct HidBackend {
    device: Arc<Mutex<Option<HidDevice>>>,
}

impl HidBackend {
    pub fn new() -> Self {
        Self { device: Arc::new(Mutex::new(None)) }
    }

    pub fn is_connected(&self) -> bool {
        self.device.lock().map(|d| d.is_some()).unwrap_or(false)
    }

    pub fn send_cmd(&self, cmd: Command) -> Result<(), String> {
        let buf = encode_command_report(cmd);
        let mut guard = self.device.lock().map_err(|_| "verrou HID empoisonné".to_string())?;
        let device = guard.as_ref().ok_or_else(|| "Boîtier USB non connecté".to_string())?;
        if let Err(e) = device.write(&buf) {
            *guard = None;
            return Err(e.to_string());
        }
        println!("Cmd HID envoyée: {:02X} {:02X}", buf[1], buf[2]);
        Ok(())
    }

    pub fn device_handle(&self) -> Arc<Mutex<Option<HidDevice>>> {
        Arc::clone(&self.device)
    }
}

/// Lecture HID en continu. Le verrou n'est tenu que pendant une lecture
/// courte (timeout 50 ms) afin que les commandes puissent toujours passer.
pub fn run_hid_reader(
    device: Arc<Mutex<Option<HidDevice>>>,
    curves: SharedCurves,
    status: SharedStatus,
    running: Arc<AtomicBool>,
) -> Result<(), String> {
    let api = HidApi::new().map_err(|e| format!("Erreur HidApi: {}", e))?;
    let mut assembler = FrameAssembler::new();
    let mut last_attempt: Option<Instant> = None;

    while running.load(Ordering::Relaxed) {
        let connected = device.lock().map(|d| d.is_some()).unwrap_or(false);
        if !connected {
            if last_attempt.map_or(true, |t| t.elapsed() >= Duration::from_secs(1)) {
                last_attempt = Some(Instant::now());
                match api.open(VID, PID) {
                    Ok(dev) => {
                        println!("CT220S ouvert.");
                        assembler.reset();
                        if let Ok(mut d) = device.lock() {
                            *d = Some(dev);
                        }
                        set_status(&status, Some("Périphérique USB connecté".into()));
                    }
                    Err(e) => {
                        set_status(&status, Some(format!("Boîtier USB absent ({})", e)));
                    }
                }
            }
            thread::sleep(Duration::from_millis(100));
            continue;
        }

        let mut buf = [0u8; READ_SIZE];
        let result = {
            let guard = match device.lock() {
                Ok(g) => g,
                Err(_) => break,
            };
            match guard.as_ref() {
                Some(dev) => dev.read_timeout(&mut buf, 50),
                None => continue,
            }
        };

        match result {
            Ok(0) => {}
            Ok(n) => {
                if let Ok(ev) = assembler.push_report(&buf[..n]) {
                    if apply_event(ev, &curves) {
                        set_status(&status, None);
                    }
                }
            }
            Err(e) => {
                eprintln!("Erreur de lecture HID: {}", e);
                if let Ok(mut d) = device.lock() {
                    *d = None;
                }
                if let Ok(mut c) = curves.lock() {
                    c.clear();
                    c.config = None;
                }
                set_status(&status, Some(format!("USB déconnecté : {}", e)));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Fichier de capture
// ---------------------------------------------------------------------------

pub fn load_capture_reports(file_path: &str) -> Result<Vec<Vec<u8>>, String> {
    let file = File::open(file_path).map_err(|e| format!("Impossible d'ouvrir {}: {}", file_path, e))?;
    let mut reports = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| format!("Erreur lecture ligne: {}", e))?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bytes = parse_hex_line(line)?;
        if bytes.len() == READ_SIZE || bytes.len() == REPORT_DATA_SIZE {
            reports.push(bytes);
        }
    }
    if reports.is_empty() {
        return Err("Aucune donnée trouvée dans le fichier".to_string());
    }
    Ok(reports)
}

/// Rejoue une capture en boucle (une courbe toutes les 50 ms).
pub fn run_file_reader(
    file_path: &str,
    curves: SharedCurves,
    status: SharedStatus,
    running: Arc<AtomicBool>,
) -> Result<(), String> {
    let reports = match load_capture_reports(file_path) {
        Ok(r) => r,
        Err(e) => {
            set_status(&status, Some(e.clone()));
            return Err(e);
        }
    };
    println!("Chargé {} rapports du fichier", reports.len());
    set_status(&status, Some(format!("Fichier chargé: {} rapports", reports.len())));

    let mut assembler = FrameAssembler::new();
    let mut frames_this_pass = 0usize;
    let mut idx = 0usize;
    while running.load(Ordering::Relaxed) {
        if idx >= reports.len() {
            if frames_this_pass == 0 {
                let msg = "Capture sans courbe complète (en-tête F0 FF + rapports)".to_string();
                set_status(&status, Some(msg.clone()));
                return Err(msg);
            }
            idx = 0;
            frames_this_pass = 0;
        }
        let ev = assembler.push_report(&reports[idx]);
        idx += 1;
        if let Ok(ev) = ev {
            if apply_event(ev, &curves) {
                frames_this_pass += 1;
                set_status(&status, None);
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Simulateur TCP
// ---------------------------------------------------------------------------

/// Envoie au simulateur TCP exactement le même rapport 65 octets qu'en HID.
pub fn send_tcp_cmd(command_stream: &Arc<Mutex<Option<TcpStream>>>, cmd: Command) -> Result<(), String> {
    let report = encode_command_report(cmd);
    let mut guard = command_stream.lock().map_err(|_| "verrou TCP empoisonné".to_string())?;
    let stream = guard.as_mut().ok_or_else(|| "Simulateur TCP non connecté".to_string())?;
    if let Err(e) = stream.write_all(&report) {
        *guard = None;
        return Err(e.to_string());
    }
    println!("Cmd TCP envoyée: {:02X} {:02X}", report[1], report[2]);
    Ok(())
}

/// Découpe un flux d'octets en rapports de 64 octets. Tant que l'alignement
/// n'est pas connu, on recherche `F0 FF` à n'importe quelle position : cette
/// séquence ne peut pas apparaître dans les données (valeurs ADC <= 0x0FFF).
#[derive(Default)]
pub struct StreamFramer {
    buffer: Vec<u8>,
    synced: bool,
}

impl StreamFramer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    pub fn next_report(&mut self) -> Option<[u8; REPORT_DATA_SIZE]> {
        if !self.synced {
            let pos = self.buffer.windows(2).position(|w| w == HEADER_MAGIC)?;
            self.buffer.drain(..pos);
            self.synced = true;
        }
        if self.buffer.len() < REPORT_DATA_SIZE {
            return None;
        }
        let mut report = [0u8; REPORT_DATA_SIZE];
        report.copy_from_slice(&self.buffer[..REPORT_DATA_SIZE]);
        self.buffer.drain(..REPORT_DATA_SIZE);
        Some(report)
    }

    pub fn resync(&mut self) {
        self.synced = false;
    }
}

/// Le viewer écoute sur `address` (127.0.0.1:3000 par défaut avec --tcp).
pub fn run_tcp_reader(
    address: &str,
    curves: SharedCurves,
    status: SharedStatus,
    running: Arc<AtomicBool>,
    command_stream: Arc<Mutex<Option<TcpStream>>>,
) -> Result<(), String> {
    let listener = TcpListener::bind(address).map_err(|e| format!("Impossible d'écouter sur {}: {}", address, e))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("Impossible de passer le socket en non-bloquant: {}", e))?;

    println!("Mode simulateur TCP: écoute sur {}", address);
    set_status(&status, Some(format!("Simulation TCP: attente sur {}", address)));

    while running.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut stream, peer)) => {
                println!("Simulateur connecté depuis {}", peer);
                set_status(&status, Some(format!("Simulateur connecté: {}", peer)));

                // Sous Windows, le socket accepté hérite du mode non bloquant de
                // l'écouteur : read/write renverraient WouldBlock au milieu
                // d'une trame. On repasse explicitement en mode bloquant.
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                let _ = stream.set_nodelay(true);

                match stream.try_clone() {
                    Ok(writer) => {
                        let _ = writer.set_nodelay(true);
                        if let Ok(mut c) = command_stream.lock() {
                            *c = Some(writer);
                        }
                    }
                    Err(e) => {
                        set_status(&status, Some(format!("Erreur clone TCP: {}", e)));
                        continue;
                    }
                }

                let mut framer = StreamFramer::new();
                let mut assembler = FrameAssembler::new();
                let mut tmp = [0u8; 4096];
                while running.load(Ordering::Relaxed) {
                    match stream.read(&mut tmp) {
                        Ok(0) => {
                            println!("Simulateur TCP déconnecté");
                            break;
                        }
                        Ok(n) => {
                            framer.push(&tmp[..n]);
                            while let Some(report) = framer.next_report() {
                                match assembler.push_payload(&report) {
                                    Ok(AssemblerEvent::Ignored) => framer.resync(),
                                    Ok(ev) => {
                                        if apply_event(ev, &curves) {
                                            set_status(&status, None);
                                        }
                                    }
                                    Err(_) => framer.resync(),
                                }
                            }
                        }
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                || e.kind() == std::io::ErrorKind::TimedOut
                                || e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(e) => {
                            println!("Simulateur TCP déconnecté: {}", e);
                            break;
                        }
                    }
                }
                if let Ok(mut c) = command_stream.lock() {
                    *c = None;
                }
                set_status(&status, Some(format!("Simulation TCP: attente sur {}", address)));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("Erreur accept TCP: {}", e)),
        }
    }

    if let Ok(mut c) = command_stream.lock() {
        *c = None;
    }
    Ok(())
}

/// Parsing d'une ligne hex (capture fichier).
fn parse_hex_line(line: &str) -> Result<Vec<u8>, String> {
    let clean: String = line.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let mut bytes = Vec::with_capacity(clean.len() / 2);
    for i in (0..clean.len()).step_by(2) {
        if i + 1 < clean.len() {
            let s = &clean[i..i + 2];
            bytes.push(u8::from_str_radix(s, 16).map_err(|e| format!("Erreur parsing hex '{}': {}", s, e))?);
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod protocol_tests {
    use super::*;

    #[test]
    fn existing_hid_tcp_command_wire_format() {
        for (cmd, opcode, value) in [
            (Command::SetMode(1), 0xFF, 1),
            (Command::SetRes(2), 0xFB, 2),
            (Command::SetFreq(3), 0xFC, 3),
            (Command::SetVolt(4), 0xFD, 4),
        ] {
            let report = encode_command_report(cmd);
            assert_eq!(report.len(), READ_SIZE);
            assert_eq!(&report[..3], &[0, opcode, value]);
            assert!(report[3..].iter().all(|&b| b == 0));
        }
    }

    fn frame_bytes(channel: u8) -> Vec<u8> {
        let mut v = vec![0u8; REPORT_DATA_SIZE];
        v[..3].copy_from_slice(&[0xF0, 0xFF, channel]);
        for k in 0..512u16 {
            v.extend_from_slice(&(2048 + (k % 7)).to_le_bytes());
            v.extend_from_slice(&(1000 + k).to_le_bytes());
        }
        v
    }

    #[test]
    fn framer_resyncs_after_garbage_and_split_reads() {
        let mut stream = vec![0x12, 0x34, 0x56];
        stream.extend(frame_bytes(1));
        stream.extend(frame_bytes(0));
        let mut framer = StreamFramer::new();
        let mut asm = FrameAssembler::new();
        let mut frames = Vec::new();
        for chunk in stream.chunks(37) {
            framer.push(chunk);
            while let Some(r) = framer.next_report() {
                if let Ok(AssemblerEvent::Frame(f)) = asm.push_payload(&r) {
                    frames.push(f);
                }
            }
        }
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].channel, 1);
        assert_eq!(frames[1].channel, 0);
        assert_eq!(frames[0].voltage[10], 1010);
        assert_eq!(frames[0].current[10], 2048 + 3);
    }

    #[test]
    fn hex_line_parsing() {
        assert_eq!(parse_hex_line("F0 ff-0a").unwrap(), vec![0xF0, 0xFF, 0x0A]);
    }
}
