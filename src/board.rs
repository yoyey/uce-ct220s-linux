//! Image de la carte testée (Form2 du logiciel constructeur) : chargement,
//! texture d'affichage et conversion entre coordonnées écran et image.

use eframe::egui;
use image::RgbImage;

/// Taille maximale de la texture affichée (les très grandes photos sont
/// réduites pour l'affichage, les coordonnées restent celles de l'original).
const MAX_TEXTURE_SIDE: u32 = 4096;

#[allow(dead_code)]
pub struct BoardImage {
    pub path: String,
    pub rgb: RgbImage,
    display: RgbImage,
    texture: Option<egui::TextureHandle>,
}

impl BoardImage {
    pub fn load(path: &str) -> Result<Self, String> {
        let img = image::open(path).map_err(|e| format!("{}: {}", path, e))?.to_rgb8();
        let display = if img.width() > MAX_TEXTURE_SIDE || img.height() > MAX_TEXTURE_SIDE {
            let k = MAX_TEXTURE_SIDE as f32 / img.width().max(img.height()) as f32;
            image::imageops::resize(
                &img,
                ((img.width() as f32 * k) as u32).max(1),
                ((img.height() as f32 * k) as u32).max(1),
                image::imageops::FilterType::Triangle,
            )
        } else {
            img.clone()
        };
        Ok(Self { path: path.to_string(), rgb: img, display, texture: None })
    }

    pub fn width(&self) -> u32 {
        self.rgb.width()
    }

    pub fn height(&self) -> u32 {
        self.rgb.height()
    }

    pub fn texture(&mut self, ctx: &egui::Context) -> egui::TextureId {
        if self.texture.is_none() {
            let size = [self.display.width() as usize, self.display.height() as usize];
            let color = egui::ColorImage::from_rgb(size, self.display.as_raw());
            self.texture = Some(ctx.load_texture("board_image", color, egui::TextureOptions::LINEAR));
        }
        self.texture.as_ref().unwrap().id()
    }
}
