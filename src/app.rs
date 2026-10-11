// src/app.rs

use crate::backend::{run_file_reader, run_hid_reader, Command, HidBackend};
use crate::curve::{
    calculate_electrical_values, format_capacitance, format_inductance, format_resistance,
    CurveData, DualCurveData,
};
use crate::image_export::{save_curve_as_png, save_dual_curves_as_png};

use eframe::egui;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    Ch0,
    Ch1,
    Dual,
}

pub struct CT220SApp {
    pub curve_data: Arc<Mutex<DualCurveData>>,
    pub error_message: Arc<Mutex<Option<String>>>,
    pub running: Arc<Mutex<bool>>,
    pub use_file_mode: bool,
    pub file_path: String,
    pub display_mode: DisplayMode,
    pub hid_backend: Option<Arc<Mutex<HidBackend>>>,
    pub frequency_hz: f32,
    pub source_resistance_ohm: f32,
    pub voltage_v: f32,
}

impl CT220SApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, file_arg: Option<String>) -> Self {
        let curve_data = Arc::new(Mutex::new(DualCurveData::new()));
        let error_message = Arc::new(Mutex::new(None));
        let running = Arc::new(Mutex::new(true));

        let (use_file_mode, file_path) = if let Some(path) = file_arg {
            (true, path)
        } else {
            let default_path = "capture.txt".to_string();
            let exists = Path::new(&default_path).exists();
            (exists, default_path)
        };

        let curve_data_clone = Arc::clone(&curve_data);
        let error_clone = Arc::clone(&error_message);
        let running_clone = Arc::clone(&running);
        let file_path_clone = file_path.clone();

        let hid_backend = if !use_file_mode {
            match HidBackend::new() {
                Ok(backend) => {
                    let backend_arc = Arc::new(Mutex::new(backend));
                    let device = backend_arc.lock().unwrap().clone_device();
                    *error_message.lock().unwrap() = Some("Périphérique USB connecté".to_string());

                    thread::spawn(move || {
                        println!("Mode périphérique USB - lecture démarrée");
                        if let Err(e) = run_hid_reader(
                            device,
                            curve_data_clone,
                            error_clone,
                            running_clone,
                        ) {
                            eprintln!("Erreur HID reader: {}", e);
                        }
                    });
                    Some(backend_arc)
                }
                Err(e) => {
                    eprintln!("Impossible de créer le backend HID: {}", e);
                    *error_message.lock().unwrap() = Some(format!("Erreur USB: {}", e));
                    None
                }
            }
        } else {
            thread::spawn(move || {
                println!("Mode fichier: lecture de {}", file_path_clone);
                if let Err(e) =
                    run_file_reader(&file_path_clone, curve_data_clone, error_clone, running_clone)
                {
                    eprintln!("Erreur lecture fichier: {}", e);
                }
            });
            None
        };

