//! The Sonix Music Driver's songs (`tiny.*` and `snx.*`), as the Amiga games that
//! use it (Stellar 7, A-10 Tank Killer, the Carmen Sandiego series, Antheads, It
//! Came From the Desert) play them.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (file formats, per-tick arithmetic, the order of Paula register
//! writes), and this player was written from it alone and checked against the
//! Paula register writes of UADE's Sonix Music Driver player.
//!
//! A song is four tracks, one per voice, each a stream of 16-bit events (notes,
//! waits, instrument and volume selects). The newer `snx` songs run on a tempo that
//! sets the timer (one tick per interrupt) and carry the names of their instruments;
//! the older `tiny` songs have a note length in steps of a fixed number of
//! interrupts and 64 four-letter instrument slots. The instruments are files in the
//! song's `Instruments` folder (found in any case, as on the Amiga): synthesis
//! (a 128-byte wave run through 64 precomputed filters, mixed or widened by the
//! phase of a second oscillator, with an envelope and an LFO), sampled sound
//! (`.instr` and `.ss`, one block per octave with a repeat part), AIFF (one or more
//! zones) and IFF 8SVX. A sampled-type note first mutes its channel (DMA off, period
//! 2), writes the sample's first block and starts DMA after a wait of about nine
//! raster lines, and on the next tick writes the loop block; a synthesis voice
//! writes its pointer, length, period and volume every tick, its wave synthesised
//! anew into one of two 128-byte buffers.
//!
//! Timing: a tick is the CIA timer's value plus one E clocks (5 colour clocks each);
//! each write lands at the tick's start plus half the 68000 cycles of the code
//! paths taken before it (per build: SNX and TINY differ), fitted to the
//! reference's write logs; the DMA-on write follows the beam wait (nine changes of
//! the raster line, polled). Not modelled: the raster-line phase of the
//! reference's song start (its load time), the SNX build's extra work on every
//! tick of a one-shot sample, and the synthesis paths' cycles.
//!
//! Recognition is by content (the format's two header tests), and a file that
//! starts `FORM` is left to the IFF-SMUS replayer, which plays the same instruments
//! under a different sequencer.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "Sonix Music Driver", detect, new };

const SEMI: [u16; 12] =
    [0x8000, 0x78D1, 0x7209, 0x6BA2, 0x6598, 0x5FE4, 0x5A82, 0x556E, 0x50A3, 0x4C1C, 0x47D6, 0x43CE];

const BEND: [u16; 64] = [
    0xC24D, 0xBFC8, 0xBD4C, 0xBAD8, 0xB86C, 0xB608, 0xB3AC, 0xB158, 0xAF0C, 0xACC7, 0xAA8A, 0xA854, 0xA626, 0xA3FF, 0xA1DF,
    0x9FC6, 0x9DB4, 0x9BA9, 0x99A4, 0x97A6, 0x95AF, 0x93BF, 0x91D5, 0x8FF1, 0x8E13, 0x8C3C, 0x8A6B, 0x88A0, 0x86DA, 0x851B,
    0x8362, 0x81AE, 0x8000, 0x7E57, 0x7CB4, 0x7B16, 0x797E, 0x77EB, 0x765D, 0x74D4, 0x7351, 0x71D2, 0x7059, 0x6EE4, 0x6D74,
    0x6C09, 0x6AA2, 0x6941, 0x67E4, 0x668B, 0x6537, 0x63E7, 0x629C, 0x6154, 0x6012, 0x5ED3, 0x5D98, 0x5C62, 0x5B2F, 0x5A01,
    0x58D6, 0x57B0, 0x568D, 0x556E,
];

const FILT: [u16; 64] = [
    0x8000, 0x7683, 0x6DBA, 0x6597, 0x5E10, 0x5717, 0x50A2, 0x4AA8, 0x451F, 0x4000, 0x3B41, 0x36DD, 0x32CB, 0x2F08, 0x2B8B,
    0x2851, 0x2554, 0x228F, 0x2000, 0x1DA0, 0x1B6E, 0x1965, 0x1784, 0x15C5, 0x1428, 0x12AA, 0x1147, 0x1000, 0x0ED0, 0x0DB7,
    0x0CB2, 0x0BC2, 0x0AE2, 0x0A14, 0x0955, 0x08A3, 0x0800, 0x0768, 0x06DB, 0x0659, 0x05E1, 0x0571, 0x050A, 0x04AA, 0x0451,
    0x0400, 0x03B4, 0x036D, 0x032C, 0x02F0, 0x02B8, 0x0285, 0x0255, 0x0228, 0x0200, 0x01DA, 0x01B6, 0x0196, 0x0178, 0x015C,
    0x0142, 0x012A, 0x0114, 0x0100,
];

const TEMPO: [u16; 128] = [
    0xFA83, 0xF525, 0xEFE4, 0xEAC0, 0xE5B9, 0xE0CC, 0xDBFB, 0xD744, 0xD2A8, 0xCE24, 0xC9B9, 0xC567, 0xC12C, 0xBD08, 0xB8FB,
    0xB504, 0xB123, 0xAD58, 0xA9A1, 0xA5FE, 0xA270, 0x9EF5, 0x9B8D, 0x9837, 0x94F4, 0x91C3, 0x8EA4, 0x8B95, 0x8898, 0x85AA,
    0x82CD, 0x8000, 0x7D41, 0x7A92, 0x77F2, 0x7560, 0x72DC, 0x7066, 0x6DFD, 0x6BA2, 0x6954, 0x6712, 0x64DC, 0x62B3, 0x6096,
    0x5E84, 0x5C7D, 0x5A82, 0x5891, 0x56AC, 0x54D0, 0x52FF, 0x5138, 0x4F7A, 0x4DC6, 0x4C1B, 0x4A7A, 0x48E1, 0x4752, 0x45CA,
    0x444C, 0x42D5, 0x4166, 0x4000, 0x3EA0, 0x3D49, 0x3BF9, 0x3AB0, 0x396E, 0x3833, 0x36FE, 0x35D1, 0x34AA, 0x3389, 0x326E,
    0x3159, 0x304B, 0x2F42, 0x2E3E, 0x2D41, 0x2C48, 0x2B56, 0x2A68, 0x297F, 0x289C, 0x27BD, 0x26E3, 0x260D, 0x253D, 0x2470,
    0x23A9, 0x22E5, 0x2226, 0x216A, 0x20B3, 0x2000, 0x1F50, 0x1EA4, 0x1DFC, 0x1D58, 0x1CB7, 0x1C19, 0x1B7F, 0x1AE8, 0x1A55,
    0x19C4, 0x1937, 0x18AC, 0x1825, 0x17A1, 0x171F, 0x16A0, 0x1624, 0x15AB, 0x1534, 0x14BF, 0x144E, 0x13DE, 0x1371, 0x1306,
    0x129E, 0x1238, 0x11D4, 0x1172, 0x1113, 0x10B5, 0x1059, 0x1000,
];

/// The SNX timer's numerator: the host's 50 Hz timer value times 125.
const SNX_CLOCK: u32 = 1_773_375;
/// The NTSC colour clock, which the AIFF and 8SVX periods divide even on a PAL machine.
const NTSC_CLOCK: u32 = 3_579_545;
/// Chip memory per synthesis voice: two 128-byte wave buffers.
const BUF: u32 = 256;

