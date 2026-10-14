//! Protocole HID du CT220S / CT221L, tel qu'il est utilisé par le logiciel
//! constructeur PC_CT22X (Form1.cs : DeviceDataReceived, CommandMessage...).
//!
//! - Commandes : rapport de 65 octets, `00 / commande / paramètres...`.
//! - Réception : rapports de 64 octets utiles (65 avec le Report ID sous Windows).
//!   * `F0 FF ...` : rapport de configuration (canal, calibres, n° de série...).
//!   * sinon 16 couples `(CH2 = courant, CH1 = tension)` en u16 little-endian.
//!   * une courbe = `packets` rapports de données (octet 58, 32 en pratique).

use crate::config::{
    HEADER_MAGIC, MAX_REPORTS_PER_CURVE, POINTS_PER_REPORT, READ_SIZE, REPORTS_PER_CURVE,
    REPORT_DATA_SIZE,
};

pub const REPORT_LEN: usize = READ_SIZE;

/// Commandes reconnues par le firmware (codes relevés dans Form1.cs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// FF : 0 = Single, 1 = Dual, 2 = OPC.
    /// (Les versions précédentes de ce viewer utilisaient FA par erreur.)
    SetMode(u8),
    /// FB : 0 = 10 K, 1 = 1 K, 2 = 47 R.
    SetRes(u8),
    /// FC : index du bouton de fréquence (0..3).
    SetFreq(u8),
    /// FD : 0 = 2.5 V, 1 = 5 V, 2 = 10 V, 3 = 20 V, 4 = 1.0 V.
    SetVolt(u8),
    /// FE : balayage automatique des tensions.
    SetVoltageScan(bool),
    /// F4 : balayage automatique des fréquences.
    SetFrequencyScan(bool),
    /// F9 : masque des tensions exclues du balayage (bit à 1 = exclue).
    SetVoltageScanMask(u8),
    /// F5 : masque des fréquences exclues du balayage (bit à 1 = exclue).
    SetFrequencyScanMask(u8),
    /// F8 : tolérance de comparaison en %, mémorisée par le boîtier.
    SetTolerance(u8),
    /// F7 : affectation des 4 boutons de fréquence dans la table de 20 valeurs.
    SetFrequencyTable([u8; 4]),
    /// F6 01 : acquittement de l'alarme surtension.
    ResetOvervoltage,
    /// FA 01 : envoyé par le logiciel constructeur à la fermeture.
    Close,
}

impl Command {
    pub fn opcode(&self) -> u8 {
        match self {
            Command::SetMode(_) => 0xFF,
            Command::SetRes(_) => 0xFB,
            Command::SetFreq(_) => 0xFC,
            Command::SetVolt(_) => 0xFD,
            Command::SetVoltageScan(_) => 0xFE,
            Command::SetFrequencyScan(_) => 0xF4,
            Command::SetVoltageScanMask(_) => 0xF9,
            Command::SetFrequencyScanMask(_) => 0xF5,
            Command::SetTolerance(_) => 0xF8,
            Command::SetFrequencyTable(_) => 0xF7,
            Command::ResetOvervoltage => 0xF6,
            Command::Close => 0xFA,
        }
    }

    pub fn encode(&self) -> [u8; REPORT_LEN] {
        let params: Vec<u8> = match *self {
            Command::SetMode(v)
            | Command::SetRes(v)
            | Command::SetFreq(v)
            | Command::SetVolt(v)
            | Command::SetVoltageScanMask(v)
            | Command::SetFrequencyScanMask(v)
            | Command::SetTolerance(v) => vec![v],
            Command::SetVoltageScan(b) | Command::SetFrequencyScan(b) => vec![b as u8],
            Command::SetFrequencyTable(t) => t.to_vec(),
            Command::ResetOvervoltage | Command::Close => vec![1],
        };
        command(self.opcode(), &params).expect("paramètres de commande CT220S valides")
    }
}

