//! Alerte sonore conforme / échec (onaySesi / hataSesi du constructeur).
//! Disponible avec `cargo run --features son` (dépendance rodio).

#[derive(Debug, Clone, Copy)]
pub enum Beep {
    Pass,
    Fail,
}

pub const AVAILABLE: bool = cfg!(feature = "son");

#[cfg(feature = "son")]
pub fn play(beep: Beep) {
    use rodio::source::{SineWave, Source};
    use std::time::Duration;
    std::thread::spawn(move || {
        let Ok((_stream, handle)) = rodio::OutputStream::try_default() else { return };
        let Ok(sink) = rodio::Sink::try_new(&handle) else { return };
        match beep {
            Beep::Pass => {
                sink.append(SineWave::new(1320.0).take_duration(Duration::from_millis(90)).amplify(0.25));
                sink.append(SineWave::new(1760.0).take_duration(Duration::from_millis(140)).amplify(0.25));
            }
            Beep::Fail => {
                sink.append(SineWave::new(330.0).take_duration(Duration::from_millis(450)).amplify(0.3));
            }
        }
        sink.sleep_until_end();
    });
}

#[cfg(not(feature = "son"))]
pub fn play(_beep: Beep) {}