// The original's 68000 cycles per code path, fitted exactly to the reference's writes (from the
// write logs of a run and of a run started one cycle late): the SNX build's and the TINY build's
// (a path not listed costs nothing more than the paths it is counted with). Per voice where the
// path's cost differs by voice.
const SNX_COST: &[(&str, i64)] = &[
    ("ai.down", 88),
    ("ai.gate", -41),
    ("ai.per2", 48),
    ("ai.rel", 39),
    ("ai.start", 1295),
    ("ai.up", 115),
    ("ai.zone", 151),
    ("env.at.gt", 6),
    ("env.down", -14),
    ("env.next", 30),
    ("env.up", -6),
    ("env3", 24),
    ("ev", 385),
    ("ev.note", 620),
    ("ev.rel", -280),
    ("ev.vol", -246),
    ("ev.wait", 48),
    ("rel.off", 140),
    ("rel.on", 282),
    ("sa.lfo", 665),
    ("sa.lv0", 1093),
    ("sa.per2", 48),
    ("sa.rel", 39),
    ("sa.start.end", 3),
    ("seq.tempo", -1),
    ("sq.ended", 473),
    ("sq.read", 183),
    ("sq.wait", 477),
    ("vo.post", 1104),
];
const SNX_COST_CH: &[(&str, [i64; 4])] = &[
    ("out.b0", [-5, 0, -19, 0]),
    ("out.b1", [19, 25, 25, 47]),
    ("out.b1.end", [20, 19, 0, -3]),
    ("out.b1.loc", [16, 16, 16, 16]),
    ("out.b2", [-6, 0, 0, 22]),
    ("out.b2.end", [357, 449, 523, 544]),
    ("out.b2.loc", [16, 16, 16, 16]),
    ("out.end", [265, 265, 243, 163]),
    ("out.idle", [0, 70, 48, 0]),
    ("out.per", [0, 1, 20, 23]),
    ("out.vol", [214, 237, 260, 283]),
    ("sq", [0, 0, 3, 0]),
    ("vo.aif.n", [-18, 18, 0, 0]),
    ("vo.aif.r", [-18, 18, 0, 0]),
    ("vo.aif.s", [66, 18, 0, 0]),
    ("vo.skip", [222, 79, 0, 79]),
];
const TINY_COST: &[(&str, i64)] = &[
    ("env.at.gt", 6),
    ("env.down", 10),
    ("env.next", 30),
    ("env.up", -6),
    ("out", 264),
    ("out.dma", 8),
    ("rel.off", -1),
    ("rel.on", 15),
    ("sa.dly", 1028),
    ("sa.lfo", 1134),
    ("sa.new", 62),
    ("sa.lv0", -39),
    ("sa.oneshot", -166),
    ("sa.per2", 48),
    ("sa.range", 250),
    ("sa.rel", 49),
    ("sa.start", 1190),
    ("sa.tri", 8),
    ("sn", 828),
    ("sn.new", -63),
    ("te", 201),
    ("te.0", -114),
    ("te.end.again", -33),
    ("te.wait", 70),
    ("tick", 340),
    ("tk.dec", 417),
    ("tk.stop", -264),
    ("tq.gate", 156),
    ("tq.gaterel", 235),
    ("tq.read", 57),
    ("tq.restart", 1308),
    ("tq.wait", 181),
    ("vo.post", 447),
];
const TINY_COST_CH: &[(&str, [i64; 4])] = &[
    ("out.b0", [-25, -45, 0, 0]),
    ("out.b1", [78, 78, 123, 123]),
    ("out.b1.end", [20, 0, 0, 0]),
    ("out.b1.loc", [28, 28, 28, 28]),
    ("out.b2", [0, 0, 45, 45]),
    ("out.b2.end", [369, 442, 535, 559]),
    ("out.b2.loc", [28, 28, 28, 28]),
    ("out.end", [426, 381, 381, 248]),
    ("out.idle", [70, 25, 70, -53]),
    ("out.per", [0, 20, 20, 20]),
    ("out.vol", [226, 249, 272, 295]),
    ("tq", [0, 0, 0, 2]),
    ("vo.skip", [153, 153, 153, 153]),
];


/// The original's 68000 cycles since the tick's start; each write lands at the tick's start plus
/// half of them.
#[derive(Default)]
struct Cy {
    t0: u64,
    cy: i64,
    tiny: bool,
}

impl Cy {
    fn c(&mut self, k: &str) {
        let t = if self.tiny { TINY_COST } else { SNX_COST };
        self.cy += t.iter().find(|e| e.0 == k).map_or(0, |e| e.1);
    }
    fn cc(&mut self, k: &str, ch: usize) {
        let t = if self.tiny { TINY_COST_CH } else { SNX_COST_CH };
        self.cy += t.iter().find(|e| e.0 == k).map_or(0, |e| e.1[ch]);
    }
    /// A path taken `n` times.
    fn n(&mut self, k: &str, n: u32) {
        let t = if self.tiny { TINY_COST } else { SNX_COST };
        self.cy += t.iter().find(|e| e.0 == k).map_or(0, |e| e.1) * n as i64;
    }
    /// Up to the next write's moment.
    fn w(&mut self, hw: &mut Hw) {
        let t = self.t0 + self.cy.max(0) as u64 / 2;
        if hw.bus.clock < t {
            hw.bus.clock = t;
        }
    }
}

// ------------------------------------------------------------------ recognition

fn rd16(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o.checked_add(2)?).map(|b| u16::from_be_bytes([b[0], b[1]]))
}

fn rd32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o.checked_add(4)?).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn is_tiny(d: &[u8]) -> bool {
    rd16(d, 0).is_some_and(|w| w & 0x00F0 != 0)
}

fn detect_tiny(d: &[u8]) -> Option<()> {
    if d.len() <= 332 || rd32(d, 0x30)? != 0x140 {
        return None;
    }
    for t in [0x34, 0x38, 0x3C] {
        let o = rd32(d, t)? as usize;
        if o == 0 || o & 0x8000_0000 != 0 || o & 1 != 0 || o >= d.len() {
            return None;
        }
        if rd16(d, o)? != 0xFFFF {
            let b = *d.get(o + 6)?;
            if rd32(d, o)? != 0 || rd16(d, o + 4)? != 0 || b & 0x80 == 0 || b > 0x82 {
                return None;
            }
        }
    }
    Some(())
}

/// The offsets of the four track blocks of an SNX song, and where the name list starts.
fn snx_layout(d: &[u8]) -> Option<([usize; 4], usize)> {
    let mut at = [0; 4];
    let mut p = 20usize;
    for i in 0..4 {
        let s = rd32(d, 4 * i)? as usize;
        if s == 0 || s & 0x8000_0000 != 0 || s & 1 != 0 {
            return None;
        }
        at[i] = p;
        p = p.checked_add(s)?;
    }
    if p >= d.len() {
        return None;
    }
    Some((at, p))
}

fn detect_snx(d: &[u8]) -> Option<()> {
    let (at, names) = snx_layout(d)?;
    for a in at {
        let b = *d.get(a)?;
        if b & 0x80 == 0 || (rd16(d, a)? != 0xFFFF && b > 0x84) {
            return None;
        }
    }
    if d[names] == 0 {
        return None;
    }
    Some(())
}

fn detect(d: &[u8]) -> bool {
    if d.len() < 4 || &d[..4] == b"FORM" {
        return false;
    }
    if is_tiny(d) { detect_tiny(d) } else { detect_snx(d) }.is_some()
}

// ------------------------------------------------------------------ arithmetic

/// `divu.w`: the 16-bit quotient, or the dividend's low word when it overflows.
fn divu(n: u32, d: u16) -> u16 {
    if d == 0 {
        return 0;
    }
    let q = n / d as u32;
    if q > 0xFFFF { n as u16 } else { q as u16 }
}

fn bend(p: u32, b: u8) -> u32 {
    if b == 0 {
        return p;
    }
    let i = ((b as usize + 0x80) & 0xFF) >> 2;
    ((p & 0xFFFF).wrapping_mul(BEND[i] as u32).wrapping_mul(2) >> 16) & 0xFFFF
}

// ------------------------------------------------------------------ instruments

struct Synth {
    amp_base: u16,
    amp_env: u16,
    amp_lfo: i16,
    glide: u16,
    pitch_lfo: i16,
    filt_base: u16,
    filt_env: u16,
    filt_lfo: i16,
    lfo_rate: u16,
    lfo_mode: i16,
    lfo_delay: u16,
    wave_x: u16,
    wave_y: i16,
    lv: [u16; 4],
    rt: [u16; 4],
    modtab: [i8; 256],
    /// The 64 filtered copies of the base wave, 128 bytes each.
    bank: Vec<u8>,
}

struct Sampled {
    amp: u16,
    lv: [u16; 4],
    rt: [u16; 4],
    depth: i16,
    rate: u16,
    delay: u16,
    w: u32,
    one: u32,
    lo: i8,
    hi: i8,
    /// Guest address of the sample data (after the 62-byte header).
    data: u32,
}

struct Zone {
    rate: u16,
    words: u32,
    data: u32,
    /// base, low, high note of an INST chunk.
    inst: Option<(u8, u8, u8)>,
    /// Sustain loop: begin and end, in words.
    sustain: Option<(u32, u32)>,
    attack: u16,
    release: u16,
}

struct Svx {
    one: u32,
    rep: u32,
    octaves: i32,
    shift: i32,
    vol: u32,
    body: u32,
}

enum Kind {
    Synth(Box<Synth>),
    Sampled(Sampled),
    Aiff(Vec<Zone>),
    Svx(Svx),
}

impl Kind {
    fn tech(&self) -> u8 {
        match self {
            Kind::Synth(_) => 0,
            Kind::Sampled(_) => 1,
            Kind::Aiff(_) => 2,
            Kind::Svx(_) => 3,
        }
    }
}

fn words_of(d: &[u8], o: usize, n: usize) -> Vec<u16> {
    (0..n).map(|i| rd16(d, o + 2 * i).unwrap_or(0)).collect()
}

/// The filter bank of a synthesis instrument's base wave.
fn filter_bank(w: &[i8]) -> Vec<u8> {
    let mut s1: u16 = 0;
    let mut s2: u16 = ((w[127] as i32) << 7) as u16;
    let mut out = Vec::with_capacity(64 * 128);
    for c in FILT {
        let g = (((0x8000 - c as u32) * 0xE666) >> 16) as i16 as i32;
        let c2 = (c >> 1) as i16 as i32;
        for &x in w.iter().take(128) {
            let d = (((x as i32) << 7) as u16).wrapping_sub(s2);
            let d = ((d as i16 as i32).wrapping_mul(c2).wrapping_shl(2) >> 16) as u16;
            s1 = s1.wrapping_add(d);
            s2 = s2.wrapping_add(s1);
            out.push((s2 >> 7) as u8);
            s1 = ((s1 as i16 as i32).wrapping_mul(g).wrapping_shl(1) >> 16) as u16;
        }
    }
    out
}

