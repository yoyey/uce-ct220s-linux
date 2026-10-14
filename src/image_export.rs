// src/image_export.rs
//
// Rendu hors écran des courbes (export image, rapport PDF) et export texte
// au format du logiciel constructeur.

use crate::curve::CurveData;
use image::{Rgb, RgbImage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlotScale {
    /// Points ADC 0..4095 sur les deux axes (affichage constructeur).
    Fixed,
    /// Courbe centrée et agrandie (échelle commune V/I).
    Auto,
}

pub struct Trace<'a> {
    pub curve: &'a CurveData,
    pub color: [u8; 3],
    pub width: i32,
}

fn put(img: &mut RgbImage, x: i32, y: i32, c: Rgb<u8>) {
    if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
        img.put_pixel(x as u32, y as u32, c);
    }
}

fn line(img: &mut RgbImage, (x0, y0): (f32, f32), (x1, y1): (f32, f32), c: Rgb<u8>, width: i32) {
    let steps = ((x1 - x0).abs().max((y1 - y0).abs()).ceil() as i32).max(1);
    let r = width / 2;
    for s in 0..=steps {
        let t = s as f32 / steps as f32;
        let x = (x0 + (x1 - x0) * t).round() as i32;
        let y = (y0 + (y1 - y0) * t).round() as i32;
        for dy in -r..=r {
            for dx in -r..=r {
                put(img, x + dx, y + dy, c);
            }
        }
    }
}

/// Dessine une ou plusieurs courbes V-I sur une image carrée à fond blanc.
pub fn render_xy(traces: &[Trace], size: u32, scale: PlotScale) -> RgbImage {
    let mut img = RgbImage::from_pixel(size, size, Rgb([255, 255, 255]));
    let s = size as f32;
    let grid = Rgb([205, 205, 205]);
    for k in 0..=10 {
        let p = k as f32 * (s - 1.0) / 10.0;
        line(&mut img, (p, 0.0), (p, s - 1.0), grid, 1);
        line(&mut img, (0.0, p), (s - 1.0, p), grid, 1);
    }
    let axis = Rgb([0, 120, 0]);
    line(&mut img, (s / 2.0, 0.0), (s / 2.0, s - 1.0), axis, 1);
    line(&mut img, (0.0, s / 2.0), (s - 1.0, s / 2.0), axis, 1);

    for tr in traces {
        let pts: Vec<(f32, f32)> = match scale {
            PlotScale::Fixed if tr.curve.has_raw() => tr
                .curve
                .raw_voltage
                .iter()
                .zip(&tr.curve.raw_current)
                .filter(|(v, i)| **v > 0 && **i > 0)
                .map(|(&v, &i)| (v as f32 / 4095.0 * (s - 1.0), (1.0 - i as f32 / 4095.0) * (s - 1.0)))
                .collect(),
            _ => tr
                .curve
                .voltage
                .iter()
                .zip(&tr.curve.current)
                .map(|(&v, &i)| (s / 2.0 + v * s * 0.45, s / 2.0 - i * s * 0.45))
                .collect(),
        };
        let c = Rgb(tr.color);
        for w in pts.windows(2) {
            line(&mut img, w[0], w[1], c, tr.width);
        }
    }
    img
}

/// Enregistre une image ; le format dépend de l'extension (png, jpg, bmp, gif).
pub fn save_image(img: &RgbImage, path: &str) -> Result<(), String> {
    img.save(path).map_err(|e| format!("Erreur sauvegarde image {}: {}", path, e))
}

/// Export texte constructeur (« Save As Text ») : 512 lignes de tension
/// (CH1) puis 512 lignes de courant (CH2), valeurs ADC brutes.
pub fn curve_to_text(curve: &CurveData) -> Result<String, String> {
    if !curve.has_raw() {
        return Err("Courbe sans données brutes".to_string());
    }
    let mut s = String::new();
    for v in &curve.raw_voltage {
        s.push_str(&format!("{}\r\n", v));
    }
    for i in &curve.raw_current {
        s.push_str(&format!("{}\r\n", i));
    }
    Ok(s)
}

pub fn save_curve_text(curve: &CurveData, path: &str) -> Result<(), String> {
    std::fs::write(path, curve_to_text(curve)?).map_err(|e| format!("{}: {}", path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_export_layout() {
        let c = CurveData::from_raw(0, vec![1, 2, 3], vec![4, 5, 6]).unwrap();
        assert_eq!(curve_to_text(&c).unwrap(), "1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n");
    }

    #[test]
    fn render_does_not_panic_on_edges() {
        let c = CurveData::from_raw(0, vec![0, 4095, 2048, 1], vec![4095, 0, 2048, 1]).unwrap();
        let img = render_xy(&[Trace { curve: &c, color: [255, 0, 0], width: 3 }], 50, PlotScale::Fixed);
        assert_eq!(img.width(), 50);
        let img = render_xy(&[Trace { curve: &c, color: [255, 0, 0], width: 3 }], 50, PlotScale::Auto);
        assert_eq!(img.height(), 50);
    }
}
