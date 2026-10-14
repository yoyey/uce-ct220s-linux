// src/config.rs
//
// Paramètres matériels et tables reprises du logiciel constructeur PC_CT22X
// (Form1.cs : signal_volt_table, resistor_table, signal_freq_table...).

pub const VID: u16 = 0x0483;
pub const PID: u16 = 0x5750;
pub const REPORT_DATA_SIZE: usize = 64;
pub const READ_SIZE: usize = 65;
pub const POINTS_PER_CURVE: usize = 512;
/// Nombre de rapports de données par courbe quand le boîtier ne le précise pas
/// (octet 58 de la trame de configuration à 0, cas du simulateur historique).
pub const REPORTS_PER_CURVE: usize = 32;
/// Le logiciel constructeur dimensionne ses tableaux à 612 points (38 rapports).
pub const MAX_REPORTS_PER_CURVE: usize = 38;
pub const POINTS_PER_REPORT: usize = 16;
pub const HEADER_MAGIC: [u8; 2] = [0xf0, 0xff];

/// Index de tension = octet 6 de la configuration / paramètre de la commande FD.
pub const VOLTAGE_LABELS: [&str; 5] = ["2.5 V", "5.0 V", "10 V", "20 V", "1.0 V"];
/// Pleine échelle ADC (±2048 points) en volts, par index de tension.
pub const VOLTAGE_FACTORS: [f64; 5] = [3.125, 6.25, 12.5, 25.0, 1.25];
/// Ordre d'affichage du constructeur (1.0 V en dernier dans la table mais
/// logiquement le plus petit calibre).
pub const VOLTAGE_DISPLAY_ORDER: [u8; 5] = [4, 0, 1, 2, 3];

/// Index de résistance = octet 9 / paramètre de la commande FB.
pub const RESISTOR_LABELS: [&str; 3] = ["10 K", "1 K", "47 R"];
/// Valeurs nominales utilisées par le constructeur (kΩ). 47 R vaut 48,1 Ω.
pub const RESISTORS_KOHM: [f64; 3] = [10.0, 1.0, 0.0481];
pub const RESISTOR_DISPLAY_ORDER: [u8; 3] = [2, 1, 0];

/// Table des 20 fréquences programmables (commande F7) ; les 4 boutons de
/// fréquence pointent par défaut sur les index 0..3.
pub const FREQUENCY_TABLE_HZ: [f64; 20] = [
    10.0, 100.0, 500.0, 2000.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 200.0, 300.0,
    400.0, 600.0, 700.0, 800.0, 900.0, 1000.0,
];
pub const FREQUENCY_LABELS: [&str; 20] = [
    "10 Hz", "100 Hz", "500 Hz", "2 kHz", "20 Hz", "30 Hz", "40 Hz", "50 Hz", "60 Hz", "70 Hz",
    "80 Hz", "90 Hz", "200 Hz", "300 Hz", "400 Hz", "600 Hz", "700 Hz", "800 Hz", "900 Hz",
    "1 kHz",
];
pub const DEFAULT_FREQUENCY_SLOTS: [u8; 4] = [0, 1, 2, 3];

pub fn resistor_kohm(index: u8) -> f64 {
    RESISTORS_KOHM.get(index as usize).copied().unwrap_or(RESISTORS_KOHM[1])
}

pub fn frequency_hz(table_index: u8) -> f64 {
    FREQUENCY_TABLE_HZ.get(table_index as usize).copied().unwrap_or(FREQUENCY_TABLE_HZ[0])
}

pub fn frequency_label(table_index: u8) -> &'static str {
    FREQUENCY_LABELS.get(table_index as usize).copied().unwrap_or("?")
}
