//! Mode base de données du logiciel constructeur (onglet Advanced) :
//! liste de points de test avec courbe de référence, fichier `.uce`
//! compatible avec PC_CT22X, test automatique et statistiques de rapport.

use crate::config::POINTS_PER_CURVE;
use std::fs;

/// Nombre de lignes de données par point : 512 V + 512 I + V/F/R + X/Y.
const DATA_LINES: usize = 2 * POINTS_PER_CURVE + 5;

#[derive(Debug, Clone, PartialEq)]
pub struct TestPoint {
    pub name: String,
    /// Points ADC bruts de la référence (CH1 constructeur).
    pub voltage: Vec<u16>,
    /// Points ADC bruts de la référence (CH2 constructeur).
    pub current: Vec<u16>,
    pub voltage_index: u8,
    pub frequency_index: u8,
    pub resistor_index: u8,
    /// Position du point sur l'image de la carte (pixels de l'image d'origine).
    pub x: i32,
    pub y: i32,
}

impl TestPoint {
    fn to_uce_block(&self) -> String {
        let mut s = String::new();
        s.push_str(&self.name);
        s.push('\n');
        for v in &self.voltage {
            s.push_str(&v.to_string());
            s.push('\n');
        }
        for i in &self.current {
            s.push_str(&i.to_string());
            s.push('\n');
        }
        for n in [self.voltage_index as i32, self.frequency_index as i32, self.resistor_index as i32, self.x, self.y] {
            s.push_str(&n.to_string());
            s.push('\n');
        }
        // Le constructeur écrit le bloc avec WriteLine : ligne vide finale.
        s.push_str("\r\n");
        s
    }
}

fn parse_number(line: &str) -> Result<f64, String> {
    let t = line.trim().replace(',', ".");
    t.parse::<f64>().map_err(|_| format!("valeur invalide '{}'", line.trim()))
}

/// Lit un fichier `.uce` (format PC_CT22X : blocs de 1031 lignes).
pub fn parse_uce(contents: &str) -> Result<Vec<TestPoint>, String> {
    let lines: Vec<&str> = contents.lines().map(|l| l.trim_end_matches('\r')).collect();
    let mut points = Vec::new();
    let mut idx = 0;
    while idx < lines.len() {
        // Ignorer les lignes vides entre les blocs.
        if lines[idx].trim().is_empty() {
            idx += 1;
            continue;
        }
        if idx + 1 + DATA_LINES > lines.len() {
            return Err(format!("bloc incomplet à la ligne {}", idx + 1));
        }
        let name = lines[idx].to_string();
        let data = &lines[idx + 1..idx + 1 + DATA_LINES];
        let mut voltage = Vec::with_capacity(POINTS_PER_CURVE);
        let mut current = Vec::with_capacity(POINTS_PER_CURVE);
        for (k, l) in data.iter().enumerate().take(2 * POINTS_PER_CURVE) {
            let v = parse_number(l).map_err(|e| format!("point '{}', ligne {}: {}", name, idx + 2 + k, e))?;
            let v = v.round().clamp(0.0, 65535.0) as u16;
            if k < POINTS_PER_CURVE {
                voltage.push(v);
            } else {
                current.push(v);
            }
        }
        let tail: Result<Vec<f64>, String> =
            data[2 * POINTS_PER_CURVE..].iter().map(|l| parse_number(l)).collect();
        let tail = tail.map_err(|e| format!("point '{}': {}", name, e))?;
        points.push(TestPoint {
            name,
            voltage,
            current,
            voltage_index: tail[0].clamp(0.0, 4.0) as u8,
            frequency_index: tail[1].clamp(0.0, 3.0) as u8,
            resistor_index: tail[2].clamp(0.0, 2.0) as u8,
            x: tail[3] as i32,
            y: tail[4] as i32,
        });
        idx += 1 + DATA_LINES;
    }
    Ok(points)
}

pub fn to_uce(points: &[TestPoint]) -> String {
    points.iter().map(TestPoint::to_uce_block).collect()
}

pub fn load_uce(path: &str) -> Result<Vec<TestPoint>, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {}", path, e))?;
    parse_uce(&String::from_utf8_lossy(&bytes))
}

pub fn save_uce(path: &str, points: &[TestPoint]) -> Result<(), String> {
    fs::write(path, to_uce(points)).map_err(|e| format!("{}: {}", path, e))
}

/// Résultat d'un test enregistré pour le rapport.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TestRecord {
    pub number: usize,
    pub point_index: usize,
    pub name: String,
    pub passed: bool,
    pub difference_pct: f64,
    pub tolerance_pct: f64,
    pub time: String,
    pub live_voltage: Vec<u16>,
    pub live_current: Vec<u16>,
    pub ref_voltage: Vec<u16>,
    pub ref_current: Vec<u16>,
    pub x: i32,
    pub y: i32,
}

/// Base de points + état du test automatique.
#[derive(Debug, Default)]
pub struct TestDatabase {
    pub points: Vec<TestPoint>,
    pub selected: Option<usize>,
    pub file_path: String,
    /// Résultat par point (un succès ultérieur efface un échec, comme le
    /// constructeur ; un échec n'efface pas un succès).
    pub results: Vec<Option<bool>>,
    pub records: Vec<TestRecord>,
}

