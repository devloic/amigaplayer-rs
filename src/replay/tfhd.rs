//! TFMX single-file container (`TFHD`): the Eagleplayers TFMX-1.5-TFHD, TFMX-Pro-TFHD and
//! TFMX-7V-TFHD.
//!
//! Clean-room, written from a behaviour specification (drawn up from register-write logs of
//! the three players on two real files and 29 synthetic ones made from real `mdat`/`smpl`
//! pairs) and checked against those logs; no original code. The specification's finding is
//! that the container is the only new thing for 7 voices, and that 1.5 and Pro are the plain
//! engines of `tfmx.rs` with a few differences, so this file only takes the container apart,
//! decides which player takes a file, and hands the song part and the samples to the engines
//! (`tfmx::new_15_tfhd`, `new_pro_tfhd`, `new_7v_tfhd`), which carry the differences.
//!
//! The file: `TFHD`, the header size H (the offset of the song part, $12 in every known file),
//! the type byte (0: unspecified, 1: TFMX 1.5, 2: TFMX Pro, 3: TFMX 7V; bit 7 ignored), a zero
//! byte, the length M of the song part (`mdat`) and the length S of the sample part (`smpl`),
//! all big-endian; then the song part at H and the samples at H + M, byte for byte. The
//! players use the lengths of the header, not the file's size. Nothing is converted: the
//! engines run on the song part in place, and sample offsets are relative to H + M.
//!
//! A file whose type is the player's own is taken without a look at the song part; type 0 goes
//! through the player's plain-file check, applied to the song part; any other type is refused.
//!
//! Decisions where the specification leaves the choice to the port:
//! * a song part shorter than $200 bytes (too short to hold the song table), or one that does
//!   not lie inside the file, is refused whatever the type (the engines read the song table);
//! * the 7V player's side effect on the loaded module's table offsets (the fixed-table values
//!   in $1D0..$1D8, undone when its check fails) is left to the engine, which sets them when
//!   the module has none;
//! * the subsong numbering of Pro is the compacted table's (the table is rewritten in guest
//!   memory like the player does), and a file with no kept entry plays entry 0;
//! * the CIA register writes of the Pro player's start-up (timer and filter) are not made;
//!   the engine's timer is started at the time the specification gives.

use super::{be16, be32, tfmx, Open, Replayer};
use crate::machine::Machine;

pub const OPEN_15: Open = Open { name: "TFMX-1.5-TFHD", detect: detect_15, new: new_15 };
pub const OPEN_7V: Open = Open { name: "TFMX-7V-TFHD", detect: detect_7v, new: new_7v };
pub const OPEN_PRO: Open = Open { name: "TFMX-Pro-TFHD", detect: detect_pro, new: new_pro };

const MAGIC: u32 = 0x5446_4844;

/// The container's fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Head {
    /// The type (low 7 bits of byte 8).
    ty: u8,
    /// The offset of the song part.
    h: usize,
    /// The song part's length.
    m: usize,
    /// The sample part's length.
    s: usize,
}

fn parse(d: &[u8]) -> Option<Head> {
    if be32(d, 0) != MAGIC || d.len() < 0x12 {
        return None;
    }
    let h = be32(d, 4) as usize;
    let m = be32(d, 10) as usize;
    let s = be32(d, 14) as usize;
    // the song part must be inside the file, and long enough for the song table
    if h < 0x12 || m < 0x200 || h.checked_add(m)? > d.len() {
        return None;
    }
    Some(Head { ty: d[8] & 0x7f, h, m, s })
}

/// The file goes to the player whose own type is `own` (always), or, for type 0, when `plain`
/// accepts the song part.
fn accepts(d: &[u8], own: u8, plain: fn(&[u8]) -> bool) -> bool {
    match parse(d) {
        Some(p) if p.ty == own => true,
        Some(p) if p.ty == 0 => plain(&d[p.h..p.h + p.m]),
        _ => false,
    }
}

