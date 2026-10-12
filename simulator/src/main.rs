use eframe::egui;
use std::f32::consts::PI;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

const REPORT_SIZE: usize = 64;
const COMMAND_REPORT_SIZE: usize = 65;
const POINTS: usize = 512;
const CENTER: f32 = 2048.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComponentType {
    R,
    L,
    C,
    D,
    Dz,
}

#[derive(Debug, Clone)]
struct ChannelConfig {
    enabled: bool,
    component: ComponentType,
    resistance_ohm: f32,
    inductance_mh: f32,
    capacitance_uf: f32,
    esr_ohm: f32,
    diode_vf: f32,
    diode_rd_ohm: f32,
    zener_v: f32,
    zener_rd_ohm: f32,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            component: ComponentType::R,
            resistance_ohm: 100.0,
            inductance_mh: 100.0,
            capacitance_uf: 470.0,
            esr_ohm: 6.0,
            diode_vf: 0.7,
            diode_rd_ohm: 10.0,
            zener_v: 5.1,
            zener_rd_ohm: 20.0,
        }
    }
}

struct SimulatorApp {
    address: String,
    stream: Option<TcpStream>,
    command_stream: Option<TcpStream>,
    command_rx: Vec<u8>,
    last_error: Option<String>,
    streaming: bool,
    frequency_hz: f32,
    source_resistance_ohm: f32,
    voltage_range_v: f32,
    mode_index: u8,
    frame_rate_hz: f32,
    ch0: ChannelConfig,
    ch1: ChannelConfig,
    last_send: Instant,
}

impl Default for SimulatorApp {
    fn default() -> Self {
        let mut ch1 = ChannelConfig::default();
        ch1.component = ComponentType::C;
        Self {
            address: "127.0.0.1:3000".to_string(),
            stream: None,
            command_stream: None,
            command_rx: Vec::new(),
            last_error: None,
            streaming: false,
            frequency_hz: 10.0,
            source_resistance_ohm: 1_000.0,
            voltage_range_v: 2.5,
            mode_index: 0,
            frame_rate_hz: 20.0,
            ch0: ChannelConfig::default(),
            ch1,
            last_send: Instant::now(),
        }
    }
}