pub fn command(command: u8, parameters: &[u8]) -> Result<[u8; REPORT_LEN], &'static str> {
    if parameters.len() > 63 {
        return Err("plus de 63 octets de parametres");
    }
    let mut report = [0u8; REPORT_LEN];
    report[1] = command;
    report[2..2 + parameters.len()].copy_from_slice(parameters);
    Ok(report)
}

/// Contenu du rapport de configuration `F0 FF`.
/// Les commentaires indiquent l'index dans le rapport Windows de 65 octets.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VendorConfig {
    /// [3] canal de la courbe qui suit (0 = CH0, 1 = CH1 / référence OPC).
    pub dual_channel: u8,
    /// [4] mode : 0 Single, 1 Dual, 2 OPC.
    pub dual_mode: u8,
    /// [5] balayage tension actif.
    pub scan: u8,
    /// [6] index de tension.
    pub voltage_index: u8,
    /// [7] masque des tensions exclues du balayage (CH2VoltDiv constructeur).
    pub voltage_scan_mask: u8,
    /// [8] index du bouton de fréquence.
    pub frequency_index: u8,
    /// [9] index de résistance.
    pub resistance_index: u8,
    /// [10] second indicateur de mode (Dual_Mode2).
    pub dual_mode2: u8,
    /// [11] tolérance mémorisée (%).
    pub tolerance: u8,
    /// [12] masque des fréquences exclues du balayage (TimeDiv2).
    pub frequency_scan_mask: u8,
    /// [13] balayage fréquence actif.
    pub scan_frequency: u8,
    /// [47..48] résistance de sortie calibrée, en kΩ.
    pub output_resistance_kohm: f64,
    /// [49] numéro de matériel (0 = configuration invalide / simulateur).
    pub hardware_number: u8,
    /// [50] surtension détectée.
    pub overvoltage: bool,
    /// [51] demande d'arrêt du logiciel.
    pub shutdown_request: bool,
    /// [54] / [55] états du mode OPC.
    pub opc_flag: u8,
    pub opc_meas: u8,
    /// [56] version firmware.
    pub firmware_version: u8,
    /// [57] résolution ADC.
    pub data_bits: u8,
    /// [58] nombre de rapports de données par courbe.
    pub packets_per_frame: u8,
    /// [59] 220 = CT220S, 221 = CT221L.
    pub product_id: u8,
    /// [60..63] numéro de série (big-endian).
    pub serial: u32,
    /// [64] somme de contrôle du numéro de série.
    pub serial_checksum: u8,
}

impl VendorConfig {
    /// Vrai pour une trame issue d'un vrai boîtier (le simulateur historique
    /// n'envoie que l'en-tête et le canal).
    pub fn is_hardware(&self) -> bool {
        self.hardware_number != 0
    }

    pub fn frame_reports(&self) -> usize {
        let p = self.packets_per_frame as usize;
        if (1..=MAX_REPORTS_PER_CURVE).contains(&p) {
            p
        } else {
            REPORTS_PER_CURVE
        }
    }

    pub fn device_name(&self) -> &'static str {
        match self.product_id {
            220 => "UCE-CT220S",
            221 => "UCE-CT221L",
            _ => "UCE-CT22X",
        }
    }

    pub fn firmware_label(&self) -> String {
        if self.firmware_version < 10 {
            format!("V1.0{}", self.firmware_version)
        } else {
            format!("V1.{}", self.firmware_version)
        }
    }

    pub fn serial_label(&self) -> Option<String> {
        if self.serial > 0 && self.serial_checksum > 0 {
            Some(format!(
                "R{}{}{}{:02X}",
                self.hardware_number, self.product_id, self.serial, self.serial_checksum
            ))
        } else {
            None
        }
    }

    /// Même règle que le constructeur ("Device firmware version is old").
    pub fn firmware_too_old(&self) -> bool {
        self.hardware_number == 0
            || (self.firmware_version < 14 && self.product_id == 221)
            || (self.firmware_version < 10 && self.product_id == 220)
    }

    /// Même règle que le constructeur ("Your device is uncalibrated").
    pub fn uncalibrated(&self) -> bool {
        self.output_resistance_kohm == 0.0 && self.product_id == 221
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    BadLength(usize),
    InvalidPacketsPerFrame(u8),
    NoConfiguration,
}

