//! Coverage recording: where the dispatcher had to fall back to the interpreter.
//!
//! [`crate::runtime::run_with`] records every address at which it *starts* interpreting (control
//! arrived at an address with no usable native code), together with the address of the instruction
//! that transferred control there when it is known. The recompiler reads the result back with
//! `--coverage FILE` to add those addresses as seeds.
//!
//! File format (text, one record per line, sorted, deduplicated):
//!
//! ```text
//! 02a1f4 02a1e0
//! 02b000
//! ```
//!
//! The first column is the address where interpretation started, the optional second column the
//! address of the instruction that jumped/returned/branched there; both are 6 lowercase hex digits
//! without `0x`. An address reached from several sources has one line per source; an address with
//! no known source has one single-column line (only when it has no sourced line).
use std::collections::BTreeSet;
use std::fmt::Write as _;

#[derive(Clone, Debug, Default)]
pub struct Coverage {
    /// (start address, source instruction address if known).
    pub entries: BTreeSet<(u32, Option<u32>)>,
    /// Find the exact source instruction of a native exit. This runs native code one basic block
    /// at a time while recording (so the block that left can be decoded), which costs speed. When
    /// false, native exits are recorded without a source.
    pub exact_sources: bool,
}

impl Coverage {
    /// A recorder that finds exact sources (block-stepped native code).
    pub fn new() -> Coverage {
        Coverage { entries: BTreeSet::new(), exact_sources: true }
    }
    /// A recorder that leaves native code at full speed and records native exits without sources.
    pub fn fast() -> Coverage {
        Coverage { entries: BTreeSet::new(), exact_sources: false }
    }
    pub fn record(&mut self, addr: u32, src: Option<u32>) {
        self.entries.insert((addr & 0xFF_FFFF, src.map(|s| s & 0xFF_FFFF)));
    }
    /// Distinct start addresses.
    pub fn addresses(&self) -> BTreeSet<u32> {
        self.entries.iter().map(|e| e.0).collect()
    }
    pub fn merge(&mut self, o: &Coverage) {
        self.entries.extend(o.entries.iter().copied());
    }
    /// The file contents in the agreed format.
    pub fn to_text(&self) -> String {
        let sourced: BTreeSet<u32> = self.entries.iter().filter(|e| e.1.is_some()).map(|e| e.0).collect();
        let mut s = String::new();
        // BTreeSet order: by address, then None before Some(src) ascending.
        for &(a, src) in &self.entries {
            match src {
                Some(f) => {
                    let _ = writeln!(s, "{:06x} {:06x}", a, f);
                }
                None if !sourced.contains(&a) => {
                    let _ = writeln!(s, "{:06x}", a);
                }
                None => {}
            }
        }
        s
    }
    pub fn write(&self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_text())
    }
    /// Parses the format (also accepts `0x` prefixes, blank lines and `#` comments).
    pub fn parse(text: &str) -> Result<Coverage, String> {
        let mut c = Coverage::new();
        for (n, l) in text.lines().enumerate() {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            let mut it = l.split_whitespace();
            let p = |w: &str| u32::from_str_radix(w.trim_start_matches("0x"), 16).map_err(|e| format!("line {}: {}: {}", n + 1, w, e));
            let a = p(it.next().unwrap())?;
            let src = match it.next() {
                Some(w) => Some(p(w)?),
                None => None,
            };
            c.record(a, src);
        }
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn format_roundtrip() {
        let mut c = Coverage::new();
        c.record(0x2b000, None);
        c.record(0x2a1f4, Some(0x2b008));
        c.record(0x2a1f4, Some(0x2a1e0));
        c.record(0x2a1f4, None);
        c.record(0x2a1f4, Some(0x2a1e0));
        c.record(0x1000, None);
        assert_eq!(c.to_text(), "001000\n02a1f4 02a1e0\n02a1f4 02b008\n02b000\n");
        assert_eq!(Coverage::parse(&c.to_text()).unwrap().to_text(), c.to_text());
    }
}
