//! Rapport de test PDF (équivalent du rapport PdfSharp du logiciel
//! constructeur) généré sans dépendance externe : polices standard Helvetica,
//! images RGB compressées (Flate).

use crate::database::{TestDatabase, TestRecord};
use crate::curve::CurveData;
use crate::image_export::{render_xy, PlotScale, Trace};
use image::{imageops, Rgb, RgbImage};

const PAGE_W: f64 = 595.0;
const PAGE_H: f64 = 842.0;

struct PdfImage {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

#[derive(Default)]
struct Page {
    ops: String,
    images: Vec<PdfImage>,
}

fn encode_text(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        let code = ch as u32;
        let b = if code < 0x80 || (0xA0..=0xFF).contains(&code) { code as u8 } else { b'?' };
        match b {
            b'(' | b')' | b'\\' => {
                out.push('\\');
                out.push(b as char);
            }
            32..=126 => out.push(b as char),
            _ => out.push_str(&format!("\\{:03o}", b)),
        }
    }
    out
}

impl Page {
    /// `y` mesuré depuis le haut de la page (comme PdfSharp).
    fn text(&mut self, x: f64, y: f64, size: f64, bold: bool, rgb: (f64, f64, f64), s: &str) {
        let font = if bold { "F2" } else { "F1" };
        self.ops.push_str(&format!(
            "BT {:.3} {:.3} {:.3} rg /{} {:.1} Tf {:.2} {:.2} Td ({}) Tj ET\n",
            rgb.0,
            rgb.1,
            rgb.2,
            font,
            size,
            x,
            PAGE_H - y - size,
            encode_text(s)
        ));
    }

    fn text_centered(&mut self, y: f64, size: f64, bold: bool, s: &str) {
        let w = s.chars().count() as f64 * size * 0.52;
        self.text((PAGE_W - w) / 2.0, y, size, bold, (0.0, 0.0, 0.0), s);
    }

    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, rgb: (f64, f64, f64), fill: bool) {
        let op = if fill { "f" } else { "S" };
        let col = if fill { "rg" } else { "RG" };
        self.ops.push_str(&format!(
            "{:.3} {:.3} {:.3} {} {:.2} {:.2} {:.2} {:.2} re {}\n",
            rgb.0,
            rgb.1,
            rgb.2,
            col,
            x,
            PAGE_H - y - h,
            w,
            h,
            op
        ));
    }

    fn line(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) {
        self.ops.push_str(&format!(
            "0 0 0 RG 1 w {:.2} {:.2} m {:.2} {:.2} l S\n",
            x0,
            PAGE_H - y0,
            x1,
            PAGE_H - y1
        ));
    }

    fn image(&mut self, img: &RgbImage, x: f64, y: f64, w: f64, h: f64) {
        let idx = self.images.len();
        self.images.push(PdfImage { width: img.width(), height: img.height(), data: img.as_raw().clone() });
        self.ops.push_str(&format!("q {:.2} 0 0 {:.2} {:.2} {:.2} cm /Im{} Do Q\n", w, h, x, PAGE_H - y - h, idx));
    }
}

#[derive(Default)]
pub struct PdfDoc {
    pages: Vec<Page>,
}

impl PdfDoc {
    fn new_page(&mut self) -> &mut Page {
        self.pages.push(Page::default());
        self.pages.last_mut().unwrap()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets: Vec<usize> = Vec::new();
        // Numérotation : 1 catalogue, 2 pages, 3/4 polices, puis par page :
        // page, contenu, images.
        let mut page_ids = Vec::new();
        let mut next = 5usize;
        let mut layout = Vec::new();
        for p in &self.pages {
            let page_id = next;
            let content_id = next + 1;
            let image_ids: Vec<usize> = (0..p.images.len()).map(|k| next + 2 + k).collect();
            next += 2 + p.images.len();
            page_ids.push(page_id);
            layout.push((page_id, content_id, image_ids));
        }
        let total = next - 1;
        let mut objects: Vec<Vec<u8>> = vec![Vec::new(); total + 1];
        objects[1] = b"<< /Type /Catalog /Pages 2 0 R >>".to_vec();
        let kids: Vec<String> = page_ids.iter().map(|i| format!("{} 0 R", i)).collect();
        objects[2] = format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), page_ids.len()).into_bytes();
        objects[3] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec();
        objects[4] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".to_vec();
        for (p, (page_id, content_id, image_ids)) in self.pages.iter().zip(&layout) {
            let xobjects: Vec<String> = image_ids.iter().enumerate().map(|(k, id)| format!("/Im{} {} 0 R", k, id)).collect();
            objects[*page_id] = format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> /XObject << {} >> >> /Contents {} 0 R >>",
                PAGE_W, PAGE_H, xobjects.join(" "), content_id
            )
            .into_bytes();
            let stream = miniz_oxide::deflate::compress_to_vec_zlib(p.ops.as_bytes(), 6);
            let mut o = format!("<< /Length {} /Filter /FlateDecode >>\nstream\n", stream.len()).into_bytes();
            o.extend_from_slice(&stream);
            o.extend_from_slice(b"\nendstream");
            objects[*content_id] = o;
            for (img, id) in p.images.iter().zip(image_ids) {
                let data = miniz_oxide::deflate::compress_to_vec_zlib(&img.data, 6);
                let mut o = format!(
                    "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
                    img.width, img.height, data.len()
                )
                .into_bytes();
                o.extend_from_slice(&data);
                o.extend_from_slice(b"\nendstream");
                objects[*id] = o;
            }
        }
        offsets.push(0);
        for (id, body) in objects.iter().enumerate().skip(1) {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", id).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", total + 1).as_bytes());
        for off in offsets.iter().skip(1) {
            out.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n", total + 1, xref).as_bytes(),
        );
        out
    }
}

