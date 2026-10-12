// src/backend.rs

use crate::config::*;
use crate::curve::{parse_and_normalize_curve_data, CurveData, DualCurveData};

use hidapi::{HidApi, HidDevice};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Commandes disponibles pour le CT220S
#[derive(Debug, Clone, Copy)]
pub enum Command {
    SetFreq(u8), // FC
    SetRes(u8),  // FB
    #[allow(dead_code)]
    SetMode(u8), // FA - conservé pour le protocole USB
    SetVolt(u8), // FD
}

/// Backend HID pour envoyer des commandes
pub struct HidBackend {
    device: Arc<Mutex<HidDevice>>,
}

/// Construit exactement le même rapport HID que celui envoyé au vrai boîtier.
pub fn encode_command_report(cmd: Command) -> [u8; READ_SIZE] {
    let (prefix, index) = match cmd {
        Command::SetFreq(i) => (0xFCu8, i),
        Command::SetRes(i) => (0xFBu8, i),
        Command::SetMode(i) => (0xFAu8, i),
        Command::SetVolt(i) => (0xFDu8, i),
    };

    let mut buf = [0u8; READ_SIZE];
    buf[0] = 0; // Report ID HID
    buf[1] = prefix;
    buf[2] = index;
    buf
}

impl HidBackend {
    /// Créer un nouveau backend HID
    pub fn new() -> Result<Self, String> {
        let api = HidApi::new().map_err(|e| format!("Erreur HidApi: {}", e))?;
        
        println!("Recherche du CT220S...");
        let device = api
            .open(VID, PID)
            .map_err(|e| format!("Impossible d'ouvrir le périphérique: {}", e))?;
        
        println!("Périphérique ouvert pour les commandes.");
        
        Ok(Self {
            device: Arc::new(Mutex::new(device)),
        })
    }

    /// Envoyer une commande au boîtier
    pub fn send_cmd(&self, cmd: Command) -> Result<(), String> {
        let buf = encode_command_report(cmd);
        let device = self.device.lock().unwrap();
        device.write(&buf).map_err(|e| e.to_string())?;

        println!(
            "Cmd HID envoyée: prefix=0x{:02X}, index={}",
            buf[1], buf[2]
        );
        Ok(())
    }

    /// Clone le device pour le reader thread
    pub fn clone_device(&self) -> Arc<Mutex<HidDevice>> {
        Arc::clone(&self.device)
    }
}

/// Lecture HID en continu (mode réel)
pub fn run_hid_reader(
    device: Arc<Mutex<HidDevice>>,
    curve_data: Arc<Mutex<DualCurveData>>,
    error_message: Arc<Mutex<Option<String>>>,
    running: Arc<Mutex<bool>>,
) -> Result<(), String> {
    *error_message.lock().unwrap() = Some("Lecture en cours...".to_string());

    while *running.lock().unwrap() {
        let curve = {
            let dev = device.lock().unwrap();
            read_one_curve(&*dev)
        };

        match curve {
            Ok(curve) => {
                let mut data = curve_data.lock().unwrap();
                if curve.channel == 0 {
                    data.channel0 = Some(curve);
                } else {
                    data.channel1 = Some(curve);
                }
                *error_message.lock().unwrap() = None;
            }
            Err(e) => {
                eprintln!("Erreur de lecture: {}", e);
                *error_message.lock().unwrap() = Some(format!("Erreur: {}", e));
            }
        }
        thread::sleep(Duration::from_millis(10));
    }

    Ok(())
}

