// src/app.rs

use crate::backend::{run_file_reader, run_hid_reader, run_tcp_reader, send_tcp_cmd, Command, HidBackend};
use crate::curve::{
    calculate_electrical_values, format_capacitance, format_inductance, format_resistance,
    CurveData, DualCurveData,
};
use crate::image_export::{save_curve_as_png, save_dual_curves_as_png};

use eframe::egui;
use std::net::TcpStream;
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
    pub use_tcp_mode: bool,
    pub file_path: String,
    pub tcp_address: String,
    pub display_mode: DisplayMode,
    pub hid_backend: Option<Arc<Mutex<HidBackend>>>,
    pub tcp_command_stream: Arc<Mutex<Option<TcpStream>>>,
    pub frequency_hz: f32,
    pub source_resistance_ohm: f32,
    pub voltage_v: f32,
}

impl CT220SApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, file_arg: Option<String>, tcp_arg: Option<String>) -> Self {
        let curve_data = Arc::new(Mutex::new(DualCurveData::new()));
        let error_message = Arc::new(Mutex::new(None));
        let running = Arc::new(Mutex::new(true));
        let tcp_command_stream: Arc<Mutex<Option<TcpStream>>> = Arc::new(Mutex::new(None));

        // Priorité des sources : fichier explicite > TCP explicite > USB.
        let (use_file_mode, use_tcp_mode, file_path, tcp_address) = if let Some(path) = file_arg {
            (true, false, path, "127.0.0.1:3000".to_string())
        } else if let Some(address) = tcp_arg {
            (false, true, "capture.txt".to_string(), address)
        } else {
            let default_path = "capture.txt".to_string();
            let exists = Path::new(&default_path).exists();
            (exists, false, default_path, "127.0.0.1:3000".to_string())
        };

        let curve_data_clone = Arc::clone(&curve_data);
        let error_clone = Arc::clone(&error_message);
        let running_clone = Arc::clone(&running);
        let file_path_clone = file_path.clone();
        let tcp_address_clone = tcp_address.clone();
        let tcp_command_stream_clone = Arc::clone(&tcp_command_stream);

        let hid_backend = if use_file_mode {
            thread::spawn(move || {
                println!("Mode fichier: lecture de {}", file_path_clone);
                if let Err(e) =
                    run_file_reader(&file_path_clone, curve_data_clone, error_clone, running_clone)
                {
                    eprintln!("Erreur lecture fichier: {}", e);
                }
            });
            None
        } else if use_tcp_mode {
            thread::spawn(move || {
                if let Err(e) = run_tcp_reader(
                    &tcp_address_clone,
                    curve_data_clone,
                    error_clone,
                    running_clone,
                    tcp_command_stream_clone,
                ) {
                    eprintln!("Erreur TCP reader: {}", e);
                }
            });
            None
        } else {
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
        };