impl TestDatabase {
    pub fn new() -> Self {
        Self { file_path: "test_points.uce".to_string(), ..Default::default() }
    }

    fn sync_results(&mut self) {
        self.results.resize(self.points.len(), None);
    }

    pub fn set_points(&mut self, points: Vec<TestPoint>) {
        self.points = points;
        self.results = vec![None; self.points.len()];
        self.selected = if self.points.is_empty() { None } else { Some(0) };
    }

    pub fn selected_point(&self) -> Option<&TestPoint> {
        self.selected.and_then(|i| self.points.get(i))
    }

    /// Insère après la sélection (ou à la fin) et sélectionne le nouveau point.
    pub fn add(&mut self, point: TestPoint) -> usize {
        let index = match self.selected {
            Some(i) if i < self.points.len() => i + 1,
            _ => self.points.len(),
        };
        self.points.insert(index, point);
        self.results.insert(index.min(self.results.len()), None);
        self.sync_results();
        self.selected = Some(index);
        index
    }

    pub fn delete_selected(&mut self) {
        if let Some(i) = self.selected {
            if i < self.points.len() {
                self.points.remove(i);
                if i < self.results.len() {
                    self.results.remove(i);
                }
            }
            self.selected = if self.points.is_empty() { None } else { Some(i.min(self.points.len() - 1)) };
        }
    }

    pub fn clear(&mut self) {
        self.points.clear();
        self.results.clear();
        self.records.clear();
        self.selected = None;
    }

    pub fn move_selected(&mut self, up: bool) {
        let Some(i) = self.selected else { return };
        let j = if up {
            if i == 0 {
                return;
            }
            i - 1
        } else {
            if i + 1 >= self.points.len() {
                return;
            }
            i + 1
        };
        self.points.swap(i, j);
        self.sync_results();
        self.results.swap(i, j);
        self.selected = Some(j);
    }

    /// Passe au point suivant ; renvoie faux en fin de liste.
    pub fn next(&mut self) -> bool {
        match self.selected {
            Some(i) if i + 1 < self.points.len() => {
                self.selected = Some(i + 1);
                true
            }
            None if !self.points.is_empty() => {
                self.selected = Some(0);
                true
            }
            _ => false,
        }
    }

    pub fn previous(&mut self) {
        if let Some(i) = self.selected {
            if i > 0 {
                self.selected = Some(i - 1);
            }
        }
    }

    pub fn record_result(&mut self, point_index: usize, passed: bool) {
        self.sync_results();
        if let Some(slot) = self.results.get_mut(point_index) {
            *slot = Some(slot.unwrap_or(false) || passed);
        }
    }

    pub fn reset_session(&mut self) {
        self.results = vec![None; self.points.len()];
        self.records.clear();
    }

    /// (tests effectués, points testés, points en échec).
    pub fn statistics(&self) -> (usize, usize, usize) {
        let tested = self.results.iter().filter(|r| r.is_some()).count();
        let failed = self.results.iter().filter(|r| **r == Some(false)).count();
        (self.records.len(), tested, failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(name: &str, base: u16) -> TestPoint {
        TestPoint {
            name: name.to_string(),
            voltage: (0..512).map(|k| base + k as u16).collect(),
            current: (0..512).map(|k| 4000 - k as u16).collect(),
            voltage_index: 4,
            frequency_index: 2,
            resistor_index: 1,
            x: 120,
            y: -1,
        }
    }

    #[test]
    fn uce_roundtrip_matches_vendor_layout() {
        let pts = vec![point("Test Point1", 100), point("U2 pin 3", 200)];
        let text = to_uce(&pts);
        // 1031 lignes par bloc, comme File.ReadAllLines côté constructeur.
        assert_eq!(text.split('\n').count() - 1, 2 * 1031);
        assert_eq!(parse_uce(&text).unwrap(), pts);
    }

    #[test]
    fn uce_without_trailing_blank_line() {
        let pts = vec![point("A", 10)];
        let text = to_uce(&pts);
        let trimmed = text.trim_end();
        assert_eq!(parse_uce(trimmed).unwrap(), pts);
    }

    #[test]
    fn truncated_file_is_an_error() {
        let text = to_uce(&[point("A", 10)]);
        let cut: String = text.lines().take(500).collect::<Vec<_>>().join("\n");
        assert!(parse_uce(&cut).is_err());
    }

    #[test]
    fn pass_overrides_fail_but_not_inverse() {
        let mut db = TestDatabase::new();
        db.set_points(vec![point("A", 1), point("B", 2)]);
        db.record_result(0, false);
        db.record_result(0, true);
        db.record_result(1, true);
        db.record_result(1, false);
        assert_eq!(db.results, vec![Some(true), Some(true)]);
    }

    #[test]
    fn add_inserts_after_selection() {
        let mut db = TestDatabase::new();
        db.set_points(vec![point("A", 1), point("C", 3)]);
        db.selected = Some(0);
        db.add(point("B", 2));
        let names: Vec<_> = db.points.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["A", "B", "C"]);
        assert_eq!(db.selected, Some(1));
        db.move_selected(true);
        assert_eq!(db.points[0].name, "B");
    }
}