pub struct ReportInfo {
    pub test_name: String,
    pub device_name: String,
    pub serial: String,
    pub date: String,
}

/// Image 300×300 centrée sur le point testé, avec une croix rouge.
fn board_crop(board: &RgbImage, x: i32, y: i32) -> RgbImage {
    let half = 150i32;
    let mut out = RgbImage::from_pixel(300, 300, Rgb([255, 255, 255]));
    for dy in 0..300 {
        for dx in 0..300 {
            let sx = x - half + dx;
            let sy = y - half + dy;
            if sx >= 0 && sy >= 0 && (sx as u32) < board.width() && (sy as u32) < board.height() {
                out.put_pixel(dx as u32, dy as u32, *board.get_pixel(sx as u32, sy as u32));
            }
        }
    }
    let red = Rgb([255, 0, 0]);
    for d in 3..=18 {
        for w in -1..=1 {
            for (px, py) in [(150 + w, 150 - d), (150 + w, 150 + d), (150 - d, 150 + w), (150 + d, 150 + w)] {
                out.put_pixel(px as u32, py as u32, red);
            }
        }
    }
    out
}

fn record_image(rec: &TestRecord) -> RgbImage {
    let live = CurveData::from_raw(0, rec.live_voltage.clone(), rec.live_current.clone()).ok();
    let reference = CurveData::from_raw(1, rec.ref_voltage.clone(), rec.ref_current.clone()).ok();
    let mut traces = Vec::new();
    if let Some(r) = &reference {
        traces.push(Trace { curve: r, color: [70, 120, 255], width: 3 });
    }
    if let Some(l) = &live {
        traces.push(Trace { curve: l, color: [230, 120, 0], width: 3 });
    }
    render_xy(&traces, 300, PlotScale::Fixed)
}