        Self {
            curve_data,
            error_message,
            running,
            use_file_mode,
            use_tcp_mode,
            file_path,
            tcp_address,
            display_mode: DisplayMode::Ch0,
            hid_backend,
            tcp_command_stream,
            frequency_hz: 10.0,
            source_resistance_ohm: 1_000.0,
            voltage_v: 2.5,
        }
    }

    fn send_command(&self, cmd: Command, ok_msg: &str) {
        if self.use_file_mode {
            println!("Simulation fichier - commande locale: {:?}", cmd);
            *self.error_message.lock().unwrap() = Some(format!("Simulation fichier: {}", ok_msg));
            return;
        }

        if self.use_tcp_mode {
            match send_tcp_cmd(&self.tcp_command_stream, cmd) {
                Ok(()) => {
                    *self.error_message.lock().unwrap() = Some(ok_msg.to_string());
                }
                Err(e) => {
                    *self.error_message.lock().unwrap() = Some(format!("Erreur cmd TCP: {}", e));
                }
            }
            return;
        }

        if let Some(backend) = &self.hid_backend {
            match backend.lock().unwrap().send_cmd(cmd) {
                Ok(()) => {
                    *self.error_message.lock().unwrap() = Some(ok_msg.to_string());
                }
                Err(e) => {
                    *self.error_message.lock().unwrap() = Some(format!("Erreur cmd: {}", e));
                }
            }
        } else {
            *self.error_message.lock().unwrap() = Some("Boîtier USB non connecté".to_string());
        }
    }

    fn channel_color(channel: u8) -> egui::Color32 {
        if channel == 0 {
            egui::Color32::from_rgb(255, 190, 0)
        } else {
            egui::Color32::from_rgb(70, 150, 255)
        }
    }

    fn draw_grid(
        &self,
        ui: &mut egui::Ui,
        size: f32,
    ) -> (egui::Rect, egui::Pos2, f32, egui::Painter) {
        let desired_size = egui::vec2(size, size);
        let (response, painter) = ui.allocate_painter(desired_size, egui::Sense::hover());
        let rect = response.rect;

        // Style proche du logiciel d'origine : fond noir, grille grise et axes verts.
        painter.rect_filled(rect, 0.0, egui::Color32::BLACK);

        let center = rect.center();
        let scale = rect.width().min(rect.height()) * 0.46;
        let grid_color = egui::Color32::from_gray(115);

        for i in -10..=10 {
            let offset = (i as f32) * scale / 10.0;
            painter.line_segment(
                [
                    egui::pos2(center.x + offset, rect.top()),
                    egui::pos2(center.x + offset, rect.bottom()),
                ],
                egui::Stroke::new(0.7, grid_color),
            );
            painter.line_segment(
                [
                    egui::pos2(rect.left(), center.y + offset),
                    egui::pos2(rect.right(), center.y + offset),
                ],
                egui::Stroke::new(0.7, grid_color),
            );
        }

        let axis_color = egui::Color32::from_rgb(0, 145, 0);
        painter.line_segment(
            [
                egui::pos2(rect.left(), center.y),
                egui::pos2(rect.right(), center.y),
            ],
            egui::Stroke::new(1.1, axis_color),
        );
        painter.line_segment(
            [
                egui::pos2(center.x, rect.top()),
                egui::pos2(center.x, rect.bottom()),
            ],
            egui::Stroke::new(1.1, axis_color),
        );

        (rect, center, scale, painter)
    }

    fn draw_curve(
        &self,
        painter: &egui::Painter,
        curve: &CurveData,
        center: egui::Pos2,
        scale: f32,
    ) {
        let points: Vec<egui::Pos2> = curve
            .voltage
            .iter()
            .zip(curve.current.iter())
            .map(|(&v, &i)| egui::pos2(center.x + v * scale, center.y - i * scale))
            .collect();

        if points.len() > 1 {
            painter.add(egui::Shape::line(
                points,
                egui::Stroke::new(1.6, Self::channel_color(curve.channel)),
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

        let r_text = values
            .resistance_ohm
            .map(format_resistance)
            .unwrap_or_else(|| "?".to_string());
        let c_text = values
            .capacitance_f
            .map(format_capacitance)
            .unwrap_or_else(|| "?".to_string());
        let l_text = values
            .inductance_h
            .map(format_inductance)
            .unwrap_or_else(|| "?".to_string());
        let ph_text = values
            .phase_deg
            .map(|v| format!("{:.1}°", v))
            .unwrap_or_else(|| "?".to_string());

        let lines = [
            format!("R: {}", r_text),
            format!("C: {}", c_text),
            format!("L: {}", l_text),
            format!("Ph: {}", ph_text),
        ];

        let font = egui::FontId::proportional(15.0);
        let color = Self::channel_color(curve.channel);
        // Reserve la première ligne pour le nom du canal afin d'éviter
        // le chevauchement avec R / C / L / Ph.
        let y = rect.top() + 34.0;
        let dy = 18.0;

        for (idx, line) in lines.iter().enumerate() {
            if right_side {
                painter.text(
                    egui::pos2(rect.right() - 10.0, y + idx as f32 * dy),
                    egui::Align2::RIGHT_TOP,
                    line,
                    font.clone(),
                    color,
                );
            } else {
                painter.text(
                    egui::pos2(rect.left() + 10.0, y + idx as f32 * dy),
                    egui::Align2::LEFT_TOP,
                    line,
                    font.clone(),
                    color,
                );
            }
        }

        // Nom du canal sur une ligne dédiée, dans la même couleur que sa courbe.
        // CH0 reste à gauche et CH1 à droite, y compris en mode DUAL.
        let channel_label = if curve.channel == 0 { "CH0" } else { "CH1" };
        if right_side {
            painter.text(
                egui::pos2(rect.right() - 12.0, rect.top() + 10.0),
                egui::Align2::RIGHT_TOP,
                channel_label,
                egui::FontId::proportional(14.0),
                color,
            );
        } else {
            painter.text(
                egui::pos2(rect.left() + 10.0, rect.top() + 10.0),
                egui::Align2::LEFT_TOP,
                channel_label,
                egui::FontId::proportional(14.0),
                color,
            );
        }
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
            self.draw_measurements(&painter, rect, &curve, channel == 1);
        }
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
    }

    fn draw_group_title(ui: &mut egui::Ui, title: &str) {
        ui.label(egui::RichText::new(title).strong());
    }

    fn draw_controls(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(215.0);

        // Onglets visuels proches de l'application d'origine.
        ui.horizontal(|ui| {
            let _ = ui.selectable_label(true, "Basic Mode");
            ui.add_enabled(false, egui::SelectableLabel::new(false, "Advanced Mode"));
            ui.add_enabled(false, egui::SelectableLabel::new(false, "About"));
        });
        ui.add_space(6.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            Self::draw_group_title(ui, "Mode");
            let before = self.display_mode;
            ui.radio_value(&mut self.display_mode, DisplayMode::Ch0, "CH0");
            ui.radio_value(&mut self.display_mode, DisplayMode::Ch1, "CH1");
            ui.radio_value(&mut self.display_mode, DisplayMode::Dual, "DUAL");
            if self.display_mode != before {
                match self.display_mode {
                    DisplayMode::Dual => self.send_command(Command::SetMode(1), "Mode: DUAL"),
                    DisplayMode::Ch0 | DisplayMode::Ch1 => {
                        self.send_command(Command::SetMode(0), "Mode: Simple")
                    }
                }
            }
        });

        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            Self::draw_group_title(ui, "Voltage Ranges");
            let before = self.voltage_v;
            ui.radio_value(&mut self.voltage_v, 2.5, "2.5 V");
            ui.radio_value(&mut self.voltage_v, 5.0, "5.0 V");
            ui.radio_value(&mut self.voltage_v, 10.0, "10 V");
            ui.radio_value(&mut self.voltage_v, 20.0, "20 V");
            if self.voltage_v != before {
                let (index, msg) = if self.voltage_v == 2.5 {
                    (0, "Voltage: 2.5V")
                } else if self.voltage_v == 5.0 {
                    (1, "Voltage: 5V")
                } else if self.voltage_v == 10.0 {
                    (2, "Voltage: 10V")
                } else {
                    (3, "Voltage: 20V")
                };
                self.send_command(Command::SetVolt(index), msg);
            }
        });

        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            Self::draw_group_title(ui, "Frequency");
            let before = self.frequency_hz;
            ui.radio_value(&mut self.frequency_hz, 10.0, "10 Hz");
            ui.radio_value(&mut self.frequency_hz, 100.0, "100 Hz");
            ui.radio_value(&mut self.frequency_hz, 500.0, "500 Hz");
            ui.radio_value(&mut self.frequency_hz, 2_000.0, "2 kHz");
            if self.frequency_hz != before {
                let (index, msg) = if self.frequency_hz == 10.0 {
                    (0, "Fréquence: 10Hz")
                } else if self.frequency_hz == 100.0 {
                    (1, "Fréquence: 100Hz")
                } else if self.frequency_hz == 500.0 {
                    (2, "Fréquence: 500Hz")
                } else {
                    (3, "Fréquence: 2kHz")
                };
                self.send_command(Command::SetFreq(index), msg);
            }
        });

        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            Self::draw_group_title(ui, "Resistor");
            let before = self.source_resistance_ohm;
            ui.radio_value(&mut self.source_resistance_ohm, 47.0, "47 R");
            ui.radio_value(&mut self.source_resistance_ohm, 1_000.0, "1 K");
            ui.radio_value(&mut self.source_resistance_ohm, 10_000.0, "10 K");
            if self.source_resistance_ohm != before {
                let (index, msg) = if self.source_resistance_ohm == 47.0 {
                    (2, "Résistance: 47R")
                } else if self.source_resistance_ohm == 1_000.0 {
                    (1, "Résistance: 1K")
                } else {
                    (0, "Résistance: 10K")
                };
                self.send_command(Command::SetRes(index), msg);
            }

            if ui.button("Offset").clicked() {
                self.send_command(Command::SetRes(0), "Offset");
            }
        });

        ui.add_space(10.0);

        if ui.button("Save As").clicked() {
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
                        *self.error_message.lock().unwrap() = Some("Sauvegardé".to_string());
                    }
                    Err(e) => {
                        *self.error_message.lock().unwrap() = Some(format!("Erreur: {}", e));
                    }
                }
            }
        }
    }

    fn format_voltage_label(&self) -> String {
        if self.voltage_v.fract() == 0.0 {
            format!("{:.0} V", self.voltage_v)
        } else {
            format!("{:.1} V", self.voltage_v)
        }
    }

    fn format_frequency_label(&self) -> String {
        if self.frequency_hz >= 1000.0 {
            format!("{} kHz", (self.frequency_hz / 1000.0) as i32)
        } else {
            format!("{} Hz", self.frequency_hz as i32)
        }
    }

    fn format_source_resistor_label(&self) -> String {
        if self.source_resistance_ohm >= 1000.0 {
            format!("{} K", (self.source_resistance_ohm / 1000.0) as i32)
        } else {
            format!("{} R", self.source_resistance_ohm as i32)
        }
    }

    fn draw_status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("V. Range: {}", self.format_voltage_label()));
            ui.separator();
            ui.label(format!("Frequency: {}", self.format_frequency_label()));
            ui.separator();
            ui.label(format!("Resistor: {}", self.format_source_resistor_label()));
            ui.separator();
            ui.label(match self.display_mode {
                DisplayMode::Dual => "Dual Mode",
                DisplayMode::Ch0 => "CH0 Mode",
                DisplayMode::Ch1 => "CH1 Mode",
            });
            ui.separator();

            if self.use_file_mode {
                ui.label(format!("File: {}", self.file_path));
            } else if self.use_tcp_mode {
                ui.label(format!("Simulator TCP: {}", self.tcp_address));
            } else if self.hid_backend.is_some() {
                ui.label("Device Connected");
            } else {
                ui.label("Device Disconnected");
            }
        });
    }
}

