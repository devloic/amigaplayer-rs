//! PolyTracker (PTM) modules, ported from libxmp (MIT, Copyright (C) 1996-2026
//! Claudio Matsuoka and Hipolito Carraro Jr: see LICENSE-libxmp): the loader of
//! `src/loaders/ptm_load.c` and the Scream Tracker 3 style playback it selects
//! (`player.c`, `read_event.c`, `effects.c`, `flow.c`, `lfo.c`, `period.c`) for the
//! effects a PTM module can hold.
//!
//! The format is PolyTracker 1.x's (Lone Ranger / AcmE): `PTMF` at 0x2C, up to 32
//! channels, 64-row patterns packed a cell at a time, instruments with `PTMS`
//! headers whose samples are 8 or 16 bits, stored as byte-wise deltas, and play at
//! a "C4 speed" in Hz. The playback is libxmp's:
//! - ST3 timing: a row is `speed` ticks, tempo is the CIA/BPM rate of ProTracker
//!   (50 Hz at 125), `Fxx` below 32 sets the speed and from 32 the tempo; pattern
//!   delay, loop (global target and count, a loop jump cancels a break before it
//!   but not a jump), break (in hex) and jump;
//! - a pitch is an Amiga-style period (13696 for C-0 and halved an octave up),
//!   moved by slides in period units; a note's period comes from its key, the
//!   sample's C4 speed (turned into a semitone shift and a fine tune) and `E5x`;
//! - effects 0 arpeggio, 1/2 slides (with the fine `Fx` and extra fine `Ex`
//!   forms), 3 tone portamento, 4 vibrato, 5 and 6 their combinations with a
//!   volume slide, 7 tremolo, 9 sample offset, A volume slide (`xF` and `Fx` are
//!   fine), B, C, D, E (fine slides E1/E2, glissando E3, waveforms E4/E7, fine
//!   tune E5, loop E6, retrigger E9, fine volume EA/EB, cut EC, delay ED, pattern
//!   delay EE), F, global volume, multi retrigger, fine vibrato and the four
//!   note slides (one with a retrigger);
//! - the volume is linear (0-64): the Eagleplayer hands the NotePlayer genie twice that, with no
//!   table (libxmp's `ptm_vol` is not used).
//!
//! Where this differs from libxmp, or cannot follow it:
//! - panning: the amplifier's voices have a side, left or right, from the start, as the
//!   Eagleplayer gives them to the NotePlayer genie: the module's pan is ignored and the
//!   channels alternate left, right, left ... `8xx` and `E8x` change nothing;
//! - samples are played at the amplifier's 8-bit, sample-and-hold mixing (at the genie's
//!   14477 Hz, wrapping round on overflow, as the genie's mix does), not with libxmp's
//!   interpolation; a 16-bit sample keeps its high byte;
//! - effect 1 slides the pitch down and 2 up (libxmp's loader maps them the other way round),
//!   as the Eagleplayer does (measured with synthetic modules on it, plain, fine and extra fine);
//! - the voice's pitch is not rounded to a whole Paula period (`Hw::voice_per_fine`, 1/64 colour
//!   clock): the genie's mix steps by the exact pitch, where whole periods were up to 0.1% off;
//! - a bidirectional loop plays forward only, as the Eagleplayer hands the loop to the NotePlayer
//!   genie (measured with a ramp sample on the genie);
//! - a pattern's cell for a channel beyond the module's count is read and dropped,
//!   where libxmp's loader goes on without reading the bytes that follow it;
//! - a note off silences the channel until the next instrument number (libxmp's
//!   release with no envelope); reverse (effect 17) is ignored, as in libxmp.
//!
//! The voices are the host's Eagleplayer amplifier's (`Hw::voice_*`), one a
//! channel. A note is a DMA off, LOC, LEN, period, volume and DMA on and then the
//! loop's LOC and LEN, as ProTracker writes them.

use super::{Hw, Open, Replayer};
use crate::amplifier::PER_SUB;
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "PolyTracker", detect, new };

/// The mix rate as a Paula period: the genie's, 3546895 / 245 = 14477 Hz.
const MIX_PERIOD: u16 = 245;

/// The boost in tenths over the voices of the busier side (the amplifier's own mix; the genie's
/// four-channel mix in `amplifier` scales by 4 / side in eighths, rounded down instead).
const MIX_BOOST: u32 = 39;

/// Volume of the multi retrigger: add, multiply, divide.
const RVAL: [(i32, i32, i32); 17] = [
    (0, 1, 1),
    (-1, 1, 1),
    (-2, 1, 1),
    (-4, 1, 1),
    (-8, 1, 1),
    (-16, 1, 1),
    (0, 2, 3),
    (0, 1, 2),
    (0, 1, 1),
    (1, 1, 1),
    (2, 1, 1),
    (4, 1, 1),
    (8, 1, 1),
    (16, 1, 1),
    (0, 3, 2),
    (0, 2, 1),
    (0, 0, 1),
];

/// The vibrato and tremolo sine, 64 steps.
const SINE: [i32; 64] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197, 180, 161, 141,
    120, 97, 74, 49, 24, 0, -24, -49, -74, -97, -120, -141, -161, -180, -197, -212, -224, -235, -244, -250, -253, -255, -253, -250,
    -244, -235, -224, -212, -197, -180, -161, -141, -120, -97, -74, -49, -24,
];

const HEADER: usize = 608;
const INSTR: usize = 80;
/// Period of C-0, and the period of the C4 speed's note in the colour clocks of
/// Paula over the mixer's: the voice period is `libxmp period * PAULA / (428 * 8363)`.
const PERIOD_BASE: f64 = 13696.0;
const PAULA_PAL: f64 = 3_546_895.0;
const KEY_OFF: u16 = 0xFFFF;

// Row flags, as libxmp's SET/TEST.
const VIBRATO: u32 = 1;
const TREMOLO: u32 = 1 << 1;
const VOL_SLIDE: u32 = 1 << 2;
const FINE_VOLS: u32 = 1 << 3;
const PITCHBEND: u32 = 1 << 4;
const FINE_BEND: u32 = 1 << 5;
const TONEPORTA: u32 = 1 << 6;
const RETRIG: u32 = 1 << 7;
const NOTE_SLIDE: u32 = 1 << 8;

fn u16le(d: &[u8], o: usize) -> u32 {
    d.get(o..o + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as u32)
}