fn synth_from(f: &[u8], tiny: bool) -> Synth {
    // the TINY file's offsets are the SNX ones minus 60
    let o = |snx: usize| if tiny { snx - 60 } else { snx };
    let w = |snx: usize| rd16(f, o(snx)).unwrap_or(0);
    let base: Vec<i8> = f[o(0x44)..o(0x44) + 128].iter().map(|&b| b as i8).collect();
    let mut modtab = [0i8; 256];
    for (i, m) in modtab.iter_mut().enumerate() {
        *m = f[o(0xC4) + i] as i8;
    }
    let lv = words_of(f, o(0x1E6), 4);
    let rt = words_of(f, o(0x1EE), 4);
    Synth {
        amp_base: w(0x1CC),
        amp_env: w(0x1CE),
        amp_lfo: w(0x1D0) as i16,
        glide: w(0x1D2),
        pitch_lfo: w(0x1D4) as i16,
        filt_base: w(0x1D6),
        filt_env: w(0x1D8),
        filt_lfo: w(0x1DA) as i16,
        lfo_rate: w(0x1DC),
        lfo_mode: w(0x1DE) as i16,
        lfo_delay: w(0x1E0),
        wave_x: w(0x1E2),
        wave_y: w(0x1E4) as i16,
        lv: [lv[0], lv[1], lv[2], lv[3]],
        rt: [rt[0], rt[1], rt[2], rt[3]],
        modtab,
        bank: filter_bank(&base),
    }
}

struct Loader<'a> {
    m: &'a mut Machine,
}

impl Loader<'_> {
    fn read(&self, name: &str, ext: &str) -> Option<Vec<u8>> {
        let p = self.m.os.host_path(&format!("Instruments:{name}.{ext}"), None);
        let d = self.m.os.read_file(&p).ok()?;
        log::debug!("Sonix: {name}.{ext} -> {} ({} bytes)", p.display(), d.len());
        Some(d)
    }

    /// Bytes into guest memory (with spare zeros after them), and where.
    fn put(&mut self, d: &[u8]) -> Option<u32> {
        let a = self.m.alloc(d.len() as u32 + 16);
        if a == 0 {
            return None;
        }
        self.m.bus.write_bytes(a, d);
        Some(a)
    }

    fn sampled(&mut self, f: &[u8], tiny: bool, name: &str) -> Option<Kind> {
        let ss = self.read(name, "ss")?;
        if ss.len() < 70 {
            return None;
        }
        let o = |snx: usize| if tiny { snx - 0x60 } else { snx };
        let w = |snx: usize| rd16(f, o(snx)).unwrap_or(0);
        let lv = words_of(f, o(0x6A), 4);
        let rt = words_of(f, o(0x72), 4);
        let data = self.put(&ss[0x3E..])?;
        log::debug!("Sonix: sample data {:06x}..{:06x}", data, data + ss.len() as u32 - 0x3E);
        Some(Kind::Sampled(Sampled {
            amp: w(0x68),
            lv: [lv[0], lv[1], lv[2], lv[3]],
            rt: [rt[0], rt[1], rt[2], rt[3]],
            depth: w(0x7A) as i16,
            rate: w(0x7C),
            delay: w(0x7E),
            w: rd16(&ss, 0)? as u32,
            one: rd16(&ss, 2)? as u32,
            lo: ss[4] as i8,
            hi: ss[5] as i8,
            data,
        }))
    }

    /// An SNX-flavour instrument file.
    fn snx_instr(&mut self, name: &str) -> Option<Kind> {
        let f = self.read(name, "instr")?;
        let n = f.len();
        if f.starts_with(b"Synthesis\0") || f.first() == Some(&0) {
            return (n == 502).then(|| Kind::Synth(Box::new(synth_from(&f, false))));
        }
        if f.starts_with(b"SampledSound") {
            if n != 128 {
                return None;
            }
            let nm: Vec<u8> = f[0x44..].iter().copied().take_while(|&c| c != 0).collect();
            return self.sampled(&f, false, &String::from_utf8_lossy(&nm));
        }
        if n >= 16 && &f[..4] == b"FORM" && &f[8..12] == b"8SVX" && &f[12..16] == b"VHDR" {
            return (n == 8 + rd32(&f, 4)? as usize).then(|| self.svx(&f)).flatten();
        }
        if n >= 12 && (&f[..4] == b"FORM" || &f[..4] == b"LIST") && &f[8..12] == b"AIFF" {
            return (n == 8 + rd32(&f, 4)? as usize).then(|| self.aiff(&f)).flatten();
        }
        None
    }

    fn svx(&mut self, f: &[u8]) -> Option<Kind> {
        let (mut one, mut rep, mut cyc) = (rd32(f, 20)?, rd32(f, 24)?, rd32(f, 28)?);
        let no = *f.get(34)? as i8;
        let vol = rd32(f, 36)?;
        let mut k = 0;
        loop {
            one >>= 1;
            rep >>= 1;
            cyc >>= 1;
            k += 1;
            if (one | rep) & 1 != 0 || cyc == 1 || (k + no as i32) as i8 >= 8 {
                break;
            }
        }
        // BODY, from the end of VHDR in steps of two
        let mut p = 20 + rd32(f, 16)? as usize;
        while f.get(p..p + 4)? != b"BODY" {
            p += 2;
        }
        // the shifted sizes are the sample counts of the highest octave
        let body = self.put(f.get(p + 8..)?)?;
        Some(Kind::Svx(Svx { one, rep, octaves: no as i32, shift: k, vol, body }))
    }

    fn aiff(&mut self, f: &[u8]) -> Option<Kind> {
        let forms: Vec<usize> = if &f[..4] == b"LIST" {
            let mut v = Vec::new();
            let mut p = 12;
            while p + 8 <= f.len() {
                if &f[p..p + 4] == b"FORM" {
                    v.push(p);
                }
                p += 8 + rd32(f, p + 4)? as usize;
            }
            v
        } else {
            vec![0]
        };
        let mut zones = Vec::new();
        for z in forms {
            let end = (z + 8 + rd32(f, z + 4)? as usize).min(f.len());
            let find = |id: &[u8; 4]| -> Option<usize> {
                let mut p = z + 12;
                while p + 8 <= end {
                    if &f[p..p + 4] == id {
                        return Some(p);
                    }
                    p += 8 + rd32(f, p + 4)? as usize;
                }
                None
            };
            let comm = find(b"COMM")?;
            let ssnd = find(b"SSND")?;
            let frames = rd32(f, comm + 10)?;
            let (e, m) = (rd16(f, comm + 16)? as u32, rd16(f, comm + 18)? as u32);
            let rate = m.checked_shr(0x400E_u32.wrapping_sub(e)).unwrap_or(0) as u16;
            let (mut attack, mut release) = (0, 0);
            if let Some(n) = find(b"NAME") {
                let len = rd32(f, n + 4)? as usize;
                let text = f.get(n + 8..(n + 8 + len).min(f.len()))?;
                let mut nums = [0u16; 2];
                let mut it = text.split(|&c| c == b' ').filter(|s| !s.is_empty());
                for slot in nums.iter_mut() {
                    if let Some(s) = it.next() {
                        let digits: Vec<u8> = s.iter().copied().take_while(|c| c.is_ascii_digit()).collect();
                        *slot = std::str::from_utf8(&digits).ok().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0) as u16;
                    }
                }
                (attack, release) = (nums[0], nums[1]);
            }
            let mut inst = None;
            let mut sustain = None;
            if let Some(i) = find(b"INST") {
                inst = Some((*f.get(i + 8)?, *f.get(i + 10)?, *f.get(i + 11)?));
                if rd16(f, i + 16)? != 0 {
                    let (bid, eid) = (rd16(f, i + 18)?, rd16(f, i + 20)?);
                    let mk = find(b"MARK")?;
                    let count = rd16(f, mk + 8)?;
                    let mut p = mk + 10;
                    let (mut b, mut en) = (None, None);
                    for _ in 0..count {
                        let id = rd16(f, p)?;
                        let pos = rd32(f, p + 2)? / 2;
                        if id == bid {
                            b = Some(pos);
                        }
                        if id == eid {
                            en = Some(pos);
                        }
                        let l = *f.get(p + 6)? as usize;
                        p += 6 + ((1 + l + 1) & !1);
                    }
                    sustain = Some((b?, en?));
                }
            }
            let bytes = f.get(ssnd + 16..)?;
            let data = self.put(&bytes[..bytes.len().min(frames as usize)])?;
            log::debug!("Sonix: AIFF zone data {:06x} {} frames rate {}", data, frames, rate);
            zones.push(Zone { rate, words: frames >> 1, data, inst, sustain, attack, release });
        }
        if zones.is_empty() {
            return None;
        }
        Some(Kind::Aiff(zones))
    }

    /// A TINY-flavour instrument file.
    fn tiny_instr(&mut self, name: &str) -> Option<Kind> {
        let f = self.read(name, "instr")?;
        match rd32(&f, 0)? {
            1 if f.len() == 442 => Some(Kind::Synth(Box::new(synth_from(&f, true)))),
            2 if f.len() == 32 => {
                let nm: Vec<u8> = f[4..8].iter().copied().take_while(|&c| c != 0).collect();
                if nm.is_empty() {
                    return None;
                }
                self.sampled(&f, true, &String::from_utf8_lossy(&nm))
            }
            _ => None,
        }
    }
}

