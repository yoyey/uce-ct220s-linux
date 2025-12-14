// src/backend.rs

use crate::config::*;
use crate::curve::{parse_and_normalize_curve_data, CurveData, DualCurveData};

use hidapi::{HidApi, HidDevice};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Commandes disponibles pour le CT220S
#[derive(Debug, Clone, Copy)]
pub enum Command {
    SetFreq(u8), // FC
    SetRes(u8),  // FB
    SetMode(u8), // FA
    SetVolt(u8), // FD
}

/// Backend HID pour envoyer des commandes
pub struct HidBackend {
    device: Arc<Mutex<HidDevice>>,
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
        let (prefix, index) = match cmd {
            Command::SetFreq(i) => (0xFCu8, i),
            Command::SetRes(i) => (0xFBu8, i),
            Command::SetMode(i) => (0xFAu8, i),
            Command::SetVolt(i) => (0xFDu8, i),
        };

        let mut buf = [0u8; READ_SIZE];
        buf[1] = prefix;
        buf[2] = index;

        let device = self.device.lock().unwrap();
        device.write(&buf).map_err(|e| e.to_string())?;
        
        println!(
            "Cmd HID envoyée: prefix=0x{:02X}, index={}",
            prefix, index
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
    let mut channel_id = 1u8;
    let mut header_found = false;

    while *start_idx < reports.len() {
        if let Some(payload) = extract_payload(&reports[*start_idx]) {
            if payload.len() >= 3
                && payload[0] == HEADER_MAGIC[0]
                && payload[1] == HEADER_MAGIC[1]
            {
                channel_id = payload[2];
                header_found = true;
                *start_idx += 1;
                break;
            }
        }
        *start_idx += 1;
    }

    if !header_found {
        return Err("Pas de header trouvé".to_string());
    }

    if *start_idx + REPORTS_PER_CURVE > reports.len() {
        return Err("Pas assez de rapports restants".to_string());
    }

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
    let mut channel_id = 1u8;

    // Attendre le header
    loop {
        let mut buf = [0u8; READ_SIZE];
        let n = device
            .read(&mut buf)
            .map_err(|e| format!("Erreur de lecture: {}", e))?;

        if let Some(payload) = extract_payload(&buf[..n]) {
            if payload.len() >= 3
                && payload[0] == HEADER_MAGIC[0]
                && payload[1] == HEADER_MAGIC[1]
            {
                channel_id = payload[2];
                break;
            }
        }
    }

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