fn u32le(d: &[u8], o: usize) -> u32 {
    d.get(o..o + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// A PolyTracker module: `PTMF` at 0x2C after the DOS end-of-file mark (0x1A) at 28,
/// and a header whose counts are in range (up to 256 orders, 255 instruments, 128
/// patterns, 1-32 channels) with room for the header and the instrument table. (Some
/// used instruments in the archive's modules lack the `PTMS` mark: it is not checked,
/// as libxmp does not.)
pub fn detect(d: &[u8]) -> bool {
    if d.len() < HEADER || &d[44..48] != b"PTMF" || d[28] != 0x1A {
        return false;
    }
    let (ord, ins, pat, chn) = (u16le(d, 32), u16le(d, 34), u16le(d, 36), u16le(d, 38));
    if ord == 0 || ord > 256 || ins > 255 || pat > 128 || chn == 0 || chn > 32 {
        return false;
    }
    d.len() >= HEADER + INSTR * ins as usize
}

#[derive(Clone, Copy, Default)]
struct Cell {
    /// 0 none, `KEY_OFF`, else the note + 12 (libxmp's, 1-based).
    note: u16,
    ins: u8,
    fxt: u8,
    fxp: u8,
    /// 0 none, else the volume + 1.
    vol: u8,
}

#[derive(Clone, Copy, Default)]
struct Smp {
    /// The sample is played: it has data and is a sample instrument.
    usable: bool,
    addr: u32,
    /// Bytes played from the start to the end of the loop (or the end).
    end: u32,
    /// Bytes of the loop; 0: none.
    loop_start: u32,
    loop_len: u32,
    vol: u8,
    xpo: i32,
    fin: i32,
}

#[derive(Clone, Copy, Default)]
struct Lfo {
    phase: i32,
    rate: i32,
    depth: i32,
    kind: u8,
}

impl Lfo {
    fn get(&self, rng: &mut u32) -> i32 {
        if self.rate == 0 {
            return 0;
        }
        let ph = (self.phase & 63) as usize;
        let v = match self.kind {
            0 => SINE[ph],
            1 => 255 - ((ph as i32) << 3),
            // ST3's square has no negative half
            2 => {
                if ph < 32 {
                    255
                } else {
                    0
                }
            }
            3 => {
                *rng ^= *rng << 13;
                *rng ^= *rng >> 17;
                *rng ^= *rng << 5;
                (*rng % 512) as i32 - 256
            }
            _ => return 0,
        };
        v * self.depth
    }

    fn update(&mut self) {
        self.phase = (self.phase + self.rate) & 63;
    }
}

#[derive(Clone, Copy)]
struct Ch {
    ins: i32,
    key: i32,
    note: i32,
    period: f64,
    finetune: i32,
    volume: i32,
    mastervol: i32,
    flags: u32,
    /// A note off is waiting for the next instrument number.
    released: bool,
    /// The channel was cut (`ECx`): it stays silent, whatever its volume is set to, until the
    /// next note (measured on the genie).
    cut: bool,
    has_voice: bool,
    smp: usize,
    delay: i32,
    delayed: Cell,
    glissando: bool,
    freq_slide: f64,
    freq_fslide: f64,
    freq_mem: i32,
    fine_up_mem: i32,
    fine_down_mem: i32,
    fvol_up_mem: i32,
    fvol_down_mem: i32,
    porta_slide: i32,
    porta_target: f64,
    porta_dir: i32,
    porta_mem: i32,
    vol_slide: i32,
    vol_fslide: i32,
    vol_mem: i32,
    vib: Lfo,
    vib_mem: i32,
    tre: Lfo,
    tre_mem: i32,
    arp: [i32; 3],
    arp_size: usize,
    arp_count: usize,
    retrig_val: i32,
    retrig_count: i32,
    retrig_type: usize,
    ns_slide: i32,
    ns_count: i32,
    ns_speed: i32,
    offset_val: i32,
    offset_val2: i32,
    offset_mem: i32,
    /// A voice start waiting for the tick's period and volume: the byte to start at.
    start: Option<i32>,
    last_per: i32,
    last_vol: i32,
}

impl Default for Ch {
    fn default() -> Self {
        Ch {
            ins: -1,
            key: -1,
            note: 0,
            period: 0.0,
            finetune: 0,
            volume: 64,
            mastervol: 64,
            flags: 0,
            released: false,
            cut: false,
            has_voice: false,
            smp: 0,
            delay: 0,
            delayed: Cell::default(),
            glissando: false,
            freq_slide: 0.0,
            freq_fslide: 0.0,
            freq_mem: 0,
            fine_up_mem: 0,
            fine_down_mem: 0,
            fvol_up_mem: 0,
            fvol_down_mem: 0,
            porta_slide: 0,
            porta_target: 0.0,
            porta_dir: 0,
            porta_mem: 0,
            vol_slide: 0,
            vol_fslide: 0,
            vol_mem: 0,
            vib: Lfo::default(),
            vib_mem: 0,
            tre: Lfo::default(),
            tre_mem: 0,
            arp: [0; 3],
            arp_size: 1,
            arp_count: 0,
            retrig_val: 0,
            retrig_count: 0,
            retrig_type: 0,
            ns_slide: 0,
            ns_count: 0,
            ns_speed: 0,
            offset_val: 0,
            offset_val2: 0,
            offset_mem: 0,
            start: None,
            last_per: -1,
            last_vol: -1,
        }
    }
}

/// The song's flow of control (libxmp's `flow_control`).
struct Flow {
    pbreak: bool,
    jump: i32,
    jumpline: i32,
    loop_dest: i32,
    loop_start: i32,
    loop_count: i32,
    delay: i32,
}

pub struct Ptm {
    title: String,
    n: usize,
    orders: Vec<u8>,
    npat: usize,
    /// `patterns[pattern][row * n + channel]`.
    patterns: Vec<Vec<Cell>>,
    smps: Vec<Smp>,
    silence: u32,
    ch: Vec<Ch>,
    flow: Flow,
    started: bool,
    first_tick: bool,
    ord: i32,
    row: i32,
    frame: i32,
    speed: i32,
    bpm: i32,
    gvol: i32,
    rng: u32,
    songend: bool,
    stopped: bool,
}

/// Semitone shift and fine tune (1/128 semitone) of a C4 speed.
fn c2spd_to_note(c2spd: u32) -> (i32, i32) {
    if c2spd == 0 {
        return (0, 0);
    }
    let c = (1536.0 * (c2spd as f64 / 8363.0).ln() / std::f64::consts::LN_2) as i32;
    (c / 128, c % 128)
}

fn note_to_period(n: i32, fine: i32) -> f64 {
    PERIOD_BASE / 2f64.powf((n as f64 + fine as f64 / 128.0) / 12.0)
}

fn period_to_bend(p: f64, n: i32) -> i32 {
    if n == 0 || p < 0.1 {
        return 0;
    }
    let d = note_to_period(n, 0);
    (100.0 * (1536.0 / std::f64::consts::LN_2) * (d / p).ln()).round() as i32
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let (ordnum, insnum, patnum, chn) = (u16le(d, 32) as usize, u16le(d, 34) as usize, u16le(d, 36) as usize, u16le(d, 38) as usize);
    let title = d[..28].iter().take_while(|&&c| c != 0).map(|&c| c as char).collect::<String>().trim().to_string();
    let orders = d[0x60..0x60 + 256][..ordnum].to_vec();
    // sides: the Eagleplayer ignores the module's panning and gives the channels left and right
    // alternately, starting on the left
    let pans: Vec<i16> = (0..chn).map(|c| if c % 2 == 0 { 1 } else { -1 }).collect();

    // instruments and their samples
    let mut smps = Vec::with_capacity(insnum);
    for i in 0..insnum {
        let o = HEADER + INSTR * i;
        let typ = d[o];
        let c4spd = u16le(d, o + 14);
        let (smpofs, mut len, mut lps, mut lpe) = (u32le(d, o + 18) as usize, u32le(d, o + 22) as usize, u32le(d, o + 26) as usize, u32le(d, o + 30) as usize);
        let wide = typ & 16 != 0;
        if wide {
            len >>= 1;
            lps >>= 1;
            lpe >>= 1;
        }
        let mut looped = typ & 12 != 0;
        if lpe > len {
            lpe = len;
        }
        if lps >= len || lps >= lpe {
            (lps, lpe, looped) = (0, 0, false);
        }
        let (xpo, fin) = c2spd_to_note(c4spd);
        let mut s = Smp { usable: false, addr: 0, end: 0, loop_start: 0, loop_len: 0, vol: d[o + 13].min(64), xpo, fin };
        if typ & 3 == 1 && len > 0 && len <= 16 << 20 {
            // byte-wise deltas; a short file ends the sample in zeros
            let bytes = len * if wide { 2 } else { 1 };
            let mut raw = vec![0u8; bytes];
            if let Some(src) = d.get(smpofs..) {
                let k = src.len().min(bytes);
                raw[..k].copy_from_slice(&src[..k]);
            }
            let mut acc = 0u8;
            for b in raw.iter_mut() {
                acc = acc.wrapping_add(*b);
                *b = acc;
            }
            let mut pcm: Vec<u8> = if wide { raw.chunks(2).map(|c| c[1]).collect() } else { raw };
            if looped {
                pcm.truncate(lpe);
                s.loop_len = (lpe - lps) as u32;
                s.loop_start = lps as u32;
            }
            s.end = pcm.len() as u32;
            let a = m.alloc(pcm.len() as u32 + 2);
            if a == 0 {
                return None;
            }
            m.bus.write_bytes(a, &pcm);
            s.addr = a;
            s.usable = true;
        }
        smps.push(s);
    }

    // patterns
    let mut patterns = Vec::with_capacity(patnum);
    for p in 0..patnum {
        let mut cells = vec![Cell::default(); 64 * chn];
        let seg = u16le(d, 0x160 + 2 * p) as usize;
        if seg != 0 {
            let mut at = 16 * seg;
            let mut row = 0;
            let mut seen = [false; 32];
            let next = |at: &mut usize| -> u8 {
                let b = d.get(*at).copied().unwrap_or(0);
                *at += 1;
                b
            };
            while row < 64 && at < d.len() {
                let b = next(&mut at);
                if b == 0 {
                    row += 1;
                    seen = [false; 32];
                    continue;
                }
                let c = (b & 0x1f) as usize;
                if seen[c] {
                    break;
                }
                seen[c] = true;
                let mut cell = Cell::default();
                if b & 0x20 != 0 {
                    let n = next(&mut at);
                    cell.note = match n {
                        255 => 0,
                        254 => KEY_OFF,
                        n => n as u16 + 12,
                    };
                    cell.ins = next(&mut at);
                }
                if b & 0x40 != 0 {
                    let (t, a) = (next(&mut at), next(&mut at));
                    if t <= 0x17 {
                        cell.fxt = t;
                        cell.fxp = a;
                    }
                    // E8x is a pan (ignored) and 17 a reverse (ignored)
                    if t == 0x17 || (t == 0x0e && a >> 4 == 8) {
                        cell.fxt = 0;
                        cell.fxp = 0;
                    }
                }
                if b & 0x80 != 0 {
                    cell.vol = next(&mut at).wrapping_add(1);
                }
                if c < chn {
                    cells[row * chn + c] = cell;
                }
            }
        }
        patterns.push(cells);
    }

    let silence = m.alloc(4);
    // The mix is DeliTracker's NotePlayer genie's (the 32 voice one, which plays a `_note` player
    // for as many voices): 8 bits at 14477 Hz, four Paula channels each summing a group of the
    // voices (wrapping round where it overflows), a voice counting 4/side of its sample in
    // eighths (a lone voice of a 4 channel module: twice).
    if silence == 0
        || !crate::amplifier::start_panned_mix(
            m,
            pans.len(),
            crate::eagle::epamb::DIRECT | crate::eagle::epamb::BIT8 | crate::eagle::epamb::CHIP_RAM,
            &pans,
            MIX_PERIOD,
            MIX_BOOST,
        )
    {
        return None;
    }
    Some(Box::new(Ptm {
        title,
        n: chn,
        orders,
        npat: patnum,
        patterns,
        smps,
        silence,
        ch: vec![Ch::default(); chn],
        flow: Flow { pbreak: false, jump: -1, jumpline: 0, loop_dest: -1, loop_start: -1, loop_count: 0, delay: 0 },
        started: false,
        first_tick: true,
        ord: 0,
        row: 0,
        frame: 0,
        speed: 6,
        bpm: 125,
        gvol: 64,
        rng: 0x2545_F491,
        songend: false,
        stopped: false,
    }))
}

fn msn(x: i32) -> i32 {
    (x >> 4) & 15
}
fn lsn(x: i32) -> i32 {
    x & 15
}

/// `p` if nonzero (and remembered), else the memory.
fn memory(p: i32, mem: &mut i32) -> i32 {
    if p == 0 {
        *mem
    } else {
        *mem = p;
        p
    }
}

impl Ptm {
    fn sub(&self, ins: i32) -> Option<usize> {
        (ins >= 0 && (ins as usize) < self.smps.len() && self.smps[ins as usize].usable).then_some(ins as usize)
    }

    fn valid_ins(&self, ins: i32) -> bool {
        ins >= 0 && (ins as usize) < self.smps.len()
    }

    /// Order `ord` to play is a pattern: orders that are not (254) are skipped, 255
    /// ends the song.
    fn skip_invalid_orders(&mut self) -> bool {
        loop {
            if self.ord < 0 {
                self.ord = 0;
            }
            if self.ord as usize >= self.orders.len() || self.orders[self.ord as usize] == 255 {
                return false;
            }
            if (self.orders[self.ord as usize] as usize) < self.npat {
                return true;
            }
            self.ord += 1;
        }
    }

    fn next_order(&mut self) {
        self.ord += 1;
        if !self.skip_invalid_orders() {
            // the song's end: back to the first order
            self.songend = true;
            self.ord = 0;
            self.gvol = 64;
            if !self.skip_invalid_orders() {
                self.stopped = true;
                return;
            }
        }
        if self.flow.jumpline >= 64 {
            self.flow.jumpline = 0;
        }
        self.row = self.flow.jumpline;
        self.flow.jumpline = 0;
        self.frame = 0;
    }

    fn next_row(&mut self) {
        self.frame = 0;
        self.flow.delay = 0;
        if self.flow.pbreak {
            self.flow.pbreak = false;
            self.flow.loop_dest = -1;
            if self.flow.jump != -1 {
                self.ord = self.flow.jump - 1;
                self.flow.jump = -1;
            }
            self.next_order();
        } else {
            self.row += 1;
            if self.flow.loop_dest >= 0 {
                self.row = self.flow.loop_dest;
                self.flow.loop_dest = -1;
            }
            if self.row >= 64 {
                self.next_order();
            }
        }
    }

    fn pattern_loop(&mut self, row: i32, fxp: i32) {
        let f = &mut self.flow;
        let mut looped = false;
        if fxp == 0 {
            f.loop_start = row;
        } else {
            if f.loop_start < 0 {
                f.loop_start = 0;
            }
            if f.loop_count != 0 {
                f.loop_count -= 1;
                if f.loop_count != 0 {
                    f.loop_dest = f.loop_start;
                    looped = true;
                }
            } else {
                f.loop_count = fxp;
                f.loop_dest = f.loop_start;
                looped = true;
            }
        }
        // a loop jump cancels a break before it in the row, not a jump
        if looped && f.pbreak && f.jump < 0 {
            f.pbreak = false;
        }
    }

    fn is_first_frame(&self) -> bool {
        self.frame % self.speed.max(1) == 0
    }

    fn read_row(&mut self) {
        for chn in 0..self.n {
            let pat = self.orders[self.ord as usize] as usize;
            let e = self.patterns[pat][self.row as usize * self.n + chn];
            // tempo affects the delay and comes first
            if e.fxt == 0x0f && e.fxp < 0x20 && e.fxp != 0 {
                self.speed = e.fxp as i32;
            }
            if e.fxt == 0x0e && e.fxp >> 4 == 0xd && e.fxp & 15 != 0 {
                let xc = &mut self.ch[chn];
                xc.delay = (e.fxp & 15) as i32 + 1;
                xc.delayed = e;
            } else {
                self.read_event(e, chn);
            }
        }
    }

    fn read_event(&mut self, e: Cell, chn: usize) {
        let mut is_toneporta = e.fxt == 3 || e.fxt == 5;
        let mut note: i32 = -1;
        let mut not_same_ins = false;
        let enote = if e.note == KEY_OFF { 0 } else { e.note as i32 };
        {
            let xc = &mut self.ch[chn];
            xc.flags = 0;
            if !xc.has_voice && xc.ins != e.ins as i32 - 1 {
                is_toneporta = false;
            }
            if (0..121).contains(&(enote - 1)) && !is_toneporta {
                xc.key = enote - 1;
            }
        }
        if e.ins != 0 {
            let ins = e.ins as i32 - 1;
            let key_ins = self.sub(ins);
            let valid = self.valid_ins(ins);
            let xc = &mut self.ch[chn];
            xc.offset_val = 0;
            xc.released = false;
            if valid {
                if xc.ins != ins {
                    not_same_ins = true;
                    if !is_toneporta {
                        xc.ins = ins;
                    }
                }
                if let Some(s) = key_ins {
                    if e.note != KEY_OFF {
                        xc.volume = self.smps[s].vol as i32;
                    }
                }
            } else {
                xc.flags = 0;
            }
        }
        if e.note != 0 {
            if e.note != KEY_OFF {
                self.ch[chn].cut = false;
            }
            if e.note == KEY_OFF {
                self.ch[chn].released = true;
            } else if is_toneporta {
                if not_same_ins {
                    self.ch[chn].offset_val = 0;
                }
            } else if (0..121).contains(&(enote - 1)) {
                let cur = self.ch[chn].ins;
                match self.sub(cur) {
                    Some(s) => {
                        let xc = &mut self.ch[chn];
                        note = xc.key + self.smps[s].xpo;
                        xc.smp = s;
                    }
                    None => self.ch[chn].flags = 0,
                }
            }
        }
        // the sample now playing, which with a tone portamento may not be e.ins
        let cur = self.ch[chn].ins;
        let sub = self.sub(cur);
        {
            let xc = &mut self.ch[chn];
            // set_effect_defaults
            if let Some(s) = sub {
                if note >= 0 {
                    xc.finetune = self.smps[s].fin;
                    xc.vib.phase = 0;
                    xc.tre.phase = 0;
                }
            }
            xc.delay = 0;
            xc.arp[0] = 0;
            xc.arp_count = 0;
            xc.arp_size = 1;
            if is_toneporta {
                xc.porta_slide = 0;
            }
            if e.vol != 0 {
                xc.volume = e.vol as i32 - 1;
            }
        }
        self.process_fx(chn, e, enote);
        let xc = &mut self.ch[chn];
        // set_period
        if sub.is_some() && note >= 0 {
            let per = note_to_period(note, xc.finetune);
            xc.porta_target = per;
            if xc.period < 1.0 || !is_toneporta {
                xc.period = per;
            }
        }
        if sub.is_none() {
            return;
        }
        if note >= 0 {
            xc.note = note;
            xc.has_voice = true;
            xc.start = Some(xc.offset_val);
        }
    }

    fn do_toneporta(&mut self, chn: usize, enote: i32) {
        let xc = &mut self.ch[chn];
        if (0..121).contains(&(enote - 1)) && xc.ins >= 0 && (xc.ins as usize) < self.smps.len() {
            let n = enote - 1;
            xc.porta_target = note_to_period(n + self.smps[xc.ins as usize].xpo, xc.finetune);
        }
        xc.porta_dir = if xc.period < xc.porta_target { 1 } else { -1 };
    }

    fn volslide(&mut self, chn: usize, mut fxp: i32) {
        let xc = &mut self.ch[chn];
        loop {
            let (h, l) = (msn(fxp), lsn(fxp));
            if l == 0xf && h != 0 {
                xc.vol_mem = fxp;
                xc.flags |= FINE_VOLS;
                xc.vol_fslide = h;
                return;
            } else if h == 0xf && l != 0 {
                xc.vol_mem = fxp;
                xc.flags |= FINE_VOLS;
                xc.vol_fslide = -l;
                return;
            }
            if fxp == 0 {
                fxp = xc.vol_mem;
                if fxp != 0 {
                    continue;
                }
            }
            break;
        }
        if fxp != 0 {
            xc.flags |= VOL_SLIDE;
            xc.vol_mem = fxp;
            let (h, l) = (msn(fxp), lsn(fxp));
            xc.vol_slide = if l != 0 { -l } else { h };
        }
        if msn(xc.vol_mem) == 0xf || lsn(xc.vol_mem) == 0xf {
            xc.flags |= FINE_VOLS;
            xc.vol_fslide = xc.vol_slide;
        }
    }

    fn process_fx(&mut self, chn: usize, e: Cell, enote: i32) {
        let (fxt, mut fxp) = (e.fxt, e.fxp as i32);
        match fxt {
            0 => {
                let xc = &mut self.ch[chn];
                xc.arp = [0, msn(fxp), lsn(fxp)];
                xc.arp_size = 3;
            }
            1 | 2 => {
                let xc = &mut self.ch[chn];
                // PolyTracker's 1xx slides the pitch down and 2xx up (measured on the Eagleplayer;
                // libxmp's loader maps them the other way round)
                let sign = if fxt == 1 { 1.0 } else { -1.0 };
                fxp = memory(fxp, &mut xc.freq_mem);
                match msn(fxp) {
                    0xf => {
                        fxp &= 15;
                        if fxp != 0 {
                            xc.flags |= FINE_BEND;
                            xc.freq_fslide = sign * fxp as f64;
                        }
                    }
                    0xe => {
                        fxp &= 15;
                        xc.flags |= FINE_BEND;
                        xc.freq_fslide = sign * 0.25 * fxp as f64;
                    }
                    _ => {
                        if fxp != 0 {
                            xc.flags |= PITCHBEND;
                            xc.freq_slide = sign * fxp as f64;
                        }
                    }
                }
            }
            3 => {
                let xc = &mut self.ch[chn];
                fxp = memory(fxp, &mut xc.porta_mem);
                if fxp != 0 {
                    xc.porta_slide += fxp;
                }
                if !self.valid_ins(self.ch[chn].ins) {
                    return;
                }
                self.do_toneporta(chn, enote);
                self.ch[chn].flags |= TONEPORTA;
            }
            4 | 0x12 => {
                let xc = &mut self.ch[chn];
                fxp = memory(fxp, &mut xc.vib_mem);
                xc.flags |= VIBRATO;
                let depth = if fxt == 4 { lsn(fxp) << 2 } else { lsn(fxp) };
                if depth != 0 {
                    xc.vib.depth = depth;
                }
                if msn(fxp) != 0 {
                    xc.vib.rate = msn(fxp);
                }
            }
            5 => {
                let xc = &mut self.ch[chn];
                xc.porta_slide += xc.porta_mem;
                if self.valid_ins(self.ch[chn].ins) {
                    self.do_toneporta(chn, enote);
                    self.ch[chn].flags |= TONEPORTA;
                    self.volslide(chn, fxp);
                }
            }
            6 => {
                self.ch[chn].flags |= VIBRATO;
                self.volslide(chn, fxp);
            }
            7 => {
                let xc = &mut self.ch[chn];
                fxp = memory(fxp, &mut xc.tre_mem);
                xc.flags |= TREMOLO;
                if lsn(fxp) != 0 {
                    xc.tre.depth = lsn(fxp);
                }
                if msn(fxp) != 0 {
                    xc.tre.rate = msn(fxp);
                }
            }
            9 => {
                let xc = &mut self.ch[chn];
                fxp = memory(fxp, &mut xc.offset_mem);
                if enote != 0 {
                    xc.offset_val = (xc.offset_val & !0xffff) | (fxp << 8);
                    xc.offset_val2 = fxp << 8;
                }
                if e.ins != 0 {
                    xc.offset_val2 = fxp << 8;
                }
            }
            0x0a => self.volslide(chn, fxp),
            0x0b => {
                self.flow.pbreak = true;
                self.flow.jump = fxp;
                self.flow.jumpline = 0;
            }
            0x0c => self.ch[chn].volume = fxp,
            0x0d => {
                self.flow.pbreak = true;
                self.flow.jumpline = fxp;
            }
            0x0e => {
                let x = fxp & 15;
                let xc = &mut self.ch[chn];
                match msn(fxp) {
                    1 => {
                        let x = memory(x, &mut xc.fine_up_mem);
                        if x != 0 {
                            xc.flags |= FINE_BEND;
                            xc.freq_fslide = -x as f64;
                        }
                    }
                    2 => {
                        let x = memory(x, &mut xc.fine_down_mem);
                        if x != 0 {
                            xc.flags |= FINE_BEND;
                            xc.freq_fslide = x as f64;
                        }
                    }
                    3 => xc.glissando = x != 0,
                    4 => xc.vib.kind = (x & 3) as u8,
                    5 => xc.finetune = ((x << 4) as u8 as i8) as i32,
                    6 => {
                        let row = self.row;
                        self.pattern_loop(row, x);
                    }
                    7 => xc.tre.kind = (x & 3) as u8,
                    9 => {
                        xc.flags |= RETRIG;
                        xc.retrig_val = x;
                        xc.retrig_count = x + 1;
                        xc.retrig_type = 0;
                    }
                    0xa => {
                        let x = memory(x, &mut xc.fvol_up_mem);
                        xc.flags |= FINE_VOLS;
                        xc.vol_fslide = x;
                    }
                    0xb => {
                        let x = memory(x, &mut xc.fvol_down_mem);
                        xc.flags |= FINE_VOLS;
                        xc.vol_fslide = -x;
                    }
                    0xc => {
                        xc.flags |= RETRIG;
                        xc.retrig_val = x + 1;
                        xc.retrig_count = xc.retrig_val;
                        xc.retrig_type = 0x10;
                    }
                    0xe => {
                        if self.flow.delay == 0 {
                            self.flow.delay = x;
                        }
                    }
                    _ => {}
                }
            }
            0x0f => {
                if fxp < 0x20 {
                    if fxp != 0 {
                        self.speed = fxp;
                    }
                } else {
                    self.bpm = fxp.max(20);
                }
            }
            0x10 => self.gvol = fxp.min(64),
            0x11 => {
                let xc = &mut self.ch[chn];
                if fxp != 0 {
                    xc.retrig_val = lsn(fxp);
                    xc.retrig_type = msn(fxp) as usize;
                }
                if enote != 0 {
                    xc.retrig_count = xc.retrig_val + 1;
                }
                xc.flags |= RETRIG;
            }
            0x13..=0x16 => {
                let xc = &mut self.ch[chn];
                if fxp != 0 {
                    if fxt >= 0x15 {
                        xc.retrig_val = msn(fxp);
                        xc.retrig_count = msn(fxp) + 1;
                        xc.retrig_type = 0;
                    }
                    xc.ns_slide = if fxt == 0x14 || fxt == 0x16 { lsn(fxp) } else { -lsn(fxp) };
                    xc.ns_speed = msn(fxp);
                    xc.ns_count = msn(fxp);
                }
                if fxt >= 0x15 {
                    xc.flags |= RETRIG;
                }
                xc.flags |= NOTE_SLIDE;
            }
            _ => {}
        }
    }

    /// One tick of one channel. Returns the voice's period and volume, if it plays.
    fn play_channel(&mut self, chn: usize) -> Option<(i32, i32)> {
        if self.ch[chn].delay > 0 {
            self.ch[chn].delay -= 1;
            if self.ch[chn].delay == 0 {
                let e = self.ch[chn].delayed;
                self.read_event(e, chn);
            }
        }
        let first = self.is_first_frame();
        let gvol = self.gvol;
        let xc = &mut self.ch[chn];
        if xc.ins < 0 {
            return None;
        }
        // cut and retrigger
        if xc.flags & RETRIG != 0 {
            xc.retrig_count -= 1;
            if xc.retrig_count <= 0 {
                if xc.retrig_type < 0x10 && xc.has_voice {
                    xc.start = Some(0);
                }
                if xc.retrig_type == 0x10 {
                    // the cut mutes the voice and leaves the channel's volume alone
                    xc.cut = true;
                } else {
                    let (s, mu, dv) = RVAL[xc.retrig_type.min(16)];
                    xc.volume += s;
                    xc.volume *= mu;
                    xc.volume /= dv;
                }
                xc.retrig_count = xc.retrig_val;
            }
        }
        // update_volume
        if !first {
            if xc.flags & VOL_SLIDE != 0 {
                xc.volume += xc.vol_slide;
            }
        }
        if first && xc.flags & FINE_VOLS != 0 {
            xc.volume += xc.vol_fslide;
        }
        xc.volume = xc.volume.clamp(0, 64);
        // update_frequency
        if !first {
            if xc.flags & PITCHBEND != 0 {
                xc.period += xc.freq_slide;
            }
            if xc.flags & TONEPORTA != 0 && xc.porta_target > 0.0 {
                let end;
                if xc.porta_dir > 0 {
                    xc.period += xc.porta_slide as f64;
                    end = xc.period >= xc.porta_target;
                } else {
                    xc.period -= xc.porta_slide as f64;
                    end = xc.period <= xc.porta_target;
                }
                if end {
                    xc.period = xc.porta_target;
                    xc.porta_dir = 0;
                    xc.flags &= !TONEPORTA;
                }
            }
        }
        if first && xc.flags & FINE_BEND != 0 {
            xc.period += xc.freq_fslide;
        }
        // process_volume
        let mut vol = xc.volume;
        if xc.flags & TREMOLO != 0 {
            vol += xc.tre.get(&mut self.rng) / 64;
            if !first {
                xc.tre.update();
            }
        }
        let vol = vol.clamp(0, 64);
        let out_vol = if xc.released || xc.cut {
            0
        } else {
            // linear: the Eagleplayer hands the genie twice this (0-128)
            (((gvol * xc.mastervol * vol) >> 8) >> 4).clamp(0, 64)
        };
        // process_frequency: note slide, vibrato, arpeggio, glissando
        if xc.flags & NOTE_SLIDE != 0 {
            if xc.ns_count == 0 {
                xc.note += xc.ns_slide;
                xc.period = note_to_period(xc.note, xc.finetune);
                xc.ns_count = xc.ns_speed;
            }
            xc.ns_count -= 1;
        }
        let mut vibrato = 0.0;
        if xc.flags & VIBRATO != 0 {
            vibrato = (xc.vib.get(&mut self.rng) / 512) as f64;
            if !first {
                xc.vib.update();
            }
        }
        let period = xc.period.max(0.1);
        let arp = xc.arp[xc.arp_count];
        xc.arp_count = (xc.arp_count + 1) % xc.arp_size;
        let mut bend = period_to_bend(period + vibrato, xc.note);
        if xc.glissando && xc.flags & TONEPORTA != 0 {
            if bend > 0 {
                bend = (bend + 6400) / 12800 * 12800;
            } else if bend < 0 {
                bend = (bend - 6400) / 12800 * 12800;
            }
        }
        if arp != 0 {
            bend += 12800 * arp;
        }
        let fin = PERIOD_BASE / 2f64.powf((xc.note as f64 + bend as f64 / 12800.0) / 12.0);
        let per = (fin * PAULA_PAL / (428.0 * 8363.0) * PER_SUB as f64).round().clamp(PER_SUB as f64, 65535.0 * PER_SUB as f64) as i32;
        Some((per, out_vol))
    }

    fn voice_start(&self, hw: &mut Hw, v: usize, off: i32) {
        let s = &self.smps[self.ch[v].smp];
        hw.voice_dma(1 << v, false);
        let mut off = off.max(0) as u32;
        if off >= s.end {
            if s.loop_len == 0 {
                hw.voice_loc_byte(v, self.silence);
                hw.voice_len(v, 1);
                return;
            }
            off = s.loop_start;
        }
        hw.voice_loc_byte(v, s.addr + off);
        hw.voice_len_bytes(v, s.end - off);
    }

    fn voice_loop(&self, hw: &mut Hw, v: usize) {
        let s = &self.smps[self.ch[v].smp];
        if s.loop_len != 0 {
            hw.voice_loc_byte(v, s.addr + s.loop_start);
            hw.voice_len_bytes(v, s.loop_len);
        } else {
            hw.voice_loc_byte(v, self.silence);
            hw.voice_len(v, 1);
        }
    }
}

impl Replayer for Ptm {
    fn format(&self) -> &'static str {
        "PolyTracker"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    /// The CIA timer of ProTracker's tempo: 1773447 / BPM, plus one, E-clock ticks of
    /// 5 colour clocks.
    fn tick_clocks(&self) -> u64 {
        (1_773_447 / self.bpm.max(1) as u64 + 1) * 5
    }

    fn tick(&mut self, hw: &mut Hw) {
        if self.first_tick {
            self.first_tick = false;
            hw.voice_dma(u32::MAX >> (32 - self.n), false);
            for v in 0..self.n {
                hw.voice_loc_byte(v, self.silence);
                hw.voice_len(v, 1);
                hw.voice_per(v, 0x71);
                hw.voice_vol(v, 0);
                self.ch[v].last_vol = 0;
            }
        }
        if self.stopped {
            return;
        }
        if !self.started {
            self.started = true;
            self.ord = -1;
            self.next_order();
            if self.stopped {
                return;
            }
        } else {
            self.frame += 1;
            if self.frame >= self.speed * (1 + self.flow.delay) {
                self.next_row();
                if self.stopped {
                    return;
                }
            }
        }
        if self.frame == 0 {
            self.read_row();
        }
        for v in 0..self.n {
            let r = self.play_channel(v);
            let (restart, loop_after) = (self.ch[v].start.take(), self.ch[v].has_voice);
            if let Some(off) = restart {
                self.voice_start(hw, v, off);
                self.ch[v].last_per = -1;
                self.ch[v].last_vol = -1;
            }
            if let (Some((per, vol)), true) = (r, loop_after) {
                if per != self.ch[v].last_per {
                    self.ch[v].last_per = per;
                    hw.voice_per_fine(v, per as u32);
                }
                if vol != self.ch[v].last_vol {
                    self.ch[v].last_vol = vol;
                    hw.voice_vol(v, vol as u16);
                }
            }
            if restart.is_some() {
                hw.voice_dma(1 << v, true);
                self.voice_loop(hw, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// A module of `n` channels with one 64-byte looped 8-bit sample (loop from byte
    /// 16) of C4 speed `c4spd`, and one 64-row pattern holding `cells` at (row,
    /// channel) as (note byte, instrument, effect, parameter, volume byte or 255).
    fn module(n: usize, c4spd: u16, cells: &[(usize, usize, u8, u8, u8, u8, u8)]) -> Vec<u8> {
        let mut d = vec![0u8; HEADER + INSTR];
        d[..8].copy_from_slice(b"test ptm");
        d[28] = 0x1A;
        d[32..34].copy_from_slice(&2u16.to_le_bytes());
        d[34..36].copy_from_slice(&1u16.to_le_bytes());
        d[36..38].copy_from_slice(&1u16.to_le_bytes());
        d[38..40].copy_from_slice(&(n as u16).to_le_bytes());
        d[44..48].copy_from_slice(b"PTMF");
        for c in 0..n {
            d[0x40 + c] = if c % 2 == 0 { 3 } else { 12 };
        }
        d[0x60] = 0;
        d[0x61] = 255;
        let o = HEADER;
        d[o] = 1 | 4;
        d[o + 13] = 48;
        d[o + 14..o + 16].copy_from_slice(&c4spd.to_le_bytes());
        d[o + 22..o + 26].copy_from_slice(&64u32.to_le_bytes());
        d[o + 26..o + 30].copy_from_slice(&16u32.to_le_bytes());
        d[o + 30..o + 34].copy_from_slice(&64u32.to_le_bytes());
        d[o + 76..o + 80].copy_from_slice(b"PTMS");
        // the pattern at a paragraph
        while d.len() % 16 != 0 {
            d.push(0);
        }
        let seg = (d.len() / 16) as u16;
        d[0x160..0x162].copy_from_slice(&seg.to_le_bytes());
        for row in 0..64 {
            for ch in 0..n {
                for &(r, c, note, ins, fxt, fxp, vol) in cells {
                    if r == row && c == ch {
                        let mut flags = ch as u8;
                        if note != 255 || ins != 0 {
                            flags |= 0x20;
                        }
                        if fxt != 0 || fxp != 0 {
                            flags |= 0x40;
                        }
                        if vol != 255 {
                            flags |= 0x80;
                        }
                        d.push(flags);
                        if flags & 0x20 != 0 {
                            d.push(note);
                            d.push(ins);
                        }
                        if flags & 0x40 != 0 {
                            d.push(fxt);
                            d.push(fxp);
                        }
                        if flags & 0x80 != 0 {
                            d.push(vol);
                        }
                    }
                }
            }
            d.push(0);
        }
        while d.len() % 16 != 0 {
            d.push(0);
        }
        let at = d.len();
        d[o + 18..o + 22].copy_from_slice(&(at as u32).to_le_bytes());
        // the deltas of a ramp
        d.extend((0..64u8).map(|i| if i == 0 { 0 } else { 3 }));
        d
    }

    /// The format and the amplifier's pokes: voice, register and value.
    fn play(d: Vec<u8>, ticks: usize) -> (String, Vec<(u32, u16)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.ptm"), d).unwrap();
        let format = e.playing.as_ref().unwrap().format.clone();
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        (format, log.iter().filter(|w| w.0 & 0xFF_0000 == crate::amplifier::LOG_BASE).map(|w| (w.0 & 0xFFFF, w.1)).collect())
    }

    /// The amplifier's pokes as (register with the voice, value, colour clock).
    fn play_clk(d: Vec<u8>, ticks: usize) -> Vec<(u32, u16, u64)> {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.ptm"), d).unwrap();
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        log.iter().filter(|w| w.0 & 0xFF_0000 == crate::amplifier::LOG_BASE).map(|w| (w.0 & 0xFFFF, w.1, w.2)).collect()
    }

    fn reg(log: &[(u32, u16)], voice: usize, r: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == ((voice as u32) << 4 | r)).map(|w| w.1).collect()
    }

    #[test]
    fn claims_polytracker_modules_only() {
        assert!(detect(&module(4, 8363, &[])));
        let mut d = module(4, 8363, &[]);
        d[44] = b'X';
        assert!(!detect(&d));
        let mut d = module(4, 8363, &[]);
        d[38] = 33;
        assert!(!detect(&d));
        let mut d = module(4, 8363, &[]);
        d[28] = 0;
        assert!(!detect(&d));
        assert!(!detect(&module(4, 8363, &[])[..600]));
        // not a ProTracker module, not an S3M
        let mut s3m = vec![0u8; 2000];
        s3m[44..48].copy_from_slice(b"SCRM");
        assert!(!detect(&s3m));
    }

    #[test]
    fn plays_a_note_through_the_amplifier() {
        // C-4 (the C4 speed's note) on channel 0 and the same an octave up
        let d = module(2, 8363, &[(0, 0, 48, 1, 0, 0, 255), (0, 1, 60, 1, 0, 0, 255)]);
        let (format, log) = play(d, 3);
        assert_eq!(format, "PolyTracker");
        let per0 = reg(&log, 0, 6);
        let per1 = reg(&log, 1, 6);
        // the voice's period at the C4 speed is 428 PT periods, scaled to the colour clock
        let p0 = *per0.last().unwrap() as f64;
        let p1 = *per1.last().unwrap() as f64;
        assert!((p0 / p1 - 0.5).abs() < 0.01 || (p0 / p1 - 2.0).abs() < 0.01, "{p0} {p1}");
        // volume 48, linear
        assert_eq!(*reg(&log, 0, 8).last().unwrap(), 48);
        assert_eq!(*reg(&log, 0, 0xA).last().unwrap(), 0x8000);
    }

    #[test]
    fn sample_speed_shifts_the_pitch() {
        let (_, a) = play(module(1, 8363, &[(0, 0, 48, 1, 0, 0, 255)]), 2);
        let (_, b) = play(module(1, 16726, &[(0, 0, 48, 1, 0, 0, 255)]), 2);
        let (pa, pb) = (*reg(&a, 0, 6).last().unwrap() as f64, *reg(&b, 0, 6).last().unwrap() as f64);
        assert!((pa / pb - 2.0).abs() < 0.02, "{pa} {pb}");
    }

    #[test]
    fn volume_column_and_slide() {
        // volume column 20, then a slide down by 4 each tick after the first (D04)
        let d = module(1, 8363, &[(0, 0, 48, 1, 0x0a, 0x04, 20)]);
        let (_, log) = play(d, 4);
        let v = reg(&log, 0, 8);
        let first = 20u16;
        assert!(v.contains(&first), "{v:?}");
        assert!(v.windows(2).any(|w| w[1] < w[0]), "{v:?}");
    }

    #[test]
    fn speed_and_tempo_effect() {
        // C-4 at row 0 with speed 3, another at row 1: the second starts 3 ticks later
        // (a tick at 125 BPM is 70940 colour clocks)
        let d = module(1, 8363, &[(0, 0, 48, 1, 0x0f, 0x03, 255), (1, 0, 48, 1, 0, 0, 255)]);
        let log = play_clk(d, 8);
        let on: Vec<u64> = log.iter().filter(|w| w.0 == 0xA && w.1 == 0x8000).map(|w| w.2).collect();
        assert_eq!(on.len(), 2, "{on:?}");
        assert_eq!(on[1] - on[0], 3 * 70940);
        // F3C (60 BPM) then makes the tick longer
        let d = module(1, 8363, &[(0, 0, 48, 1, 0x0f, 0x03, 255), (1, 0, 48, 1, 0x0f, 0x3c, 255), (2, 0, 48, 1, 0, 0, 255)]);
        let log = play_clk(d, 12);
        let on: Vec<u64> = log.iter().filter(|w| w.0 == 0xA && w.1 == 0x8000).map(|w| w.2).collect();
        assert_eq!(on.len(), 3, "{on:?}");
        // the tick that sets the tempo ends at the old rate, as a reloaded CIA timer does
        assert_eq!(on[2] - on[1], 70940 + 2 * (1_773_447 / 60 + 1) * 5);
    }

    #[test]
    fn pattern_break_and_the_song_end() {
        // D00 on row 3: the pattern ends after 4 rows of 6 ticks and the song with it
        let d = module(1, 8363, &[(0, 0, 48, 1, 0, 0, 255), (3, 0, 255, 0, 0x0d, 0x00, 255)]);
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.play_data(Path::new("test.ptm"), d).unwrap();
        let mut out = vec![0f32; 2 * 960 * 20];
        e.render(&mut out);
        assert!(!e.songend());
        e.render(&mut out);
        assert!(e.songend());
    }

    #[test]
    fn note_cut_and_slides() {
        // EC2: the volume is 0 from tick 2
        let d = module(1, 8363, &[(0, 0, 48, 1, 0x0e, 0xc2, 255)]);
        let (_, log) = play(d, 4);
        assert_eq!(*reg(&log, 0, 8).last().unwrap(), 0);
        // effect 2 slides up by 4 each tick but the first: the period falls (effect 1 slides down)
        let d = module(1, 8363, &[(0, 0, 48, 1, 0x02, 0x04, 255)]);
        let (_, log) = play(d, 6);
        let per = reg(&log, 0, 6);
        assert!(per.windows(2).any(|w| w[1] < w[0]), "{per:?}");
        // fine portamento down F2 (effect 1): once, on the first tick, so the period does not fall
        let d = module(1, 8363, &[(0, 0, 48, 1, 0x01, 0xf2, 255)]);
        let (_, log) = play(d, 6);
        let per = reg(&log, 0, 6);
        assert!(per.windows(2).all(|w| w[1] >= w[0] || w[0] == 0x71), "{per:?}");
    }

}
