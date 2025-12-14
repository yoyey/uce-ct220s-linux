// src/main.rs

mod config;
mod curve;
mod backend;
mod image_export;
mod app;

use app::CT220SApp;
use clap::Parser;
use eframe::egui;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Chemin vers un fichier de capture hexadécimal
    #[arg(short, long)]
    file: Option<String>,
}

fn main() -> Result<(), eframe::Error> {
    let args = Args::parse();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 700.0]),
        ..Default::default()
    };

    eframe::run_native(
        "CT220S V-I Curve Viewer",
        options,
        Box::new(move |cc| Box::new(CT220SApp::new(cc, args.file.clone()))),
    )
}