/// Ramène un rapport de 64 ou 65 octets à sa charge utile de 64 octets.
pub fn payload_of(report: &[u8]) -> Option<&[u8]> {
    match report.len() {
        READ_SIZE => Some(&report[1..]),
        REPORT_DATA_SIZE => Some(report),
        _ => None,
    }
}

/// Décode un rapport de configuration à partir de sa charge utile (64 octets,
/// sans Report ID). `payload[k]` correspond à `data[k + 1]` côté constructeur.
pub fn parse_config_payload(payload: &[u8]) -> Option<VendorConfig> {
    if payload.len() < REPORT_DATA_SIZE || payload[0] != HEADER_MAGIC[0] || payload[1] != HEADER_MAGIC[1] {
        return None;
    }
    let d = |i: usize| payload[i - 1];
    Some(VendorConfig {
        dual_channel: d(3),
        dual_mode: d(4),
        scan: d(5),
        voltage_index: d(6),
        voltage_scan_mask: d(7),
        frequency_index: d(8),
        resistance_index: d(9),
        dual_mode2: d(10),
        tolerance: d(11),
        frequency_scan_mask: d(12),
        scan_frequency: d(13),
        output_resistance_kohm: (d(47) as f64 + d(48) as f64 / 100.0) / 1000.0,
        hardware_number: d(49),
        overvoltage: d(50) == 1,
        shutdown_request: d(51) == 1,
        opc_flag: d(54),
        opc_meas: d(55),
        firmware_version: d(56),
        data_bits: d(57),
        packets_per_frame: d(58),
        product_id: d(59),
        serial: u32::from_be_bytes([d(60), d(61), d(62), d(63)]),
        serial_checksum: d(64),
    })
}

/// Variante sur rapport Windows complet de 65 octets.
#[allow(dead_code)]
pub fn parse_config(report: &[u8]) -> Result<Option<VendorConfig>, DecodeError> {
    if report.len() != REPORT_LEN {
        return Err(DecodeError::BadLength(report.len()));
    }
    match parse_config_payload(&report[1..]) {
        Some(cfg) if cfg.packets_per_frame == 0 => {
            Err(DecodeError::InvalidPacketsPerFrame(0))
        }
        other => Ok(other),
    }
}

/// Ordre logique renvoyé : (CH1 = tension / axe X, CH2 = courant / axe Y).
pub fn decode_samples_payload(payload: &[u8]) -> [(u16, u16); POINTS_PER_REPORT] {
    let mut samples = [(0u16, 0u16); POINTS_PER_REPORT];
    for (i, sample) in samples.iter_mut().enumerate() {
        let pos = 4 * i;
        let ch2 = u16::from_le_bytes([payload[pos], payload[pos + 1]]);
        let ch1 = u16::from_le_bytes([payload[pos + 2], payload[pos + 3]]);
        *sample = (ch1, ch2);
    }
    samples
}

#[allow(dead_code)]
pub fn decode_samples(report: &[u8]) -> Result<[(u16, u16); POINTS_PER_REPORT], DecodeError> {
    if report.len() != REPORT_LEN {
        return Err(DecodeError::BadLength(report.len()));
    }
    Ok(decode_samples_payload(&report[1..]))
}

