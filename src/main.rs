// Cargo.toml dependencies needed:
// [dependencies]
// eframe = "0.24"
// egui = "0.24"
// hidapi = "2.4"
// byteorder = "1.5"
// clap = { version = "4.4", features = ["derive"] }
// image = "0.24"

use eframe::egui;
use hidapi::{HidApi, HidDevice};
use byteorder::{ByteOrder, LittleEndian};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use image::{ImageBuffer, Rgba};
use clap::Parser;

// Paramètres
const VID: u16 = 0x0483;
const PID: u16 = 0x5750;
const REPORT_DATA_SIZE: usize = 64;
const READ_SIZE: usize = 65;
const POINTS_PER_CURVE: usize = 512;
const REPORTS_PER_CURVE: usize = 32;
const HEADER_MAGIC: [u8; 2] = [0xf0, 0xff];

/// Visualiseur de courbes V-I pour CT220S
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Chemin vers un fichier de capture hexadécimal
    #[arg(short, long)]
    file: Option<String>,
}

#[derive(Clone)]
struct CurveData {
    voltage: Vec<f32>,
    current: Vec<f32>,
    channel: u8,
}

struct DualCurveData {
    channel0: Option<CurveData>,
    channel1: Option<CurveData>,
}

struct CT220SApp {
    curve_data: Arc<Mutex<DualCurveData>>,
    error_message: Arc<Mutex<Option<String>>>,
    running: Arc<Mutex<bool>>,
    use_file_mode: bool,
    file_path: String,
    dual_mode: bool,
}

impl CT220SApp {
    fn new(_cc: &eframe::CreationContext<'_>, file_arg: Option<String>) -> Self {
        let curve_data = Arc::new(Mutex::new(DualCurveData {
            channel0: None,
            channel1: None,
        }));
        let error_message = Arc::new(Mutex::new(None));
        let running = Arc::new(Mutex::new(true));

        let (use_file_mode, file_path) = if let Some(path) = file_arg {
            (true, path)
        } else {
            let default_path = "capture.txt".to_string();
            let exists = Path::new(&default_path).exists();
            (exists, default_path)
        };

        let dual_mode = use_file_mode;

        let curve_data_clone = Arc::clone(&curve_data);
        let error_clone = Arc::clone(&error_message);
        let running_clone = Arc::clone(&running);
        let file_path_clone = file_path.clone();

        thread::spawn(move || {
            if use_file_mode {
                println!("Mode fichier: lecture de {}", file_path_clone);
                if let Err(e) = run_file_reader(
                    &file_path_clone,
                    curve_data_clone,
                    error_clone,
                    running_clone,
                ) {
                    eprintln!("Erreur lecture fichier: {}", e);
                }
            } else {
                println!("Mode périphérique USB");
                if let Err(e) = run_hid_reader(curve_data_clone, error_clone, running_clone) {
                    eprintln!("Erreur HID: {}", e);
                }
            }
        });

        Self {
            curve_data,
            error_message,
            running,
            use_file_mode,
            file_path,
            dual_mode,
        }
    }

