// Paramètres
pub const VID: u16 = 0x0483;
pub const PID: u16 = 0x5750;
pub const REPORT_DATA_SIZE: usize = 64;
pub const READ_SIZE: usize = 65;
pub const POINTS_PER_CURVE: usize = 512;
pub const REPORTS_PER_CURVE: usize = 32;
pub const HEADER_MAGIC: [u8; 2] = [0xf0, 0xff];
