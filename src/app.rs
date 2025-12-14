// src/app.rs

use crate::backend::{run_file_reader, run_hid_reader, HidBackend, Command};
use crate::curve::DualCurveData;
use crate::image_export::{save_curve_as_png, save_dual_curves_as_png};

use eframe::egui;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

pub struct CT220SApp {
    pub curve_data: Arc<Mutex<DualCurveData>>,
    pub error_message: Arc<Mutex<Option<String>>>,
    pub running: Arc<Mutex<bool>>,
    pub use_file_mode: bool,
    pub file_path: String,
    pub dual_mode: bool,
    pub hid_backend: Option<Arc<Mutex<HidBackend>>>,
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

        let dual_mode = use_file_mode;

        let curve_data_clone = Arc::clone(&curve_data);
        let error_clone = Arc::clone(&error_message);
        let running_clone = Arc::clone(&running);
        let file_path_clone = file_path.clone();

        let hid_backend = if !use_file_mode {
            // Mode USB : créer le backend HID
            match HidBackend::new() {
                Ok(backend) => {
                    let backend_arc = Arc::new(Mutex::new(backend));
                    let device = backend_arc.lock().unwrap().clone_device();
                    
                    *error_message.lock().unwrap() = Some("Périphérique USB connecté".to_string());
                    
                    // Lancer le thread de lecture
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
            // Mode fichier
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
            dual_mode,
            hid_backend,
        }
    }

    fn draw_single_channel(&self, ui: &mut egui::Ui, channel: u8, size: f32) {
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

        let axis_color = egui::Color32::BLACK;
        painter.line_segment(
            [egui::pos2(rect.left(), center.y), egui::pos2(rect.right(), center.y)],
            egui::Stroke::new(1.0, axis_color),
        );
        painter.line_segment(
            [egui::pos2(center.x, rect.top()), egui::pos2(center.x, rect.bottom())],
            egui::Stroke::new(1.0, axis_color),
        );

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
                    .map(|(&v, &i)| egui::pos2(center.x + v * scale, center.y - i * scale))
                    .collect();

                if points.len() > 1 {
                    let color = if channel == 0 {
                        egui::Color32::from_rgb(255, 100, 0)
                    } else {
                        egui::Color32::BLUE
                    };

                    painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, color)));
                }
            }
        }

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

        let axis_color = egui::Color32::BLACK;
        painter.line_segment(
            [egui::pos2(rect.left(), center.y), egui::pos2(rect.right(), center.y)],
            egui::Stroke::new(1.0, axis_color),
        );
        painter.line_segment(
            [egui::pos2(center.x, rect.top()), egui::pos2(center.x, rect.bottom())],
            egui::Stroke::new(1.0, axis_color),
        );

        if let Ok(data) = self.curve_data.lock() {
            if let Some(curve) = &data.channel0 {
                let points: Vec<egui::Pos2> = curve
                    .voltage
                    .iter()
                    .zip(curve.current.iter())
                    .map(|(&v, &i)| egui::pos2(center.x + v * scale, center.y - i * scale))
                    .collect();

                if points.len() > 1 {
                    painter.add(egui::Shape::line(
                        points,
                        egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 100, 0)),
                    ));
                }
            }

            if let Some(curve) = &data.channel1 {
                let points: Vec<egui::Pos2> = curve
                    .voltage
                    .iter()
                    .zip(curve.current.iter())
                    .map(|(&v, &i)| egui::pos2(center.x + v * scale, center.y - i * scale))
                    .collect();

                if points.len() > 1 {
                    painter.add(egui::Shape::line(
                        points,
                        egui::Stroke::new(2.0, egui::Color32::BLUE),
                    ));
                }
            }
        }

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

            // Panneau de commandes USB (uniquement en mode USB)
            if let Some(backend) = &self.hid_backend {
                ui.separator();
                ui.heading("⚡ Commandes");

                ui.horizontal(|ui| {
                    ui.label("Fréquence:");
                    if ui.button("100Hz").clicked() {
                        if let Err(e) = backend.lock().unwrap().send_cmd(Command::SetFreq(0)) {
                            *self.error_message.lock().unwrap() =
                                Some(format!("❌ Erreur cmd: {}", e));
                        } else {
                            *self.error_message.lock().unwrap() =
                                Some("✅ Fréquence: 100Hz".to_string());
                        }
                    }
                    if ui.button("1kHz").clicked() {
                        if let Err(e) = backend.lock().unwrap().send_cmd(Command::SetFreq(1)) {
                            *self.error_message.lock().unwrap() =
                                Some(format!("❌ Erreur cmd: {}", e));
                        } else {
                            *self.error_message.lock().unwrap() =
                                Some("✅ Fréquence: 1kHz".to_string());
                        }
                    }
                    if ui.button("10kHz").clicked() {
                        if let Err(e) = backend.lock().unwrap().send_cmd(Command::SetFreq(2)) {
                            *self.error_message.lock().unwrap() =
                                Some(format!("❌ Erreur cmd: {}", e));
                        } else {
                            *self.error_message.lock().unwrap() =
                                Some("✅ Fréquence: 10kHz".to_string());
                        }
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Résolution:");
                    if ui.button("Basse").clicked() {
                        if let Err(e) = backend.lock().unwrap().send_cmd(Command::SetRes(0)) {
                            *self.error_message.lock().unwrap() =
                                Some(format!("❌ Erreur cmd: {}", e));
                        } else {
                            *self.error_message.lock().unwrap() =
                                Some("✅ Résolution: Basse".to_string());
                        }
                    }
                    if ui.button("Haute").clicked() {
                        if let Err(e) = backend.lock().unwrap().send_cmd(Command::SetRes(1)) {
                            *self.error_message.lock().unwrap() =
                                Some(format!("❌ Erreur cmd: {}", e));
                        } else {
                            *self.error_message.lock().unwrap() =
                                Some("✅ Résolution: Haute".to_string());
                        }
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Mode:");
                    if ui.button("Simple").clicked() {
                        if let Err(e) = backend.lock().unwrap().send_cmd(Command::SetMode(0)) {
                            *self.error_message.lock().unwrap() =
                                Some(format!("❌ Erreur cmd: {}", e));
                        } else {
                            *self.error_message.lock().unwrap() =
                                Some("✅ Mode: Simple".to_string());
                        }
                    }
                    if ui.button("Dual").clicked() {
                        if let Err(e) = backend.lock().unwrap().send_cmd(Command::SetMode(1)) {
                            *self.error_message.lock().unwrap() =
                                Some(format!("❌ Erreur cmd: {}", e));
                        } else {
                            *self.error_message.lock().unwrap() =
                                Some("✅ Mode: Dual".to_string());
                        }
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Voltage:");
                    if ui.button("3.3V").clicked() {
                        if let Err(e) = backend.lock().unwrap().send_cmd(Command::SetVolt(0)) {
                            *self.error_message.lock().unwrap() =
                                Some(format!("❌ Erreur cmd: {}", e));
                        } else {
                            *self.error_message.lock().unwrap() =
                                Some("✅ Voltage: 3.3V".to_string());
                        }
                    }
                    if ui.button("5V").clicked() {
                        if let Err(e) = backend.lock().unwrap().send_cmd(Command::SetVolt(1)) {
                            *self.error_message.lock().unwrap() =
                                Some(format!("❌ Erreur cmd: {}", e));
                        } else {
                            *self.error_message.lock().unwrap() =
                                Some("✅ Voltage: 5V".to_string());
                        }
                    }
                });
            }

            ui.separator();

            if ui.button("💾 Sauvegarder PNG").clicked() {
                if let Ok(data) = self.curve_data.lock() {
                    let result = if self.dual_mode {
                        save_dual_curves_as_png(&data, "curves_export.png")
                    } else if let Some(ch1) = &data.channel1 {
                        save_curve_as_png(ch1, "curve_ch1_export.png")
                    } else {
                        Err("Pas de données CH1".to_string())
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
                    ui.colored_label(egui::Color32::from_rgb(0, 150, 0), msg);
                }
            }

            ui.separator();

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
