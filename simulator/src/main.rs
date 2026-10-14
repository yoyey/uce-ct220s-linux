use eframe::egui;
use std::f32::consts::PI;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

const REPORT_SIZE: usize = 64;
const COMMAND_REPORT_SIZE: usize = 65;
const POINTS: usize = 512;
const CENTER: f32 = 2048.0;
const REPORTS_PER_CURVE: usize = 32;

/// Tables identiques au logiciel constructeur.
const VOLTAGE_LABELS: [&str; 5] = ["2.5 V", "5.0 V", "10 V", "20 V", "1.0 V"];
const VOLTAGE_NOMINAL_V: [f32; 5] = [2.5, 5.0, 10.0, 20.0, 1.0];
const VOLTAGE_FACTORS: [f32; 5] = [3.125, 6.25, 12.5, 25.0, 1.25];
const RESISTOR_LABELS: [&str; 3] = ["10 K", "1 K", "47 R"];
const RESISTORS_OHM: [f32; 3] = [10_000.0, 1_000.0, 48.1];
const FREQUENCY_TABLE_HZ: [f32; 20] = [
    10.0, 100.0, 500.0, 2000.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 200.0, 300.0,
    400.0, 600.0, 700.0, 800.0, 900.0, 1000.0,
];
const FREQUENCY_LABELS: [&str; 20] = [
    "10 Hz", "100 Hz", "500 Hz", "2 kHz", "20 Hz", "30 Hz", "40 Hz", "50 Hz", "60 Hz", "70 Hz",
    "80 Hz", "90 Hz", "200 Hz", "300 Hz", "400 Hz", "600 Hz", "700 Hz", "800 Hz", "900 Hz", "1 kHz",
];

/// Paramètres électriques d'une trame.
#[derive(Debug, Clone, Copy)]
struct Excitation {
    frequency_hz: f32,
    source_resistance_ohm: f32,
    voltage_index: usize,
}

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

/// État du boîtier simulé, tel qu'il est renvoyé dans la trame F0 FF.
#[derive(Debug, Clone)]
struct DeviceState {
    mode: u8,
    voltage_index: u8,
    frequency_button: u8,
    resistor_index: u8,
    frequency_slots: [u8; 4],
    voltage_scan: bool,
    frequency_scan: bool,
    voltage_mask: u8,
    frequency_mask: u8,
    tolerance: u8,
    overvoltage: bool,
    send_full_config: bool,
}

impl Default for DeviceState {
    fn default() -> Self {
        Self {
            mode: 0,
            voltage_index: 0,
            frequency_button: 0,
            resistor_index: 1,
            frequency_slots: [0, 1, 2, 3],
            voltage_scan: false,
            frequency_scan: false,
            voltage_mask: 0,
            frequency_mask: 0,
            tolerance: 3,
            overvoltage: false,
            send_full_config: true,
        }
    }
}

impl DeviceState {
    fn excitation(&self) -> Excitation {
        let slot = self.frequency_slots[self.frequency_button.min(3) as usize] as usize;
        Excitation {
            frequency_hz: FREQUENCY_TABLE_HZ[slot.min(19)],
            source_resistance_ohm: RESISTORS_OHM[self.resistor_index.min(2) as usize],
            voltage_index: self.voltage_index.min(4) as usize,
        }
    }

    /// Rapport de configuration de 64 octets (sans Report ID) :
    /// payload[k] = data[k + 1] côté logiciel constructeur.
    fn config_report(&self, channel: u8) -> [u8; REPORT_SIZE] {
        let mut p = [0u8; REPORT_SIZE];
        p[0] = 0xF0;
        p[1] = 0xFF;
        p[2] = channel;
        if !self.send_full_config {
            return p;
        }
        let mut set = |data_index: usize, v: u8| p[data_index - 1] = v;
        set(4, self.mode);
        set(5, self.voltage_scan as u8);
        set(6, self.voltage_index);
        set(7, self.voltage_mask);
        set(8, self.frequency_button);
        set(9, self.resistor_index);
        set(10, if self.mode == 1 { 1 } else { 0 });
        set(11, self.tolerance);
        set(12, self.frequency_mask);
        set(13, self.frequency_scan as u8);
        set(47, 0); // résistance de sortie calibrée : 0 Ω
        set(48, 0);
        set(49, 1); // numéro de matériel non nul = vraie configuration
        set(50, self.overvoltage as u8);
        set(56, 12); // firmware V1.12
        set(57, 12); // ADC 12 bits
        set(58, REPORTS_PER_CURVE as u8);
        set(59, 220); // UCE-CT220S
        for (k, b) in 12345u32.to_be_bytes().iter().enumerate() {
            set(60 + k, *b);
        }
        set(64, 0x5A);
        p
    }