        Self {
            curve_data,
            error_message,
            running,
            use_file_mode,
            file_path,
            display_mode: DisplayMode::Ch0,
            hid_backend,
            frequency_hz: 10.0,
            source_resistance_ohm: 1_000.0,
            voltage_v: 2.5,
        }
    }

    fn send_command(&self, cmd: Command, ok_msg: &str) {
        if self.use_file_mode {
            println!("Simulation fichier - commande: {:?}", cmd);
            *self.error_message.lock().unwrap() = Some(format!("🧪 Simulation: {}", ok_msg));
            return;
        }

        if let Some(backend) = &self.hid_backend {
            match backend.lock().unwrap().send_cmd(cmd) {
                Ok(()) => {
                    *self.error_message.lock().unwrap() = Some(format!("✅ {}", ok_msg));
                }
                Err(e) => {
                    *self.error_message.lock().unwrap() = Some(format!("❌ Erreur cmd: {}", e));
                }
            }
        } else {
            *self.error_message.lock().unwrap() = Some("⚠️ Boîtier USB non connecté".to_string());
        }
    }

    fn channel_color(channel: u8) -> egui::Color32 {
        if channel == 0 {
            egui::Color32::from_rgb(255, 100, 0)
        } else {
            egui::Color32::BLUE
        }
    }

    fn draw_grid(&self, ui: &mut egui::Ui, size: f32) -> (egui::Rect, egui::Pos2, f32, egui::Painter) {
        let desired_size = egui::vec2(size, size);
        let (response, painter) = ui.allocate_painter(desired_size, egui::Sense::hover());
        let rect = response.rect;
        painter.rect_filled(rect, 0.0, egui::Color32::WHITE);

        let center = rect.center();
        let scale = rect.width().min(rect.height()) * 0.45;
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

        painter.line_segment(
            [egui::pos2(rect.left(), center.y), egui::pos2(rect.right(), center.y)],
            egui::Stroke::new(1.0, egui::Color32::BLACK),
        );
        painter.line_segment(
            [egui::pos2(center.x, rect.top()), egui::pos2(center.x, rect.bottom())],
            egui::Stroke::new(1.0, egui::Color32::BLACK),
        );

        (rect, center, scale, painter)
    }

    fn draw_curve(&self, painter: &egui::Painter, curve: &CurveData, center: egui::Pos2, scale: f32) {
        let points: Vec<egui::Pos2> = curve
            .voltage
            .iter()
            .zip(curve.current.iter())
            .map(|(&v, &i)| egui::pos2(center.x + v * scale, center.y - i * scale))
            .collect();

        if points.len() > 1 {
            painter.add(egui::Shape::line(
                points,
                egui::Stroke::new(1.8, Self::channel_color(curve.channel)),
            ));
        }
    }

    fn draw_measurements(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        curve: &CurveData,
        right_side: bool,
    ) {
        let values = calculate_electrical_values(
            curve,
            self.frequency_hz,
            self.source_resistance_ohm,
        );

        let r_text = values.resistance_ohm.map(format_resistance).unwrap_or_default();
        let c_text = values.capacitance_f.map(format_capacitance).unwrap_or_default();
        let l_text = values.inductance_h.map(format_inductance).unwrap_or_default();
        let ph_text = values
            .phase_deg
            .map(|v| format!("{:.1}°", v))
            .unwrap_or_default();

        let lines = [
            format!("R: {}", r_text),
            format!("C: {}", c_text),
            format!("L: {}", l_text),
            format!("Ph: {}", ph_text),
        ];

        let font = egui::FontId::proportional(16.0);
        let color = Self::channel_color(curve.channel);
        let y = rect.top() + 12.0;
        let dy = 21.0;

        for (idx, line) in lines.iter().enumerate() {
            if right_side {
                painter.text(
                    egui::pos2(rect.right() - 18.0, y + idx as f32 * dy),
                    egui::Align2::RIGHT_TOP,
                    line,
                    font.clone(),
                    color,
                );
            } else {
                painter.text(
                    egui::pos2(rect.left() + 18.0, y + idx as f32 * dy),
                    egui::Align2::LEFT_TOP,
                    line,
                    font.clone(),
                    color,
                );
            }
        }
    }

    fn draw_axes_labels(&self, painter: &egui::Painter, rect: egui::Rect, center: egui::Pos2) {
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

    fn draw_single_channel(&self, ui: &mut egui::Ui, channel: u8, size: f32) {
        let (rect, center, scale, painter) = self.draw_grid(ui, size);

        let curve = if let Ok(data) = self.curve_data.lock() {
            if channel == 0 {
                data.channel0.clone()
            } else {
                data.channel1.clone()
            }
        } else {
            None
        };

        if let Some(curve) = curve {
            self.draw_curve(&painter, &curve, center, scale);
            // CH0 en haut à gauche, CH1 en haut à droite.
            self.draw_measurements(&painter, rect, &curve, channel == 1);
        }

        self.draw_axes_labels(&painter, rect, center);
    }

    fn draw_dual_overlay(&self, ui: &mut egui::Ui, size: f32) {
        let (rect, center, scale, painter) = self.draw_grid(ui, size);

        let (ch0, ch1) = if let Ok(data) = self.curve_data.lock() {
            (data.channel0.clone(), data.channel1.clone())
        } else {
            (None, None)
        };

        if let Some(curve) = ch0 {
            self.draw_curve(&painter, &curve, center, scale);
            self.draw_measurements(&painter, rect, &curve, false);
        }
        if let Some(curve) = ch1 {
            self.draw_curve(&painter, &curve, center, scale);
            self.draw_measurements(&painter, rect, &curve, true);
        }

        self.draw_axes_labels(&painter, rect, center);
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

            ui.separator();
            ui.heading("⚡ Commandes");

            ui.horizontal(|ui| {
                ui.label("Fréquence:");
                if ui.selectable_label(self.frequency_hz == 10.0, "10Hz").clicked() {
                    self.frequency_hz = 10.0;
                    self.send_command(Command::SetFreq(0), "Fréquence: 10Hz");
                }
                if ui.selectable_label(self.frequency_hz == 100.0, "100Hz").clicked() {
                    self.frequency_hz = 100.0;
                    self.send_command(Command::SetFreq(1), "Fréquence: 100Hz");
                }
                if ui.selectable_label(self.frequency_hz == 500.0, "500Hz").clicked() {
                    self.frequency_hz = 500.0;
                    self.send_command(Command::SetFreq(2), "Fréquence: 500Hz");
                }
                if ui.selectable_label(self.frequency_hz == 2_000.0, "2kHz").clicked() {
                    self.frequency_hz = 2_000.0;
                    self.send_command(Command::SetFreq(3), "Fréquence: 2kHz");
                }
            });

            ui.horizontal(|ui| {
                ui.label("Résistance:");
                if ui.selectable_label(self.source_resistance_ohm == 47.0, "47R").clicked() {
                    self.source_resistance_ohm = 47.0;
                    self.send_command(Command::SetRes(2), "Résistance: 47R");
                }
                if ui.selectable_label(self.source_resistance_ohm == 1_000.0, "1K").clicked() {
                    self.source_resistance_ohm = 1_000.0;
                    self.send_command(Command::SetRes(1), "Résistance: 1K");
                }
                if ui.selectable_label(self.source_resistance_ohm == 10_000.0, "10K").clicked() {
                    self.source_resistance_ohm = 10_000.0;
                    self.send_command(Command::SetRes(0), "Résistance: 10K");
                }
                if ui.button("offset").clicked() {
                    self.send_command(Command::SetRes(0), "Offset");
                }
            });

            ui.horizontal(|ui| {
                ui.label("Mode:");
                if ui.selectable_label(self.display_mode == DisplayMode::Ch0, "CH0").clicked() {
                    self.display_mode = DisplayMode::Ch0;
                    // Le protocole déjà validé distingue Simple / Dual. CH0 et CH1
                    // restent des choix d'affichage en mode Simple.
                    self.send_command(Command::SetMode(0), "Mode: CH0");
                }
                if ui.selectable_label(self.display_mode == DisplayMode::Ch1, "CH1").clicked() {
                    self.display_mode = DisplayMode::Ch1;
                    self.send_command(Command::SetMode(0), "Mode: CH1");
                }
                if ui.selectable_label(self.display_mode == DisplayMode::Dual, "DUAL").clicked() {
                    self.display_mode = DisplayMode::Dual;
                    self.send_command(Command::SetMode(1), "Mode: DUAL");
                }
            });

            ui.horizontal(|ui| {
                ui.label("Voltage:");
                if ui.selectable_label(self.voltage_v == 2.5, "2.5V").clicked() {
                    self.voltage_v = 2.5;
                    self.send_command(Command::SetVolt(0), "Voltage: 2.5V");
                }
                if ui.selectable_label(self.voltage_v == 5.0, "5V").clicked() {
                    self.voltage_v = 5.0;
                    self.send_command(Command::SetVolt(1), "Voltage: 5V");
                }
                if ui.selectable_label(self.voltage_v == 10.0, "10V").clicked() {
                    self.voltage_v = 10.0;
                    self.send_command(Command::SetVolt(2), "Voltage: 10V");
                }
                if ui.selectable_label(self.voltage_v == 20.0, "20V").clicked() {
                    self.voltage_v = 20.0;
                    self.send_command(Command::SetVolt(3), "Voltage: 20V");
                }
            });

            ui.separator();

            if ui.button("💾 Sauvegarder PNG").clicked() {
                if let Ok(data) = self.curve_data.lock() {
                    let result = match self.display_mode {
                        DisplayMode::Dual => save_dual_curves_as_png(&data, "curves_export.png"),
                        DisplayMode::Ch0 => {
                            if let Some(ch0) = &data.channel0 {
                                save_curve_as_png(ch0, "curve_ch0_export.png")
                            } else {
                                Err("Pas de données CH0".to_string())
                            }
                        }
                        DisplayMode::Ch1 => {
                            if let Some(ch1) = &data.channel1 {
                                save_curve_as_png(ch1, "curve_ch1_export.png")
                            } else {
                                Err("Pas de données CH1".to_string())
                            }
                        }
                    };

                    match result {
                        Ok(_) => {
                            *self.error_message.lock().unwrap() = Some("✅ Sauvegardé".to_string());
                        }
                        Err(e) => {
                            *self.error_message.lock().unwrap() = Some(format!("❌ Erreur: {}", e));
                        }
                    }
                }
            }

            ui.separator();

            if let Ok(err) = self.error_message.lock() {
                if let Some(msg) = &*err {
                    ui.colored_label(egui::Color32::from_rgb(0, 150, 0), msg);
                }
            }

            ui.separator();

            match self.display_mode {
                DisplayMode::Ch0 => self.draw_single_channel(ui, 0, 600.0),
                DisplayMode::Ch1 => self.draw_single_channel(ui, 1, 600.0),
                DisplayMode::Dual => self.draw_dual_overlay(ui, 600.0),
            }
        });

        ctx.request_repaint();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        *self.running.lock().unwrap() = false;
    }
}
