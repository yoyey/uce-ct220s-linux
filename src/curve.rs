// src/curve.rs

use crate::config::POINTS_PER_CURVE;
use byteorder::{ByteOrder, LittleEndian};

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

/// Parse les bytes bruts d'une courbe + normalisation comme dans ton Python.
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

    currents.sort_by(|a, b| a.partial_cmp(b).unwrap());
    voltages.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let median_current = currents[currents.len() / 2];
    let median_voltage = voltages[voltages.len() / 2];

    let mut i_vec = Vec::with_capacity(pairs.len());
    let mut v_vec = Vec::with_capacity(pairs.len());

    for (c, v) in pairs {
        i_vec.push(c - median_current);
        v_vec.push(v - median_voltage);
    }

    let max_i = i_vec.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
    let max_v = v_vec.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
    let scale = max_i.max(max_v).max(1.0);

    let v_norm: Vec<f32> = v_vec.iter().map(|x| x / scale).collect();
    let i_norm: Vec<f32> = i_vec.iter().map(|x| x / scale).collect();

    Ok((v_norm, i_norm))
}