impl eframe::App for CT220SApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // On conserve le thème global existant, mais la zone principale est réorganisée
        // pour reproduire l'ergonomie du logiciel CT220S d'origine.
        egui::CentralPanel::default()
            .frame(egui::Frame::none().inner_margin(egui::Margin::same(6.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui_left| {
                        // Taille adaptée à la hauteur disponible, tout en gardant un graphe carré.
                        let max_size = ui_left.available_height().min(680.0).max(420.0);
                        match self.display_mode {
                            DisplayMode::Ch0 => self.draw_single_channel(ui_left, 0, max_size),
                            DisplayMode::Ch1 => self.draw_single_channel(ui_left, 1, max_size),
                            DisplayMode::Dual => self.draw_dual_overlay(ui_left, max_size),
                        }
                    });

                    ui.separator();

                    ui.vertical(|ui_right| {
                        self.draw_controls(ui_right);

                        ui_right.add_space(8.0);
                        if let Ok(err) = self.error_message.lock() {
                            if let Some(msg) = &*err {
                                ui_right.small(msg);
                            }
                        }
                    });
                });

                ui.separator();
                self.draw_status_bar(ui);
            });

        // Le changement CH0 / CH1 reste un choix d'affichage en Simple ; DUAL utilise
        // le même protocole qu'avant. On n'envoie donc la commande Mode qu'en cas de
        // bascule explicite via le contrôle ci-dessus, sans modifier le backend.
        ctx.request_repaint();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        *self.running.lock().unwrap() = false;
    }
}