// ------------------------------------------------------------------ the player

#[derive(Clone, Copy, PartialEq, Default)]
enum Cmd {
    #[default]
    None,
    Start,
    Release,
}

#[derive(Clone, Copy, PartialEq, Default)]
enum Status {
    #[default]
    Idle,
    On,
    Rel,
}

#[derive(Default)]
struct Voice {
    cmd: Cmd,
    status: Status,
    instr: Option<usize>,
    note: u8,
    volume: u8,
    // envelope, shared by the technologies
    stage: usize,
    level: u32,
    // sampled
    base_period: u16,
    phase: u16,
    delay: u16,
    lfo: i16,
    /// 2: the first block is to be written, 1: the loop block, 0: neither.
    block: u8,
    first: (u32, u16),
    looped: (u32, u16),
    // AIFF and 8SVX
    gate: bool,
    attack: u16,
    release: u16,
    // synthesis
    base: u16,
    glide_count: u16,
    glide_step: u16,
    toggle: u32,
    lfo_pos: u16,
    lfo_wait: u16,
    wave_phase: u16,
    dir: i16,
    started: bool,
    /// The voice's first note: no instrument before it.
    fresh: bool,
    out: (u32, u16),
    // what the write routine writes
    per: u16,
    vol: u16,
}

#[derive(Clone, Copy)]
struct Track {
    /// Offset of the next event; `None` once the track has ended.
    pos: Option<usize>,
    wait: u16,
    /// TINY: the note's gate counter.
    gate: u16,
    instr: Option<usize>,
    volume: u16,
    bend: u8,
    done: bool,
}

pub struct Sonix {
    tiny: bool,
    d: Vec<u8>,
    instrs: Vec<Option<Kind>>,
    v: [Voice; 4],
    t: [Track; 4],
    /// Chip memory: the 8 silent bytes, then 4 synthesis buffer pairs.
    silent: u32,
    bufs: u32,
    // SNX
    tempo: u16,
    applied: Option<u16>,
    // timer value in force, and the one the running interval was started with
    timer: u32,
    interval: u32,
    /// The rate scale of the envelopes and LFOs (SNX `R`, TINY `r22`).
    rate: u16,
    // TINY
    count: u16,
    songend: bool,
    cy: Cy,
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let tiny = is_tiny(d);
    let mut l = Loader { m };
    let mut instrs: Vec<Option<Kind>> = Vec::new();
    if tiny {
        for s in 0..64 {
            let slot = &d[0x40 + 4 * s..0x44 + 4 * s];
            if slot == [0; 4] {
                instrs.push(None);
                continue;
            }
            let name: Vec<u8> = slot.iter().copied().take_while(|&c| c != 0).collect();
            match l.tiny_instr(&String::from_utf8_lossy(&name)) {
                Some(k) => instrs.push(Some(k)),
                None => break,
            }
        }
        if instrs.iter().all(|i| i.is_none()) {
            return None;
        }
    } else {
        let (_, names) = snx_layout(d)?;
        let mut list = Vec::new();
        for n in d[names..].split(|&c| c == 0) {
            if n.is_empty() {
                break;
            }
            list.push(String::from_utf8_lossy(n).to_string());
        }
        if list.len() > 64 {
            return None;
        }
        for n in &list {
            match l.snx_instr(n) {
                Some(k) => instrs.push(Some(k)),
                None => break,
            }
        }
        if instrs.is_empty() {
            return None;
        }
    }
    let mem = l.m.alloc(8 + 4 * BUF);
    if mem == 0 {
        return None;
    }
    log::debug!("Sonix: silent {:06x}, buffers {:06x}..{:06x}", mem, mem + 8, mem + 8 + 4 * BUF);
    let mut s = Sonix {
        tiny,
        d: d.to_vec(),
        instrs,
        v: Default::default(),
        t: [Track { pos: None, wait: 1, gate: 0, instr: None, volume: 255, bend: 0, done: false }; 4],
        silent: mem,
        bufs: mem + 8,
        tempo: rd16(d, 0x12).unwrap_or(0),
        applied: None,
        timer: 0,
        interval: 0,
        rate: 0,
        count: 0,
        songend: false,
        cy: Cy { tiny, ..Default::default() },
    };
    s.begin();
    if tiny {
        s.tiny_tempo();
    } else {
        // the first interval comes from the first event when that sets the tempo
        let first = if d[20] == 0x82 { d[21] as u16 } else { s.tempo };
        s.timer = SNX_CLOCK.checked_div(first as u32).unwrap_or(0) & 0xFFFF;
    }
    s.interval = s.timer;
    Some(Box::new(s))
}

impl Sonix {
    /// The tracks from their starts (the initial state, and a restart).
    fn begin(&mut self) {
        for v in 0..4 {
            let start = if self.tiny {
                match rd32(&self.d, 0x30 + 4 * v).unwrap_or(0) as usize {
                    0 => None,
                    o => Some(o),
                }
            } else {
                let (at, _) = snx_layout(&self.d).unwrap();
                (rd16(&self.d, at[v]) != Some(0xFFFF)).then_some(at[v])
            };
            self.t[v] = Track {
                pos: start,
                wait: 1,
                gate: 0,
                instr: None,
                volume: if self.tiny { rd16(&self.d, 0x12 + 4 * v).unwrap_or(0) } else { 255 },
                bend: 0,
                done: false,
            };
        }
    }

    fn snx_tempo(&mut self) {
        let t = self.tempo as u32;
        if t == 0 {
            return;
        }
        self.applied = Some(self.tempo);
        self.timer = SNX_CLOCK / t & 0xFFFF;
        let q = 0x4B0000 / t;
        self.rate = if q > 0xFFFF { 0 } else { q as u16 };
    }

    fn tiny_tempo(&mut self) -> u16 {
        let x = TEMPO[(rd16(&self.d, 2).unwrap_or(0) as usize >> 1) & 127] as u32;
        let n = x >> 12;
        let r22 = ((x << 15) / (n << 12).max(1)) as u16;
        self.rate = r22;
        self.timer = (r22 as u32 * 0x2E9C) >> 15;
        n as u16
    }

    // ---- the voice operations

    fn release_note(&mut self, ch: usize) {
        let v = &mut self.v[ch];
        v.cmd = Cmd::None;
        if v.status == Status::On {
            self.cy.c("rel.on");
            v.cmd = Cmd::Release;
        } else {
            self.cy.c("rel.off");
        }
    }

    fn stop_note(&mut self, ch: usize, hw: &mut Hw) {
        let v = &mut self.v[ch];
        v.cmd = Cmd::None;
        if v.status != Status::Idle {
            v.status = Status::Idle;
            self.cy.cc("stop", ch);
            self.cy.w(hw);
            hw.dmacon(1 << ch);
            self.cy.cc("stop.per", ch);
            self.cy.w(hw);
            hw.per(ch, 2);
            self.cy.cc("stop.end", ch);
        }
    }

    fn start_note(&mut self, ch: usize, instr: Option<usize>, note: u8, volume: u8, hw: &mut Hw) {
        let Some(i) = instr else { return };
        let Some(k) = &self.instrs[i] else { return };
        let tech = k.tech();
        let old = self.v[ch].instr.and_then(|o| self.instrs[o].as_ref()).map(|k| k.tech());
        self.cy.c("sn");
        if self.v[ch].instr.is_none() {
            self.cy.c("sn.new");
            self.v[ch].fresh = true;
        }
        if self.v[ch].status != Status::Idle && old.is_some_and(|o| o != tech) {
            self.stop_note(ch, hw);
        }
        let v = &mut self.v[ch];
        v.instr = Some(i);
        v.note = note;
        v.volume = volume;
        v.cmd = Cmd::Start;
    }

    // ---- SNX sequencer