impl SimulatorApp {
    fn connect(&mut self) {
        match TcpStream::connect(&self.address) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                match stream.try_clone() {
                    Ok(command_stream) => {
                        let _ = command_stream.set_nonblocking(true);
                        let _ = command_stream.set_nodelay(true);
                        self.command_stream = Some(command_stream);
                        self.stream = Some(stream);
                        self.command_rx.clear();
                        self.last_error = None;
                    }
                    Err(e) => {
                        self.stream = None;
                        self.command_stream = None;
                        self.last_error = Some(format!("Clone socket impossible: {}", e));
                    }
                }
            }
            Err(e) => {
                self.stream = None;
                self.command_stream = None;
                self.last_error = Some(format!("Connexion impossible: {}", e));
            }
        }
    }

    fn disconnect(&mut self) {
        self.stream = None;
        self.command_stream = None;
        self.command_rx.clear();
        self.streaming = false;
    }

    /// Lit les commandes exactement dans le format HID réel :
    /// 65 octets, report ID en [0], FA/FB/FC/FD en [1], index en [2].
    fn poll_commands(&mut self) {
        let mut disconnect_reason: Option<String> = None;

        {
            let Some(stream) = self.command_stream.as_mut() else {
                return;
            };

            let mut tmp = [0u8; 512];
            loop {
                match stream.read(&mut tmp) {
                    Ok(0) => {
                        disconnect_reason = Some("Viewer déconnecté".to_string());
                        break;
                    }
                    Ok(n) => self.command_rx.extend_from_slice(&tmp[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => {
                        disconnect_reason = Some(format!("Lecture commande interrompue: {}", e));
                        break;
                    }
                }
            }
        }

        if let Some(reason) = disconnect_reason {
            self.last_error = Some(reason);
            self.disconnect();
            return;
        }

        while self.command_rx.len() >= COMMAND_REPORT_SIZE {
            let report: Vec<u8> = self.command_rx.drain(..COMMAND_REPORT_SIZE).collect();
            self.apply_command_report(&report);
        }
    }

    fn apply_command_report(&mut self, report: &[u8]) {
        if report.len() != COMMAND_REPORT_SIZE || report[0] != 0 {
            return;
        }

        let prefix = report[1];
        let index = report[2];

        match prefix {
            // FC : fréquence, mapping déjà utilisé par l'application réelle
            0xFC => {
                self.frequency_hz = match index {
                    0 => 10.0,
                    1 => 100.0,
                    2 => 500.0,
                    3 => 2_000.0,
                    _ => self.frequency_hz,
                };
            }
            // FB : résistance source, mapping conservé tel quel
            0xFB => {
                self.source_resistance_ohm = match index {
                    0 => 10_000.0,
                    1 => 1_000.0,
                    2 => 47.0,
                    _ => self.source_resistance_ohm,
                };
            }
            // FA : mode. On mémorise uniquement l'index reçu ; les cases CH0/CH1
            // du simulateur restent indépendantes pour permettre tous les tests.
            0xFA => {
                self.mode_index = index;
            }
            // FD : plage de tension, mapping déjà utilisé par l'application réelle
            0xFD => {
                self.voltage_range_v = match index {
                    0 => 2.5,
                    1 => 5.0,
                    2 => 10.0,
                    3 => 20.0,
                    _ => self.voltage_range_v,
                };
            }
            _ => {}
        }

        println!(
            "Commande reçue du viewer: {:02X} {:02X} | mode={} f={}Hz Rsrc={}Ω V={}V",
            prefix, index, self.mode_index, self.frequency_hz, self.source_resistance_ohm, self.voltage_range_v
        );
    }

    fn component_ui(ui: &mut egui::Ui, name: &str, cfg: &mut ChannelConfig) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut cfg.enabled, "");
                ui.heading(name);
            });
            ui.separator();

            ui.horizontal_wrapped(|ui| {
                ui.radio_value(&mut cfg.component, ComponentType::R, "R");
                ui.radio_value(&mut cfg.component, ComponentType::L, "L");
                ui.radio_value(&mut cfg.component, ComponentType::C, "C");
                ui.radio_value(&mut cfg.component, ComponentType::D, "D");
                ui.radio_value(&mut cfg.component, ComponentType::Dz, "Dz");
            });

            ui.add_space(6.0);
            match cfg.component {
                ComponentType::R => {
                    ui.label("Résistance");
                    ui.add(egui::Slider::new(&mut cfg.resistance_ohm, 0.0..=10_000.0).text("Ω"));
                    ui.add(egui::DragValue::new(&mut cfg.resistance_ohm).speed(1.0).suffix(" Ω"));
                }
                ComponentType::L => {
                    ui.label("Inductance");
                    ui.add(egui::Slider::new(&mut cfg.inductance_mh, 0.0..=1_000.0).text("mH"));
                    ui.add(egui::DragValue::new(&mut cfg.inductance_mh).speed(1.0).suffix(" mH"));
                    ui.label("Résistance série");
                    ui.add(egui::DragValue::new(&mut cfg.esr_ohm).speed(0.1).suffix(" Ω"));
                }
                ComponentType::C => {
                    ui.label("Capacité");
                    ui.add(egui::Slider::new(&mut cfg.capacitance_uf, 0.1..=10_000.0).logarithmic(true).text("µF"));
                    ui.add(egui::DragValue::new(&mut cfg.capacitance_uf).speed(1.0).suffix(" µF"));
                    ui.label("ESR");
                    ui.add(egui::Slider::new(&mut cfg.esr_ohm, 0.0..=100.0).text("Ω"));
                    ui.add(egui::DragValue::new(&mut cfg.esr_ohm).speed(0.1).suffix(" Ω"));
                }
                ComponentType::D => {
                    ui.label("Diode - seuil direct");
                    ui.add(egui::DragValue::new(&mut cfg.diode_vf).speed(0.01).suffix(" V"));
                    ui.label("Résistance dynamique");
                    ui.add(egui::DragValue::new(&mut cfg.diode_rd_ohm).speed(0.5).suffix(" Ω"));
                }
                ComponentType::Dz => {
                    ui.label("Zener - tension Vz");
                    ui.add(egui::DragValue::new(&mut cfg.zener_v).speed(0.1).suffix(" V"));
                    ui.label("Seuil direct");
                    ui.add(egui::DragValue::new(&mut cfg.diode_vf).speed(0.01).suffix(" V"));
                    ui.label("Résistance dynamique Zener");
                    ui.add(egui::DragValue::new(&mut cfg.zener_rd_ohm).speed(0.5).suffix(" Ω"));
                }
            }
        });
    }

    fn generate_frame(
        channel: u8,
        cfg: &ChannelConfig,
        frequency_hz: f32,
        source_resistance_ohm: f32,
        voltage_range_v: f32,
    ) -> Vec<u8> {
        let mut frame = Vec::with_capacity(REPORT_SIZE * 33);
        let mut header = [0u8; REPORT_SIZE];
        header[0] = 0xF0;
        header[1] = 0xFF;
        header[2] = channel;
        frame.extend_from_slice(&header);

        let f = frequency_hz.max(0.01);
        let rsrc = source_resistance_ohm.max(0.1);
        let vrange = voltage_range_v.max(0.1);

        match cfg.component {
            ComponentType::R | ComponentType::L | ComponentType::C => {
                let (r, x) = match cfg.component {
                    ComponentType::R => (cfg.resistance_ohm.max(0.0), 0.0),
                    ComponentType::L => {
                        let l_h = cfg.inductance_mh.max(0.0) / 1000.0;
                        (cfg.esr_ohm.max(0.0), 2.0 * PI * f * l_h)
                    }
                    ComponentType::C => {
                        let c_f = (cfg.capacitance_uf.max(0.0001)) * 1.0e-6;
                        (cfg.esr_ohm.max(0.0), -1.0 / (2.0 * PI * f * c_f))
                    }
                    _ => unreachable!(),
                };

                let z = (r * r + x * x).sqrt();
                let phase = x.atan2(r.max(0.0));
                let ratio = z / rsrc;

                // Garde les deux voies dans la plage ADC tout en conservant Av/Ai.
                let amp_i = 1500.0 / ratio.max(1.0);
                let amp_v = amp_i * ratio;

                for k in 0..POINTS {
                    let theta = 2.0 * PI * (k as f32) / (POINTS as f32);
                    let i = CENTER + amp_i * theta.cos();
                    let v = CENTER + amp_v * (theta + phase).cos();
                    push_pair(&mut frame, i, v);
                }
            }
            ComponentType::D | ComponentType::Dz => {
                // Modèle piècewise simple mais utile pour tester la forme V-I :
                // Vs = Vdut + I*Rsource. La voie courant encode I*Rsource,
                // afin de garder la même convention que les captures du CT220S.
                let counts_per_volt = 1500.0 / vrange;
                for k in 0..POINTS {
                    let theta = 2.0 * PI * (k as f32) / (POINTS as f32);
                    let vs = vrange * theta.sin();
                    let (vdut, current_a) = solve_nonlinear(vs, rsrc, cfg);
                    let current_equiv_v = current_a * rsrc;
                    let i = CENTER + current_equiv_v * counts_per_volt;
                    let v = CENTER + vdut * counts_per_volt;
                    push_pair(&mut frame, i, v);
                }
            }
        }

        frame
    }

    fn send_now(&mut self) {
        let mut frames = Vec::new();
        if self.ch0.enabled {
            frames.push(Self::generate_frame(
                0,
                &self.ch0,
                self.frequency_hz,
                self.source_resistance_ohm,
                self.voltage_range_v,
            ));
        }
        if self.ch1.enabled {
            frames.push(Self::generate_frame(
                1,
                &self.ch1,
                self.frequency_hz,
                self.source_resistance_ohm,
                self.voltage_range_v,
            ));
        }

        let mut send_error = None;
        if let Some(stream) = self.stream.as_mut() {
            for frame in frames {
                if let Err(e) = stream.write_all(&frame) {
                    send_error = Some(format!("Envoi interrompu: {}", e));
                    break;
                }
            }
        } else {
            return;
        }

        if let Some(message) = send_error {
            self.last_error = Some(message);
            self.stream = None;
            self.streaming = false;
        }
    }
}