/// The Pro player's plain check (spec 2): one of the texts `tfmxsong`, `TFMX`, `TFMX-Song`;
/// not a module with neither tables nor a second long; and the step at `$1D0` plus 16 times
/// the first song's start (for a fixed-table module this reads at 16 times the start, as the
/// player does) is not the 7-voice step.
fn plain_pro(d: &[u8]) -> bool {
    if !(d.starts_with(b"tfmxsong") || d.starts_with(b"TFMX") || d.starts_with(b"TFMX-Song")) {
        return false;
    }
    if be32(d, 0x1d0) == 0 && be32(d, 4) == 0 {
        return false;
    }
    let step = be32(d, 0x1d0).wrapping_add(16u32.wrapping_mul(be16(d, 0x100) as u32)) as usize;
    be32(d, step) != 0xeffe_0003
}

fn detect_15(d: &[u8]) -> bool {
    accepts(d, 1, tfmx::detect_15)
}

fn detect_7v(d: &[u8]) -> bool {
    accepts(d, 3, tfmx::detect_7v)
}

fn detect_pro(d: &[u8]) -> bool {
    accepts(d, 2, plain_pro)
}

/// The song part's place in guest memory, and the samples' (after it).
fn places<'a>(d: &'a [u8], base: u32, m: &mut Machine) -> Option<(Head, &'a [u8], u32, u32)> {
    let p = parse(d)?;
    let song = base.checked_add(p.h as u32)?;
    let smpl = song.checked_add(p.m as u32)?;
    // the player writes the type it took the file for back into the loaded file
    m.bus.w8(base + 8, p.ty);
    Some((p, &d[p.h..p.h + p.m], song, smpl))
}

fn new_15(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect_15(d) {
        return None;
    }
    let (_, song, at, smpl) = places(d, base, m)?;
    tfmx::new_15_tfhd(song, at, smpl, m)
}

fn new_7v(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect_7v(d) {
        return None;
    }
    let (_, song, at, smpl) = places(d, base, m)?;
    tfmx::new_7v_tfhd(song, at, smpl, m)
}

fn new_pro(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect_pro(d) {
        return None;
    }
    let (_, song, at, smpl) = places(d, base, m)?;
    let n = compact_songs(&mut m.bus, at);
    tfmx::new_pro_tfhd(song, at, smpl, n, m)
}

/// The song table the Pro player rewrites at load (spec 5.5): 32 entries, start steps at $100,
/// last steps at $140, tempos at $180 (words). Walks entries 0 to 30; a run of entries with
/// the same start and last counts as one (and is dropped when start = last); an entry is kept
/// when start and last differ, or when they are equal, there was no run and start is not 0, or
/// when both are 0, there was no run and the tempo is neither 0 nor 5. The kept ones are copied
/// to the front of the three tables, in order. Returns how many were kept (the subsongs).
fn compact_songs(bus: &mut crate::bus::Bus, a: u32) -> u8 {
    let get = |bus: &crate::bus::Bus, i: u32| (bus.r16(a + 0x100 + 2 * i), bus.r16(a + 0x140 + 2 * i), bus.r16(a + 0x180 + 2 * i));
    let mut kept = Vec::new();
    let mut i = 0u32;
    while i <= 30 {
        let (st, la, _) = get(bus, i);
        let mut dup = false;
        while i + 1 <= 30 && {
            let (s2, l2, _) = get(bus, i + 1);
            s2 == st && l2 == la
        } {
            i += 1;
            dup = true;
        }
        let tempo = get(bus, i).2;
        let keep = st != la
            || (!dup && st != 0)
            || (st == 0 && la == 0 && !dup && tempo != 0 && tempo != 5);
        if keep {
            kept.push((st, la, tempo));
        }
        i += 1;
    }
    for (k, (st, la, tempo)) in kept.iter().enumerate() {
        let k = k as u32;
        bus.w16(a + 0x100 + 2 * k, *st);
        bus.w16(a + 0x140 + 2 * k, *la);
        bus.w16(a + 0x180 + 2 * k, *tempo);
    }
    kept.len() as u8
}

#[cfg(test)]
#[path = "tfhd_tests.rs"]
mod tests;