    fn draw_single_channel(&self, ui: &mut egui::Ui, channel: u8, size: f32) {
        let desired_size = egui::vec2(size, size);
        let (response, painter) = ui.allocate_painter(desired_size, egui::Sense::hover());
        let rect = response.rect;

        painter.rect_filled(rect, 0.0, egui::Color32::WHITE);

        let center = rect.center();
        let scale = rect.width().min(rect.height()) * 0.45;

        // Grille
        let grid_color = egui::Color32::from_gray(200);
        for i in -10..=10 {
            let offset = (i as f32) * scale / 10.0;
            painter.line_segment(
                [
                    egui::pos2(center.x + offset, rect.top()),
                    egui::pos2(center.x + offset, rect.bottom()),
                ],
                egui::Stroke::new(0.5, grid_color),
            );
            painter.line_segment(
                [
                    egui::pos2(rect.left(), center.y + offset),
                    egui::pos2(rect.right(), center.y + offset),
                ],
                egui::Stroke::new(0.5, grid_color),
            );
        }

        // Axes
        let axis_color = egui::Color32::BLACK;
        painter.line_segment(
            [egui::pos2(rect.left(), center.y), egui::pos2(rect.right(), center.y)],
            egui::Stroke::new(1.0, axis_color),
        );
        painter.line_segment(
            [egui::pos2(center.x, rect.top()), egui::pos2(center.x, rect.bottom())],
            egui::Stroke::new(1.0, axis_color),
        );

        // Dessiner la courbe
        if let Ok(data) = self.curve_data.lock() {
            let curve_opt = if channel == 0 {
                &data.channel0
            } else {
                &data.channel1
            };
            
            if let Some(curve) = curve_opt {
                let points: Vec<egui::Pos2> = curve
                    .voltage
                    .iter()
                    .zip(curve.current.iter())
                    .map(|(&v, &i)| {
                        egui::pos2(
                            center.x + v * scale,
                            center.y - i * scale,
                        )
                    })
                    .collect();

                if points.len() > 1 {
                    let color = if channel == 0 {
                        egui::Color32::from_rgb(255, 100, 0)
                    } else {
                        egui::Color32::BLUE
                    };
                    
                    painter.add(egui::Shape::line(
                        points,
                        egui::Stroke::new(1.5, color),
                    ));
                }
            }
        }

        // Labels
        let channel_name = if channel == 0 { "CH0" } else { "CH1" };
        painter.text(
            egui::pos2(rect.left() + 30.0, rect.top() + 15.0),
            egui::Align2::LEFT_TOP,
            channel_name,
            egui::FontId::proportional(16.0),
            egui::Color32::BLACK,
        );
        painter.text(
            egui::pos2(rect.right() - 60.0, center.y - 15.0),
            egui::Align2::CENTER_CENTER,
            "Tension",
            egui::FontId::default(),
            egui::Color32::BLACK,
        );
        painter.text(
            egui::pos2(center.x + 15.0, rect.top() + 20.0),
            egui::Align2::CENTER_CENTER,
            "Courant",
            egui::FontId::default(),
            egui::Color32::BLACK,
        );
    }

    fn draw_dual_overlay(&self, ui: &mut egui::Ui, size: f32) {
        let desired_size = egui::vec2(size, size);
        let (response, painter) = ui.allocate_painter(desired_size, egui::Sense::hover());
        let rect = response.rect;

        painter.rect_filled(rect, 0.0, egui::Color32::WHITE);

        let center = rect.center();
        let scale = rect.width().min(rect.height()) * 0.45;

        // Grille
        let grid_color = egui::Color32::from_gray(200);
        for i in -10..=10 {
            let offset = (i as f32) * scale / 10.0;
            painter.line_segment(
                [
                    egui::pos2(center.x + offset, rect.top()),
                    egui::pos2(center.x + offset, rect.bottom()),
                ],
                egui::Stroke::new(0.5, grid_color),
            );
            painter.line_segment(
                [
                    egui::pos2(rect.left(), center.y + offset),
                    egui::pos2(rect.right(), center.y + offset),
                ],
                egui::Stroke::new(0.5, grid_color),
            );
        }

        // Axes
        let axis_color = egui::Color32::BLACK;
        painter.line_segment(
            [egui::pos2(rect.left(), center.y), egui::pos2(rect.right(), center.y)],
            egui::Stroke::new(1.0, axis_color),
        );
        painter.line_segment(
            [egui::pos2(center.x, rect.top()), egui::pos2(center.x, rect.bottom())],
            egui::Stroke::new(1.0, axis_color),
        );

        // Dessiner les deux courbes superposées
        if let Ok(data) = self.curve_data.lock() {
            // CH0 en orange
            if let Some(curve) = &data.channel0 {
                let points: Vec<egui::Pos2> = curve
                    .voltage
                    .iter()
                    .zip(curve.current.iter())
                    .map(|(&v, &i)| {
                        egui::pos2(
                            center.x + v * scale,
                            center.y - i * scale,
                        )
                    })
                    .collect();

                if points.len() > 1 {
                    painter.add(egui::Shape::line(
                        points,
                        egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 100, 0)),
                    ));
                }
            }