impl eframe::App for SimulatorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_commands();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Simulateur boîtier CT220S");
            ui.small("Envoie des trames CT220S simulées au viewer via TCP.");
            ui.separator();

            ui.horizontal(|ui| {
                ui.label("Viewer:");
                ui.text_edit_singleline(&mut self.address);
                if self.stream.is_none() {
                    if ui.button("Connecter").clicked() {
                        self.connect();
                    }
                } else if ui.button("Déconnecter").clicked() {
                    self.disconnect();
                }

                let status = if self.stream.is_some() { "Connecté" } else { "Déconnecté" };
                ui.label(status);
            });

            if let Some(err) = &self.last_error {
                ui.colored_label(egui::Color32::LIGHT_RED, err);
            }

            ui.add_space(6.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(egui::RichText::new("Configuration du test").strong());
                ui.horizontal_wrapped(|ui| {
                    ui.label("Fréquence:");
                    ui.radio_value(&mut self.frequency_hz, 10.0, "10 Hz");
                    ui.radio_value(&mut self.frequency_hz, 100.0, "100 Hz");
                    ui.radio_value(&mut self.frequency_hz, 500.0, "500 Hz");
                    ui.radio_value(&mut self.frequency_hz, 2000.0, "2 kHz");
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Tension:");
                    ui.radio_value(&mut self.voltage_range_v, 2.5, "2.5 V");
                    ui.radio_value(&mut self.voltage_range_v, 5.0, "5 V");
                    ui.radio_value(&mut self.voltage_range_v, 10.0, "10 V");
                    ui.radio_value(&mut self.voltage_range_v, 20.0, "20 V");
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Résistance source:");
                    ui.radio_value(&mut self.source_resistance_ohm, 47.0, "47 R");
                    ui.radio_value(&mut self.source_resistance_ohm, 1000.0, "1 K");
                    ui.radio_value(&mut self.source_resistance_ohm, 10000.0, "10 K");
                });
                ui.label(format!(
                    "Mode reçu du viewer: {} (FA {:02X})",
                    if self.mode_index == 1 { "DUAL" } else { "SIMPLE" },
                    self.mode_index
                ));
            });

            ui.add_space(8.0);
            ui.columns(2, |cols| {
                Self::component_ui(&mut cols[0], "CH0", &mut self.ch0);
                Self::component_ui(&mut cols[1], "CH1", &mut self.ch1);
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut self.frame_rate_hz, 1.0..=50.0).text("trames/s"));
                if self.stream.is_some() {
                    if ui.button(if self.streaming { "Pause" } else { "Démarrer" }).clicked() {
                        self.streaming = !self.streaming;
                        self.last_send = Instant::now();
                    }
                    if ui.button("Envoyer une fois").clicked() {
                        self.send_now();
                    }
                } else {
                    ui.add_enabled(false, egui::Button::new("Démarrer"));
                    ui.add_enabled(false, egui::Button::new("Envoyer une fois"));
                }
            });

            ui.small("La fréquence, la résistance source, la tension et le mode peuvent être pilotés directement par le viewer avec les mêmes trames FA/FB/FC/FD que le boîtier réel.");
        });

        if self.streaming && self.stream.is_some() {
            let period = Duration::from_secs_f32(1.0 / self.frame_rate_hz.max(1.0));
            if self.last_send.elapsed() >= period {
                self.send_now();
                self.last_send = Instant::now();
            }
            ctx.request_repaint_after(Duration::from_millis(10));
        } else {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

fn solve_nonlinear(vs: f32, rsrc: f32, cfg: &ChannelConfig) -> (f32, f32) {
    let vf = cfg.diode_vf.max(0.0);
    let rd = cfg.diode_rd_ohm.max(0.1);

    if vs > vf {
        let i = (vs - vf) / (rsrc + rd);
        return (vf + i * rd, i);
    }

    if cfg.component == ComponentType::Dz {
        let vz = cfg.zener_v.max(0.1);
        let rz = cfg.zener_rd_ohm.max(0.1);
        if vs < -vz {
            let i = (vs + vz) / (rsrc + rz); // courant négatif
            return (-vz + i * rz, i);
        }
    }

    // Bloqué : aucun courant, la tension du DUT suit la source.
    (vs, 0.0)
}

fn push_pair(frame: &mut Vec<u8>, current: f32, voltage: f32) {
    let i = current.round().clamp(0.0, 4095.0) as u16;
    let v = voltage.round().clamp(0.0, 4095.0) as u16;
    frame.extend_from_slice(&i.to_le_bytes());
    frame.extend_from_slice(&v.to_le_bytes());
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 650.0]),
        ..Default::default()
    };

    eframe::run_native(
        "CT220S Simulator",
        options,
        Box::new(|_cc| Box::new(SimulatorApp::default())),
    )
}