    fn snx_pass(&mut self, hw: &mut Hw) -> usize {
        let mut ended = 0;
        for ch in 0..4 {
            self.cy.cc("sq", ch);
            if self.t[ch].wait != 0 {
                self.t[ch].wait -= 1;
                if self.t[ch].wait != 0 {
                    self.cy.c("sq.wait");
                    continue;
                }
            }
            let Some(mut p) = self.t[ch].pos else {
                self.cy.c("sq.ended");
                ended += 1;
                continue;
            };
            self.cy.c("sq.read");
            loop {
                let Some(w) = rd16(&self.d, p) else {
                    self.t[ch].pos = None;
                    ended += 1;
                    break;
                };
                p += 2;
                self.cy.c("ev");
                let (hi, lo) = ((w >> 8) as u8, w as u8);
                if w == 0 {
                    self.cy.c("ev.0");
                    continue;
                }
                if w == 0xFFFF {
                    self.cy.c("ev.end");
                    self.t[ch].pos = None;
                    ended += 1;
                    break;
                }
                if w >= 0xC000 {
                    let n = w & 0x3FFF;
                    if n == 0 {
                        self.cy.c("ev.w0");
                        continue;
                    }
                    self.cy.c("ev.wait");
                    self.t[ch].wait = n;
                    self.t[ch].pos = Some(p);
                    break;
                }
                match hi {
                    0x80 => {
                        self.cy.c("ev.instr");
                        self.t[ch].instr = ((lo as usize) < self.instrs.len()).then_some(lo as usize)
                    }
                    0x81 => {
                        self.cy.c("ev.vol");
                        self.t[ch].volume = lo as u16
                    }
                    0x82 => {
                        self.cy.c("ev.tempo");
                        self.tempo = lo as u16;
                        self.snx_tempo();
                    }
                    0x83 => {
                        self.cy.c("ev.bend");
                        self.t[ch].bend = lo
                    }
                    0x84..=0xBF => self.cy.c("ev.x"),
                    _ if lo == 0 => {
                        self.cy.c("ev.rel");
                        self.release_note(ch)
                    }
                    _ => {
                        self.cy.c("ev.note");
                        if let Some(i) = self.t[ch].instr {
                            let vol = ((((self.t[ch].volume + 1) as u32 * (2 * lo as u32 + 1)) & 0xFFFF) >> 8) as u8;
                            self.start_note(ch, Some(i), hi, vol, hw);
                        }
                    }
                }
            }
        }
        ended
    }

    fn snx_sequence(&mut self, hw: &mut Hw) {
        self.cy.c("seq");
        if self.applied != Some(self.tempo) {
            self.cy.c("seq.tempo");
            self.snx_tempo();
        }
        let ended = self.snx_pass(hw);
        if ended == 4 {
            self.cy.c("seq.restart");
            self.songend = true;
            for ch in 0..4 {
                self.release_note(ch);
            }
            self.begin();
            self.snx_pass(hw);
        }
    }

    // ---- TINY sequencer

    fn tiny_step(&mut self, hw: &mut Hw) {
        let mut ended = self.tiny_pass(hw);
        if ended == 4 {
            self.cy.c("tq.restart");
            self.songend = true;
            for ch in 0..4 {
                if rd32(&self.d, 0x20 + 4 * ch).unwrap_or(0) != 0 {
                    self.release_note(ch);
                }
            }
            self.begin();
            ended = self.tiny_pass(hw);
            let _ = ended;
        }
    }

    fn tiny_pass(&mut self, hw: &mut Hw) -> usize {
        let mut ended = 0;
        let transpose = (rd16(&self.d, 4).unwrap_or(0) as i16 >> 4) - 8;
        for ch in 0..4 {
            self.cy.cc("tq", ch);
            let mode = rd32(&self.d, 0x20 + 4 * ch).unwrap_or(0);
            if self.t[ch].gate != 0 {
                self.t[ch].gate -= 1;
                self.cy.c("tq.gate");
                if self.t[ch].gate == 0 && mode != 0 {
                    self.cy.c("tq.gaterel");
                    self.release_note(ch);
                }
                continue;
            }
            if self.t[ch].wait != 0 {
                self.t[ch].wait -= 1;
                if self.t[ch].wait != 0 {
                    self.cy.c("tq.wait");
                    continue;
                }
            }
            let Some(mut p) = self.t[ch].pos else {
                self.cy.c("tq.ended");
                ended += 1;
                continue;
            };
            self.cy.c("tq.read");
            loop {
                let Some(w) = rd16(&self.d, p) else {
                    ended += 1;
                    break;
                };
                self.cy.c("te");
                if w == 0 {
                    self.cy.c("te.0");
                    p += 2;
                    continue;
                }
                if w == 0xFFFF {
                    self.cy.c(if self.t[ch].done { "te.end.again" } else { "te.end" });
                    self.t[ch].done = true;
                    // the pointer stays on the end marker
                    self.t[ch].pos = Some(p);
                    ended += 1;
                    break;
                }
                p += 2;
                let (hi, dd) = ((w >> 8) as u8, w & 0xFF);
                match hi {
                    0x81 => {
                        self.cy.c("te.instr");
                        let slot = dd as usize;
                        self.t[ch].instr = (slot < 64 && self.instrs.get(slot).is_some_and(|i| i.is_some())).then_some(slot);
                    }
                    0x80 => {
                        self.cy.c("te.wait");
                        self.t[ch].wait = dd;
                        self.t[ch].pos = Some(p);
                        break;
                    }
                    0x82..=0xFF => self.cy.c("te.x"),
                    _ => {
                        self.t[ch].pos = Some(p);
                        match self.t[ch].instr {
                            Some(i) if mode != 0 => {
                                self.cy.c("te.note");
                                let n = (transpose + hi as i16) as u8;
                                let mut vol = self.t[ch].volume;
                                if mode & 0xFFFF != 1 {
                                    self.cy.c("te.half");
                                    vol >>= 1;
                                }
                                self.start_note(ch, Some(i), n, vol as u8, hw);
                                let g = ((dd as u32 * 0xC000) >> 16) as u16;
                                self.t[ch].gate = g;
                                self.t[ch].wait = dd - g;
                            }
                            _ => {
                                self.cy.c("te.rest");
                                self.t[ch].wait = dd
                            }
                        }
                        break;
                    }
                }
            }
        }
        ended
    }

    // ---- the per-tick pipeline

    fn update(&mut self, ch: usize, hw: &mut Hw) -> bool {
        let Some(i) = self.v[ch].instr else {
            self.cy.cc("vo.none", ch);
            self.v[ch].status = Status::Idle;
            return false;
        };
        let x = Ctx {
            tiny: self.tiny,
            r: self.rate,
            wheel: rd16(&self.d, 6).unwrap_or(0),
            master: rd16(&self.d, 0).unwrap_or(0),
            silent: self.silent,
            buf: self.bufs + ch as u32 * BUF,
        };
        let bend_b = self.t[ch].bend;
        let v = &mut self.v[ch];
        let cy = &mut self.cy;
        let cmd = match v.cmd {
            Cmd::Start => "s",
            Cmd::Release => "r",
            Cmd::None => "n",
        };
        let stop = match self.instrs[i].as_ref().unwrap() {
            Kind::Synth(s) => {
                cy.cc(&format!("vo.syn.{cmd}"), ch);
                update_synth(s, v, &x, bend_b, hw, cy)
            }
            Kind::Sampled(s) => {
                cy.cc(&format!("vo.sam.{cmd}"), ch);
                update_sampled(s, v, &x, ch, bend_b, hw, cy)
            }
            Kind::Aiff(z) => {
                cy.cc(&format!("vo.aif.{cmd}"), ch);
                update_aiff(z, v, &x, ch, bend_b, hw, cy)
            }
            Kind::Svx(s) => {
                cy.cc(&format!("vo.svx.{cmd}"), ch);
                update_svx(s, v, &x, ch, bend_b, hw, cy)
            }
        };
        let v = &mut self.v[ch];
        match v.cmd {
            Cmd::Start => v.status = Status::On,
            Cmd::Release => v.status = Status::Rel,
            Cmd::None => {}
        }
        self.cy.c("vo.post");
        v.cmd = Cmd::None;
        stop
    }

    fn output(&mut self, hw: &mut Hw) {
        let mut mask = 0u16;
        self.cy.c("out");
        for ch in 0..4 {
            let v = &mut self.v[ch];
            let cy = &mut self.cy;
            if v.status == Status::Idle {
                cy.cc("out.idle", ch);
                continue;
            }
            let synth = matches!(v.instr.and_then(|i| self.instrs[i].as_ref()), Some(Kind::Synth(_)));
            if synth {
                cy.cc("out.syn", ch);
                cy.w(hw);
                hw.loc(ch, v.out.0);
                cy.cc("out.syn.len", ch);
                cy.w(hw);
                hw.len(ch, v.out.1);
                cy.cc("out.syn.per", ch);
                cy.w(hw);
                hw.per(ch, v.per);
                cy.cc("out.syn.vol", ch);
                cy.w(hw);
                hw.vol(ch, v.vol & 0x7F);
                if v.started {
                    cy.c("out.syn.st");
                    v.started = false;
                    mask |= 1 << ch;
                }
                cy.cc("out.syn.end", ch);
            } else {
                let started = v.block == 2;
                match v.block {
                    2 => {
                        cy.cc("out.b2", ch);
                        cy.w(hw);
                        hw.len(ch, v.first.1);
                        cy.cc("out.b2.loc", ch);
                        cy.w(hw);
                        hw.loc(ch, v.first.0);
                        cy.cc("out.b2.end", ch);
                        mask |= 1 << ch;
                        v.block = 1;
                    }
                    1 => {
                        cy.cc("out.b1", ch);
                        cy.w(hw);
                        hw.len(ch, v.looped.1);
                        cy.cc("out.b1.loc", ch);
                        cy.w(hw);
                        hw.loc(ch, v.looped.0);
                        cy.cc("out.b1.end", ch);
                        v.block = 0;
                    }
                    _ => cy.cc("out.b0", ch),
                }
                cy.cc("out.per", ch);
                cy.w(hw);
                hw.per(ch, v.per);
                cy.cc("out.vol", ch);
                cy.w(hw);
                hw.vol(ch, v.vol & 0x7F);
                if started {
                    cy.c("out.dma");
                }
                cy.cc("out.end", ch);
            }
        }
        self.cy.c("wait.pre");
        self.beam_wait();
        self.cy.w(hw);
        hw.dmacon(0x8000 | mask);
    }