pub fn build_report(info: &ReportInfo, db: &TestDatabase, board: Option<&RgbImage>) -> PdfDoc {
    let mut doc = PdfDoc::default();
    let (tests, tested, failed) = db.statistics();
    {
        let p = doc.new_page();
        p.text_centered(90.0, 20.0, true, "Rapport de test");
        let rows = [
            ("Date", info.date.clone()),
            ("Nom du test", info.test_name.clone()),
            ("Points de test (base)", db.points.len().to_string()),
            ("Nombre de tests", tests.to_string()),
            ("Points testés", tested.to_string()),
            ("Points en échec", failed.to_string()),
            ("Points conformes", (tested - failed).to_string()),
            ("Appareil", info.device_name.clone()),
            ("Numéro de série", info.serial.clone()),
        ];
        for (k, (label, value)) in rows.iter().enumerate() {
            let y = 140.0 + 20.0 * k as f64;
            p.text(50.0, y, 11.0, true, (0.0, 0.0, 0.0), label);
            let col = match k {
                5 => (0.85, 0.0, 0.0),
                6 => (0.0, 0.55, 0.0),
                _ => (0.0, 0.0, 0.0),
            };
            p.text(190.0, y, 11.0, false, col, &format!(": {}", value));
        }
        p.text(420.0, 450.0, 11.0, true, (0.0, 0.0, 0.0), "Signature");
        p.line(420.0, 510.0, 540.0, 510.0);
    }

    if let Some(b) = board {
        let p = doc.new_page();
        p.text(50.0, 80.0, 12.0, true, (0.0, 0.0, 0.0), "Carte testée");
        let mut img = b.clone();
        for (k, pt) in db.points.iter().enumerate() {
            let col = match db.results.get(k).copied().flatten() {
                Some(true) => Rgb([0, 200, 0]),
                Some(false) => Rgb([230, 0, 0]),
                None => Rgb([255, 200, 0]),
            };
            for dy in -6i32..=6 {
                for dx in -6i32..=6 {
                    if dx * dx + dy * dy <= 36 {
                        let (x, y) = (pt.x + dx, pt.y + dy);
                        if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
                            img.put_pixel(x as u32, y as u32, col);
                        }
                    }
                }
            }
        }
        let max_w = PAGE_W - 100.0;
        let max_h = PAGE_H - 190.0;
        let k = (max_w / img.width() as f64).min(max_h / img.height() as f64);
        // Limite la taille embarquée.
        let target_w = (img.width() as f64).min(1400.0);
        if (img.width() as f64) > target_w {
            let h = (img.height() as f64 * target_w / img.width() as f64) as u32;
            img = imageops::resize(&img, target_w as u32, h.max(1), imageops::FilterType::Triangle);
        }
        let (w, h) = (b.width() as f64 * k, b.height() as f64 * k);
        p.image(&img, 50.0, 100.0, w, h);
    }

    for (n, rec) in db.records.iter().enumerate() {
        if n % 4 == 0 {
            doc.new_page();
        }
        let p = doc.pages.last_mut().unwrap();
        let y = 100.0 + (n % 4) as f64 * 170.0;
        p.text(50.0, y, 10.0, true, (0.0, 0.0, 0.0), &format!("Test {} : {}", rec.number, rec.name));
        p.text(50.0, y + 15.0, 10.0, false, (0.0, 0.0, 0.0), "Résultat :");
        if rec.passed {
            p.text(105.0, y + 15.0, 10.0, true, (0.0, 0.55, 0.0), "Conforme");
        } else {
            p.text(105.0, y + 15.0, 10.0, true, (0.85, 0.0, 0.0), "Échec");
        }
        p.text(50.0, y + 30.0, 10.0, false, (0.0, 0.0, 0.0), &format!(
            "Différence d'impédance : {}",
            crate::analysis::format_difference(rec.difference_pct)
        ));
        p.text(50.0, y + 45.0, 10.0, false, (0.0, 0.0, 0.0), &format!("Tolérance : {:.0} %", rec.tolerance_pct));
        p.text(50.0, y + 60.0, 10.0, false, (0.0, 0.0, 0.0), &format!("Heure : {}", rec.time));
        p.text(50.0, y + 75.0, 8.0, false, (0.3, 0.3, 0.3), "Orange : mesure, bleu : référence");
        p.image(&record_image(rec), 230.0, y, 150.0, 150.0);
        p.rect(230.0, y, 150.0, 150.0, (0.0, 0.0, 0.0), false);
        if let Some(b) = board {
            p.image(&board_crop(b, rec.x, rec.y), 390.0, y, 150.0, 150.0);
            p.rect(390.0, y, 150.0, 150.0, (0.0, 0.0, 0.0), false);
        }
    }

    let count = doc.pages.len();
    for (k, p) in doc.pages.iter_mut().enumerate() {
        let col = (112.0 / 255.0, 146.0 / 255.0, 190.0 / 255.0);
        p.rect(50.0, 70.0, PAGE_W - 100.0, 3.0, col, true);
        p.rect(50.0, PAGE_H - 60.0, PAGE_W - 100.0, 3.0, col, true);
        p.text(50.0, 50.0, 10.0, true, (0.0, 0.0, 0.0), "CT220S Viewer");
        p.text(50.0, PAGE_H - 52.0, 8.0, false, (0.0, 0.0, 0.0), &info.test_name);
        p.text(PAGE_W - 120.0, PAGE_H - 52.0, 8.0, false, (0.0, 0.0, 0.0), &format!("Page {} / {}", k + 1, count));
    }
    doc
}

pub fn save_report(path: &str, info: &ReportInfo, db: &TestDatabase, board: Option<&RgbImage>) -> Result<(), String> {
    let doc = build_report(info, db, board);
    std::fs::write(path, doc.to_bytes()).map_err(|e| format!("{}: {}", path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{TestDatabase, TestRecord};

    #[test]
    fn pdf_structure_is_consistent() {
        let mut db = TestDatabase::new();
        let v: Vec<u16> = (0..512).map(|k| 1000 + k as u16).collect();
        for n in 0..5 {
            db.records.push(TestRecord {
                number: n + 1,
                point_index: 0,
                name: format!("Point é{}", n),
                passed: n % 2 == 0,
                difference_pct: 4.2,
                tolerance_pct: 3.0,
                time: "12:00:00".into(),
                live_voltage: v.clone(),
                live_current: v.clone(),
                ref_voltage: v.clone(),
                ref_current: v.clone(),
                x: 10,
                y: 10,
            });
        }
        let board = RgbImage::from_pixel(64, 48, Rgb([10, 200, 10]));
        let info = ReportInfo { test_name: "carte (A)".into(), device_name: "UCE-CT220S".into(), serial: "-".into(), date: "x".into() };
        let bytes = build_report(&info, &db, Some(&board)).to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.contains("/Count 4"), "1 garde + 1 carte + 2 pages de tests");
        assert!(text.trim_end().ends_with("%%EOF"));
        // Vérifie que chaque offset de la table xref pointe sur "N 0 obj".
        let xref_pos: usize = text.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
        let xref = &bytes[xref_pos..];
        let xref_text = String::from_utf8_lossy(xref);
        for (n, line) in xref_text.lines().skip(3).take_while(|l| l.ends_with(" n ")).enumerate() {
            let off: usize = line[..10].parse().unwrap();
            let expect = format!("{} 0 obj", n + 1);
            assert!(bytes[off..].starts_with(expect.as_bytes()), "objet {}", n + 1);
        }
        assert_eq!(encode_text("a(b)\\é"), "a\\(b\\)\\\\\\351");
    }
}
