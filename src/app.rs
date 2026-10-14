// src/app.rs
//
// Interface principale. Reprend l'ergonomie et les fonctions du logiciel
// constructeur PC_CT22X : Basic / Advanced / base de données / Debug / About.

use crate::analysis::{
    self, format_difference, ChannelAnalyzer, ImpedanceComparison, MeasurementParams, ProbeSession,
    SessionEvent, VendorMeasurement,
};
use crate::backend::{run_file_reader, run_hid_reader, run_tcp_reader, send_tcp_cmd, Command, HidBackend};
use crate::board::BoardImage;
use crate::config::*;
use crate::ct220s_vendor::{adc_to_physical, format_current_ma, VendorConfig};
use crate::curve::{
    calculate_electrical_values, format_capacitance, format_inductance, format_resistance, CurveData,
    DualCurveData,
};
use crate::database::{TestDatabase, TestPoint, TestRecord};
use crate::image_export::{render_xy, save_curve_text, save_image, PlotScale, Trace};
use crate::report::{save_report, ReportInfo};
use crate::sound::{self, Beep};

use eframe::egui;
use egui::{Color32, Pos2, Rect, Stroke};
use std::net::TcpStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Pas de resynchronisation depuis le boîtier pendant ce délai après une
/// commande locale (équivalent du ComTimeout constructeur).
const LOCAL_COMMAND_HOLD: Duration = Duration::from_millis(1200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    Ch0,
    Ch1,
    Dual,
    Opc,
}