    fn next_in_scan(current: u8, count: u8, mask: u8) -> u8 {
        for step in 1..=count {
            let candidate = (current + step) % count;
            if mask & (1 << candidate) == 0 {
                return candidate;
            }
        }
        current
    }

    /// Balayage automatique (une position toutes les 10 trames).
    fn advance_scan(&mut self) {
        if self.voltage_scan {
            self.voltage_index = Self::next_in_scan(self.voltage_index, 5, self.voltage_mask);
            if self.resistor_index == 2 && (self.voltage_index == 2 || self.voltage_index == 3) {
                self.voltage_index = Self::next_in_scan(self.voltage_index, 5, self.voltage_mask | 0b01100);
            }
        }
        if self.frequency_scan {
            self.frequency_button = Self::next_in_scan(self.frequency_button, 4, self.frequency_mask);
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
    device: DeviceState,
    frame_rate_hz: f32,
    frames_sent: u64,
    ch0: ChannelConfig,
    ch1: ChannelConfig,
    last_send: Instant,
    command_count: u64,
    last_command: String,
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
            device: DeviceState::default(),
            frame_rate_hz: 20.0,
            frames_sent: 0,
            ch0: ChannelConfig::default(),
            ch1,
            last_send: Instant::now(),
            command_count: 0,
            last_command: "Aucune commande reçue".to_string(),
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
                        // Pas de set_nonblocking sur un clone : l'option est partagée
                        // avec le socket qui envoie les courbes.
                        let _ = command_stream.set_read_timeout(Some(Duration::from_millis(1)));
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

    /// Lit les commandes au format HID réel : 65 octets, report ID en [0],
    /// commande en [1], paramètres à partir de [2].
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
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => break,
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
        let (description, applied) = apply_command(&mut self.device, report);
        if !applied {
            return;
        }
        self.command_count += 1;
        self.last_command = format!("{:02X} {:02X} — {}", report[1], report[2], description);
        println!("Commande reçue du viewer: {}", self.last_command);
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

    fn generate_frame(channel: u8, cfg: &ChannelConfig, device: &DeviceState) -> Vec<u8> {
        let mut frame = Vec::with_capacity(REPORT_SIZE * (REPORTS_PER_CURVE + 1));
        frame.extend_from_slice(&device.config_report(channel));
        for (i, v) in synthesize(cfg, device.excitation()) {
            push_pair(&mut frame, i, v);
        }
        frame
    }

    fn send_now(&mut self) {
        let mut frames = Vec::new();
        if self.ch0.enabled {
            frames.push(Self::generate_frame(0, &self.ch0, &self.device));
        }
        if self.ch1.enabled {
            frames.push(Self::generate_frame(1, &self.ch1, &self.device));
        }
        let Some(stream) = self.stream.as_mut() else { return };
        let mut send_error = None;
        for frame in frames {
            if let Err(e) = stream.write_all(&frame) {
                send_error = Some(format!("Envoi interrompu: {}", e));
                break;
            }
        }
        if let Some(message) = send_error {
            self.last_error = Some(message);
            self.stream = None;
            self.streaming = false;
            return;
        }
        self.frames_sent += 1;
        if self.frames_sent % 10 == 0 {
            self.device.advance_scan();
        }
    }
}

/// Applique une commande ; renvoie (description, reconnue).
fn apply_command(d: &mut DeviceState, report: &[u8]) -> (String, bool) {
    let v = report[2];
    let desc = match report[1] {
        0xFF => {
            d.mode = v.min(2);
            format!("mode {}", ["Single", "Dual", "OPC"][d.mode as usize])
        }
        0xFB => {
            if v < 3 {
                d.resistor_index = v;
            }
            format!("résistance {}", RESISTOR_LABELS[d.resistor_index as usize])
        }
        0xFC => {
            if v < 4 {
                d.frequency_button = v;
            }
            format!("fréquence {}", FREQUENCY_LABELS[d.frequency_slots[d.frequency_button as usize] as usize])
        }
        0xFD => {
            if v < 5 {
                d.voltage_index = v;
            }
            format!("tension {}", VOLTAGE_LABELS[d.voltage_index as usize])
        }
        0xFE => {
            d.voltage_scan = v != 0;
            format!("balayage tension {}", if d.voltage_scan { "ON" } else { "OFF" })
        }
        0xF4 => {
            d.frequency_scan = v != 0;
            format!("balayage fréquence {}", if d.frequency_scan { "ON" } else { "OFF" })
        }
        0xF9 => {
            d.voltage_mask = v & 0x1F;
            format!("masque tension {:05b}", d.voltage_mask)
        }
        0xF5 => {
            d.frequency_mask = v & 0x0F;
            format!("masque fréquence {:04b}", d.frequency_mask)
        }
        0xF8 => {
            d.tolerance = v;
            format!("tolérance {} %", v)
        }
        0xF7 => {
            for k in 0..4 {
                d.frequency_slots[k] = report[2 + k].min(19);
            }
            let labels: Vec<_> = d.frequency_slots.iter().map(|&s| FREQUENCY_LABELS[s as usize]).collect();
            format!("table de fréquences {}", labels.join(" / "))
        }
        0xF6 => {
            d.overvoltage = false;
            "acquittement surtension".to_string()
        }
        0xFA => "fermeture du logiciel (FA 01)".to_string(),
        _ => return (String::new(), false),
    };
    (desc, true)
}

/// Génère 512 couples (courant, tension) en points ADC.
/// Le boîtier mesure V aux bornes du composant et I via la tension aux bornes
/// de la résistance série, sur la même échelle.
fn synthesize(cfg: &ChannelConfig, ex: Excitation) -> Vec<(f32, f32)> {
    let counts_per_volt = 2048.0 / VOLTAGE_FACTORS[ex.voltage_index];
    let vs_amp = VOLTAGE_NOMINAL_V[ex.voltage_index];
    let rs = ex.source_resistance_ohm.max(0.1);
    let f = ex.frequency_hz.max(0.01);
    let mut out = Vec::with_capacity(POINTS);
    match cfg.component {
        ComponentType::R | ComponentType::L | ComponentType::C => {
            let (r, x) = match cfg.component {
                ComponentType::R => (cfg.resistance_ohm.max(0.0), 0.0),
                ComponentType::L => (cfg.esr_ohm.max(0.0), 2.0 * PI * f * cfg.inductance_mh.max(0.0) / 1000.0),
                _ => (cfg.esr_ohm.max(0.0), -1.0 / (2.0 * PI * f * cfg.capacitance_uf.max(0.0001) * 1.0e-6)),
            };
            // V = Vs·Z/(Z+Rs), I·Rs = Vs·Rs/(Z+Rs) en complexe.
            let (den_re, den_im) = (r + rs, x);
            let den2 = den_re * den_re + den_im * den_im;
            let gv = ((r * den_re + x * den_im) / den2, (x * den_re - r * den_im) / den2);
            let gi = (rs * den_re / den2, -rs * den_im / den2);
            let (av, pv) = ((gv.0 * gv.0 + gv.1 * gv.1).sqrt(), gv.1.atan2(gv.0));
            let (ai, pi) = ((gi.0 * gi.0 + gi.1 * gi.1).sqrt(), gi.1.atan2(gi.0));
            for k in 0..POINTS {
                let t = 2.0 * PI * k as f32 / POINTS as f32;
                let v = CENTER + vs_amp * counts_per_volt * av * (t + pv).sin();
                let i = CENTER + vs_amp * counts_per_volt * ai * (t + pi).sin();
                out.push((i, v));
            }
        }
        ComponentType::D | ComponentType::Dz => {
            for k in 0..POINTS {
                let t = 2.0 * PI * k as f32 / POINTS as f32;
                let vs = vs_amp * t.sin();
                let (vdut, current_a) = solve_nonlinear(vs, rs, cfg);
                out.push((CENTER + current_a * rs * counts_per_volt, CENTER + vdut * counts_per_volt));
            }
        }
    }
    out
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
                ui.label(if self.stream.is_some() { "Connecté" } else { "Déconnecté" });
            });
            if let Some(err) = &self.last_error {
                ui.colored_label(egui::Color32::LIGHT_RED, err);
            }
            ui.label(format!("Commandes TCP reçues : {}", self.command_count));
            ui.monospace(&self.last_command);
            ui.add_space(6.0);