            // CH1 en bleu
            if let Some(curve) = &data.channel1 {
                let points: Vec<egui::Pos2> = curve
                    .voltage
                    .iter()
                    .zip(curve.current.iter())
                    .map(|(&v, &i)| {
                        egui::pos2(
                            center.x + v * scale,
                            center.y - i * scale,
                        )
                    })
                    .collect();

                if points.len() > 1 {
                    painter.add(egui::Shape::line(
                        points,
                        egui::Stroke::new(2.0, egui::Color32::BLUE),
                    ));
                }
            }
        }

        // Légende
        painter.text(
            egui::pos2(rect.left() + 30.0, rect.top() + 15.0),
            egui::Align2::LEFT_TOP,
            "CH0",
            egui::FontId::proportional(16.0),
            egui::Color32::from_rgb(255, 100, 0),
        );
        painter.text(
            egui::pos2(rect.left() + 30.0, rect.top() + 35.0),
            egui::Align2::LEFT_TOP,
            "CH1",
            egui::FontId::proportional(16.0),
            egui::Color32::BLUE,
        );
        
        painter.text(
            egui::pos2(rect.right() - 60.0, center.y - 15.0),
            egui::Align2::CENTER_CENTER,
            "Tension",
            egui::FontId::default(),
            egui::Color32::BLACK,
        );
        painter.text(
            egui::pos2(center.x + 15.0, rect.top() + 20.0),
            egui::Align2::CENTER_CENTER,
            "Courant",
            egui::FontId::default(),
            egui::Color32::BLACK,
        );
    }
}

impl eframe::App for CT220SApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("CT220S - Courbe V-I");
            
            if self.use_file_mode {
                ui.label(format!("📁 Mode fichier: {}", self.file_path));
            } else {
                ui.label("🔌 Mode périphérique USB");
            }
            
            ui.horizontal(|ui| {
                ui.label("Mode:");
                ui.radio_value(&mut self.dual_mode, false, "Single CH1");
                ui.radio_value(&mut self.dual_mode, true, "Dual Overlay");
            });
            
            if ui.button("💾 Sauvegarder PNG").clicked() {
                if let Ok(data) = self.curve_data.lock() {
                    let result = if self.dual_mode {
                        save_dual_curves_as_png(&data, "curves_export.png")
                    } else {
                        if let Some(ch1) = &data.channel1 {
                            save_curve_as_png(ch1, "curve_ch1_export.png")
                        } else {
                            Err("Pas de données CH1".to_string())
                        }
                    };
                    
                    match result {
                        Ok(_) => {
                            *self.error_message.lock().unwrap() = 
                                Some("✅ Sauvegardé".to_string());
                        }
                        Err(e) => {
                            *self.error_message.lock().unwrap() = 
                                Some(format!("❌ Erreur: {}", e));
                        }
                    }
                }
            }
            
            ui.separator();

            if let Ok(err) = self.error_message.lock() {
                if let Some(msg) = &*err {
                    ui.colored_label(egui::Color32::RED, msg);
                }
            }

            if self.dual_mode {
                self.draw_dual_overlay(ui, 600.0);
            } else {
                self.draw_single_channel(ui, 1, 600.0);
            }
        });

        ctx.request_repaint();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        *self.running.lock().unwrap() = false;
    }
}

