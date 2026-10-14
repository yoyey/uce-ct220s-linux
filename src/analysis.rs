//! Algorithmes de mesure et de comparaison repris du logiciel constructeur
//! PC_CT22X (Form1.cs : find_MinMax, fft_hesap, Calculate_RMS, calculate_phase,
//! RLC_calculation, calculate_RLC, show_values, compare_signals, DrawChart).
//!
//! Toutes les grandeurs travaillent sur les points ADC bruts (0..4095,
//! centre 2048). Les unités internes sont celles du constructeur :
//! résistance en kΩ, capacité en µF, inductance en mH ; -1 = invalide.

use std::f64::consts::PI;

pub const ADC_CENTER: i64 = 2048;
const DFT_LEN: usize = 128;
const HARMONIC_SMOOTHING: f64 = 0.899_999_976_158_142_1; // 0.9f constructeur
const DEG3: f64 = 0.052_359_877_559_829_89; // 3°
const DEG5: f64 = PI / 36.0; // 5°
const DEG88: f64 = PI * 22.0 / 45.0; // 88°
const DEG89: f64 = 1.553_343_034_274_953_2; // 89°
const DEG92: f64 = 1.605_702_911_834_783_2; // 92°

/// Tables sinus/cosinus 512 points, amplitude 2047 (identiques à celles du
/// constructeur : arrondi de 2047·sin(2πk/512)).
fn trig_tables() -> &'static ([i64; 512], [i64; 512]) {
    use std::sync::OnceLock;
    static TABLES: OnceLock<([i64; 512], [i64; 512])> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut s = [0i64; 512];
        let mut c = [0i64; 512];
        for k in 0..512 {
            let t = 2.0 * PI * k as f64 / 512.0;
            s[k] = (2047.0 * t.sin()).round() as i64;
            c[k] = (2047.0 * t.cos()).round() as i64;
        }
        (s, c)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProbeStatus {
    /// Sonde en l'air : tension pleine échelle, pas de courant (ch_status 0).
    Open,
    /// Court-circuit : tension nulle, courant pleine échelle (ch_status 1).
    Short,
    /// Composant mesurable (ch_status 2).
    Measured,
    /// Signal trop faible / non interprétable.
    #[default]
    Undefined,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VendorMeasurement {
    pub status: ProbeStatus,
    pub v_min: i32,
    pub v_max: i32,
    pub i_min: i32,
    pub i_max: i32,
    /// Valeurs efficaces en points ADC (centre 2048).
    pub vrms: f64,
    pub irms: f64,
    pub phase_rad: f64,
    pub thd_v: f64,
    pub thd_i: f64,
    pub res_kohm: f64,
    pub cap_uf: f64,
    pub ind_mh: f64,
}

impl Default for VendorMeasurement {
    fn default() -> Self {
        Self {
            status: ProbeStatus::Undefined,
            v_min: 0,
            v_max: 0,
            i_min: 0,
            i_max: 0,
            vrms: 0.0,
            irms: 0.0,
            phase_rad: 0.0,
            thd_v: 0.0,
            thd_i: 0.0,
            res_kohm: -1.0,
            cap_uf: -1.0,
            ind_mh: -1.0,
        }
    }
}

impl VendorMeasurement {
    pub fn v_span(&self) -> i32 {
        self.v_max - self.v_min
    }
    pub fn i_span(&self) -> i32 {
        self.i_max - self.i_min
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MeasurementParams {
    /// Fréquence réelle du signal (Hz).
    pub frequency_hz: f64,
    /// Résistance série totale : table constructeur + résistance de sortie
    /// calibrée, en kΩ.
    pub resistor_kohm: f64,
}

/// Lissage exponentiel des harmoniques, conservé d'une trame à l'autre
/// (comme les tableaux HormonicsV1/I1 du constructeur).
#[derive(Debug, Clone)]
pub struct ChannelAnalyzer {
    harm_v: [f64; 32],
    harm_i: [f64; 32],
    /// 31 harmoniques pour la voie principale, 7 pour la seconde voie
    /// (comportement constructeur).
    harmonics: usize,
}

impl ChannelAnalyzer {
    pub fn new(primary: bool) -> Self {
        Self { harm_v: [0.0; 32], harm_i: [0.0; 32], harmonics: if primary { 31 } else { 7 } }
    }

    pub fn reset(&mut self) {
        self.harm_v = [0.0; 32];
        self.harm_i = [0.0; 32];
    }

    pub fn harmonics_v(&self) -> &[f64] {
        &self.harm_v[1..=self.harmonics]
    }

    pub fn harmonics_i(&self) -> &[f64] {
        &self.harm_i[1..=self.harmonics]
    }

    pub fn analyze(&mut self, raw_v: &[u16], raw_i: &[u16], params: MeasurementParams) -> VendorMeasurement {
        let n = raw_v.len().min(raw_i.len());
        let mut m = VendorMeasurement::default();
        if n == 0 {
            return m;
        }
        let (v_min, v_max) = find_min_max(&raw_v[..n]);
        let (i_min, i_max) = find_min_max(&raw_i[..n]);
        m.v_min = v_min;
        m.v_max = v_max;
        m.i_min = i_min;
        m.i_max = i_max;

        m.thd_v = thd(&raw_v[..n], &mut self.harm_v, self.harmonics);
        m.thd_i = thd(&raw_i[..n], &mut self.harm_i, self.harmonics);
        let (vrms, irms) = rms(&raw_v[..n], &raw_i[..n]);
        m.vrms = vrms;
        m.irms = irms;

        if m.v_span() > 50 && m.i_span() > 25 {
            m.status = ProbeStatus::Measured;
            m.phase_rad = phase(&raw_v[..n], &raw_i[..n]);
            let thd_ok = (m.thd_v < 30.0 && m.thd_i < 10.0) || (m.thd_v < 10.0 && m.thd_i < 30.0);
            if thd_ok {
                rlc(&mut m, params);
            }
        } else if v_max < 2073 && v_min > 2023 && m.i_span() > 2500 {
            m.status = ProbeStatus::Short;
        }
        if m.v_span() > 2500 && m.i_span() < 80 {
            m.status = ProbeStatus::Open;
        }
        m
    }
}

/// Min/max en ignorant les points nuls (find_MinMax).
pub fn find_min_max(values: &[u16]) -> (i32, i32) {
    let mut min = 5000i32;
    let mut max = 0i32;
    for &x in values {
        let x = x as i32;
        if x > 0 && x < min {
            min = x;
        }
        if x > 0 && x < 5000 && x > max {
            max = x;
        }
    }
    (min, max)
}

/// THD (%) à partir d'une DFT 128 points sur un échantillon sur 8, avec les
/// valeurs impaires à zéro, exactement comme fft_hesap.
fn thd(raw: &[u16], harmonics_state: &mut [f64; 32], count: usize) -> f64 {
    let mut x = [0.0f64; DFT_LEN];
    for t in (0..DFT_LEN).step_by(2) {
        let idx = t * 4;
        let s = raw.get(idx).copied().unwrap_or(2048) as f64;
        x[t] = (s - 2048.0) / 2048.0;
    }
    let scale = 2f64.sqrt() / DFT_LEN as f64;
    let mut sum_sq = 0.0;
    for j in 1..=count {
        let w = 2.0 * PI * j as f64 / DFT_LEN as f64;
        let (mut re, mut im) = (0.0, 0.0);
        for (t, &xt) in x.iter().enumerate() {
            re += xt * (w * t as f64).cos() * scale;
            im -= xt * (w * t as f64).sin() * scale;
        }
        let mag = (re * re + im * im).sqrt();
        harmonics_state[j] += (mag - harmonics_state[j]) * HARMONIC_SMOOTHING;
        if j != 1 {
            sum_sq += harmonics_state[j] * harmonics_state[j];
        }
    }
    sum_sq.sqrt() * 100.0 / (harmonics_state[1] + 1e-8)
}

/// Valeurs efficaces autour de 2048 (Calculate_RMS).
pub fn rms(raw_v: &[u16], raw_i: &[u16]) -> (f64, f64) {
    let n = raw_v.len().min(raw_i.len());
    if n == 0 {
        return (0.0, 0.0);
    }
    let (mut sv, mut si) = (0i64, 0i64);
    for k in 0..n {
        let dv = raw_v[k] as i64 - ADC_CENTER;
        let di = raw_i[k] as i64 - ADC_CENTER;
        sv += dv * dv;
        si += di * di;
    }
    // Le constructeur divise toujours par 512.
    let div = if n >= 512 { 512.0 } else { n as f64 };
    ((sv as f64 / div).sqrt(), (si as f64 / div).sqrt())
}

/// Déphasage tension/courant par corrélation avec les tables sin/cos
/// (calculate_phase). Les sommes sont en i64 (le constructeur utilise un int
/// 32 bits qui peut déborder sur des signaux pleine échelle).
pub fn phase(raw_v: &[u16], raw_i: &[u16]) -> f64 {
    let (sin_t, cos_t) = trig_tables();
    let n = raw_v.len().min(raw_i.len()).min(512);
    let (mut vs, mut vc, mut is, mut ic) = (0i64, 0i64, 0i64, 0i64);
    for k in 0..n {
        let v = raw_v[k] as i64;
        let i = raw_i[k] as i64;
        vs += v * sin_t[k];
        vc += v * cos_t[k];
        is += i * sin_t[k];
        ic += i * cos_t[k];
    }
    if vs == 0 || vc == 0 || is == 0 || ic == 0 {
        return 0.0;
    }
    let mut p = (vc as f64).atan2(vs as f64) - (ic as f64).atan2(is as f64);
    if p > PI {
        p -= 2.0 * PI;
    } else if p < -PI {
        p += 2.0 * PI;
    }
    p
}

/// RLC_calculation : |Z| = Vrms·R/Irms, puis décomposition selon la phase.
fn rlc(m: &mut VendorMeasurement, params: MeasurementParams) {
    let f = params.frequency_hz;
    let z = m.vrms * params.resistor_kohm / (m.irms + 1e-8);
    m.cap_uf = -1.0;
    m.ind_mh = -1.0;
    m.res_kohm = z * m.phase_rad.cos();
    if m.phase_rad <= -DEG3 {
        let x = z * m.phase_rad.sin();
        m.cap_uf = -1000.0 / (2.0 * PI * f * x);
    } else if m.phase_rad >= DEG3 {
        let x = z * m.phase_rad.sin();
        m.ind_mh = x * 1_000_000.0 / (2.0 * PI * f) - 0.004_999_999_888_241_291;
    }
}

/// Lignes d'affichage R / LC / Ph identiques au constructeur (show_values).
/// `channel` = 0 pour la voie principale, 1 pour la seconde.
/// `resistor_index` = 2 pour la plage 47 R (une décimale de plus).
pub fn format_vendor_lines(m: &VendorMeasurement, channel: u8, resistor_index: u8) -> [String; 3] {
    let p = m.phase_rad;
    let lc = if m.cap_uf > 0.0 && p > -DEG92 && p < -DEG5 {
        format!("C: {}", format_cap_uf(m.cap_uf))
    } else if m.ind_mh > 0.0 && p < DEG92 && p > DEG3 {
        format!("L: {}", format_ind_mh(m.ind_mh))
    } else {
        "LC: ?".to_string()
    };

    let r_limit = if channel == 0 { DEG88 } else { DEG89 };
    let r = if m.status == ProbeStatus::Short {
        "R: 0 R (court-circuit)".to_string()
    } else if m.res_kohm > 0.0 && p > -r_limit && p < r_limit {
        format!("R: {}", format_res_kohm(m.res_kohm, resistor_index))
    } else {
        "R: ?".to_string()
    };

    let ph = if p < DEG92 && p > -DEG92 && (m.res_kohm >= 0.0 || m.cap_uf >= 0.0 || m.ind_mh >= 0.0) {
        format!("Ph: {:.1} °", p.to_degrees())
    } else {
        "Ph: ?".to_string()
    };
    [r, lc, ph]
}

pub fn format_res_kohm(r: f64, resistor_index: u8) -> String {
    if r < 1.0 {
        let ohm = r * 1000.0;
        if ohm < 100.0 && resistor_index == 2 {
            format!("{:.1} R", ohm)
        } else {
            format!("{:.0} R", ohm)
        }
    } else if r < 10.0 {
        format!("{:.2} K", r)
    } else if r < 100.0 {
        format!("{:.1} K", r)
    } else {
        format!("{:.0} K", r)
    }
}

pub fn format_cap_uf(c: f64) -> String {
    if c < 1.0 {
        format!("{:.1} nF", c * 1000.0)
    } else if c < 10.0 {
        format!("{:.2} uF", c)
    } else if c < 100.0 {
        format!("{:.1} uF", c)
    } else if c < 1000.0 {
        format!("{:.0} uF", c)
    } else {
        format!("{:.2} mF", c / 1000.0)
    }
}

pub fn format_ind_mh(l: f64) -> String {
    if l < 1.0 {
        format!("{:.0} uH", l * 1000.0)
    } else if l < 10.0 {
        format!("{:.2} mH", l)
    } else if l < 100.0 {
        format!("{:.1} mH", l)
    } else if l < 1000.0 {
        format!("{:.0} mH", l)
    } else if l < 10_000.0 {
        format!("{:.2} H", l / 1000.0)
    } else if l < 100_000.0 {
        format!("{:.1} H", l / 1000.0)
    } else {
        format!("{:.0} mH", l)
    }
}

// ---------------------------------------------------------------------------
// Comparaison (« Impedance difference »)
// ---------------------------------------------------------------------------

/// Coefficient de corrélation autour de 2048 (compare_signals).
pub fn correlation(a: &[u16], b: &[u16]) -> f64 {
    let n = a.len().min(b.len());
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for k in 0..n {
        let x = a[k] as f64 - 2048.0;
        let y = b[k] as f64 - 2048.0;
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    ab / ((aa * bb).sqrt() + 1e-12)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImpedanceComparison {
    pub corr_v: f64,
    pub corr_i: f64,
    pub ratio_v: f64,
    pub ratio_i: f64,
    /// « Impedance difference » en %.
    pub difference_pct: f64,
}

impl ImpedanceComparison {
    pub fn passes(&self, tolerance_pct: f64) -> bool {
        self.difference_pct <= tolerance_pct
    }

    pub fn label(&self) -> String {
        format!("Différence d'impédance : {}", format_difference(self.difference_pct))
    }
}

pub fn format_difference(d: f64) -> String {
    if d < 100.0 {
        format!("{:.1} %", d)
    } else if d < 1000.0 {
        format!("{:.0} %", d)
    } else {
        "<<>>".to_string()
    }
}

fn ratio(a: f64, b: f64) -> f64 {
    if a == 0.0 && b == 0.0 {
        return 1.0;
    }
    if b == 0.0 {
        return 0.0;
    }
    let r = a / b;
    if r > 1.0 {
        1.0 / r
    } else {
        r
    }
}

/// Compare deux courbes brutes (live / référence ou CH0 / CH1).
pub fn compare(
    live_v: &[u16],
    live_i: &[u16],
    live: &VendorMeasurement,
    ref_v: &[u16],
    ref_i: &[u16],
    reference: &VendorMeasurement,
) -> ImpedanceComparison {
    let both_short = live.status == ProbeStatus::Short && reference.status == ProbeStatus::Short;
    let (corr_v, ratio_v) = if both_short {
        (1.0, 1.0)
    } else {
        (correlation(live_v, ref_v), ratio(live.vrms, reference.vrms))
    };
    let ratio_i = ratio(live.irms, reference.irms);
    let corr_i = correlation(live_i, ref_i);
    let difference_pct = ((4.0 - corr_v - corr_i - ratio_i - ratio_v) * 100.0).max(0.0);
    ImpedanceComparison { corr_v, corr_i, ratio_v, ratio_i, difference_pct }
}

/// Machine d'état « contact sonde » (sesFlag / sesTimeout du constructeur) :
/// - sonde en l'air : armement ;
/// - contact : on attend 5 trames avant de comparer, 15 avant de conclure ;
/// - un seul verdict par contact, il faut relever la sonde pour réarmer.
#[derive(Debug, Clone, Default)]
pub struct ProbeSession {
    flags: [u8; 2],
    counter: u8,
    pub last_comparison: Option<ImpedanceComparison>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SessionEvent {
    None,
    Verdict { passed: bool, comparison: ImpedanceComparison },
}

impl ProbeSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn is_armed(&self) -> bool {
        self.flags.contains(&2)
    }

    pub fn in_contact(&self) -> bool {
        self.flags.contains(&1)
    }

    /// Mise à jour d'une voie (0 = principale, 1 = seconde voie / référence).
    /// Retourne vrai si la sonde vient de passer en l'air.
    pub fn update_status(&mut self, slot: usize, status: ProbeStatus) -> bool {
        let slot = slot.min(1);
        if status == ProbeStatus::Open {
            let lifted = self.flags[slot] != 2;
            self.flags[slot] = 2;
            lifted
        } else {
            if self.flags[slot] == 2 {
                self.flags[slot] = 1;
            }
            false
        }
    }

    /// À appeler une fois par trame reçue, quand la comparaison est active.
    pub fn step<F: FnOnce() -> ImpedanceComparison>(&mut self, tolerance_pct: f64, compare: F) -> SessionEvent {
        if !self.in_contact() {
            return SessionEvent::None;
        }
        if self.counter >= 5 {
            self.last_comparison = Some(compare());
        }
        self.counter = self.counter.saturating_add(1);
        if self.counter >= 15 {
            self.flags = [0, 0];
            self.counter = 0;
            if let Some(c) = self.last_comparison {
                return SessionEvent::Verdict { passed: c.passes(tolerance_pct), comparison: c };
            }
        }
        SessionEvent::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Génère une courbe comme le boîtier : V et I mesurés aux bornes du
    /// composant et de la résistance série, même échelle.
    fn synth(z_re: f64, z_im: f64, r_series_ohm: f64) -> (Vec<u16>, Vec<u16>) {
        let amp = 1600.0;
        // I·Rs = Vs·Rs/(Z+Rs), V = Vs·Z/(Z+Rs)
        let den_re = z_re + r_series_ohm;
        let den_im = z_im;
        let den2 = den_re * den_re + den_im * den_im;
        let (gi_re, gi_im) = (r_series_ohm * den_re / den2, -r_series_ohm * den_im / den2);
        let (gv_re, gv_im) = ((z_re * den_re + z_im * den_im) / den2, (z_im * den_re - z_re * den_im) / den2);
        let mut v = Vec::new();
        let mut i = Vec::new();
        for k in 0..512 {
            let t = 2.0 * PI * k as f64 / 512.0;
            let s = |g_re: f64, g_im: f64| {
                let mag = (g_re * g_re + g_im * g_im).sqrt();
                let ph = g_im.atan2(g_re);
                (2048.0 + amp * mag * (t + ph).sin()).round().clamp(0.0, 4095.0) as u16
            };
            v.push(s(gv_re, gv_im));
            i.push(s(gi_re, gi_im));
        }
        (v, i)
    }

    fn measure(v: &[u16], i: &[u16], f: f64, r_kohm: f64) -> VendorMeasurement {
        let mut a = ChannelAnalyzer::new(true);
        let p = MeasurementParams { frequency_hz: f, resistor_kohm: r_kohm };
        let mut m = a.analyze(v, i, p);
        for _ in 0..20 {
            m = a.analyze(v, i, p);
        }
        m
    }

    #[test]
    fn tables_match_vendor_samples() {
        let (s, c) = trig_tables();
        assert_eq!((s[1], s[5], s[100], s[200], s[300], s[511]), (25, 126, 1927, 1299, -1052, -25));
        assert_eq!((c[0], c[7], c[300]), (2047, 2039, -1756));
        assert_eq!(s.iter().map(|x| x.abs()).sum::<i64>(), 667206);
    }

    #[test]
    fn resistor_is_measured() {
        let (v, i) = synth(1000.0, 0.0, 1000.0);
        let m = measure(&v, &i, 100.0, 1.0);
        assert_eq!(m.status, ProbeStatus::Measured);
        assert!(m.phase_rad.abs() < 0.02, "phase {}", m.phase_rad);
        assert!((m.res_kohm - 1.0).abs() < 0.02, "R {}", m.res_kohm);
        let lines = format_vendor_lines(&m, 0, 1);
        assert!(lines[0].starts_with("R: "), "{:?}", lines);
        assert_eq!(lines[1], "LC: ?");
    }

    #[test]
    fn capacitor_is_measured() {
        let f = 100.0;
        let c = 1.0e-6;
        let xc = -1.0 / (2.0 * PI * f * c); // ~ -1591 Ω
        let (v, i) = synth(0.0, xc, 1000.0);
        let m = measure(&v, &i, f, 1.0);
        assert_eq!(m.status, ProbeStatus::Measured);
        assert!(m.phase_rad < -1.4, "phase {}", m.phase_rad);
        assert!((m.cap_uf - 1.0).abs() < 0.05, "C {}", m.cap_uf);
        assert!(format_vendor_lines(&m, 0, 1)[1].starts_with("C: "));
    }

    #[test]
    fn inductor_is_measured() {
        let f = 500.0;
        let l = 0.5; // H -> XL ≈ 1571 Ω
        let (v, i) = synth(0.0, 2.0 * PI * f * l, 1000.0);
        let m = measure(&v, &i, f, 1.0);
        assert!(m.phase_rad > 1.4);
        assert!((m.ind_mh - 500.0).abs() < 15.0, "L {}", m.ind_mh);
    }

    #[test]
    fn open_and_short_status() {
        let (v, i) = synth(1.0e9, 0.0, 1000.0);
        assert_eq!(measure(&v, &i, 10.0, 1.0).status, ProbeStatus::Open);
        let (v, i) = synth(0.0, 0.0, 1000.0);
        assert_eq!(measure(&v, &i, 10.0, 1.0).status, ProbeStatus::Short);
    }

    #[test]
    fn identical_curves_have_zero_difference() {
        let (v, i) = synth(1500.0, -800.0, 1000.0);
        let m = measure(&v, &i, 10.0, 1.0);
        let c = compare(&v, &i, &m, &v, &i, &m);
        assert!(c.difference_pct < 0.01);
        let (v2, i2) = synth(300.0, 0.0, 1000.0);
        let m2 = measure(&v2, &i2, 10.0, 1.0);
        assert!(compare(&v, &i, &m, &v2, &i2, &m2).difference_pct > 20.0);
    }

    #[test]
    fn session_needs_probe_lift_and_15_frames() {
        let mut s = ProbeSession::new();
        let cmp = ImpedanceComparison { corr_v: 1.0, corr_i: 1.0, ratio_v: 1.0, ratio_i: 1.0, difference_pct: 0.0 };
        // Pas armé : aucun verdict.
        s.update_status(0, ProbeStatus::Measured);
        for _ in 0..20 {
            assert_eq!(s.step(10.0, || cmp), SessionEvent::None);
        }
        assert!(s.update_status(0, ProbeStatus::Open));
        s.update_status(0, ProbeStatus::Measured);
        let mut verdicts = 0;
        for _ in 0..30 {
            if let SessionEvent::Verdict { passed, .. } = s.step(10.0, || cmp) {
                assert!(passed);
                verdicts += 1;
            }
        }
        assert_eq!(verdicts, 1);
    }

    #[test]
    fn vendor_formats() {
        assert_eq!(format_res_kohm(0.0481, 2), "48.1 R");
        assert_eq!(format_res_kohm(4.7, 1), "4.70 K");
        assert_eq!(format_cap_uf(0.1), "100.0 nF");
        assert_eq!(format_cap_uf(470.0), "470 uF");
        assert_eq!(format_ind_mh(0.5), "500 uH");
        assert_eq!(format_difference(1234.0), "<<>>");
    }
}