    /// The wait before DMA is switched on: nine changes of the beam's line, each polled from a
    /// read of the line (the first poll 16 cycles after it, then every 25), the next round's read
    /// 35 cycles after the poll that saw the change; the DMACON write 79 cycles after the last.
    fn beam_wait(&mut self) {
        let t0 = self.cy.t0;
        let line = |c: i64| (t0 + c.max(0) as u64 / 2) / LINE;
        let mut q = self.cy.cy;
        let mut p = q;
        for _ in 0..9 {
            let l0 = line(q);
            p = q + 16;
            while line(p) == l0 {
                p += 25;
            }
            q = p + 35;
        }
        self.cy.cy = p + 79;
    }

    fn voices(&mut self, hw: &mut Hw) -> u16 {
        let mut stop = 0u16;
        for ch in 0..4 {
            if self.v[ch].cmd != Cmd::None || self.v[ch].status != Status::Idle {
                if self.update(ch, hw) {
                    stop |= 1 << ch;
                }
            } else {
                self.cy.cc("vo.skip", ch);
            }
        }
        stop
    }
}

struct Ctx {
    tiny: bool,
    r: u16,
    wheel: u16,
    master: u16,
    silent: u32,
    buf: u32,
}

/// One step of the envelope (stage 0-2 move toward their level; stage 3 is the release).
fn envelope(stage: &mut usize, level: &mut u32, lv: &[u16; 4], rt: &[u16; 4], rscale: u16, cy: &mut Cy) {
    let target = (lv[*stage] as u32) << 16;
    let rc = rt[*stage] as u32;
    cy.c(&format!("env{}", *stage));
    cy.n("env.sh", ((rc >> 5) ^ 7) & 63);
    let step = ((((rc & 0x1F) + 0x21) * rscale as u32).wrapping_shl(3)).checked_shr(((rc >> 5) ^ 7) & 63).unwrap_or(0);
    let (t, l) = (target as i32 as i64, *level as i32 as i64);
    if (t - l).abs() <= step as i64 {
        cy.c(if t == l { "env.at.eq" } else if l < t { "env.at.lt" } else { "env.at.gt" });
        *level = target;
        if *stage < 2 {
            cy.c("env.next");
            *stage += 1;
        }
    } else if l < t {
        cy.c("env.up");
        *level = level.wrapping_add(step);
    } else {
        cy.c("env.down");
        *level = level.wrapping_sub(step);
    }
}

fn update_sampled(s: &Sampled, v: &mut Voice, x: &Ctx, ch: usize, bend_b: u8, hw: &mut Hw, cy: &mut Cy) -> bool {
    let mut stop = false;
    match v.cmd {
        Cmd::Start => {
            if v.fresh {
                cy.c("sa.new");
                v.fresh = false;
            }
            let n = v.note as i32;
            let o = 10 - n / 12;
            if o < 0 || o > s.hi as i32 || o < s.lo as i32 {
                cy.c("sa.range");
                v.cmd = Cmd::None;
                if v.status == Status::Idle {
                    return false;
                }
            } else {
                cy.c("sa.start");
                cy.n("sa.oct", o as u32);
                let r = (n % 12) as usize;
                v.base_period = ((if x.tiny { 0xD5C8u32 } else { 0x1AB9 } * SEMI[r] as u32) >> 15) as u16;
                let a = s.data + s.w * ((1u32 << o) - (1u32 << s.lo.max(0))) ;
                let (p, q) = (s.one << o, (s.w - s.one.min(s.w)) << o);
                v.first = (a, ((p + q) / 2) as u16);
                v.looped = if q == 0 { (x.silent, 4) } else { (a + p, (q / 2) as u16) };
                if q == 0 {
                    cy.c("sa.oneshot");
                }
                if !x.tiny || v.status == Status::Idle {
                    cy.c("sa.lv0");
                    v.level = 0;
                }
                if !x.tiny || v.status != Status::On {
                    cy.c("sa.st0");
                    v.stage = 0;
                }
                v.phase = 0;
                cy.c("sa.div");
                v.delay = divu((s.delay as u32) << 15, x.r) >> 1;
                cy.w(hw);
                hw.dmacon(1 << ch);
                cy.c("sa.per2");
                cy.w(hw);
                hw.per(ch, 2);
                cy.c("sa.start.end");
                v.block = 2;
                stop = x.tiny;
            }
        }
        Cmd::Release => {
            cy.c("sa.rel");
            v.stage = 3
        }
        Cmd::None => {}
    }
    // the LFO
    if v.delay != 0 {
        cy.c("sa.dly");
        v.delay -= 1;
    } else {
        cy.c("sa.lfo");
        v.phase = v.phase.wrapping_add((((s.rate as u32 * x.r as u32).wrapping_shl(7)) >> 16) as u16).wrapping_add(0x40);
    }
    let mut t = (v.phase >> 7).wrapping_add(0x80);
    if t & 0x100 != 0 {
        cy.c("sa.tri");
        t ^= 0xFF;
    }
    t ^= 0x80;
    v.lfo = -(t as u8 as i8 as i16);
    envelope(&mut v.stage, &mut v.level, &s.lv, &s.rt, x.r, cy);
    let m = ((v.lfo as i32 * s.depth as i32) as i16 as i32) >> 7;
    let f = (0x1000 + m) as u16 as u32;
    let p = if x.tiny {
        let g = (0x1000 + m - (x.wheel as i32 - 0x80)) as u16 as u32;
        (v.base_period as u32 * g) >> 19
    } else {
        bend_c((v.base_period as u32 * f) >> 16, bend_b, cy)
    };
    v.per = p as u16;
    let level = v.level >> 16;
    v.vol = if x.tiny {
        let m1 = ((((x.master as u32 + 1) * v.volume as u32) & 0xFFFF) >> 8) as u32;
        let m2 = (((m1 + 1) * s.amp as u32) & 0xFFFF) >> 8;
        (((m2 * level) & 0xFFFF) >> 10) as u16
    } else {
        let rv = (0xFF00u32 * (v.volume as u32 + 1)) >> 16;
        let a = (((rv + 1) * s.amp as u32) & 0xFFFF) >> 8;
        (((a * level) & 0xFFFF) >> 10) as u16
    };
    stop
}

fn bend_c(p: u32, b: u8, cy: &mut Cy) -> u32 {
    if b == 0 {
        cy.c("bend0");
    } else {
        cy.c("bend");
    }
    bend(p, b)
}

fn zone_period(z: &[Zone], n: u8) -> Option<(usize, u16)> {
    for (i, zone) in z.iter().enumerate() {
        let mut rate = zone.rate as u32;
        let f = if n == 0 {
            rate
        } else if n >= 128 {
            n as u32
        } else {
            let mut dn = match zone.inst {
                None => (n as i32 - 0x3C) as i8,
                Some((base, lo, hi)) => {
                    if n < lo || n > hi {
                        continue;
                    }
                    (n as i32 - base as i32) as i8
                }
            };
            while dn < 0 {
                rate >>= 1;
                dn = dn.wrapping_add(12);
            }
            while dn >= 12 {
                rate *= 2;
                if rate > 0xFFFF {
                    return None;
                }
                dn -= 12;
            }
            if dn != 0 {
                let q = (rate << 15) / SEMI[dn as usize] as u32;
                if q > 0xFFFF {
                    return None;
                }
                rate = q;
            }
            rate
        };
        if f == 0 {
            return None;
        }
        return Some((i, divu(NTSC_CLOCK, f as u16)));
    }
    None
}

