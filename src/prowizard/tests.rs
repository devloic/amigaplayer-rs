//! Helpers for the formats' tests: a synthetic song, the ProTracker module it is,
//! and the conversion through every format. Each format's tests pack the song as
//! that format does and check that it converts back to the module.

use super::core::*;

/// A small song: what a packer packs and the converted module must hold.
pub struct Song {
    /// (length in words, fine tune, volume, loop start, loop length) per sample.
    pub samples: Vec<(u16, u8, u8, u16, u16)>,
    pub positions: Vec<u8>,
    pub restart: u8,
    /// 64 rows x 4 voices of (sample, period, effect, parameter) per pattern.
    pub patterns: Vec<Vec<[(u8, u16, u8, u8); 4]>>,
}

impl Song {
    pub fn new(npat: usize) -> Song {
        let periods = [856u16, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 214, 113];
        let mut patterns = Vec::new();
        for p in 0..npat {
            let mut rows = Vec::new();
            for r in 0..64 {
                let mut row = [(0u8, 0u16, 0u8, 0u8); 4];
                for (v, n) in row.iter_mut().enumerate() {
                    let k = p * 7 + r * 3 + v;
                    if k % 5 == 1 {
                        *n = (1 + (k % 2) as u8, periods[k % periods.len()], 0, 0);
                    } else if k % 11 == 3 {
                        *n = (0, 0, 0xc, (k % 64) as u8);
                    }
                }
                rows.push(row);
            }
            patterns.push(rows);
        }
        Song {
            samples: vec![(8, 0, 64, 0, 1), (16, 3, 32, 4, 12)],
            positions: (0..npat as u8).chain([0]).collect(),
            restart: 0,
            patterns,
        }
    }
    pub fn sample_data(&self, i: usize) -> Vec<u8> {
        (0..self.samples[i].0 as usize * 2).map(|k| (k * 37 + i * 11) as u8).collect()
    }
    pub fn note(n: (u8, u16, u8, u8)) -> [u8; 4] {
        [(n.0 & 0xf0) | (n.1 >> 8) as u8, n.1 as u8, (n.0 << 4) | n.2, n.3]
    }
    /// The ProTracker module (title empty, unnamed samples).
    pub fn module(&self, restart: u8) -> Vec<u8> {
        let mut m = vec![0u8; 20];
        for i in 0..31 {
            m.extend([0u8; 22]);
            let s = self.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            m.extend(s.0.to_be_bytes());
            m.extend([s.1, s.2]);
            m.extend(s.3.to_be_bytes());
            m.extend(s.4.to_be_bytes());
        }
        m.push(self.positions.len() as u8);
        m.push(restart);
        let mut pl = self.positions.clone();
        pl.resize(128, 0);
        m.extend(pl);
        m.extend(b"M.K.");
        for p in &self.patterns {
            for row in p {
                for n in row {
                    m.extend(Self::note(*n));
                }
            }
        }
        for i in 0..self.samples.len() {
            m.extend(self.sample_data(i));
        }
        m
    }
}

/// Identify and convert with every format, as `identify_convert` does but without
/// its plain-module check: the format's name and the module.
pub fn convert_any(packed: &[u8]) -> Option<(&'static str, Vec<u8>)> {
    super::formats().into_iter().find_map(|mut f| run(f.as_mut(), packed, &NoFiles).map(|m| (f.name(), m)))
}