impl DisplayMode {
    fn device_mode(self) -> u8 {
        match self {
            DisplayMode::Ch0 | DisplayMode::Ch1 => 0,
            DisplayMode::Dual => 1,
            DisplayMode::Opc => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    Basic,
    Advanced,
    Database,
    Debug,
    About,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Xy,
    Xt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Algorithm {
    Vendor,
    Dft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    File,
    Tcp,
    Usb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WarningKind {
    Info,
    Overvoltage,
}

struct Warning {
    title: String,
    text: String,
    kind: WarningKind,
}

#[derive(Debug, Clone, Copy)]
enum Cursor {
    Xy { adc_v: f64, adc_i: f64 },
    Xt { sample: usize },
}

pub struct CT220SApp {
    curve_data: Arc<Mutex<DualCurveData>>,
    status: Arc<Mutex<Option<String>>>,
    running: Arc<AtomicBool>,
    transport: Transport,
    file_path: String,
    tcp_address: String,
    hid: Option<HidBackend>,
    tcp_command_stream: Arc<Mutex<Option<TcpStream>>>,

    // Réglages (miroir de l'état du boîtier).
    display_mode: DisplayMode,
    voltage_index: u8,
    freq_button: u8,
    freq_slots: [u8; 4],
    resistor_index: u8,
    voltage_scan: bool,
    frequency_scan: bool,
    voltage_mask: u8,
    frequency_mask: u8,
    tolerance_pct: u8,
    last_local_command: Option<Instant>,

    // Affichage.
    active_tab: SettingsTab,
    view: ViewMode,
    scale: PlotScale,
    algorithm: Algorithm,
    cursor: Option<Cursor>,
    debug: bool,
    debug_current_harmonics: bool,

    // Dernières données reçues.
    live0: Option<CurveData>,
    live1: Option<CurveData>,
    device: Option<VendorConfig>,
    seen_config_seq: u64,
    fps: u32,
    fps_frames: u64,
    fps_time: Instant,

    // Mesures constructeur.
    analyzers: [ChannelAnalyzer; 2],
    measurements: [Option<VendorMeasurement>; 2],
    analyzed_seq: [u64; 2],
    session: ProbeSession,
    verdict: Option<bool>,
    impedance_label: String,

    // Comparaison rapide (références locales).
    reference_ch0: Option<CurveData>,
    reference_ch1: Option<CurveData>,
    show_reference: bool,
    reference_file: String,
    audible_warning: bool,

    // Base de données / test automatique.
    db: TestDatabase,
    db_mode: bool,
    db_reference: Option<CurveData>,
    auto_test: bool,
    next_point_pending: bool,
    point_name: String,
    auto_increment: bool,
    increment_counter: u32,
    report_path: String,
    board_path: String,
    board: Option<BoardImage>,
    show_board: bool,
    board_zoom: f32,
    board_show_results: bool,
    marker: (i32, i32),

    // Divers.
    image_path: String,
    text_path: String,
    show_freq_table: bool,
    freq_table_edit: [u8; 4],
    warnings: Vec<Warning>,
    overvoltage_ack: Option<Instant>,
    warned_serial: Option<u32>,
    last_command_status: String,
    was_tcp_connected: bool,
}

impl CT220SApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, file_arg: Option<String>, tcp_arg: Option<String>) -> Self {
        let curve_data = Arc::new(Mutex::new(DualCurveData::new()));
        let status = Arc::new(Mutex::new(None));
        let running = Arc::new(AtomicBool::new(true));
        let tcp_command_stream: Arc<Mutex<Option<TcpStream>>> = Arc::new(Mutex::new(None));

        // Priorité des sources : fichier explicite > TCP explicite > USB.
        // Sans argument, un capture.txt présent dans le dossier courant est rejoué.
        let (transport, file_path, tcp_address) = if let Some(path) = file_arg {
            (Transport::File, path, "127.0.0.1:3000".to_string())
        } else if let Some(address) = tcp_arg {
            (Transport::Tcp, "capture.txt".to_string(), address)
        } else if Path::new("capture.txt").exists() {
            (Transport::File, "capture.txt".to_string(), "127.0.0.1:3000".to_string())
        } else {
            (Transport::Usb, "capture.txt".to_string(), "127.0.0.1:3000".to_string())
        };

        let (cd, st, rn) = (Arc::clone(&curve_data), Arc::clone(&status), Arc::clone(&running));
        let mut hid = None;
        match transport {
            Transport::File => {
                let path = file_path.clone();
                thread::spawn(move || {
                    println!("Mode fichier: lecture de {}", path);
                    if let Err(e) = run_file_reader(&path, cd, st, rn) {
                        eprintln!("Erreur lecture fichier: {}", e);
                    }
                });
            }
            Transport::Tcp => {
                let address = tcp_address.clone();
                let cs = Arc::clone(&tcp_command_stream);
                thread::spawn(move || {
                    if let Err(e) = run_tcp_reader(&address, cd, Arc::clone(&st), rn, cs) {
                        eprintln!("Erreur TCP reader: {}", e);
                        if let Ok(mut s) = st.lock() {
                            *s = Some(e);
                        }
                    }
                });
            }
            Transport::Usb => {
                let backend = HidBackend::new();
                let device = backend.device_handle();
                thread::spawn(move || {
                    if let Err(e) = run_hid_reader(device, cd, Arc::clone(&st), rn) {
                        eprintln!("Erreur HID reader: {}", e);
                        if let Ok(mut s) = st.lock() {
                            *s = Some(e);
                        }
                    }
                });
                hid = Some(backend);
            }
        }

        Self {
            curve_data,
            status,
            running,
            transport,
            file_path,
            tcp_address,
            hid,
            tcp_command_stream,
            display_mode: DisplayMode::Ch0,
            voltage_index: 0,
            freq_button: 0,
            freq_slots: DEFAULT_FREQUENCY_SLOTS,
            resistor_index: 1,
            voltage_scan: false,
            frequency_scan: false,
            voltage_mask: 0,
            frequency_mask: 0,
            tolerance_pct: 3,
            last_local_command: None,
            active_tab: SettingsTab::Basic,
            view: ViewMode::Xy,
            scale: PlotScale::Fixed,
            algorithm: Algorithm::Vendor,
            cursor: None,
            debug: false,
            debug_current_harmonics: false,
            live0: None,
            live1: None,
            device: None,
            seen_config_seq: 0,
            fps: 0,
            fps_frames: 0,
            fps_time: Instant::now(),
            analyzers: [ChannelAnalyzer::new(true), ChannelAnalyzer::new(false)],
            measurements: [None, None],
            analyzed_seq: [0, 0],
            session: ProbeSession::new(),
            verdict: None,
            impedance_label: String::new(),
            reference_ch0: None,
            reference_ch1: None,
            show_reference: true,
            reference_file: "ct220s_references.csv".to_string(),
            audible_warning: sound::AVAILABLE,
            db: TestDatabase::new(),
            db_mode: false,
            db_reference: None,
            auto_test: false,
            next_point_pending: false,
            point_name: "Test Point".to_string(),
            auto_increment: true,
            increment_counter: 1,
            report_path: "rapport_test.pdf".to_string(),
            board_path: "carte.jpg".to_string(),
            board: None,
            show_board: false,
            board_zoom: 1.0,
            board_show_results: false,
            marker: (0, 0),
            image_path: "courbe.png".to_string(),
            text_path: "courbe.txt".to_string(),
            show_freq_table: false,
            freq_table_edit: DEFAULT_FREQUENCY_SLOTS,
            warnings: Vec::new(),
            overvoltage_ack: None,
            warned_serial: None,
            last_command_status: "Aucune commande envoyée".to_string(),
            was_tcp_connected: false,
        }
    }

    // -----------------------------------------------------------------------
    // Commandes
    // -----------------------------------------------------------------------

    fn set_status(&self, msg: impl Into<String>) {
        if let Ok(mut s) = self.status.lock() {
            *s = Some(msg.into());
        }
    }

    fn send(&mut self, cmd: Command, msg: &str) {
        self.last_local_command = Some(Instant::now());
        self.last_command_status = format!("{} ({:?})", msg, cmd);
        let result = match self.transport {
            Transport::File => {
                // En mode fichier, les commandes ne pilotent que l'état local.
                self.set_status(format!("Simulation fichier: {}", msg));
                return;
            }
            Transport::Tcp => send_tcp_cmd(&self.tcp_command_stream, cmd),
            Transport::Usb => match &self.hid {
                Some(h) => h.send_cmd(cmd),
                None => Err("Boîtier USB non connecté".to_string()),
            },
        };
        match result {
            Ok(()) => self.set_status(msg.to_string()),
            Err(e) => self.set_status(format!("Erreur commande: {}", e)),
        }
    }

    fn is_connected(&self) -> bool {
        match self.transport {
            Transport::File => true,
            Transport::Tcp => self.tcp_command_stream.lock().map(|s| s.is_some()).unwrap_or(false),
            Transport::Usb => self.hid.as_ref().map(|h| h.is_connected()).unwrap_or(false),
        }
    }

    fn sync_all_to_device(&mut self) {
        let mode = self.display_mode.device_mode();
        self.send(Command::SetMode(mode), "Sync mode");
        self.send(Command::SetRes(self.resistor_index), "Sync résistance");
        self.send(Command::SetFreq(self.freq_button), "Sync fréquence");
        self.send(Command::SetVolt(self.voltage_index), "Sync tension");
        self.send(Command::SetVoltageScan(self.voltage_scan), "Sync balayage tension");
        self.send(Command::SetFrequencyScan(self.frequency_scan), "Sync balayage fréquence");
        self.send(Command::SetVoltageScanMask(self.voltage_mask), "Sync masque tension");
        self.send(Command::SetFrequencyScanMask(self.frequency_mask), "Sync masque fréquence");
        self.send(Command::SetTolerance(self.tolerance_pct), "Sync tolérance");
    }

    /// 47 R est interdit avec les calibres 10 V et 20 V (règle constructeur).
    fn voltage_allowed(&self, index: u8) -> bool {
        !(self.resistor_index == 2 && (index == 2 || index == 3))
    }

    fn resistor_allowed(&self, index: u8) -> bool {
        !(index == 2 && (self.voltage_index == 2 || self.voltage_index == 3))
    }

    fn set_voltage(&mut self, index: u8) {
        if self.voltage_index == index || !self.voltage_allowed(index) {
            return;
        }
        self.voltage_index = index;
        self.cursor = None;
        let msg = format!("Tension: {}", VOLTAGE_LABELS[index as usize]);
        self.send(Command::SetVolt(index), &msg);
    }

    fn set_frequency(&mut self, button: u8) {
        if self.freq_button == button {
            return;
        }
        self.freq_button = button;
        let msg = format!("Fréquence: {}", frequency_label(self.freq_slots[button as usize]));
        self.send(Command::SetFreq(button), &msg);
    }

    fn set_resistor(&mut self, index: u8) {
        if self.resistor_index == index || !self.resistor_allowed(index) {
            return;
        }
        self.resistor_index = index;
        self.cursor = None;
        let msg = format!("Résistance: {}", RESISTOR_LABELS[index as usize]);
        self.send(Command::SetRes(index), &msg);
    }

    fn set_display_mode(&mut self, mode: DisplayMode) {
        if self.display_mode == mode {
            return;
        }
        let before = self.display_mode.device_mode();
        self.display_mode = mode;
        self.verdict = None;
        self.impedance_label.clear();
        self.session.reset();
        if mode.device_mode() != before {
            let msg = match mode {
                DisplayMode::Dual => "Mode: Dual",
                DisplayMode::Opc => "Mode: OPC",
                _ => "Mode: Single",
            };
            self.send(Command::SetMode(mode.device_mode()), msg);
        }
    }

    fn output_resistance_kohm(&self) -> f64 {
        self.device.as_ref().filter(|c| c.is_hardware()).map(|c| c.output_resistance_kohm).unwrap_or(0.0)
    }

    fn total_resistor_kohm(&self) -> f64 {
        resistor_kohm(self.resistor_index) + self.output_resistance_kohm()
    }

    fn current_frequency_hz(&self) -> f64 {
        frequency_hz(self.freq_slots[self.freq_button.min(3) as usize])
    }

    fn params(&self) -> MeasurementParams {
        MeasurementParams { frequency_hz: self.current_frequency_hz(), resistor_kohm: self.total_resistor_kohm() }
    }
}

// ---------------------------------------------------------------------------
// Traitement des données reçues
// ---------------------------------------------------------------------------

impl CT220SApp {
    fn compare_active(&self) -> bool {
        if self.db_mode {
            return self.auto_test;
        }
        let hw = self.device.as_ref().filter(|c| c.is_hardware());
        match self.display_mode {
            DisplayMode::Dual => match hw {
                Some(c) => c.dual_mode2 == 1 && (c.scan != 1 || c.voltage_scan_mask != 31),
                None => true,
            },
            DisplayMode::Opc => match hw {
                Some(c) => c.opc_flag == c.opc_meas.wrapping_add(2),
                None => false,
            },
            _ => false,
        }
    }

    /// Courbe utilisée comme seconde voie : référence en mode base de
    /// données, CH1 sinon.
    fn second_curve(&self) -> Option<&CurveData> {
        if self.db_mode {
            self.db_reference.as_ref()
        } else {
            self.live1.as_ref()
        }
    }

    fn process_incoming(&mut self) {
        let snapshot = self.curve_data.lock().ok().map(|d| {
            (d.channel0.clone(), d.channel1.clone(), d.config.clone(), d.config_seq, d.frame_count)
        });
        let Some((ch0, ch1, cfg, cfg_seq, frames)) = snapshot else { return };

        if self.fps_time.elapsed() >= Duration::from_secs(1) {
            self.fps = frames.saturating_sub(self.fps_frames) as u32;
            self.fps_frames = frames;
            self.fps_time = Instant::now();
        }

        self.live0 = ch0;
        self.live1 = ch1;
        if cfg_seq != self.seen_config_seq {
            self.seen_config_seq = cfg_seq;
            if let Some(c) = cfg {
                self.on_config(c);
            }
        }

        let params = self.params();
        let seq0 = self.live0.as_ref().map(|c| c.seq).unwrap_or(0);
        let seq1 = self.live1.as_ref().map(|c| c.seq).unwrap_or(0);
        let new0 = seq0 != 0 && seq0 != self.analyzed_seq[0];
        let new1 = !self.db_mode && seq1 != 0 && seq1 != self.analyzed_seq[1];

        if new0 {
            self.analyzed_seq[0] = seq0;
            if let Some(c) = self.live0.as_ref().filter(|c| c.has_raw()) {
                self.measurements[0] = Some(self.analyzers[0].analyze(&c.raw_voltage, &c.raw_current, params));
            }
            if self.db_mode {
                let m = self
                    .db_reference
                    .as_ref()
                    .filter(|c| c.has_raw())
                    .map(|c| (c.raw_voltage.clone(), c.raw_current.clone()));
                self.measurements[1] = m.map(|(v, i)| self.analyzers[1].analyze(&v, &i, params));
            }
        }
        if new1 {
            self.analyzed_seq[1] = seq1;
            if let Some(c) = self.live1.as_ref().filter(|c| c.has_raw()) {
                self.measurements[1] = Some(self.analyzers[1].analyze(&c.raw_voltage, &c.raw_current, params));
            }
        }
        if !(new0 || new1) {
            return;
        }

        // Machine d'état « contact sonde » (DrawChart / calculate_RLC).
        let mut lifted = false;
        if let Some(m) = self.measurements[0] {
            lifted |= self.session.update_status(0, m.status);
        }
        let active = self.compare_active();
        if active {
            if let Some(m) = self.measurements[1] {
                lifted |= self.session.update_status(1, m.status);
            }
        }
        if lifted {
            self.verdict = None;
            if active {
                self.impedance_label.clear();
            }
            if self.next_point_pending {
                self.next_point_pending = false;
                self.advance_point();
            }
        }
        if !active {
            self.impedance_label.clear();
            return;
        }

        let tolerance = self.tolerance_pct as f64;
        let compare_input = match (&self.live0, self.second_curve(), self.measurements[0], self.measurements[1]) {
            (Some(a), Some(b), Some(ma), Some(mb)) if a.has_raw() && b.has_raw() => {
                Some((a.raw_voltage.clone(), a.raw_current.clone(), ma, b.raw_voltage.clone(), b.raw_current.clone(), mb))
            }
            _ => None,
        };
        let Some((av, ai, ma, bv, bi, mb)) = compare_input else { return };
        let event = self.session.step(tolerance, || analysis::compare(&av, &ai, &ma, &bv, &bi, &mb));
        if let Some(c) = self.session.last_comparison {
            self.impedance_label = c.label();
        }
        if let SessionEvent::Verdict { passed, comparison } = event {
            self.on_verdict(passed, comparison, av, ai, bv, bi);
        }
    }

    fn on_verdict(&mut self, passed: bool, cmp: ImpedanceComparison, av: Vec<u16>, ai: Vec<u16>, bv: Vec<u16>, bi: Vec<u16>) {
        self.verdict = Some(passed);
        self.impedance_label = cmp.label();
        if self.audible_warning {
            sound::play(if passed { Beep::Pass } else { Beep::Fail });
        }
        if self.db_mode && self.auto_test {
            if let Some(idx) = self.db.selected {
                let (name, x, y) = self
                    .db
                    .points
                    .get(idx)
                    .map(|p| (p.name.clone(), p.x, p.y))
                    .unwrap_or_default();
                self.db.record_result(idx, passed);
                let number = self.db.records.len() + 1;
                self.db.records.push(TestRecord {
                    number,
                    point_index: idx,
                    name,
                    passed,
                    difference_pct: cmp.difference_pct,
                    tolerance_pct: self.tolerance_pct as f64,
                    time: chrono::Local::now().format("%H:%M:%S").to_string(),
                    live_voltage: av,
                    live_current: ai,
                    ref_voltage: bv,
                    ref_current: bi,
                    x,
                    y,
                });
            }
            // Comme le constructeur : on passe au point suivant au prochain
            // relevé de sonde, uniquement si le test est conforme.
            self.next_point_pending = passed;
        }
    }

    fn on_config(&mut self, c: VendorConfig) {
        if c.overvoltage {
            let pending = self.warnings.iter().any(|w| w.kind == WarningKind::Overvoltage);
            let recently_ack = self.overvoltage_ack.map_or(false, |t| t.elapsed() < Duration::from_secs(2));
            if !pending && !recently_ack {
                self.warnings.push(Warning {
                    title: "Attention".into(),
                    text: "Haute tension détectée !\nArrêtez la mesure.".into(),
                    kind: WarningKind::Overvoltage,
                });
            }
        }
        if c.is_hardware() && self.warned_serial != Some(c.serial) {
            self.warned_serial = Some(c.serial);
            if c.firmware_too_old() {
                self.warnings.push(Warning {
                    title: "Firmware".into(),
                    text: format!("Le firmware du boîtier ({}) est ancien.\nMettez-le à jour.", c.firmware_label()),
                    kind: WarningKind::Info,
                });
            }
            if c.uncalibrated() {
                self.warnings.push(Warning {
                    title: "Calibration".into(),
                    text: "Le boîtier n'est pas calibré ou est mal calibré.\nCalibrez-le.".into(),
                    kind: WarningKind::Info,
                });
            }
        }
        if c.shutdown_request && !self.warnings.iter().any(|w| w.title == "Arrêt demandé") {
            self.warnings.push(Warning {
                title: "Arrêt demandé".into(),
                text: "Le boîtier demande la fermeture du logiciel.".into(),
                kind: WarningKind::Info,
            });
        }

        let hold = self.last_local_command.map_or(false, |t| t.elapsed() < LOCAL_COMMAND_HOLD);
        if c.is_hardware() && !hold {
            self.sync_from_device(&c);
        }
        self.device = Some(c);
    }

    /// Met l'interface dans l'état réel du boîtier (DrawChart, ComTimeout == 0).
    fn sync_from_device(&mut self, c: &VendorConfig) {
        if (c.voltage_index as usize) < VOLTAGE_LABELS.len() {
            self.voltage_index = c.voltage_index;
        }
        if c.frequency_index < 4 {
            self.freq_button = c.frequency_index;
        }
        if (c.resistance_index as usize) < RESISTOR_LABELS.len() {
            self.resistor_index = c.resistance_index;
        }
        self.voltage_scan = c.scan == 1;
        self.frequency_scan = c.scan_frequency == 1;
        self.voltage_mask = c.voltage_scan_mask & 0x1F;
        self.frequency_mask = c.frequency_scan_mask & 0x0F;
        if c.tolerance > 0 {
            self.tolerance_pct = c.tolerance.min(100);
        }
        let device_mode = if c.dual_mode2 == 1 || c.dual_mode == 1 {
            1
        } else if c.dual_mode2 == 2 || c.dual_mode == 2 {
            2
        } else {
            0
        };
        if self.db_mode {
            if device_mode != 0 {
                self.send(Command::SetMode(0), "Mode base de données : Single");
            }
            self.display_mode = DisplayMode::Ch0;
        } else {
            self.display_mode = match device_mode {
                1 => DisplayMode::Dual,
                2 => DisplayMode::Opc,
                _ if matches!(self.display_mode, DisplayMode::Dual | DisplayMode::Opc) => DisplayMode::Ch0,
                _ => self.display_mode,
            };
        }
    }

    // -----------------------------------------------------------------------
    // Base de données
    // -----------------------------------------------------------------------

    fn set_db_mode(&mut self, on: bool) {
        self.db_mode = on;
        self.auto_test = false;
        self.next_point_pending = false;
        self.session.reset();
        self.verdict = None;
        self.impedance_label.clear();
        self.measurements[1] = None;
        self.analyzers[1].reset();
        if on {
            if self.voltage_scan {
                self.voltage_scan = false;
                self.send(Command::SetVoltageScan(false), "Balayage tension: OFF");
            }
            if self.frequency_scan {
                self.frequency_scan = false;
                self.send(Command::SetFrequencyScan(false), "Balayage fréquence: OFF");
            }
            self.set_display_mode(DisplayMode::Ch0);
            self.select_point(self.db.selected);
        } else {
            self.db_reference = None;
        }
    }

    /// Sélection d'un point : affiche sa référence et applique ses réglages
    /// au boîtier (show_on_chart).
    fn select_point(&mut self, index: Option<usize>) {
        self.db.selected = index.filter(|&i| i < self.db.points.len());
        self.analyzers[1].reset();
        let Some(p) = self.db.selected_point().cloned() else {
            self.db_reference = None;
            return;
        };
        self.db_reference = CurveData::from_raw(1, p.voltage.clone(), p.current.clone()).ok();
        self.marker = (p.x, p.y);
        if (p.voltage_index as usize) < VOLTAGE_LABELS.len() && p.voltage_index != self.voltage_index {
            // Le changement de calibre peut être bloqué par 47 R : on règle d'abord R.
            if p.resistor_index != 2 {
                self.set_resistor(p.resistor_index);
            }
            self.voltage_index = p.voltage_index;
            self.send(Command::SetVolt(p.voltage_index), "Tension du point");
        }
        if p.frequency_index < 4 {
            self.set_frequency(p.frequency_index);
        }
        if (p.resistor_index as usize) < RESISTOR_LABELS.len() && p.resistor_index != self.resistor_index {
            self.resistor_index = p.resistor_index;
            self.send(Command::SetRes(p.resistor_index), "Résistance du point");
        }
    }

    fn advance_point(&mut self) {
        if self.db.next() {
            self.select_point(self.db.selected);
        } else if self.auto_test {
            self.set_auto_test(false);
            self.warnings.push(Warning {
                title: "Information".into(),
                text: "Test terminé !".into(),
                kind: WarningKind::Info,
            });
        }
    }

    fn set_auto_test(&mut self, on: bool) {
        self.auto_test = on;
        self.next_point_pending = false;
        self.session.reset();
        self.verdict = None;
        if on {
            self.db.reset_session();
            if self.db.selected.is_none() && !self.db.points.is_empty() {
                self.db.selected = Some(0);
            }
            self.select_point(self.db.selected);
        }
    }

    fn add_point(&mut self) -> Result<(), String> {
        let live = self.live0.as_ref().filter(|c| c.has_raw()).ok_or("Aucune courbe CH0 reçue")?;
        if live.raw_voltage.len() < POINTS_PER_CURVE {
            return Err("Courbe incomplète (512 points requis)".into());
        }
        let mut name = self.point_name.clone();
        if self.auto_increment {
            name = format!("{}{}", self.point_name, self.increment_counter);
            self.increment_counter = (self.increment_counter + 1).min(1000);
        }
        let point = TestPoint {
            name,
            voltage: live.raw_voltage[..POINTS_PER_CURVE].to_vec(),
            current: live.raw_current[..POINTS_PER_CURVE].to_vec(),
            voltage_index: self.voltage_index,
            frequency_index: self.freq_button,
            resistor_index: self.resistor_index,
            x: self.marker.0,
            y: self.marker.1,
        };
        let idx = self.db.add(point);
        self.select_point(Some(idx));
        Ok(())
    }

    fn generate_report(&mut self) -> Result<(), String> {
        let device = self.device.as_ref().filter(|c| c.is_hardware());
        let info = ReportInfo {
            test_name: Path::new(&self.db.file_path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| self.db.file_path.clone()),
            device_name: device.map(|c| c.device_name().to_string()).unwrap_or_else(|| "UCE-CT22X".into()),
            serial: device.and_then(|c| c.serial_label()).unwrap_or_else(|| "-".into()),
            date: chrono::Local::now().format("%d/%m/%Y %H:%M:%S").to_string(),
        };
        save_report(&self.report_path, &info, &self.db, self.board.as_ref().map(|b| &b.rgb))
    }

    // -----------------------------------------------------------------------
    // Références rapides (CSV)
    // -----------------------------------------------------------------------

    fn save_references(&self) -> Result<(), String> {
        use std::io::Write;
        let mut file = std::fs::File::create(&self.reference_file).map_err(|e| e.to_string())?;
        writeln!(file, "CT220S_REFERENCE_V2").map_err(|e| e.to_string())?;
        for curve in [&self.reference_ch0, &self.reference_ch1].into_iter().flatten() {
            if curve.has_raw() {
                for (v, i) in curve.raw_voltage.iter().zip(&curve.raw_current) {
                    writeln!(file, "{},{},{}", curve.channel, v, i).map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    }

    fn load_references(&mut self) -> Result<(), String> {
        let contents = std::fs::read_to_string(&self.reference_file).map_err(|e| e.to_string())?;
        let mut lines = contents.lines();
        let v2 = match lines.next().map(str::trim) {
            Some("CT220S_REFERENCE_V1") => false,
            Some("CT220S_REFERENCE_V2") => true,
            _ => return Err("Format de référence incompatible".to_string()),
        };
        let mut v: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
        let mut i: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
        for line in lines.filter(|l| !l.trim().is_empty()) {
            let parts: Vec<_> = line.trim().split(',').collect();
            if parts.len() != 3 {
                return Err("Ligne CSV invalide".to_string());
            }
            let ch: usize = parts[0].parse().map_err(|_| "Canal CSV invalide".to_string())?;
            if ch > 1 {
                return Err("Canal hors limites".to_string());
            }
            let a: f64 = parts[1].parse().map_err(|_| "Tension CSV invalide".to_string())?;
            let b: f64 = parts[2].parse().map_err(|_| "Courant CSV invalide".to_string())?;
            if !a.is_finite() || !b.is_finite() {
                return Err("Valeur non finie".to_string());
            }
            v[ch].push(a);
            i[ch].push(b);
        }
        let build = |ch: usize| -> Option<CurveData> {
            if v[ch].is_empty() {
                return None;
            }
            if v2 {
                let rv = v[ch].iter().map(|x| x.round().clamp(0.0, 4095.0) as u16).collect();
                let ri = i[ch].iter().map(|x| x.round().clamp(0.0, 4095.0) as u16).collect();
                CurveData::from_raw(ch as u8, rv, ri).ok()
            } else {
                Some(CurveData::from_normalized(
                    ch as u8,
                    v[ch].iter().map(|&x| x as f32).collect(),
                    i[ch].iter().map(|&x| x as f32).collect(),
                ))
            }
        };
        self.reference_ch0 = build(0);
        self.reference_ch1 = build(1);
        Ok(())
    }

    /// Différence d'impédance constructeur entre une courbe et une référence.
    fn quick_difference(reference: &CurveData, live: &CurveData, params: MeasurementParams) -> Option<f64> {
        if !reference.has_raw() || !live.has_raw() {
            return None;
        }
        let mr = ChannelAnalyzer::new(true).analyze(&reference.raw_voltage, &reference.raw_current, params);
        let ml = ChannelAnalyzer::new(true).analyze(&live.raw_voltage, &live.raw_current, params);
        Some(
            analysis::compare(&live.raw_voltage, &live.raw_current, &ml, &reference.raw_voltage, &reference.raw_current, &mr)
                .difference_pct,
        )
    }

    /// Écart RMS point par point en coordonnées normalisées (version Rust).
    fn rms_error(reference: &CurveData, live: &CurveData) -> Option<f32> {
        let n = reference.voltage.len().min(reference.current.len()).min(live.voltage.len()).min(live.current.len());
        if n == 0 {
            return None;
        }
        let mut sum = 0.0f64;
        for k in 0..n {
            let dx = (reference.voltage[k] - live.voltage[k]) as f64;
            let dy = (reference.current[k] - live.current[k]) as f64;
            sum += (dx * dx + dy * dy) / 2.0;
        }
        Some(((sum / n as f64).sqrt() * 100.0) as f32)
    }
}

// ---------------------------------------------------------------------------
// Graphe
// ---------------------------------------------------------------------------

const CH0_COLOR: Color32 = Color32::from_rgb(255, 190, 0);
const CH1_COLOR: Color32 = Color32::from_rgb(70, 150, 255);
const REF_COLOR: Color32 = Color32::from_rgb(150, 150, 150);
const DB_REF_COLOR: Color32 = Color32::from_rgb(0, 200, 200);

fn channel_color(channel: u8) -> Color32 {
    if channel == 0 {
        CH0_COLOR
    } else {
        CH1_COLOR
    }
}

struct PlotGeom {
    rect: Rect,
    scale: PlotScale,
}

impl PlotGeom {
    fn center(&self) -> Pos2 {
        self.rect.center()
    }
    fn auto_scale(&self) -> f32 {
        self.rect.width().min(self.rect.height()) * 0.46
    }
    fn adc_to_screen(&self, v: f32, i: f32) -> Pos2 {
        Pos2::new(
            self.rect.left() + v / 4095.0 * self.rect.width(),
            self.rect.bottom() - i / 4095.0 * self.rect.height(),
        )
    }
    fn norm_to_screen(&self, v: f32, i: f32) -> Pos2 {
        let c = self.center();
        let s = self.auto_scale();
        Pos2::new(c.x + v * s, c.y - i * s)
    }

    fn xy_points(&self, curve: &CurveData) -> Vec<Pos2> {
        match self.scale {
            PlotScale::Fixed if curve.has_raw() => curve
                .raw_voltage
                .iter()
                .zip(&curve.raw_current)
                .filter(|(v, i)| **v > 0 && **i > 0)
                .map(|(&v, &i)| self.adc_to_screen(v as f32, i as f32))
                .collect(),
            // Référence CSV V1 sans données brutes : approximation centrée.
            PlotScale::Fixed => curve
                .voltage
                .iter()
                .zip(&curve.current)
                .map(|(&v, &i)| self.adc_to_screen(2048.0 + v * 2047.0, 2048.0 + i * 2047.0))
                .collect(),
            PlotScale::Auto => curve.voltage.iter().zip(&curve.current).map(|(&v, &i)| self.norm_to_screen(v, i)).collect(),
        }
    }
}

impl CT220SApp {
    fn draw_grid(painter: &egui::Painter, rect: Rect) {
        painter.rect_filled(rect, 0.0, Color32::BLACK);
        let grid = Stroke::new(0.7, Color32::from_gray(115));
        for k in 0..=10 {
            let x = rect.left() + rect.width() * k as f32 / 10.0;
            let y = rect.top() + rect.height() * k as f32 / 10.0;
            painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], grid);
            painter.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], grid);
        }
        let axis = Stroke::new(1.1, Color32::from_rgb(0, 145, 0));
        let c = rect.center();
        painter.line_segment([Pos2::new(rect.left(), c.y), Pos2::new(rect.right(), c.y)], axis);
        painter.line_segment([Pos2::new(c.x, rect.top()), Pos2::new(c.x, rect.bottom())], axis);
    }

    fn visible_live_curves(&self) -> Vec<&CurveData> {
        let mut out = Vec::new();
        let show1 = match self.display_mode {
            DisplayMode::Ch0 => false,
            DisplayMode::Ch1 | DisplayMode::Dual => true,
            DisplayMode::Opc => self
                .device
                .as_ref()
                .filter(|c| c.is_hardware())
                .map_or(true, |c| c.opc_flag >= c.opc_meas),
        };
        if self.display_mode != DisplayMode::Ch1 {
            if let Some(c) = &self.live0 {
                out.push(c);
            }
        }
        if show1 && !self.db_mode {
            if let Some(c) = &self.live1 {
                out.push(c);
            }
        }
        out
    }

    fn measurement_lines(&self, slot: usize, curve: &CurveData) -> Vec<String> {
        match self.algorithm {
            Algorithm::Vendor => match self.measurements[slot] {
                Some(m) => {
                    let mut lines = analysis::format_vendor_lines(&m, slot as u8, self.resistor_index).to_vec();
                    if self.debug {
                        lines.push(format!("THD V {:.1} % / I {:.1} %", m.thd_v, m.thd_i));
                    }
                    lines
                }
                None => vec!["R: ?".into(), "LC: ?".into(), "Ph: ?".into()],
            },
            Algorithm::Dft => {
                let v = calculate_electrical_values(
                    curve,
                    self.current_frequency_hz() as f32,
                    (self.total_resistor_kohm() * 1000.0) as f32,
                );
                vec![
                    format!("R: {}", v.resistance_ohm.map(format_resistance).unwrap_or_else(|| "?".into())),
                    format!("C: {}", v.capacitance_f.map(format_capacitance).unwrap_or_else(|| "?".into())),
                    format!("L: {}", v.inductance_h.map(format_inductance).unwrap_or_else(|| "?".into())),
                    format!("Ph: {}", v.phase_deg.map(|p| format!("{:.1}°", p)).unwrap_or_else(|| "?".into())),
                ]
            }
        }
    }

    fn draw_measurements(&self, painter: &egui::Painter, rect: Rect, slot: usize, curve: &CurveData, right: bool) {
        let color = channel_color(slot as u8);
        let label = if slot == 0 { "CH0" } else { "CH1" };
        let (x, align) = if right {
            (rect.right() - 10.0, egui::Align2::RIGHT_TOP)
        } else {
            (rect.left() + 10.0, egui::Align2::LEFT_TOP)
        };
        painter.text(Pos2::new(x, rect.top() + 10.0), align, label, egui::FontId::proportional(14.0), color);
        for (k, line) in self.measurement_lines(slot, curve).iter().enumerate() {
            painter.text(
                Pos2::new(x, rect.top() + 34.0 + k as f32 * 18.0),
                align,
                line,
                egui::FontId::proportional(15.0),
                color,
            );
        }
    }

    fn physical(&self, adc_v: f64, adc_i: f64) -> Option<(f64, f64)> {
        adc_to_physical(adc_v, adc_i, self.voltage_index, self.total_resistor_kohm())
    }

    fn draw_plot(&mut self, ui: &mut egui::Ui, size: f32) {
        let (response, painter) = ui.allocate_painter(egui::vec2(size, size), egui::Sense::click_and_drag());
        let rect = response.rect;
        Self::draw_grid(&painter, rect);
        let geom = PlotGeom { rect, scale: self.scale };

        match self.view {
            ViewMode::Xy => self.draw_xy(&painter, &geom),
            ViewMode::Xt => self.draw_xt(&painter, &geom),
        }

        // Curseur : clic gauche / glisser, clic droit pour l'effacer.
        if response.secondary_clicked() {
            self.cursor = None;
        } else if (response.dragged_by(egui::PointerButton::Primary) || response.clicked()) && !self.voltage_scan {
            if let Some(pos) = response.interact_pointer_pos() {
                self.cursor = self.cursor_from_screen(&geom, pos);
            }
        }
        self.draw_cursor(&painter, &geom);

        // Cadre vert / rouge du verdict (panel1.BackColor constructeur).
        if let Some(passed) = self.verdict {
            let c = if passed { Color32::from_rgb(0, 230, 0) } else { Color32::from_rgb(230, 0, 0) };
            painter.rect_stroke(rect.shrink(2.0), 0.0, Stroke::new(5.0, c));
        }
        if !self.impedance_label.is_empty() {
            painter.text(
                Pos2::new(rect.center().x, rect.top() + 8.0),
                egui::Align2::CENTER_TOP,
                &self.impedance_label,
                egui::FontId::proportional(16.0),
                Color32::WHITE,
            );
        }
        if self.device.as_ref().map_or(false, |c| c.overvoltage) {
            painter.text(
                Pos2::new(rect.center().x, rect.bottom() - 30.0),
                egui::Align2::CENTER_TOP,
                "HAUTE TENSION DÉTECTÉE",
                egui::FontId::proportional(18.0),
                Color32::RED,
            );
        }
    }

    fn draw_xy(&self, painter: &egui::Painter, geom: &PlotGeom) {
        let line = |pts: Vec<Pos2>, color: Color32, w: f32| {
            if pts.len() > 1 {
                painter.add(egui::Shape::line(pts, Stroke::new(w, color)));
            }
        };
        if self.show_reference && !self.db_mode {
            let show1 = self.display_mode != DisplayMode::Ch0;
            if self.display_mode != DisplayMode::Ch1 {
                if let Some(r) = &self.reference_ch0 {
                    line(geom.xy_points(r), REF_COLOR, 1.0);
                }
            }
            if show1 {
                if let Some(r) = &self.reference_ch1 {
                    line(geom.xy_points(r), REF_COLOR, 1.0);
                }
            }
        }
        if self.db_mode {
            if let Some(r) = &self.db_reference {
                line(geom.xy_points(r), DB_REF_COLOR, 2.5);
            }
        }
        let curves = self.visible_live_curves();
        let dual = curves.len() > 1;
        for c in &curves {
            line(geom.xy_points(c), channel_color(c.channel), 1.6);
        }
        for c in curves {
            let slot = if c.channel == 0 { 0 } else { 1 };
            let right = slot == 1 && (dual || self.display_mode == DisplayMode::Ch1);
            self.draw_measurements(painter, geom.rect, slot, c, right);
        }
    }

    fn draw_xt(&self, painter: &egui::Painter, geom: &PlotGeom) {
        let Some(c) = self.live0.as_ref().or(self.live1.as_ref()) else { return };
        let rect = geom.rect;
        let n = c.voltage.len().max(2);
        let x_at = |k: usize| rect.left() + k as f32 / (n - 1) as f32 * rect.width();
        let series: Vec<(Vec<Pos2>, Color32, &str)> = match geom.scale {
            PlotScale::Fixed if c.has_raw() => vec![
                (
                    c.raw_voltage.iter().enumerate().map(|(k, &v)| Pos2::new(x_at(k), rect.bottom() - v as f32 / 4095.0 * rect.height())).collect(),
                    CH0_COLOR,
                    "V",
                ),
                (
                    c.raw_current.iter().enumerate().map(|(k, &i)| Pos2::new(x_at(k), rect.bottom() - i as f32 / 4095.0 * rect.height())).collect(),
                    CH1_COLOR,
                    "I",
                ),
            ],
            _ => {
                let s = rect.height() * 0.46;
                vec![
                    (c.voltage.iter().enumerate().map(|(k, &v)| Pos2::new(x_at(k), rect.center().y - v * s)).collect(), CH0_COLOR, "V"),
                    (c.current.iter().enumerate().map(|(k, &i)| Pos2::new(x_at(k), rect.center().y - i * s)).collect(), CH1_COLOR, "I"),
                ]
            }
        };
        for (k, (pts, color, name)) in series.into_iter().enumerate() {
            painter.text(
                Pos2::new(rect.left() + 10.0 + 30.0 * k as f32, rect.top() + 10.0),
                egui::Align2::LEFT_TOP,
                name,
                egui::FontId::proportional(14.0),
                color,
            );
            painter.add(egui::Shape::line(pts, Stroke::new(1.4, color)));
        }
    }

    fn cursor_from_screen(&self, geom: &PlotGeom, pos: Pos2) -> Option<Cursor> {
        let rect = geom.rect;
        let fx = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let fy = ((rect.bottom() - pos.y) / rect.height()).clamp(0.0, 1.0);
        match self.view {
            ViewMode::Xt => {
                let n = self.live0.as_ref().map(|c| c.voltage.len()).unwrap_or(POINTS_PER_CURVE).max(1);
                Some(Cursor::Xt { sample: ((fx * (n - 1) as f32).round() as usize).min(n - 1) })
            }
            ViewMode::Xy => match geom.scale {
                PlotScale::Fixed => Some(Cursor::Xy { adc_v: fx as f64 * 4095.0, adc_i: fy as f64 * 4095.0 }),
                PlotScale::Auto => {
                    let c = self.live0.as_ref().or(self.live1.as_ref())?;
                    let s = geom.auto_scale();
                    let v = (pos.x - geom.center().x) / s;
                    let i = (geom.center().y - pos.y) / s;
                    let (adc_v, adc_i) = c.normalized_to_adc(v, i);
                    Some(Cursor::Xy { adc_v, adc_i })
                }
            },
        }
    }

    fn draw_cursor(&self, painter: &egui::Painter, geom: &PlotGeom) {
        let Some(cursor) = self.cursor else { return };
        let rect = geom.rect;
        let stroke = Stroke::new(1.0, Color32::from_rgb(255, 80, 255));
        let font = egui::FontId::proportional(14.0);
        match cursor {
            Cursor::Xy { adc_v, adc_i } => {
                let p = match geom.scale {
                    PlotScale::Fixed => geom.adc_to_screen(adc_v as f32, adc_i as f32),
                    PlotScale::Auto => {
                        let Some(c) = self.live0.as_ref().or(self.live1.as_ref()) else { return };
                        geom.norm_to_screen(
                            (adc_v as f32 - c.norm.center_v) / c.norm.scale,
                            (adc_i as f32 - c.norm.center_i) / c.norm.scale,
                        )
                    }
                };
                painter.line_segment([Pos2::new(p.x, rect.top()), Pos2::new(p.x, rect.bottom())], stroke);
                painter.line_segment([Pos2::new(rect.left(), p.y), Pos2::new(rect.right(), p.y)], stroke);
                if let Some((v, i)) = self.physical(adc_v, adc_i) {
                    let text = format!("Tension = {:.2} V\nCourant = {}", v, format_current_ma(i));
                    painter.text(Pos2::new(rect.left() + 10.0, rect.bottom() - 44.0), egui::Align2::LEFT_TOP, text, font, stroke.color);
                }
            }
            Cursor::Xt { sample } => {
                let Some(c) = self.live0.as_ref().or(self.live1.as_ref()) else { return };
                let n = c.voltage.len().max(2);
                let x = rect.left() + sample as f32 / (n - 1) as f32 * rect.width();
                painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], stroke);
                if c.has_raw() && sample < c.raw_voltage.len() {
                    if let Some((v, i)) = self.physical(c.raw_voltage[sample] as f64, c.raw_current[sample] as f64) {
                        let text = format!("V1 = {:.2} V\nI1 = {}", v, format_current_ma(i));
                        painter.text(Pos2::new(rect.left() + 10.0, rect.bottom() - 44.0), egui::Align2::LEFT_TOP, text, font, stroke.color);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Panneaux de commande
// ---------------------------------------------------------------------------

fn group<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::group(ui.style())
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(title).strong());
            add(ui)
        })
        .inner
}

impl CT220SApp {
    fn draw_basic(&mut self, ui: &mut egui::Ui) {
        group(ui, "Mode sonde", |ui| {
            ui.add_enabled_ui(!self.db_mode, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (mode, label) in [
                        (DisplayMode::Ch0, "CH0"),
                        (DisplayMode::Ch1, "CH1"),
                        (DisplayMode::Dual, "DUAL"),
                    ] {
                        if ui.radio(self.display_mode == mode, label).clicked() {
                            self.set_display_mode(mode);
                        }
                    }
                    let opc_ok = !self.voltage_scan && !self.frequency_scan;
                    if ui.add_enabled(opc_ok, egui::RadioButton::new(self.display_mode == DisplayMode::Opc, "OPC")).clicked() {
                        self.set_display_mode(DisplayMode::Opc);
                    }
                });
            });
            if self.db_mode {
                ui.small("Mode base de données : Single imposé.");
            }
        });

        ui.add_space(6.0);
        group(ui, "Calibre de tension", |ui| {
            ui.horizontal_wrapped(|ui| {
                for idx in VOLTAGE_DISPLAY_ORDER {
                    let enabled = !self.voltage_scan && self.voltage_allowed(idx);
                    if ui
                        .add_enabled(enabled, egui::RadioButton::new(self.voltage_index == idx, VOLTAGE_LABELS[idx as usize]))
                        .clicked()
                    {
                        self.set_voltage(idx);
                    }
                }
            });
            ui.add_enabled_ui(!self.db_mode, |ui| {
                let mut scan = self.voltage_scan;
                if ui.checkbox(&mut scan, "Balayage").changed() {
                    self.voltage_scan = scan;
                    self.cursor = None;
                    self.send(Command::SetVoltageScan(scan), if scan { "Balayage tension: ON" } else { "Balayage tension: OFF" });
                }
            });
            if self.voltage_scan {
                ui.horizontal_wrapped(|ui| {
                    ui.small("Inclus :");
                    let mut changed = false;
                    for idx in VOLTAGE_DISPLAY_ORDER {
                        let mut included = self.voltage_mask & (1 << idx) == 0;
                        if ui.checkbox(&mut included, VOLTAGE_LABELS[idx as usize]).changed() {
                            self.voltage_mask ^= 1 << idx;
                            changed = true;
                        }
                    }
                    if changed {
                        let mask = if self.voltage_mask > 31 { 0 } else { self.voltage_mask };
                        self.send(Command::SetVoltageScanMask(mask), "Masque balayage tension");
                    }
                });
            }
        });

        ui.add_space(6.0);
        group(ui, "Fréquence", |ui| {
            ui.horizontal_wrapped(|ui| {
                for b in 0..4u8 {
                    let label = frequency_label(self.freq_slots[b as usize]);
                    if ui.add_enabled(!self.frequency_scan, egui::RadioButton::new(self.freq_button == b, label)).clicked() {
                        self.set_frequency(b);
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!self.db_mode, |ui| {
                    let mut scan = self.frequency_scan;
                    if ui.checkbox(&mut scan, "Balayage").changed() {
                        self.frequency_scan = scan;
                        self.send(
                            Command::SetFrequencyScan(scan),
                            if scan { "Balayage fréquence: ON" } else { "Balayage fréquence: OFF" },
                        );
                    }
                });
                if ui.button("Régler…").clicked() {
                    self.freq_table_edit = self.freq_slots;
                    self.show_freq_table = true;
                }
            });
            if self.frequency_scan {
                ui.horizontal_wrapped(|ui| {
                    ui.small("Inclus :");
                    let mut changed = false;
                    for b in 0..4u8 {
                        let mut included = self.frequency_mask & (1 << b) == 0;
                        if ui.checkbox(&mut included, frequency_label(self.freq_slots[b as usize])).changed() {
                            self.frequency_mask ^= 1 << b;
                            changed = true;
                        }
                    }
                    if changed {
                        let mask = if self.frequency_mask > 15 { 0 } else { self.frequency_mask };
                        self.send(Command::SetFrequencyScanMask(mask), "Masque balayage fréquence");
                    }
                });
            }
        });

        ui.add_space(6.0);
        group(ui, "Résistance", |ui| {
            ui.horizontal_wrapped(|ui| {
                for idx in RESISTOR_DISPLAY_ORDER {
                    if ui
                        .add_enabled(
                            self.resistor_allowed(idx),
                            egui::RadioButton::new(self.resistor_index == idx, RESISTOR_LABELS[idx as usize]),
                        )
                        .clicked()
                    {
                        self.set_resistor(idx);
                    }
                }
            });
        });

        ui.add_space(6.0);
        group(ui, "Affichage", |ui| {
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.view, ViewMode::Xy, "XY");
                ui.radio_value(&mut self.view, ViewMode::Xt, "XT");
                ui.separator();
                ui.radio_value(&mut self.scale, PlotScale::Fixed, "Échelle fixe");
                ui.radio_value(&mut self.scale, PlotScale::Auto, "Auto");
            });
            ui.horizontal(|ui| {
                ui.label("Calcul :");
                ui.radio_value(&mut self.algorithm, Algorithm::Vendor, "Constructeur");
                ui.radio_value(&mut self.algorithm, Algorithm::Dft, "DFT Rust");
            });
            ui.small("Curseur : clic gauche sur le graphe, clic droit pour l'effacer.");
        });

        ui.add_space(6.0);
        group(ui, "Export", |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.image_path).desired_width(130.0));
                if ui.button("Image").clicked() {
                    let r = self.export_image();
                    self.last_command_status = match r {
                        Ok(()) => format!("Image enregistrée : {}", self.image_path),
                        Err(e) => format!("Erreur : {}", e),
                    };
                }
            });
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.text_path).desired_width(130.0));
                if ui.button("Texte").clicked() {
                    let r = match &self.live0 {
                        Some(c) => save_curve_text(c, &self.text_path),
                        None => Err("Pas de données CH0".into()),
                    };
                    self.last_command_status = match r {
                        Ok(()) => format!("Texte enregistré : {}", self.text_path),
                        Err(e) => format!("Erreur : {}", e),
                    };
                }
            });
            ui.small("Image : png, jpg, bmp ou gif selon l'extension. Texte : 512 V puis 512 I (format constructeur).");
        });
    }

    fn export_image(&self) -> Result<(), String> {
        let mut traces = Vec::new();
        if self.db_mode {
            if let Some(r) = &self.db_reference {
                traces.push(Trace { curve: r, color: [0, 170, 170], width: 3 });
            }
        }
        for c in self.visible_live_curves() {
            let color = if c.channel == 0 { [230, 130, 0] } else { [40, 110, 230] };
            traces.push(Trace { curve: c, color, width: 2 });
        }
        if traces.is_empty() {
            return Err("Aucune courbe à exporter".into());
        }
        save_image(&render_xy(&traces, 800, self.scale), &self.image_path)
    }

    fn draw_advanced(&mut self, ui: &mut egui::Ui) {
        group(ui, "Comparaison rapide", |ui| {
            ui.small("Références locales en mémoire, sans commande au boîtier.");
            ui.horizontal(|ui| {
                ui.label("Tolérance :");
                let r = ui.add(egui::DragValue::new(&mut self.tolerance_pct).clamp_range(1..=100).suffix(" %"));
                if r.changed() {
                    self.send(Command::SetTolerance(self.tolerance_pct), "Tolérance");
                }
            });
            ui.checkbox(&mut self.show_reference, "Afficher la référence (gris)");
            ui.add_enabled_ui(sound::AVAILABLE, |ui| {
                let label = if sound::AVAILABLE { "Alerte sonore" } else { "Alerte sonore (compiler avec --features son)" };
                ui.checkbox(&mut self.audible_warning, label);
            });
            ui.horizontal(|ui| {
                if ui.button("Capturer référence").clicked() {
                    self.reference_ch0 = self.live0.clone();
                    self.reference_ch1 = self.live1.clone();
                }
                if ui.button("Effacer").clicked() {
                    self.reference_ch0 = None;
                    self.reference_ch1 = None;
                }
            });
            let params = self.params();
            for (name, reference, live) in [("CH0", &self.reference_ch0, &self.live0), ("CH1", &self.reference_ch1, &self.live1)] {
                ui.group(|ui| {
                    ui.label(egui::RichText::new(name).strong());
                    match (reference, live) {
                        (Some(r), Some(l)) => {
                            if let Some(d) = Self::quick_difference(r, l, params) {
                                let pass = d <= self.tolerance_pct as f64;
                                let color = if pass { Color32::LIGHT_GREEN } else { Color32::LIGHT_RED };
                                ui.colored_label(
                                    color,
                                    format!("Diff. impédance : {} — {}", format_difference(d), if pass { "OK" } else { "HORS TOLÉRANCE" }),
                                );
                            }
                            if let Some(e) = Self::rms_error(r, l) {
                                ui.small(format!("Écart RMS normalisé : {:.2} %", e));
                            }
                        }
                        (None, _) => {
                            ui.label("Aucune référence enregistrée");
                        }
                        (_, None) => {
                            ui.label("En attente de mesures");
                        }
                    }
                });
            }
            ui.separator();
            ui.label("Fichier de références (CSV)");
            ui.text_edit_singleline(&mut self.reference_file);
            ui.horizontal(|ui| {
                if ui.button("Enregistrer").clicked() {
                    self.last_command_status = match self.save_references() {
                        Ok(()) => format!("Références enregistrées : {}", self.reference_file),
                        Err(e) => format!("Erreur enregistrement : {}", e),
                    };
                }
                if ui.button("Charger").clicked() {
                    self.last_command_status = match self.load_references() {
                        Ok(()) => format!("Références chargées : {}", self.reference_file),
                        Err(e) => format!("Erreur chargement : {}", e),
                    };
                }
            });
            ui.small("Format V2 (points ADC bruts). Les fichiers V1 restent lisibles.");
        });
        ui.add_space(6.0);
        ui.small("En mode DUAL, CH0 est comparé à CH1 avec l'algorithme constructeur : verdict après chaque contact de sonde (relever la sonde pour réarmer).");
    }

    fn draw_database(&mut self, ui: &mut egui::Ui) {
        let mut db_mode = self.db_mode;
        if ui.checkbox(&mut db_mode, "Activer le mode base de données").changed() {
            self.set_db_mode(db_mode);
        }
        let editable = self.db_mode && !self.auto_test;

        ui.add_enabled_ui(self.db_mode, |ui| {
            group(ui, "Points de test", |ui| {
                ui.add_enabled_ui(editable, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.point_name).desired_width(110.0));
                        ui.checkbox(&mut self.auto_increment, "inc.");
                        ui.add(egui::DragValue::new(&mut self.increment_counter).clamp_range(0..=1000));
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Ajouter").clicked() {
                            if let Err(e) = self.add_point() {
                                self.last_command_status = format!("Ajout impossible : {}", e);
                            }
                        }
                        if ui.button("Supprimer").clicked() {
                            self.db.delete_selected();
                            self.select_point(self.db.selected);
                        }
                        if ui.button("Tout supprimer").clicked() {
                            self.db.clear();
                            self.select_point(None);
                            self.increment_counter = 1;
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Monter").clicked() {
                            self.db.move_selected(true);
                        }
                        if ui.button("Descendre").clicked() {
                            self.db.move_selected(false);
                        }
                    });
                });

                let mut clicked = None;
                egui::ScrollArea::vertical().max_height(170.0).auto_shrink([false, true]).show(ui, |ui| {
                    for (k, p) in self.db.points.iter().enumerate() {
                        let mark = match self.db.results.get(k).copied().flatten() {
                            Some(true) => "✔ ",
                            Some(false) => "✘ ",
                            None => "   ",
                        };
                        let text = format!("{}{:>3}  {}", mark, k, p.name);
                        if ui.selectable_label(self.db.selected == Some(k), text).clicked() && !self.auto_test {
                            clicked = Some(k);
                        }
                    }
                });
                if let Some(k) = clicked {
                    self.select_point(Some(k));
                }

                ui.horizontal(|ui| {
                    if ui.button("<<<").clicked() {
                        self.db.previous();
                        self.select_point(self.db.selected);
                    }
                    ui.label(match self.db.selected {
                        Some(i) => format!("ID : {}", i),
                        None => "ID : -".into(),
                    });
                    if ui.button(">>>").clicked() {
                        self.advance_point();
                    }
                });
                if let (Some(r), Some(l)) = (&self.db_reference, &self.live0) {
                    if let Some(d) = Self::quick_difference(r, l, self.params()) {
                        ui.small(format!("Différence actuelle : {}", format_difference(d)));
                    }
                }
            });

            ui.add_space(4.0);
            group(ui, "Fichier .uce", |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.db.file_path).desired_width(f32::INFINITY));
                ui.add_enabled_ui(editable, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("Charger").clicked() {
                            match crate::database::load_uce(&self.db.file_path) {
                                Ok(points) => {
                                    let n = points.len();
                                    self.db.set_points(points);
                                    self.select_point(self.db.selected);
                                    self.last_command_status = format!("{} points chargés", n);
                                }
                                Err(e) => {
                                    self.last_command_status = format!("Fichier incorrect ou corrompu : {}", e);
                                }
                            }
                        }
                        if ui.button("Enregistrer").clicked() {
                            self.last_command_status = match crate::database::save_uce(&self.db.file_path, &self.db.points) {
                                Ok(()) => format!("Base enregistrée : {}", self.db.file_path),
                                Err(e) => format!("Erreur : {}", e),
                            };
                        }
                    });
                });
            });

            ui.add_space(4.0);
            group(ui, "Image de la carte", |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.board_path).desired_width(f32::INFINITY));
                ui.horizontal(|ui| {
                    if ui.add_enabled(editable, egui::Button::new("Charger image")).clicked() {
                        match BoardImage::load(&self.board_path) {
                            Ok(b) => {
                                self.board = Some(b);
                                self.show_board = true;
                            }
                            Err(e) => self.last_command_status = format!("Image : {}", e),
                        }
                    }
                    if ui.add_enabled(self.board.is_some(), egui::Button::new("Afficher")).clicked() {
                        self.show_board = true;
                    }
                });
                ui.small(format!("Repère : X {} / Y {}", self.marker.0, self.marker.1));
            });

            ui.add_space(4.0);
            group(ui, "Test automatique", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Tolérance :");
                    let r = ui.add(egui::DragValue::new(&mut self.tolerance_pct).clamp_range(1..=100).suffix(" %"));
                    if r.changed() {
                        self.send(Command::SetTolerance(self.tolerance_pct), "Tolérance");
                    }
                });
                ui.add_enabled_ui(sound::AVAILABLE, |ui| {
                    ui.checkbox(&mut self.audible_warning, "Alerte sonore");
                });
                let mut at = self.auto_test;
                if ui.add_enabled(!self.db.points.is_empty(), egui::Checkbox::new(&mut at, "Test automatique")).changed() {
                    self.set_auto_test(at);
                }
                let (tests, tested, failed) = self.db.statistics();
                ui.small(format!(
                    "Tests : {}  •  points testés : {}  •  échecs : {}  •  conformes : {}",
                    tests,
                    tested,
                    failed,
                    tested - failed
                ));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.report_path).desired_width(130.0));
                    if ui.add_enabled(!self.auto_test && tests > 0, egui::Button::new("Rapport PDF")).clicked() {
                        self.last_command_status = match self.generate_report() {
                            Ok(()) => format!("Rapport enregistré : {}", self.report_path),
                            Err(e) => format!("Erreur rapport : {}", e),
                        };
                    }
                });
                ui.small("Toucher le point, attendre le verdict, relever la sonde : passage automatique au point suivant si conforme.");
            });
        });
    }

    fn draw_debug(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.debug, "Afficher le THD sur le graphe");
        ui.checkbox(&mut self.debug_current_harmonics, "Harmoniques de courant (sinon tension)");
        for slot in 0..2 {
            let Some(m) = self.measurements[slot] else { continue };
            group(ui, if slot == 0 { "Voie 0" } else { "Voie 1 / référence" }, |ui| {
                ui.monospace(format!(
                    "État  : {:?}\nV min/max : {} / {}\nI min/max : {} / {}\nVrms/Irms : {:.1} / {:.1}\nPhase : {:.2}°\nTHD V/I : {:.2} % / {:.2} %\nR {:.4} kΩ  C {:.4} µF  L {:.4} mH",
                    m.status, m.v_min, m.v_max, m.i_min, m.i_max, m.vrms, m.irms, m.phase_rad.to_degrees(),
                    m.thd_v, m.thd_i, m.res_kohm, m.cap_uf, m.ind_mh
                ));
                let harm: Vec<f64> = if self.debug_current_harmonics {
                    self.analyzers[slot].harmonics_i().to_vec()
                } else {
                    self.analyzers[slot].harmonics_v().to_vec()
                };
                let max = harm.iter().cloned().fold(1e-9, f64::max);
                let (resp, painter) = ui.allocate_painter(egui::vec2(ui.available_width(), 50.0), egui::Sense::hover());
                let w = resp.rect.width() / harm.len().max(1) as f32;
                for (k, h) in harm.iter().enumerate() {
                    let hh = (h / max) as f32 * resp.rect.height();
                    let x = resp.rect.left() + k as f32 * w;
                    painter.rect_filled(
                        Rect::from_min_max(Pos2::new(x + 1.0, resp.rect.bottom() - hh), Pos2::new(x + w - 1.0, resp.rect.bottom())),
                        0.0,
                        CH0_COLOR,
                    );
                }
                ui.small("Harmoniques (rang 1 à gauche).");
            });
        }
        ui.small(format!(
            "Session : armée {} / contact {}",
            self.session.is_armed(),
            self.session.in_contact()
        ));
        if let Some(c) = &self.device {
            ui.collapsing("Configuration brute", |ui| {
                ui.monospace(format!("{:#?}", c));
            });
        }
    }

    fn draw_about(&mut self, ui: &mut egui::Ui) {
        ui.heading("CT220S Viewer");
        ui.label(format!("Version Rust : {}", env!("CARGO_PKG_VERSION")));
        ui.separator();
        match self.device.as_ref().filter(|c| c.is_hardware()) {
            Some(c) => {
                ui.label(format!("Appareil : {}", c.device_name()));
                ui.label(format!("Firmware : {}", c.firmware_label()));
                ui.label(format!("N° de série : {}", c.serial_label().unwrap_or_else(|| "-".into())));
                ui.label(format!("Matériel : {}  •  ADC {} bits", c.hardware_number, c.data_bits));
                ui.label(format!("Résistance de sortie calibrée : {:.2} Ω", c.output_resistance_kohm * 1000.0));
            }
            None => {
                ui.label("Aucune configuration matérielle reçue.");
            }
        }
        ui.separator();
        ui.label(egui::RichText::new("Connexion").strong());
        match self.transport {
            Transport::File => ui.label(format!("Fichier : {}", self.file_path)),
            Transport::Tcp => ui.label(format!("TCP : {} — connecté : {}", self.tcp_address, self.is_connected())),
            Transport::Usb => ui.label(format!("USB HID 0483:5750 — connecté : {}", self.is_connected())),
        };
        ui.separator();
        ui.small("Logiciel indépendant. Les mesures et la comparaison reprennent les algorithmes du logiciel PC_CT22X.");
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("V. Range : {}", VOLTAGE_LABELS[self.voltage_index.min(4) as usize]));
            ui.separator();
            ui.label(format!("Fréquence : {}", frequency_label(self.freq_slots[self.freq_button.min(3) as usize])));
            ui.separator();
            ui.label(format!("Résistance : {}", RESISTOR_LABELS[self.resistor_index.min(2) as usize]));
            ui.separator();
            ui.label(if self.voltage_scan || self.frequency_scan { "Scan : ON" } else { "Scan : OFF" });
            ui.separator();
            ui.label(match self.display_mode {
                DisplayMode::Dual => "Dual Mode",
                DisplayMode::Opc => "OPC Mode",
                _ => "Single Mode",
            });
            ui.separator();
            if let Some(sn) = self.device.as_ref().filter(|c| c.is_hardware()).and_then(|c| c.serial_label()) {
                ui.label(format!("SN : {}", sn));
                ui.separator();
            }
            let conn = match self.transport {
                Transport::File => format!("Fichier : {}", self.file_path),
                Transport::Tcp if self.is_connected() => format!("Simulateur TCP connecté ({})", self.tcp_address),
                Transport::Tcp => format!("Simulateur TCP : attente sur {}", self.tcp_address),
                Transport::Usb if self.is_connected() => "Boîtier connecté".to_string(),
                Transport::Usb => "Boîtier déconnecté".to_string(),
            };
            ui.label(conn);
            ui.separator();
            ui.label(format!("{} fps", self.fps));
        });
    }

    // -----------------------------------------------------------------------
    // Fenêtres
    // -----------------------------------------------------------------------

    fn draw_windows(&mut self, ctx: &egui::Context) {
        if let Some(w) = self.warnings.first() {
            let (title, text, kind) = (w.title.clone(), w.text.clone(), w.kind);
            let mut ok = false;
            egui::Window::new(title)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(egui::RichText::new(text).size(16.0));
                    ui.add_space(8.0);
                    ok = ui.button("OK").clicked();
                });
            if ok {
                self.warnings.remove(0);
                if kind == WarningKind::Overvoltage {
                    self.overvoltage_ack = Some(Instant::now());
                    self.send(Command::ResetOvervoltage, "Acquittement surtension");
                }
            }
        }

        if self.show_freq_table {
            let mut open = true;
            let mut send = false;
            egui::Window::new("Fréquences des boutons").open(&mut open).resizable(false).show(ctx, |ui| {
                for b in 0..4 {
                    egui::ComboBox::from_label(format!("Bouton F{}", b + 1))
                        .selected_text(frequency_label(self.freq_table_edit[b]))
                        .show_ui(ui, |ui| {
                            for (k, label) in FREQUENCY_LABELS.iter().enumerate() {
                                ui.selectable_value(&mut self.freq_table_edit[b], k as u8, *label);
                            }
                        });
                }
                ui.horizontal(|ui| {
                    send = ui.button("Envoyer au boîtier").clicked();
                    if ui.button("Défaut").clicked() {
                        self.freq_table_edit = DEFAULT_FREQUENCY_SLOTS;
                    }
                });
                ui.small("Commande F7. Les calculs C/L utilisent ensuite ces fréquences.");
            });
            if send {
                self.freq_slots = self.freq_table_edit;
                self.send(Command::SetFrequencyTable(self.freq_slots), "Table de fréquences");
            }
            self.show_freq_table = open;
        }

        if self.show_board && self.board.is_some() {
            let mut open = true;
            egui::Window::new("Image de la carte")
                .open(&mut open)
                .default_size([640.0, 520.0])
                .vscroll(false)
                .show(ctx, |ui| self.draw_board(ui));
            self.show_board = open;
        }
    }

    fn draw_board(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut self.board_zoom, 0.1..=4.0).logarithmic(true).text("zoom"));
            ui.checkbox(&mut self.board_show_results, "Résultats");
            let can_move = !self.auto_test && self.db.selected.is_some();
            if ui.add_enabled(can_move, egui::Button::new("Placer le point sélectionné")).clicked() {
                let (x, y) = self.marker;
                if let Some(p) = self.db.selected.and_then(|i| self.db.points.get_mut(i)) {
                    p.x = x;
                    p.y = y;
                }
            }
        });
        let ctx = ui.ctx().clone();
        let Some(board) = self.board.as_mut() else { return };
        let tex = board.texture(&ctx);
        let (w, h) = (board.width() as f32, board.height() as f32);
        let size = egui::vec2(w * self.board_zoom, h * self.board_zoom);
        let auto_test = self.auto_test;
        let mut new_marker = None;
        egui::ScrollArea::both().show(ui, |ui| {
            let (resp, painter) = ui.allocate_painter(size, egui::Sense::click());
            let rect = resp.rect;
            painter.image(tex, rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            let to_screen = |x: i32, y: i32| Pos2::new(rect.left() + x as f32 * self.board_zoom, rect.top() + y as f32 * self.board_zoom);
            for (k, p) in self.db.points.iter().enumerate() {
                let pos = to_screen(p.x, p.y);
                match (self.board_show_results, self.db.results.get(k).copied().flatten()) {
                    (true, Some(ok)) => {
                        let c = if ok { Color32::from_rgba_unmultiplied(0, 255, 0, 140) } else { Color32::from_rgba_unmultiplied(255, 0, 0, 140) };
                        painter.circle_filled(pos, 7.0, c);
                    }
                    _ => {
                        painter.circle_stroke(pos, 4.0, Stroke::new(2.0, Color32::RED));
                    }
                }
            }
            let m = to_screen(self.marker.0, self.marker.1);
            for (s, c) in [(3.0, Color32::WHITE), (2.0, Color32::RED)] {
                painter.line_segment([m - egui::vec2(15.0, 0.0), m + egui::vec2(15.0, 0.0)], Stroke::new(s, c));
                painter.line_segment([m - egui::vec2(0.0, 15.0), m + egui::vec2(0.0, 15.0)], Stroke::new(s, c));
            }
            if resp.clicked() && !auto_test {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let x = ((pos.x - rect.left()) / self.board_zoom).round() as i32;
                    let y = ((pos.y - rect.top()) / self.board_zoom).round() as i32;
                    new_marker = Some((x.clamp(0, w as i32 - 1), y.clamp(0, h as i32 - 1)));
                }
            }
        });
        if let Some(mk) = new_marker {
            self.marker = mk;
        }
        ui.small(format!("X : {} - Y : {}  (clic pour placer le repère du prochain point ajouté)", self.marker.0, self.marker.1));
    }
}

