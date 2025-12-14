// src/image_export.rs

use crate::curve::{CurveData, DualCurveData};
use image::{ImageBuffer, Rgba};

pub fn save_curve_as_png(curve: &CurveData, filename: &str) -> Result<(), String> {
    let width = 800;
    let height = 800;

    let mut img = ImageBuffer::from_fn(width, height, |_, _| {
        Rgba([255u8, 255u8, 255u8, 255u8])
    });

    let center_x = width as f32 / 2.0;
    let center_y = height as f32 / 2.0;
    let scale = (width.min(height) as f32) * 0.45;

    let grid_color = Rgba([200u8, 200u8, 200u8, 255u8]);
    for i in -10..=10 {
        let offset = (i as f32) * scale / 10.0;

        let x = (center_x + offset) as i32;
        if x >= 0 && x < width as i32 {
            for y in 0..height {
                if let Some(pixel) = img.get_pixel_mut_checked(x as u32, y) {
                    *pixel = grid_color;
                }
            }
        }

        let y = (center_y + offset) as i32;
        if y >= 0 && y < height as i32 {
            for x in 0..width {
                if let Some(pixel) = img.get_pixel_mut_checked(x, y as u32) {
                    *pixel = grid_color;
                }
            }
        }
    }

    let axis_color = Rgba([0u8, 0u8, 0u8, 255u8]);

    let cy = center_y as i32;
    for x in 0..width {
        for dy in -1..=1 {
            let y = cy + dy;
            if y >= 0 && y < height as i32 {
                if let Some(pixel) = img.get_pixel_mut_checked(x, y as u32) {
                    *pixel = axis_color;
                }
            }
        }
    }

    let cx = center_x as i32;
    for y in 0..height {
        for dx in -1..=1 {
            let x = cx + dx;
            if x >= 0 && x < width as i32 {
                if let Some(pixel) = img.get_pixel_mut_checked(x as u32, y) {
                    *pixel = axis_color;
                }
            }
        }
    }

    let curve_color = Rgba([0u8, 100u8, 255u8, 255u8]);

    for i in 0..curve.voltage.len() {
        let v = curve.voltage[i];
        let c = curve.current[i];

        let x = (center_x + v * scale) as i32;
        let y = (center_y - c * scale) as i32;

        for dy in -1..=1 {
            for dx in -1..=1 {
                let px = x + dx;
                let py = y + dy;

                if px >= 0 && px < width as i32 && py >= 0 && py < height as i32 {
                    if let Some(pixel) = img.get_pixel_mut_checked(px as u32, py as u32) {
                        *pixel = curve_color;
                    }
                }
            }
        }
    }

    img.save(filename)
        .map_err(|e| format!("Erreur sauvegarde PNG: {}", e))?;

    println!("Image sauvegardée : {}", filename);
    Ok(())
}

pub fn save_dual_curves_as_png(data: &DualCurveData, filename: &str) -> Result<(), String> {
    let width = 1600;
    let height = 800;

    let mut img = ImageBuffer::from_fn(width, height, |_, _| {
        Rgba([255u8, 255u8, 255u8, 255u8])
    });

    if let Some(ch0) = &data.channel0 {
        draw_curve_to_image(
            &mut img,
            ch0,
            0,
            0,
            800,
            800,
            Rgba([255u8, 100u8, 0u8, 255u8]),
        );
    }

    if let Some(ch1) = &data.channel1 {
        draw_curve_to_image(
            &mut img,
            ch1,
            800,
            0,
            800,
            800,
            Rgba([0u8, 100u8, 255u8, 255u8]),
        );
    }

    img.save(filename)
        .map_err(|e| format!("Erreur sauvegarde PNG: {}", e))?;

    println!("Image dual sauvegardée : {}", filename);
    Ok(())
}

fn draw_curve_to_image(
    img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>,
    curve: &CurveData,
    offset_x: u32,
    offset_y: u32,
    w: u32,
    h: u32,
    curve_color: Rgba<u8>,
) {
    let center_x = offset_x as f32 + w as f32 / 2.0;
    let center_y = offset_y as f32 + h as f32 / 2.0;
    let scale = (w.min(h) as f32) * 0.45;

    let grid_color = Rgba([200u8, 200u8, 200u8, 255u8]);
    for i in -10..=10 {
        let off = (i as f32) * scale / 10.0;

        let x = (center_x + off) as i32;
        if x >= offset_x as i32 && x < (offset_x + w) as i32 {
            for y in offset_y..(offset_y + h) {
                if let Some(pixel) = img.get_pixel_mut_checked(x as u32, y) {
                    *pixel = grid_color;
                }
            }
        }

        let y = (center_y + off) as i32;
        if y >= offset_y as i32 && y < (offset_y + h) as i32 {
            for x in offset_x..(offset_x + w) {
                if let Some(pixel) = img.get_pixel_mut_checked(x, y as u32) {
                    *pixel = grid_color;
                }
            }
        }
    }

    let axis_color = Rgba([0u8, 0u8, 0u8, 255u8]);
    let cy = center_y as i32;
    for x in offset_x..(offset_x + w) {
        for dy in -1..=1 {
            let y = cy + dy;
            if y >= offset_y as i32 && y < (offset_y + h) as i32 {
                if let Some(pixel) = img.get_pixel_mut_checked(x, y as u32) {
                    *pixel = axis_color;
                }
            }
        }
    }

    let cx = center_x as i32;
    for y in offset_y..(offset_y + h) {
        for dx in -1..=1 {
            let x = cx + dx;
            if x >= offset_x as i32 && x < (offset_x + w) as i32 {
                if let Some(pixel) = img.get_pixel_mut_checked(x as u32, y) {
                    *pixel = axis_color;
                }
            }
        }
    }

    for i in 0..curve.voltage.len() {
        let v = curve.voltage[i];
        let c = curve.current[i];

        let x = (center_x + v * scale) as i32;
        let y = (center_y - c * scale) as i32;

        for dy in -1..=1 {
            for dx in -1..=1 {
                let px = x + dx;
                let py = y + dy;

                if px >= offset_x as i32
                    && px < (offset_x + w) as i32
                    && py >= offset_y as i32
                    && py < (offset_y + h) as i32
                {
                    if let Some(pixel) = img.get_pixel_mut_checked(px as u32, py as u32) {
                        *pixel = curve_color;
                    }
                }
            }
        }
    }
}