/// Courbe brute assemblée : valeurs ADC 12 bits (0..4095, centre 2048).
#[derive(Debug, Clone, PartialEq)]
pub struct RawFrame {
    pub channel: u8,
    pub voltage: Vec<u16>,
    pub current: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AssemblerEvent {
    /// Rapport de données ignoré : aucune configuration reçue.
    Ignored,
    Config(VendorConfig),
    Pending,
    Frame(RawFrame),
}

/// Assemble les courbes exactement comme `DeviceDataReceived` du constructeur :
/// une configuration remet le compteur à zéro, puis `packets` rapports de
/// données forment une courbe ; les courbes suivantes peuvent arriver sans
/// nouvel en-tête.
#[derive(Default)]
pub struct FrameAssembler {
    pub config: Option<VendorConfig>,
    voltage: Vec<u16>,
    current: Vec<u16>,
    received_reports: usize,
}

impl FrameAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.config = None;
        self.voltage.clear();
        self.current.clear();
        self.received_reports = 0;
    }

    pub fn push_payload(&mut self, payload: &[u8]) -> Result<AssemblerEvent, DecodeError> {
        if payload.len() != REPORT_DATA_SIZE {
            return Err(DecodeError::BadLength(payload.len()));
        }
        if let Some(cfg) = parse_config_payload(payload) {
            self.voltage.clear();
            self.current.clear();
            self.received_reports = 0;
            self.config = Some(cfg.clone());
            return Ok(AssemblerEvent::Config(cfg));
        }
        let Some(cfg) = self.config.as_ref() else {
            return Ok(AssemblerEvent::Ignored);
        };
        let expected = cfg.frame_reports();
        let channel = cfg.dual_channel;
        for (v, i) in decode_samples_payload(payload) {
            self.voltage.push(v);
            self.current.push(i);
        }
        self.received_reports += 1;
        if self.received_reports < expected {
            return Ok(AssemblerEvent::Pending);
        }
        self.received_reports = 0;
        Ok(AssemblerEvent::Frame(RawFrame {
            channel,
            voltage: std::mem::take(&mut self.voltage),
            current: std::mem::take(&mut self.current),
        }))
    }

    /// Accepte un rapport de 64 ou 65 octets.
    pub fn push_report(&mut self, report: &[u8]) -> Result<AssemblerEvent, DecodeError> {
        let payload = payload_of(report).ok_or(DecodeError::BadLength(report.len()))?;
        self.push_payload(payload)
    }
}

/// Conversion d'un point ADC en grandeurs physiques (formules constructeur).
/// Retourne (tension en V, courant en mA).
pub fn adc_to_physical(adc_v: f64, adc_i: f64, voltage_index: u8, resistance_kohm: f64) -> Option<(f64, f64)> {
    let factor = *crate::config::VOLTAGE_FACTORS.get(voltage_index as usize)?;
    if resistance_kohm <= 0.0 {
        return None;
    }
    let voltage = (adc_v - 2048.0) * factor / 2048.0;
    let current_ma = (adc_i - 2048.0) * factor / resistance_kohm / 2048.0;
    Some((voltage, current_ma))
}

#[allow(dead_code)]
pub fn to_voltage_current(point: (u16, u16), cfg: &VendorConfig) -> Option<(f64, f64)> {
    let r = *crate::config::RESISTORS_KOHM.get(cfg.resistance_index as usize)? + cfg.output_resistance_kohm;
    adc_to_physical(point.0 as f64, point.1 as f64, cfg.voltage_index, r)
}

