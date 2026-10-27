//! Dispatcher coverage feedback.
//!
//! File format (written by the dispatcher's coverage recorder): plain text, one hex address per
//! line without `0x` (e.g. `023e32`): an address where the dispatcher had to start interpreting.
//! An optional second column, separated by a space, is the hex address of the instruction that
//! jumped there (a `jmp`, `jsr`, branch or return). Empty lines, `#` comments and `0x` prefixes
//! are accepted (same as `m68k::coverage`).
//!
//! Every address is validated (even, inside a code section, outside data/excluded ranges,
//! plausible code) before it is used. An address whose source is a computed `jmp`/`jsr` becomes
//! an extra target of that instruction (a block of its function, or a callee); any other address
//! becomes a new function seed.
use crate::{plausible_code, Func, Image};
use m68k::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub addr: u32,
    pub from: Option<u32>,
}

pub fn parse(text: &str) -> Result<Vec<Hit>, String> {
    let mut v = Vec::new();
    for (n, l) in text.lines().enumerate() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let mut it = l.split_whitespace();
        let hex = |s: &str| u32::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|_| format!("line {}: bad hex {:?}", n + 1, s));
        let addr = hex(it.next().unwrap())?;
        let from = it.next().map(hex).transpose()?;
        v.push(Hit { addr, from });
    }
    Ok(v)
}

#[derive(Default, Debug)]
pub struct Plan {
    /// New function entries.
    pub seeds: Vec<u32>,
    /// Extra targets for computed transfers: instruction -> targets.
    pub targets: BTreeMap<u32, Vec<u32>>,
    pub accepted: usize,
    pub known: usize,
    pub duplicates: usize,
    /// (address, reason)
    pub rejected: Vec<(u32, &'static str)>,
}

/// Validates `hits` against the image and the functions found without coverage.
pub fn plan(img: &Image, hits: &[Hit], funcs: &BTreeMap<u32, Func>) -> Plan {
    let mut p = Plan::default();
    let known: BTreeSet<u32> = funcs.values().flat_map(|f| f.leaders.iter().copied().filter(|l| f.insns.contains_key(l))).collect();
    let tables: BTreeMap<u32, &Vec<u32>> = funcs.values().flat_map(|f| f.jump_tables.iter().map(|(a, t)| (*a, t))).collect();
    let mut seen = BTreeSet::new();
    for h in hits {
        let a = h.addr;
        if !seen.insert((a, h.from)) {
            p.duplicates += 1;
            continue;
        }
        let why = if img.seg(a).is_none() {
            Some("outside the image")
        } else if a & 1 != 0 {
            Some("odd address")
        } else if img.is_data_range(a) {
            Some("in a data range")
        } else if img.is_excluded(a) {
            Some("in an excluded range")
        } else if !img.is_code(a) {
            Some("not in a code section")
        } else if !plausible_code(img, a) {
            Some("does not decode as plausible code")
        } else {
            None
        };
        if let Some(w) = why {
            p.rejected.push((a, w));
            continue;
        }
        if known.contains(&a) {
            p.known += 1;
            continue;
        }
        p.accepted += 1;
        // Attach to the computed transfer it came from, if that is one.
        let src = h.from.and_then(|s| img.decode(s)).filter(|i| match i.op {
            Op::Jmp { ea } | Op::Jsr { ea } => !matches!(ea, Ea::AbsW(_) | Ea::AbsL(_) | Ea::PcDisp(_)),
            _ => false,
        });
        match src {
            Some(i) => {
                let e = p.targets.entry(i.pc).or_insert_with(|| tables.get(&i.pc).map(|t| t.to_vec()).unwrap_or_default());
                if !e.contains(&a) {
                    e.push(a);
                }
            }
            None => p.seeds.push(a),
        }
    }
    p
}