impl eframe::App for CT220SApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.transport == Transport::Tcp {
            let connected = self.is_connected();
            if connected && !self.was_tcp_connected {
                // Synchroniser le simulateur à la connexion.
                self.sync_all_to_device();
            }
            self.was_tcp_connected = connected;
        }
        self.process_incoming();

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.small(format!("Dernière action : {}", self.last_command_status));
            if let Ok(s) = self.status.lock() {
                if let Some(msg) = &*s {
                    ui.small(msg.as_str());
                }
            }
            self.status_bar(ui);
        });

        egui::SidePanel::right("controls").resizable(true).default_width(300.0).min_width(260.0).show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.active_tab, SettingsTab::Basic, "Basic");
                ui.selectable_value(&mut self.active_tab, SettingsTab::Advanced, "Advanced");
                ui.selectable_value(&mut self.active_tab, SettingsTab::Database, "Base");
                ui.selectable_value(&mut self.active_tab, SettingsTab::Debug, "Debug");
                ui.selectable_value(&mut self.active_tab, SettingsTab::About, "About");
            });
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.active_tab {
                SettingsTab::Basic => self.draw_basic(ui),
                SettingsTab::Advanced => self.draw_advanced(ui),
                SettingsTab::Database => self.draw_database(ui),
                SettingsTab::Debug => self.draw_debug(ui),
                SettingsTab::About => self.draw_about(ui),
            });
        });

        egui::CentralPanel::default().frame(egui::Frame::none().inner_margin(egui::Margin::same(6.0))).show(ctx, |ui| {
            let size = ui.available_width().min(ui.available_height()).max(200.0);
            ui.vertical_centered(|ui| self.draw_plot(ui, size));
        });

        self.draw_windows(ctx);
        ctx.request_repaint_after(Duration::from_millis(16));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Comme le logiciel constructeur : FA 01 à la fermeture.
        if self.is_connected() && self.transport != Transport::File {
            self.send(Command::Close, "Fermeture");
        }
        self.running.store(false, Ordering::Relaxed);
    }
}