            let d = &mut self.device;
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(egui::RichText::new("État du boîtier simulé").strong());
                ui.horizontal_wrapped(|ui| {
                    ui.label("Mode:");
                    for (k, l) in ["Single", "Dual", "OPC"].iter().enumerate() {
                        ui.radio_value(&mut d.mode, k as u8, *l);
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Fréquence:");
                    for b in 0..4u8 {
                        let label = FREQUENCY_LABELS[d.frequency_slots[b as usize] as usize];
                        ui.radio_value(&mut d.frequency_button, b, label);
                    }
                    ui.checkbox(&mut d.frequency_scan, "balayage");
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Tension:");
                    for idx in [4u8, 0, 1, 2, 3] {
                        ui.radio_value(&mut d.voltage_index, idx, VOLTAGE_LABELS[idx as usize]);
                    }
                    ui.checkbox(&mut d.voltage_scan, "balayage");
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Résistance source:");
                    for idx in [2u8, 1, 0] {
                        ui.radio_value(&mut d.resistor_index, idx, RESISTOR_LABELS[idx as usize]);
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("Tolérance mémorisée : {} %", d.tolerance));
                    ui.checkbox(&mut d.overvoltage, "Simuler une surtension");
                    ui.checkbox(&mut d.send_full_config, "Trame de configuration complète");
                });
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
            ui.small("Le viewer pilote le simulateur avec les mêmes commandes que le boîtier réel (FF mode, FB, FC, FD, FE, F4, F9, F5, F8, F7, F6). La trame F0 FF renvoie l'état complet.");
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
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native("CT220S Simulator", options, Box::new(|_cc| Box::new(SimulatorApp::default())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_layout_and_config() {
        let d = DeviceState { voltage_index: 4, resistor_index: 2, ..Default::default() };
        let f = SimulatorApp::generate_frame(1, &ChannelConfig::default(), &d);
        assert_eq!(f.len(), 33 * REPORT_SIZE);
        assert_eq!(&f[..3], &[0xF0, 0xFF, 1]);
        // data[6] = tension, data[9] = résistance, data[58] = 32 rapports, data[59] = 220.
        assert_eq!(f[5], 4);
        assert_eq!(f[8], 2);
        assert_eq!(f[57], 32);
        assert_eq!(f[58], 220);
        assert_eq!(f[48], 1);
    }

    #[test]
    fn mode_is_ff_and_fa_is_close() {
        let mut d = DeviceState::default();
        let mut r = [0u8; COMMAND_REPORT_SIZE];
        r[1] = 0xFF;
        r[2] = 1;
        apply_command(&mut d, &r);
        assert_eq!(d.mode, 1);
        r[1] = 0xFA;
        r[2] = 1;
        let (desc, ok) = apply_command(&mut d, &r);
        assert!(ok && desc.contains("fermeture"));
        assert_eq!(d.mode, 1);
        r[1] = 0xF7;
        r[2..6].copy_from_slice(&[19, 4, 5, 6]);
        apply_command(&mut d, &r);
        assert_eq!(d.excitation().frequency_hz, 1000.0);
    }

    #[test]
    fn short_and_open_have_expected_spans() {
        let mut cfg = ChannelConfig::default();
        let ex = DeviceState::default().excitation();
        cfg.resistance_ohm = 0.0;
        let pts = synthesize(&cfg, ex);
        let vspan = pts.iter().map(|p| p.1).fold(f32::MIN, f32::max) - pts.iter().map(|p| p.1).fold(f32::MAX, f32::min);
        assert!(vspan < 5.0);
        cfg.resistance_ohm = 1.0e9;
        let pts = synthesize(&cfg, ex);
        let ispan = pts.iter().map(|p| p.0).fold(f32::MIN, f32::max) - pts.iter().map(|p| p.0).fold(f32::MAX, f32::min);
        assert!(ispan < 5.0);
    }

    #[test]
    fn scan_skips_masked_entries() {
        let mut d = DeviceState { voltage_scan: true, voltage_mask: 0b11110, ..Default::default() };
        d.advance_scan();
        assert_eq!(d.voltage_index, 0);
    }
}
