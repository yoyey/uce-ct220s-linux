// src/curve.rs

use crate::config::POINTS_PER_CURVE;
use byteorder::{ByteOrder, LittleEndian};
use std::f32::consts::PI;

#[derive(Clone)]
pub struct CurveData {
    pub voltage: Vec<f32>,
    pub current: Vec<f32>,
    pub channel: u8,
}

pub struct DualCurveData {
    pub channel0: Option<CurveData>,
    pub channel1: Option<CurveData>,
}

impl DualCurveData {
    pub fn new() -> Self {
        Self {
            channel0: None,
            channel1: None,
        }
    }
}

/// Résultats électriques dérivés d'une courbe V-I.
///
/// Les trois composants sont optionnels afin de pouvoir reproduire l'affichage
/// du logiciel d'origine : par exemple un condensateur affiche C et Ph, tandis
/// que R et L restent vides.
#[derive(Debug, Clone, Copy, Default)]
pub struct ElectricalValues {
    pub resistance_ohm: Option<f32>,
    pub capacitance_f: Option<f32>,
    pub inductance_h: Option<f32>,
    pub phase_deg: Option<f32>,
    pub impedance_ohm: Option<f32>,
}

/// Parse les bytes bruts d'une courbe + normalisation comme dans le script Python.
/// Retourne (V_norm, I_norm).
pub fn parse_and_normalize_curve_data(data_bytes: &[u8]) -> Result<(Vec<f32>, Vec<f32>), String> {
    let raw_len = (POINTS_PER_CURVE * 4).min(data_bytes.len());
    let raw = &data_bytes[..raw_len];

    let mut pairs = Vec::with_capacity(POINTS_PER_CURVE);

    for i in 0..(raw.len() / 4).min(POINTS_PER_CURVE) {
        let offset = i * 4;
        let current_raw = LittleEndian::read_u16(&raw[offset..offset + 2]) as f32;
        let voltage_raw = LittleEndian::read_u16(&raw[offset + 2..offset + 4]) as f32;
        pairs.push((current_raw, voltage_raw));
    }

    if pairs.is_empty() {
        return Err("Aucune paire de données extraite".to_string());
    }

    let mut currents: Vec<f32> = pairs.iter().map(|(c, _)| *c).collect();
    let mut voltages: Vec<f32> = pairs.iter().map(|(_, v)| *v).collect();

    currents.sort_by(|a, b| a.total_cmp(b));
    voltages.sort_by(|a, b| a.total_cmp(b));

    // Pour 512 points, np.median() fait la moyenne des deux valeurs centrales.
    fn median(sorted: &[f32]) -> f32 {
        let n = sorted.len();
        if n % 2 == 0 {
            (sorted[n / 2 - 1] + sorted[n / 2]) * 0.5
        } else {
            sorted[n / 2]
        }
    }

    let median_current = median(&currents);
    let median_voltage = median(&voltages);

    let mut i_vec = Vec::with_capacity(pairs.len());
    let mut v_vec = Vec::with_capacity(pairs.len());

    for (c, v) in pairs {
        i_vec.push(c - median_current);
        v_vec.push(v - median_voltage);
    }

    // Echelle COMMUNE, indispensable pour conserver le rapport V/I et donc
    // la forme horizontale / verticale du traceur.
    let max_i = i_vec.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
    let max_v = v_vec.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
    let scale = max_i.max(max_v).max(1.0);

    let v_norm: Vec<f32> = v_vec.iter().map(|x| x / scale).collect();
    let i_norm: Vec<f32> = i_vec.iter().map(|x| x / scale).collect();

    Ok((v_norm, i_norm))
}

/// Amplitude et phase du fondamental d'un signal contenant une période complète.
/// La convention choisie est x[n] = A*cos(theta + phase).
fn fundamental(samples: &[f32]) -> Option<(f32, f32)> {
    if samples.len() < 8 {
        return None;
    }

    let n = samples.len() as f32;
    let mean = samples.iter().sum::<f32>() / n;

    let mut cos_sum = 0.0f32;
    let mut sin_sum = 0.0f32;

    for (k, &sample) in samples.iter().enumerate() {
        let x = sample - mean;
        let theta = 2.0 * PI * (k as f32) / n;
        cos_sum += x * theta.cos();
        sin_sum += x * theta.sin();
    }

    let a = 2.0 * cos_sum / n;
    let b = 2.0 * sin_sum / n;
    let amplitude = (a * a + b * b).sqrt();

    if amplitude < 1.0e-6 {
        return None;
    }

    let phase = (-b).atan2(a);
    Some((amplitude, phase))
}

fn wrap_phase(mut phase: f32) -> f32 {
    while phase > PI {
        phase -= 2.0 * PI;
    }
    while phase <= -PI {
        phase += 2.0 * PI;
    }
    phase
}