fn update_aiff(z: &[Zone], v: &mut Voice, x: &Ctx, ch: usize, bend_b: u8, hw: &mut Hw, cy: &mut Cy) -> bool {
    match v.cmd {
        Cmd::Start => match zone_period(z, v.note) {
            None => {
                cy.c("ai.range");
                v.cmd = Cmd::None;
                if v.status == Status::Idle {
                    return false;
                }
            }
            Some((i, bp)) => {
                cy.c("ai.start");
                cy.n("ai.zone", i as u32);
                let zone = &z[i];
                v.base_period = bp;
                let (p, q) = match zone.sustain {
                    Some((b, e)) => (2 * b, 2 * e.saturating_sub(b)),
                    None => (2 * zone.words, 0),
                };
                v.first = (zone.data, ((p + q) / 2) as u16);
                v.looped = if q == 0 { (x.silent, 4) } else { (zone.data + p, (q / 2) as u16) };
                v.level = 0;
                v.attack = divu(0xFF00, zone.attack.wrapping_add(1));
                v.release = divu(0xFF00, zone.release.wrapping_add(1));
                v.gate = true;
                if zone.sustain.is_none() {
                    cy.c("ai.oneshot");
                }
                cy.w(hw);
                hw.dmacon(1 << ch);
                cy.c("ai.per2");
                cy.w(hw);
                hw.per(ch, 2);
                cy.c("ai.start.end");
                v.block = 2;
            }
        },
        Cmd::Release => {
            cy.c("ai.rel");
            v.gate = false
        }
        Cmd::None => {}
    }
    v.per = bend_c(v.base_period as u32, bend_b, cy) as u16;
    let speed = if v.gate { v.attack } else { v.release } as u64;
    let step = speed * x.r as u64 * 2;
    if v.gate {
        cy.c("ai.gate");
        if v.level != 0xFF00 {
            cy.c("ai.up");
            let l = v.level as u64 + (step >> 16);
            v.level = if step > 0xFFFF_FFFF || l >= 0xFF00 { 0xFF00 } else { l as u32 };
        }
    } else if v.level != 0 {
        cy.c("ai.down");
        v.level = if step > 0xFFFF_FFFF { 0 } else { v.level.saturating_sub((step >> 16) as u32) };
    } else {
        cy.c("ai.zero");
    }
    let rv = (0xFF00u32 * (v.volume as u32 + 1)) >> 16;
    v.vol = (((((rv + 1) * v.level) >> 16) + 1) & 0xFFFF) as u16 >> 2;
    false
}

fn update_svx(s: &Svx, v: &mut Voice, x: &Ctx, ch: usize, bend_b: u8, hw: &mut Hw, cy: &mut Cy) -> bool {
    let _ = x;
    match v.cmd {
        Cmd::Start => {
            let n = v.note as i32;
            let (bp, o) = if n >= 128 {
                (divu(NTSC_CLOCK, n as u16), s.octaves - 1)
            } else {
                let (q, r) = (n / 12, (n % 12) as usize);
                ((((0x1ACu32 * SEMI[r] as u32) * 2) >> 16) as u16, 10 - q - s.shift)
            };
            if o < 0 || o > 24 || (o as i8) as i32 >= s.octaves {
                cy.c("sv.range");
                v.cmd = Cmd::None;
                if v.status == Status::Idle {
                    return false;
                }
            } else {
                cy.c(if n >= 128 { "sv.start.hz" } else { "sv.start" });
                cy.n("sv.oct", o as u32);
                v.base_period = bp;
                let (os, rp) = (s.one, s.rep);
                let a = s.body + ((os + rp) << o) - (os + rp);
                let (p, q) = (os << o, rp << o);
                v.first = (a, ((p + q) / 2) as u16);
                v.looped = if q == 0 { (x.silent, 4) } else { (a + p, (q / 2) as u16) };
                if q == 0 {
                    cy.c("sv.oneshot");
                }
                v.gate = true;
                cy.w(hw);
                hw.dmacon(1 << ch);
                cy.c("sv.per2");
                cy.w(hw);
                hw.per(ch, 2);
                cy.c("sv.start.end");
                v.block = 2;
            }
        }
        Cmd::Release => {
            cy.c("sv.rel");
            v.gate = false
        }
        Cmd::None => {}
    }
    v.per = bend_c(v.base_period as u32, bend_b, cy) as u16;
    v.vol = if v.gate {
        cy.c("sv.gate");
        let rv = (0xFF00u32 * (v.volume as u32 + 1)) >> 16;
        (((((rv + 1) * ((s.vol >> 1) & 0xFFFF)) >> 16) & 0xFFFF) >> 1) as u16
    } else {
        cy.c("sv.off");
        0
    };
    false
}

fn update_synth(s: &Synth, v: &mut Voice, x: &Ctx, bend_b: u8, hw: &mut Hw, cy: &mut Cy) -> bool {
    let _ = hw;
    cy.c("sy");
    match v.cmd {
        Cmd::Start => {
            let n = v.note;
            if !(36..=107).contains(&n) {
                v.cmd = Cmd::None;
                if v.status == Status::Idle {
                    return false;
                }
            } else {
                let k = (n - 36) as usize;
                if v.status == Status::Idle {
                    v.level = 0;
                }
                if v.status != Status::On {
                    v.stage = 0;
                }
                let target = ((0xD5C8u32 * SEMI[k % 12] as u32) >> (k / 12 + 17)) as u16;
                if v.base == 0 {
                    v.glide_count = 0;
                    v.base = target;
                } else {
                    let nn = (divu((s.glide as u32) << 15, x.r) >> 3).wrapping_add(1);
                    let d = target.wrapping_sub(v.base) as i16 as i32;
                    let div = (nn as i16 as i32).max(1);
                    v.glide_step = (d / div) as u16;
                    v.base = target.wrapping_sub(v.glide_step.wrapping_mul(nn));
                    v.glide_count = nn;
                }
                v.dir = 1;
                if s.wave_x == 0 {
                    v.wave_phase = 0;
                }
                v.lfo_wait = 0;
                if s.lfo_mode != 0 {
                    v.lfo_pos = 0;
                    v.lfo_wait = divu((s.lfo_delay as u32) << 15, x.r) >> 2;
                    v.lfo = s.modtab[0] as i16;
                }
                cy.c("sy.start");
                v.started = true;
            }
        }
        Cmd::Release => {
            cy.c("sy.rel");
            v.stage = 3
        }
        Cmd::None => {}
    }
    // the LFO
    if (v.lfo_wait as i16) < 0 {
        cy.c("sy.lfooff");
    } else if v.lfo_wait != 0 {
        cy.c("sy.lfodly");
        v.lfo_wait -= 1;
    } else {
        cy.c("sy.lfo");
        let inc = ((s.lfo_rate as u32 * x.r as u32).wrapping_shl(6)) >> 16;
        if x.tiny {
            let t = v.lfo_pos as u32 + inc;
            if t > 0xFFFF && s.lfo_mode > 0 {
                v.lfo_wait = 0xFFFF;
            } else {
                v.lfo_pos = t as u16;
                v.lfo = s.modtab[(v.lfo_pos >> 8) as usize] as i16;
            }
        } else {
            if s.lfo_mode > 0 {
                let t = v.lfo_pos as u32 + inc;
                if t > 0xFFFF || t >= 0xFE00 {
                    v.lfo_wait = 0xFFFF;
                    v.lfo_pos = 0xFE00;
                } else {
                    v.lfo_pos = t as u16;
                }
            } else {
                v.lfo_pos = v.lfo_pos.wrapping_add(inc as u16);
            }
            v.lfo = s.modtab[(v.lfo_pos >> 8) as usize] as i16;
        }
    }
    envelope(&mut v.stage, &mut v.level, &s.lv, &s.rt, x.r, cy);
    if v.glide_count != 0 {
        cy.c("sy.glide");
        v.glide_count -= 1;
        v.base = v.base.wrapping_add(v.glide_step);
    }
    let mut xx = v.base;
    let mut sh = 5i32;
    while (xx as i16) > 0x1AC && sh > 0 {
        cy.c("sy.oct");
        xx >>= 1;
        sh -= 1;
    }
    let words = (0x40u32 >> sh) as u16;
    let m = ((v.lfo as i32 * s.pitch_lfo as i32) as i16 as i32) >> 7;
    let p = if x.tiny {
        (xx as u32 * ((0x1000 + m - (x.wheel as i32 - 0x80)) as u16 as u32)) >> 12
    } else {
        bend((xx as u32 * ((0x1000 + m) as u16 as u32)) >> 12, bend_b)
    };
    v.per = p as u16;
    let level = (v.level >> 16) as u32;
    // amplitude
    let neg = -(v.lfo as i32);
    let mut a = s.amp_base as u32;
    if x.tiny || s.amp_lfo != 0 {
        a = a.wrapping_add((((neg * s.amp_lfo as i32) as i16 as i32) >> 8) as u32) & 0xFFFF;
    }
    if s.amp_env != 0 {
        a = ((a * level) & 0xFFFF) >> 8;
    } else if v.stage == 3 {
        a = 0;
    }
    a = (a & 0xFF) + 1;
    v.vol = if x.tiny {
        let m1 = (((a * x.master as u32) & 0xFFFF) >> 8) + 1;
        let m2 = (((m1 * v.volume as u32) & 0xFFFF) >> 8) + 1;
        ((m2 & 0xFFFF) >> 2) as u16
    } else {
        let rv = (0xFF00u32 * (v.volume as u32 + 1)) >> 16;
        (((((rv * a) & 0xFFFF) >> 8) + 1) & 0xFFFF) as u16 >> 2
    };
    // the wave
    let f = (s.filt_base ^ 0xFF)
        .wrapping_sub((((level * s.filt_env as u32) & 0xFFFF) >> 8) as u16)
        .wrapping_add((((v.lfo as i32 * s.filt_lfo as i32) as i16 as i32) >> 8) as u16);
    let k = ((f & 0xFF) >> 2) as usize;
    let src = &s.bank[k * 128..k * 128 + 128];
    v.toggle ^= 0x80;
    let addr = x.buf + v.toggle;
    cy.c(if s.wave_x == 0 { "sy.w0" } else if s.wave_y == 0 { "sy.wx" } else { "sy.wxy" });
    cy.n("sy.words", words as u32);
    let wave = synth_wave(s, v, x, src, words as usize, sh as u32);
    hw.bus.write_bytes(addr, &wave);
    if log::log_enabled!(log::Level::Trace) {
        let h: String = wave.iter().map(|b| format!("{b:02x}")).collect();
        log::trace!("Sonix wave {:06x} {h}", addr);
    }
    v.out = (addr, words);
    false
}