/// Format courant identique au constructeur (mA / µA).
pub fn format_current_ma(ma: f64) -> String {
    if ma.abs() >= 10.0 {
        format!("{:.1} mA", ma)
    } else if ma.abs() >= 1.0 {
        format!("{:.2} mA", ma)
    } else {
        format!("{:.0} µA", ma * 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_report(packets: u8) -> [u8; REPORT_LEN] {
        let mut r = [0u8; REPORT_LEN];
        r[1] = 0xF0;
        r[2] = 0xFF;
        r[49] = 1;
        r[58] = packets;
        r[59] = 220;
        r
    }

    #[test]
    fn command_layout() {
        let r = command(0xFC, &[2]).unwrap();
        assert_eq!(&r[..4], &[0, 0xFC, 2, 0]);
        assert_eq!(r.len(), 65);
    }

    #[test]
    fn opcodes_match_vendor_software() {
        assert_eq!(&Command::SetMode(1).encode()[..3], &[0, 0xFF, 1]);
        assert_eq!(&Command::SetRes(2).encode()[..3], &[0, 0xFB, 2]);
        assert_eq!(&Command::SetFreq(3).encode()[..3], &[0, 0xFC, 3]);
        assert_eq!(&Command::SetVolt(4).encode()[..3], &[0, 0xFD, 4]);
        assert_eq!(&Command::SetVoltageScan(true).encode()[..3], &[0, 0xFE, 1]);
        assert_eq!(&Command::SetFrequencyScan(true).encode()[..3], &[0, 0xF4, 1]);
        assert_eq!(&Command::SetVoltageScanMask(5).encode()[..3], &[0, 0xF9, 5]);
        assert_eq!(&Command::SetFrequencyScanMask(3).encode()[..3], &[0, 0xF5, 3]);
        assert_eq!(&Command::SetTolerance(12).encode()[..3], &[0, 0xF8, 12]);
        assert_eq!(&Command::SetFrequencyTable([4, 5, 6, 7]).encode()[..6], &[0, 0xF7, 4, 5, 6, 7]);
        assert_eq!(&Command::ResetOvervoltage.encode()[..3], &[0, 0xF6, 1]);
        assert_eq!(&Command::Close.encode()[..3], &[0, 0xFA, 1]);
    }

    #[test]
    fn sample_byte_order() {
        let mut r = [0u8; 65];
        r[1..5].copy_from_slice(&[0x34, 0x12, 0x78, 0x56]);
        assert_eq!(decode_samples(&r).unwrap()[0], (0x5678, 0x1234));
    }

    #[test]
    fn config_fields() {
        let mut r = cfg_report(32);
        r[3] = 1;
        r[6] = 4;
        r[47] = 0;
        r[48] = 50;
        r[56] = 12;
        r[60..64].copy_from_slice(&[0, 0, 1, 2]);
        r[64] = 0xAB;
        let c = parse_config(&r).unwrap().unwrap();
        assert_eq!(c.dual_channel, 1);
        assert_eq!(c.voltage_index, 4);
        assert!((c.output_resistance_kohm - 0.0005).abs() < 1e-12);
        assert_eq!(c.serial, 258);
        assert_eq!(c.serial_label().unwrap(), "R1220258AB");
        assert_eq!(c.firmware_label(), "V1.12");
        assert!(!c.firmware_too_old());
    }

    #[test]
    fn config_and_assemble_frame() {
        let mut d = FrameAssembler::new();
        assert!(matches!(d.push_report(&cfg_report(2)).unwrap(), AssemblerEvent::Config(_)));
        let r = [0u8; 65];
        assert_eq!(d.push_report(&r).unwrap(), AssemblerEvent::Pending);
        match d.push_report(&r).unwrap() {
            AssemblerEvent::Frame(f) => assert_eq!(f.voltage.len(), 32),
            e => panic!("unexpected event: {e:?}"),
        }
        // Courbe suivante sans nouvel en-tête, comme le constructeur.
        assert_eq!(d.push_report(&r).unwrap(), AssemblerEvent::Pending);
    }

    #[test]
    fn simulator_header_defaults_to_32_reports() {
        let mut header = [0u8; 64];
        header[..3].copy_from_slice(&[0xF0, 0xFF, 1]);
        let mut d = FrameAssembler::new();
        d.push_payload(&header).unwrap();
        let data = [0u8; 64];
        for _ in 0..31 {
            assert_eq!(d.push_payload(&data).unwrap(), AssemblerEvent::Pending);
        }
        match d.push_payload(&data).unwrap() {
            AssemblerEvent::Frame(f) => {
                assert_eq!(f.channel, 1);
                assert_eq!(f.voltage.len(), 512);
            }
            e => panic!("unexpected {e:?}"),
        }
    }

    #[test]
    fn data_before_config_is_ignored() {
        let mut d = FrameAssembler::new();
        assert_eq!(d.push_payload(&[0u8; 64]).unwrap(), AssemblerEvent::Ignored);
    }

    #[test]
    fn conversion_zero_at_center() {
        let c = parse_config(&cfg_report(32)).unwrap().unwrap();
        assert_eq!(to_voltage_current((2048, 2048), &c), Some((0.0, 0.0)));
    }
}