/// Calcule R/C/L et la phase à partir de la courbe normalisée.
///
/// Le CT220S mesure le courant via la résistance source. Puisque V et I sont
/// normalisés avec le MEME facteur, leur rapport d'amplitude reste intact :
/// |Z| ~= Rsource * Av/Ai.
///
/// Convention :
///   phase < 0  -> capacitif
///   phase > 0  -> inductif
///   phase ~ 0  -> résistif
///
/// On n'affiche qu'une nature de composant à la fois, comme le logiciel :
/// R pour une charge principalement résistive, C pour capacitive, L pour inductive.
pub fn calculate_electrical_values(
    curve: &CurveData,
    frequency_hz: f32,
    source_resistance_ohm: f32,
) -> ElectricalValues {
    if frequency_hz <= 0.0 || source_resistance_ohm <= 0.0 {
        return ElectricalValues::default();
    }

    let n = curve.voltage.len().min(curve.current.len());
    if n < 8 {
        return ElectricalValues::default();
    }

    let current_fundamental = fundamental(&curve.current[..n]);
    let (amp_i, phase_i) = match current_fundamental {
        Some(v) => v,
        None => return ElectricalValues::default(),
    };

    if amp_i < 1.0e-6 {
        // Quasiment aucun courant : circuit ouvert / mesure inexploitable.
        return ElectricalValues::default();
    }

    let voltage_fundamental = fundamental(&curve.voltage[..n]);
    let (amp_v, phase_v) = match voltage_fundamental {
        Some(v) => v,
        None => {
            // Tension quasi nulle alors que le courant est présent : court-circuit.
            return ElectricalValues {
                resistance_ohm: Some(0.0),
                capacitance_f: None,
                inductance_h: None,
                phase_deg: None,
                impedance_ohm: Some(0.0),
            };
        }
    };

    let z_ohm = source_resistance_ohm * (amp_v / amp_i);
    if !z_ohm.is_finite() {
        return ElectricalValues::default();
    }

    // Sous ce seuil, on affiche explicitement 0 Ω comme un court-circuit.
    // Le seuil pourra être affiné après calibration avec des captures réelles.
    const SHORT_CIRCUIT_THRESHOLD_OHM: f32 = 1.0;
    if z_ohm <= SHORT_CIRCUIT_THRESHOLD_OHM {
        return ElectricalValues {
            resistance_ohm: Some(0.0),
            capacitance_f: None,
            inductance_h: None,
            phase_deg: None,
            impedance_ohm: Some(z_ohm.max(0.0)),
        };
    }

    // Phase de l'impédance : V par rapport à I.
    let phase_rad = wrap_phase(phase_v - phase_i);
    let phase_deg = phase_rad.to_degrees();

    let mut result = ElectricalValues {
        phase_deg: Some(phase_deg),
        impedance_ohm: Some(z_ohm),
        ..ElectricalValues::default()
    };

    // Décomposition complexe de l'impédance :
    //   R = |Z| cos(phi)
    //   X = |Z| sin(phi)
    // Cela permet d'afficher l'ESR d'un condensateur ou la résistance série
    // d'une inductance, en plus de C/L.
    let r_series = z_ohm * phase_rad.cos();
    let x_ohm = z_ohm * phase_rad.sin();

    // Zone morte autour de 0° afin de ne pas transformer une résistance avec
    // quelques degrés de bruit en C ou en L.
    const REACTIVE_THRESHOLD_DEG: f32 = 15.0;
    // La composante résistive est affichée même lorsqu'elle vaut exactement
    // 0 Ω. Cela permet notamment d'afficher l'ESR nul d'un condensateur idéal
    // ou la résistance série nulle d'une inductance idéale.

    if phase_deg < -REACTIVE_THRESHOLD_DEG {
        // Composant capacitif : Xc négatif.
        if r_series.is_finite() && r_series >= 0.0 {
            result.resistance_ohm = Some(r_series);
        }
        if x_ohm < -1.0e-6 {
            let c = -1.0 / (2.0 * PI * frequency_hz * x_ohm);
            if c.is_finite() && c > 0.0 {
                result.capacitance_f = Some(c);
            }
        }
    } else if phase_deg > REACTIVE_THRESHOLD_DEG {
        // Composant inductif : Xl positif.
        if r_series.is_finite() && r_series >= 0.0 {
            result.resistance_ohm = Some(r_series);
        }
        if x_ohm > 1.0e-6 {
            let l = x_ohm / (2.0 * PI * frequency_hz);
            if l.is_finite() && l > 0.0 {
                result.inductance_h = Some(l);
            }
        }
    } else {
        // Principalement résistif.
        if r_series.is_finite() && r_series >= 0.0 {
            result.resistance_ohm = Some(r_series);
        }
    }

    result
}

/// Formate une résistance avec une unité lisible.
pub fn format_resistance(value_ohm: f32) -> String {
    if value_ohm >= 1_000_000.0 {
        format!("{:.2} MΩ", value_ohm / 1_000_000.0)
    } else if value_ohm >= 1_000.0 {
        format!("{:.2} kΩ", value_ohm / 1_000.0)
    } else {
        format!("{:.1} Ω", value_ohm)
    }
}

/// Formate une capacité avec l'unité la plus adaptée.
pub fn format_capacitance(value_f: f32) -> String {
    if value_f >= 1.0e-3 {
        format!("{:.2} mF", value_f * 1.0e3)
    } else if value_f >= 1.0e-6 {
        format!("{:.2} µF", value_f * 1.0e6)
    } else if value_f >= 1.0e-9 {
        format!("{:.2} nF", value_f * 1.0e9)
    } else {
        format!("{:.2} pF", value_f * 1.0e12)
    }
}

/// Formate une inductance avec l'unité la plus adaptée.
pub fn format_inductance(value_h: f32) -> String {
    if value_h >= 1.0 {
        format!("{:.2} H", value_h)
    } else if value_h >= 1.0e-3 {
        format!("{:.2} mH", value_h * 1.0e3)
    } else if value_h >= 1.0e-6 {
        format!("{:.2} µH", value_h * 1.0e6)
    } else {
        format!("{:.2} nH", value_h * 1.0e9)
    }
}