fn synth_wave(s: &Synth, v: &mut Voice, x: &Ctx, src: &[u8], words: usize, sh: u32) -> Vec<u8> {
    let n = 2 * words;
    let step = 1usize << sh;
    let mut out = vec![0u8; n];
    if s.wave_x == 0 {
        for (i, o) in out.iter_mut().enumerate() {
            *o = src[(i * step) & 127];
        }
    } else if s.wave_y == 0 {
        v.wave_phase = v.wave_phase.wrapping_add(((s.wave_x as u32 * x.r as u32) >> 13) as u16);
        let pos = (v.wave_phase >> 9) as usize;
        for (i, o) in out.iter_mut().enumerate() {
            let a = src[(i * step) & 127] as i8 as i32;
            let b = src[(pos + i * step) & 127] as i8 as i32;
            *o = ((a + b) >> 1) as u8;
        }
    } else {
        let t = ((s.wave_x as u32 * x.r as u32) >> 11) as u16 as i16;
        let u = t.wrapping_mul(v.dir);
        let (mut r, ov) = (v.wave_phase as i16).overflowing_add(u);
        if ov {
            if r == i16::MIN {
                r = r.wrapping_add(v.dir);
            }
            v.dir = -v.dir;
            r = r.wrapping_neg();
        }
        v.wave_phase = r as u16;
        let e = (r as i32 * s.wave_y as i32) >> (17 + sh);
        let mut o = 0;
        for (len, start) in [(words as i32 + e, 0usize), (words as i32 - e, 64)] {
            if len <= 0 {
                continue;
            }
            let len = len as usize;
            let (mut pos, mut err) = (start, 0i32);
            let (q, rem) = (64 / len, (64 % len) as i32);
            for _ in 0..len {
                if o < n {
                    out[o] = src[pos & 127];
                }
                o += 1;
                err -= rem;
                let extra = if err < 0 {
                    err += len as i32;
                    1
                } else {
                    0
                };
                pos += q + extra;
            }
        }
    }
    out
}

impl Replayer for Sonix {
    fn format(&self) -> &'static str {
        "Sonix Music Driver"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        // a CIA timer counts its value and the reload: value + 1 E clocks of 5 colour clocks
        (self.interval as u64 + 1) * 5
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        // the interval that starts now was programmed before this interrupt
        self.interval = self.timer;
        self.cy.t0 = hw.bus.clock;
        self.cy.cy = 0;
        self.cy.c("tick");
        if self.tiny {
            if self.count != 0 {
                self.cy.c("tk.dec");
                self.count -= 1;
            }
            if self.count == 0 {
                self.cy.c("tk.step");
                self.count = self.tiny_tempo();
                self.tiny_step(hw);
            }
            let stop = self.voices(hw);
            self.cy.c("tk.stop");
            self.cy.w(hw);
            hw.dmacon(stop);
        } else {
            self.snx_sequence(hw);
            self.voices(hw);
        }
        self.output(hw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::{Path, PathBuf};

    /// An SNX song: track 0 selects instrument 0 (`T1`), sets tempo 120, plays note
    /// 0x30 at full velocity for 4 ticks, releases it and waits 4 more; the other
    /// tracks are empty.
    fn song() -> Vec<u8> {
        let t0: [u16; 8] = [0x8000, 0x8278, 0x307F, 0xC004, 0x3000, 0xC004, 0xFFFF, 0xFFFF];
        let t0 = &t0[..7];
        let mut d = Vec::new();
        for s in [2 * t0.len() as u32, 2, 2, 2] {
            d.extend_from_slice(&s.to_be_bytes());
        }
        d.extend_from_slice(&[0x00, 0xFF, 0x00, 0x78]);
        for w in t0.iter().chain(&[0xFFFF, 0xFFFF, 0xFFFF]) {
            d.extend_from_slice(&w.to_be_bytes());
        }
        d.extend_from_slice(b"T1\0\0");
        d
    }

    /// A sampled instrument: octaves 0 to 10 of 4 bytes << o, the first half of each
    /// block one-shot, the second repeated; full amplitude, envelope levels 255 reached
    /// at once and held.
    fn instrument() -> (Vec<u8>, Vec<u8>) {
        let mut i = vec![0u8; 128];
        i[..12].copy_from_slice(b"SampledSound");
        i[0x44..0x46].copy_from_slice(b"T1");
        let mut put = |o: usize, v: u16| i[o..o + 2].copy_from_slice(&v.to_be_bytes());
        put(0x68, 0xFF);
        for k in 0..3 {
            put(0x6A + 2 * k, 0xFF);
            put(0x72 + 2 * k, 0xE0);
        }
        put(0x78, 0xE0);
        let mut ss = vec![0u8; 62 + 4 * 2047 + 8];
        ss[..6].copy_from_slice(&[0, 4, 0, 2, 0, 10]);
        (i, ss)
    }

    fn play(d: Vec<u8>, ticks: usize) -> (String, bool, Vec<(u32, u16)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        let (i, ss) = instrument();
        e.m.os.mem_files.insert(PathBuf::from("/test/Instruments/T1.instr"), i);
        e.m.os.mem_files.insert(PathBuf::from("/test/instruments/t1.ss"), ss);
        e.play_data(Path::new("/test/snx.test"), d).unwrap();
        let format = e.playing.as_ref().unwrap().format.clone();
        let mut out = vec![0f32; 2 * 1000 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        (format, e.songend(), log.iter().map(|w| (w.0, w.1)).collect())
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn recognises_by_content() {
        let d = song();
        assert!(detect(&d));
        // a FORM (IFF-SMUS) is not ours, nor is a song with no instrument names
        let mut e = d.clone();
        e[..4].copy_from_slice(b"FORM");
        assert!(!detect(&e));
        assert!(!detect(&d[..d.len() - 4]));
        // an odd track size, a track that starts with a note
        let mut e = d.clone();
        e[3] |= 1;
        assert!(!detect(&e));
        let mut e = d.clone();
        e[20] = 0x30;
        assert!(!detect(&e));
        // a TINY header: the track offset at 0x30 must be 0x140, and tracks 1-3 empty
        let mut t = vec![0u8; 0x150];
        t[1] = 0xFE;
        t[3] = 0xA8;
        t[0x33] = 0;
        t[0x30..0x34].copy_from_slice(&0x140u32.to_be_bytes());
        for (k, o) in [0x144u32, 0x146, 0x148].iter().enumerate() {
            t[0x34 + 4 * k..0x38 + 4 * k].copy_from_slice(&o.to_be_bytes());
        }
        t[0x144..0x14A].fill(0xFF);
        assert!(detect(&t));
        t[0x31] = 1;
        assert!(!detect(&t));
    }

    #[test]
    fn tables_are_the_specified_ones() {
        // SEMI[r] = round(32768 * 2^(-r/12)); the TEMPO table's ends; the filter's first copy
        for (r, &s) in SEMI.iter().enumerate() {
            assert_eq!(s, (32768.0 * 2f64.powf(-(r as f64) / 12.0)).round() as u16);
        }
        assert_eq!((TEMPO[0], TEMPO[127]), (0xFA83, 0x1000));
        let wave: Vec<i8> = (0..128).map(|i| if i < 64 { 100 } else { -100 }).collect();
        let bank = filter_bank(&wave);
        assert_eq!(bank.len(), 64 * 128);
        // the last copy (the lowest cutoff) is a smoothed wave, not the wave
        assert_ne!(&bank[63 * 128..], &wave.iter().map(|&b| b as u8).collect::<Vec<_>>()[..]);
    }

    #[test]
    fn plays_a_sampled_note_and_ends() {
        let (format, songend, log) = play(song(), 12);
        assert_eq!(format, "Sonix Music Driver");
        // a sampled note: mute (DMA off, period 2), first block, then the loop block
        assert_eq!(&reg(&log, 0xDFF096)[..2], &[0x0001, 0x8001]);
        assert_eq!(&reg(&log, 0xDFF0A6)[..2], &[2, 0x1AB]);
        // octave 6: 256 bytes, half of them one-shot, half repeated
        assert_eq!(&reg(&log, 0xDFF0A4)[..2], &[0x80, 0x40]);
        // volume: the envelope reaches 165 in the first tick (full velocity and amplitude)
        assert_eq!(reg(&log, 0xDFF0A8)[0], 41);
        // the voices end together after the second wait; the song ends
        assert!(songend);
        // tick 5 releases the note: the level then falls, the volume with it
        let vol = reg(&log, 0xDFF0A8);
        assert!(vol.len() >= 6 && vol[5] < vol[1]);
    }
}