/// Lecture depuis un fichier de capture (mode simulation)
pub fn run_file_reader(
    file_path: &str,
    curve_data: Arc<Mutex<DualCurveData>>,
    error_message: Arc<Mutex<Option<String>>>,
    running: Arc<Mutex<bool>>,
) -> Result<(), String> {
    let file = File::open(file_path)
        .map_err(|e| format!("Impossible d'ouvrir {}: {}", file_path, e))?;
    let reader = BufReader::new(file);

    let mut reports: Vec<Vec<u8>> = Vec::new();

    for line in reader.lines() {
        let line = line.map_err(|e| format!("Erreur lecture ligne: {}", e))?;
        let line = line.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let bytes = parse_hex_line(line)?;
        if !bytes.is_empty() {
            reports.push(bytes);
        }
    }

    if reports.is_empty() {
        return Err("Aucune donnée trouvée dans le fichier".to_string());
    }

    println!("Chargé {} rapports du fichier", reports.len());
    *error_message.lock().unwrap() = Some(format!("Fichier chargé: {} rapports", reports.len()));

    let mut report_idx = 0;
    while *running.lock().unwrap() {
        match read_one_curve_from_reports(&reports, &mut report_idx) {
            Ok(curve) => {
                let mut data = curve_data.lock().unwrap();
                if curve.channel == 0 {
                    data.channel0 = Some(curve);
                } else {
                    data.channel1 = Some(curve);
                }
                *error_message.lock().unwrap() = None;
            }
            Err(e) => {
                eprintln!("Erreur lecture courbe: {}", e);
                *error_message.lock().unwrap() = Some(format!("Erreur: {}", e));
                report_idx = 0;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }

    Ok(())
}


/// Envoie au simulateur TCP exactement le même rapport 65 octets que
/// celui utilisé par HID pour le vrai boîtier (Report ID + FA/FB/FC/FD).
pub fn send_tcp_cmd(
    command_stream: &Arc<Mutex<Option<TcpStream>>>,
    cmd: Command,
) -> Result<(), String> {
    let report = encode_command_report(cmd);
    let mut guard = command_stream.lock().unwrap();
    let stream = guard
        .as_mut()
        .ok_or_else(|| "Simulateur TCP non connecté".to_string())?;
    stream.write_all(&report).map_err(|e| e.to_string())?;
    println!(
        "Cmd TCP envoyée (protocole HID inchangé): prefix=0x{:02X}, index={}",
        report[1], report[2]
    );
    Ok(())
}

/// Lecture de courbes via TCP (simulateur externe).
///
/// Le protocole est volontairement simple et reprend les rapports HID bruts :
/// - 1 rapport de 64 octets commençant par F0 FF, octet 2 = canal
/// - 32 rapports de 64 octets contenant 512 couples (I,V) u16 little-endian
///
/// Le viewer écoute sur `address` (par défaut 127.0.0.1:3000 avec --tcp).
pub fn run_tcp_reader(
    address: &str,
    curve_data: Arc<Mutex<DualCurveData>>,
    error_message: Arc<Mutex<Option<String>>>,
    running: Arc<Mutex<bool>>,
    command_stream: Arc<Mutex<Option<TcpStream>>>,
) -> Result<(), String> {
    let listener = TcpListener::bind(address)
        .map_err(|e| format!("Impossible d'écouter sur {}: {}", address, e))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("Impossible de passer le socket en non-bloquant: {}", e))?;

    println!("Mode simulateur TCP: écoute sur {}", address);
    *error_message.lock().unwrap() = Some(format!("Simulation TCP: attente sur {}", address));

    while *running.lock().unwrap() {
        match listener.accept() {
            Ok((mut stream, peer)) => {
                println!("Simulateur connecté depuis {}", peer);
                *error_message.lock().unwrap() =
                    Some(format!("Simulateur connecté: {}", peer));

                let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                let _ = stream.set_nodelay(true);

                match stream.try_clone() {
                    Ok(writer) => {
                        let _ = writer.set_nodelay(true);
                        *command_stream.lock().unwrap() = Some(writer);
                    }
                    Err(e) => {
                        *error_message.lock().unwrap() = Some(format!("Erreur clone TCP: {}", e));
                        continue;
                    }
                }

                while *running.lock().unwrap() {
                    match read_one_curve_tcp(&mut stream) {
                        Ok(curve) => {
                            let mut data = curve_data.lock().unwrap();
                            if curve.channel == 0 {
                                data.channel0 = Some(curve);
                            } else {
                                data.channel1 = Some(curve);
                            }
                            *error_message.lock().unwrap() = None;
                        }
                        Err(e) => {
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                || e.kind() == std::io::ErrorKind::TimedOut
                            {
                                continue;
                            }

                            println!("Simulateur TCP déconnecté: {}", e);
                            *command_stream.lock().unwrap() = None;
                            *error_message.lock().unwrap() =
                                Some(format!("Simulation TCP: attente sur {}", address));
                            break;
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return Err(format!("Erreur accept TCP: {}", e));
            }
        }
    }

    *command_stream.lock().unwrap() = None;
    Ok(())
}

fn read_one_curve_tcp(stream: &mut TcpStream) -> std::io::Result<CurveData> {
    // Le flux est découpé en rapports fixes de 64 octets. On resynchronise
    // simplement jusqu'à retrouver F0 FF.
    let channel_id = loop {
        let mut header = [0u8; REPORT_DATA_SIZE];
        stream.read_exact(&mut header)?;
        if header[0] == HEADER_MAGIC[0] && header[1] == HEADER_MAGIC[1] {
            break header[2];
        }
    };

    let mut data_bytes = vec![0u8; REPORTS_PER_CURVE * REPORT_DATA_SIZE];
    stream.read_exact(&mut data_bytes)?;

    let (v_norm, i_norm) = parse_and_normalize_curve_data(&data_bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    Ok(CurveData {
        voltage: v_norm,
        current: i_norm,
        channel: channel_id,
    })
}

/// Parsing d'une ligne hex (capture fichier)
fn parse_hex_line(line: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();

    let clean: String = line.chars().filter(|c| c.is_ascii_hexdigit()).collect();

    for i in (0..clean.len()).step_by(2) {
        if i + 1 < clean.len() {
            let byte_str = &clean[i..i + 2];
            let byte = u8::from_str_radix(byte_str, 16)
                .map_err(|e| format!("Erreur parsing hex '{}': {}", byte_str, e))?;
            bytes.push(byte);
        }
    }

    Ok(bytes)
}

fn extract_payload(report: &[u8]) -> Option<Vec<u8>> {
    if report.is_empty() {
        return None;
    }

    if report.len() == READ_SIZE {
        Some(report[1..].to_vec())
    } else if report.len() == REPORT_DATA_SIZE {
        Some(report.to_vec())
    } else {
        None
    }
}

fn read_one_curve_from_reports(
    reports: &[Vec<u8>],
    start_idx: &mut usize,
) -> Result<CurveData, String> {
    let channel_id = loop {
        if *start_idx >= reports.len() {
            return Err("Pas de header trouvé".to_string());
        }

        if let Some(payload) = extract_payload(&reports[*start_idx]) {
            if payload.len() >= 3
                && payload[0] == HEADER_MAGIC[0]
                && payload[1] == HEADER_MAGIC[1]
            {
                *start_idx += 1;
                break payload[2];
            }
        }

        *start_idx += 1;
    };

    let mut data_bytes = Vec::with_capacity(REPORTS_PER_CURVE * REPORT_DATA_SIZE);
    for i in 0..REPORTS_PER_CURVE {
        if let Some(payload) = extract_payload(&reports[*start_idx + i]) {
            data_bytes.extend_from_slice(&payload);
        } else {
            return Err("Payload invalide".to_string());
        }
    }
    *start_idx += REPORTS_PER_CURVE;

    let (v_norm, i_norm) = parse_and_normalize_curve_data(&data_bytes)?;

    Ok(CurveData {
        voltage: v_norm,
        current: i_norm,
        channel: channel_id,
    })
}

fn read_one_curve(device: &HidDevice) -> Result<CurveData, String> {
    // Attendre le header et récupérer directement le canal.
    let channel_id = loop {
        let mut buf = [0u8; READ_SIZE];
        let n = device
            .read(&mut buf)
            .map_err(|e| format!("Erreur de lecture: {}", e))?;

        if let Some(payload) = extract_payload(&buf[..n]) {
            if payload.len() >= 3
                && payload[0] == HEADER_MAGIC[0]
                && payload[1] == HEADER_MAGIC[1]
            {
                break payload[2];
            }
        }
    };

    // Lire les données
    let mut data_bytes = Vec::with_capacity(REPORTS_PER_CURVE * REPORT_DATA_SIZE);
    for _ in 0..REPORTS_PER_CURVE {
        let mut buf = [0u8; READ_SIZE];
        let n = device
            .read(&mut buf)
            .map_err(|e| format!("Erreur de lecture: {}", e))?;

        if let Some(payload) = extract_payload(&buf[..n]) {
            data_bytes.extend_from_slice(&payload);
        } else {
            return Err("Payload invalide".to_string());
        }
    }

    let (v_norm, i_norm) = parse_and_normalize_curve_data(&data_bytes)?;

    Ok(CurveData {
        voltage: v_norm,
        current: i_norm,
        channel: channel_id,
    })
}