fn run_hid_reader(
    curve_data: Arc<Mutex<DualCurveData>>,
    error_message: Arc<Mutex<Option<String>>>,
    running: Arc<Mutex<bool>>,
) -> Result<(), String> {
    let api = HidApi::new().map_err(|e| format!("Erreur HidApi: {}", e))?;

    println!("Recherche du CT220S...");
    let device = api
        .open(VID, PID)
        .map_err(|e| format!("Impossible d'ouvrir le périphérique: {}", e))?;

    println!("Périphérique ouvert.");
    *error_message.lock().unwrap() = Some("Périphérique connecté".to_string());

    while *running.lock().unwrap() {
        match read_one_curve(&device) {
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

fn run_file_reader(
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

fn parse_hex_line(line: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    
    let clean: String = line.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    
    for i in (0..clean.len()).step_by(2) {
        if i + 1 < clean.len() {
            let byte_str = &clean[i..i+2];
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
            if payload.len() >= 3 && payload[0] == HEADER_MAGIC[0] && payload[1] == HEADER_MAGIC[1] {
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
    loop {
        let mut buf = [0u8; READ_SIZE];
        let n = device
            .read(&mut buf)
            .map_err(|e| format!("Erreur de lecture: {}", e))?;

        if let Some(payload) = extract_payload(&buf[..n]) {
            if payload.len() >= 3 && payload[0] == HEADER_MAGIC[0] && payload[1] == HEADER_MAGIC[1] {
                channel_id = payload[2];
                break;
            }
        }
    }

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

fn parse_and_normalize_curve_data(data_bytes: &[u8]) -> Result<(Vec<f32>, Vec<f32>), String> {
    let raw = &data_bytes[..POINTS_PER_CURVE * 4.min(data_bytes.len())];
    let mut pairs = Vec::with_capacity(POINTS_PER_CURVE);

    for i in 0..(raw.len() / 4).min(POINTS_PER_CURVE) {
        let offset = i * 4;
        let current_raw = LittleEndian::read_u16(&raw[offset..offset + 2]) as f32;
        let voltage_raw = LittleEndian::read_u16(&raw[offset + 2..offset + 4]) as f32;
        pairs.push((current_raw, voltage_raw));
    }

    if pairs.is_empty() {
        return Err("Aucune paire de données extraite".to_string());
    }

    let mut currents: Vec<f32> = pairs.iter().map(|(c, _)| *c).collect();
    let mut voltages: Vec<f32> = pairs.iter().map(|(_, v)| *v).collect();

    currents.sort_by(|a, b| a.partial_cmp(b).unwrap());
    voltages.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let median_current = currents[currents.len() / 2];
    let median_voltage = voltages[voltages.len() / 2];

    let mut i_vec = Vec::with_capacity(pairs.len());
    let mut v_vec = Vec::with_capacity(pairs.len());

    for (c, v) in pairs {
        i_vec.push(c - median_current);
        v_vec.push(v - median_voltage);
    }

    let max_i = i_vec.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
    let max_v = v_vec.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
    let scale = max_i.max(max_v).max(1.0);

    let v_norm: Vec<f32> = v_vec.iter().map(|x| x / scale).collect();
    let i_norm: Vec<f32> = i_vec.iter().map(|x| x / scale).collect();

    Ok((v_norm, i_norm))
}

fn save_curve_as_png(curve: &CurveData, filename: &str) -> Result<(), String> {
    let width = 800;
    let height = 800;
    
    let mut img = ImageBuffer::from_fn(width, height, |_, _| {
        Rgba([255u8, 255u8, 255u8, 255u8])
    });
    
    let center_x = width as f32 / 2.0;
    let center_y = height as f32 / 2.0;
    let scale = (width.min(height) as f32) * 0.45;
    
    let grid_color = Rgba([200u8, 200u8, 200u8, 255u8]);
    for i in -10..=10 {
        let offset = (i as f32) * scale / 10.0;
        
        let x = (center_x + offset) as i32;
        if x >= 0 && x < width as i32 {
            for y in 0..height {
                if let Some(pixel) = img.get_pixel_mut_checked(x as u32, y) {
                    *pixel = grid_color;
                }
            }
        }
        
        let y = (center_y + offset) as i32;
        if y >= 0 && y < height as i32 {
            for x in 0..width {
                if let Some(pixel) = img.get_pixel_mut_checked(x, y as u32) {
                    *pixel = grid_color;
                }
            }
        }
    }
    
    let axis_color = Rgba([0u8, 0u8, 0u8, 255u8]);
    
    let cy = center_y as i32;
    for x in 0..width {
        for dy in -1..=1 {
            let y = cy + dy;
            if y >= 0 && y < height as i32 {
                if let Some(pixel) = img.get_pixel_mut_checked(x, y as u32) {
                    *pixel = axis_color;
                }
            }
        }
    }
    
    let cx = center_x as i32;
    for y in 0..height {
        for dx in -1..=1 {
            let x = cx + dx;
            if x >= 0 && x < width as i32 {
                if let Some(pixel) = img.get_pixel_mut_checked(x as u32, y) {
                    *pixel = axis_color;
                }
            }
        }
    }
    
    let curve_color = Rgba([0u8, 100u8, 255u8, 255u8]);
    
    for i in 0..curve.voltage.len() {
        let v = curve.voltage[i];
        let c = curve.current[i];
        
        let x = (center_x + v * scale) as i32;
        let y = (center_y - c * scale) as i32;
        
        for dy in -1..=1 {
            for dx in -1..=1 {
                let px = x + dx;
                let py = y + dy;
                
                if px >= 0 && px < width as i32 && py >= 0 && py < height as i32 {
                    if let Some(pixel) = img.get_pixel_mut_checked(px as u32, py as u32) {
                        *pixel = curve_color;
                    }
                }
            }
        }
    }
    
    img.save(filename)
        .map_err(|e| format!("Erreur sauvegarde PNG: {}", e))?;
    
    println!("Image sauvegardée : {}", filename);
    Ok(())
}

fn save_dual_curves_as_png(data: &DualCurveData, filename: &str) -> Result<(), String> {
    let width = 1600;
    let height = 800;
    
    let mut img = ImageBuffer::from_fn(width, height, |_, _| {
        Rgba([255u8, 255u8, 255u8, 255u8])
    });
    
    if let Some(ch0) = &data.channel0 {
        draw_curve_to_image(&mut img, ch0, 0, 0, 800, 800, Rgba([255u8, 100u8, 0u8, 255u8]));
    }
    
    if let Some(ch1) = &data.channel1 {
        draw_curve_to_image(&mut img, ch1, 800, 0, 800, 800, Rgba([0u8, 100u8, 255u8, 255u8]));
    }
    
    img.save(filename)
        .map_err(|e| format!("Erreur sauvegarde PNG: {}", e))?;
    
    println!("Image dual sauvegardée : {}", filename);
    Ok(())
}

fn draw_curve_to_image(
    img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>,
    curve: &CurveData,
    offset_x: u32,
    offset_y: u32,
    w: u32,
    h: u32,
    curve_color: Rgba<u8>,
) {
    let center_x = offset_x as f32 + w as f32 / 2.0;
    let center_y = offset_y as f32 + h as f32 / 2.0;
    let scale = (w.min(h) as f32) * 0.45;
    
    let grid_color = Rgba([200u8, 200u8, 200u8, 255u8]);
    for i in -10..=10 {
        let off = (i as f32) * scale / 10.0;
        
        let x = (center_x + off) as i32;
        if x >= offset_x as i32 && x < (offset_x + w) as i32 {
            for y in offset_y..(offset_y + h) {
                if let Some(pixel) = img.get_pixel_mut_checked(x as u32, y) {
                    *pixel = grid_color;
                }
            }
        }
        
        let y = (center_y + off) as i32;
        if y >= offset_y as i32 && y < (offset_y + h) as i32 {
            for x in offset_x..(offset_x + w) {
                if let Some(pixel) = img.get_pixel_mut_checked(x, y as u32) {
                    *pixel = grid_color;
                }
            }
        }
    }
    
    let axis_color = Rgba([0u8, 0u8, 0u8, 255u8]);
    let cy = center_y as i32;
    for x in offset_x..(offset_x + w) {
        for dy in -1..=1 {
            let y = cy + dy;
            if y >= offset_y as i32 && y < (offset_y + h) as i32 {
                if let Some(pixel) = img.get_pixel_mut_checked(x, y as u32) {
                    *pixel = axis_color;
                }
            }
        }
    }
    
    let cx = center_x as i32;
    for y in offset_y..(offset_y + h) {
        for dx in -1..=1 {
            let x = cx + dx;
            if x >= offset_x as i32 && x < (offset_x + w) as i32 {
                if let Some(pixel) = img.get_pixel_mut_checked(x as u32, y) {
                    *pixel = axis_color;
                }
            }
        }
    }
    
    for i in 0..curve.voltage.len() {
        let v = curve.voltage[i];
        let c = curve.current[i];
        
        let x = (center_x + v * scale) as i32;
        let y = (center_y - c * scale) as i32;
        
        for dy in -1..=1 {
            for dx in -1..=1 {
                let px = x + dx;
                let py = y + dy;
                
                if px >= offset_x as i32 && px < (offset_x + w) as i32 &&
                   py >= offset_y as i32 && py < (offset_y + h) as i32 {
                    if let Some(pixel) = img.get_pixel_mut_checked(px as u32, py as u32) {
                        *pixel = curve_color;
                    }
                }
            }
        }
    }
}

fn main() -> Result<(), eframe::Error> {
    let args = Args::parse();
    
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 700.0]),
        ..Default::default()
    };

    eframe::run_native(
        "CT220S V-I Curve Viewer",
        options,
        Box::new(move |cc| Box::new(CT220SApp::new(cc, args.file))),
    )
}
