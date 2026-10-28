//! MusicMaker V8 (Thomas Winischhofer), 4-voice songs: a translation of the
//! original 68k replayer, `MusicMaker4.asm`, "EaglePlayer for MusicMaker STD
//! (4-channel) modules/songs, Version 5.0", "MusicMaker V8" Copyright (C)
//! 1987-2017 Thomas Winischhofer, which its author published under the BSD
//! licence: the README beside it in UADE's amigasrc/players/other/music_maker
//! says "Thomas Winischhofer <thomas@winischhofer.net> was kind to publish
//! MusicMaker[48] under BSD license" (the source itself carries no licence text).
//!
//! The 8-voice songs are in `musicmaker8.rs`, which shares the module reading.
//!
//! Checked against the original as UADE runs it (`MusicMaker-4V`, built from
//! that source; DeliTracker's Eagleplayer of it stays silent in deliplayer), and
//! timed as UADE's run takes: every write within a few colour clocks of it.
//!
//! The replayer runs on its own CIA-B timer B, one-shot, re-armed at the start
//! of every interrupt with `speed * 23` E-clocks. A row's DMA off and on need
//! more time than a tick has at its end: the interrupt that turns voices off
//! sets the timer to $170 E-clocks, and the next one turns them on and writes
//! the loops, the timer then set to the rest of the tick. The port does both
//! halves in one tick with that wait between them.
//!
//! Only the single-file form is read (an IFF `FORM` `MMV8` with an `SDAT` and an
//! `INST` or packed `PINS` chunk, as MusicMaker V8 3.0 saves); the older pairs of
//! `.sdata` and `.i`/`.ip` files are not.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "MusicMaker 4V", detect, new };

/// Instruments.
pub(crate) const INSTNUM: usize = 36;
/// Macro vectors the replayer has room for.
const MACROVECS: usize = 999;

pub(crate) const NOTETABLE: [u16; 64] = [
    856, 832, 808, 784, 760, 740, 720, 700, 680, 660, 640, 622, 604, 588, 572, 556, 540, 524, 508, 494, 480, 466, 452,
    440, 428, 416, 404, 392, 380, 370, 360, 350, 340, 330, 320, 311, 302, 294, 286, 278, 270, 262, 254, 247, 240, 233,
    226, 220, 214, 208, 202, 196, 190, 185, 180, 175, 170, 160, 151, 143, 135, 127, 120, 113,
];

pub(crate) const STRTVOLUS: [u16; 16] = [0, 1, 2, 3, 5, 7, 11, 16, 22, 28, 34, 40, 46, 52, 58, 64];

pub(crate) const LOUDNESS: [u16; 64] = [
    256, 264, 268, 273, 277, 287, 292, 297, 303, 309, 315, 321, 327, 334, 341, 348, 356, 364, 372, 372, 381, 390, 399,
    399, 409, 420, 420, 431, 431, 442, 442, 455, 468, 468, 468, 481, 481, 481, 496, 496, 496, 512, 512, 512, 512, 512,
    512, 512, 512, 512, 512, 512, 511, 511, 511, 511, 511, 511, 511, 511, 511, 511, 511, 511,
];

/// A FORM MMV8 module's chunks: (offset of the data, length).
pub(crate) fn find_chunk(d: &[u8], id: &[u8; 4]) -> Option<(usize, usize)> {
    if d.len() < 12 || &d[0..4] != b"FORM" || &d[8..12] != b"MMV8" {
        return None;
    }
    let end = 8usize.saturating_add(be32(d, 4) as usize);
    let mut o = 12usize;
    // as findchunk: while the next id is before the FORM's end
    while o < end && o + 8 <= d.len() {
        let len = be32(d, o + 4) as usize;
        if &d[o..o + 4] == id {
            return Some((o + 8, len));
        }
        o = o.checked_add(8 + len)?;
    }
    None
}

/// The song data (after SDAT's internal length) and whether it is an EXT
/// (8-voice) song, for a module either replayer could play.
pub(crate) fn song(d: &[u8]) -> Option<(usize, bool)> {
    let (sdat, len) = find_chunk(d, b"SDAT")?;
    let s = sdat + 4;
    if len < 4 + 32 || s + 32 > d.len() || &d[s..s + 2] != b"SE" {
        return None;
    }
    if find_chunk(d, b"INST").is_none() && find_chunk(d, b"PINS").is_none() {
        return None;
    }
    Some((s, d[s + 22] == 0xff))
}

fn detect(d: &[u8]) -> bool {
    // Chk: an STD song whose pattern length is 16 to 64 ("highly inofficial")
    matches!(song(d), Some((s, false)) if (16..=64).contains(&d[s + 23]))
}

/// The instrument data in guest memory: the INST chunk in the module, or the
/// PINS chunk unpacked into memory of its own (`_decrunchinstrs`).
pub(crate) fn instruments(d: &[u8], base: u32, m: &mut Machine) -> Option<u32> {
    if let Some((o, _)) = find_chunk(d, b"INST") {
        // a library-disk set (only the names) is refused
        if be32(d, o) == 0xffff_ffff || be32(d, o + 4) == 0xffff_ffff {
            return None;
        }
        return Some(base + o as u32);
    }
    let (o, _) = find_chunk(d, b"PINS")?;
    let p = d.get(o..)?;
    let out = decrunch(p)?;
    let a = m.alloc(out.len() as u32);
    if a == 0 {
        return None;
    }
    m.bus.write_bytes(a, &out);
    Some(a)
}

/// `_getunpackedinstlen` and `_decrunchinstrs`: the instrument headers copied,
/// the samples delta-decoded from `fibbits`-bit codes, the LFO data copied.
fn decrunch(p: &[u8]) -> Option<Vec<u8>> {
    let mut n = 26usize;
    let mut src = 0usize;
    let mut out = Vec::new();
    if p.get(0..6)? == b"SEI1XX" {
        n = be16(p, 6) as usize;
        out.extend_from_slice(p.get(0..8)?);
        src = 8;
    }
    let heads = src;
    out.extend_from_slice(p.get(src..src + 8 * n)?);
    src += 8 * n;
    out.extend([0; 4]);
    let fibbits = be16(p, src) as u32;
    src += 2;
    if fibbits == 0 || fibbits > 8 {
        return None;
    }
    let fibtable = p.get(src..src + (1 << fibbits))?.to_vec();
    src += 1 << fibbits;
    for i in 0..n {
        let len = be16(p, heads + 8 * i) as usize;
        if len == 0 {
            continue;
        }
        // decompress: fibbits bits a code, the codes' table values summed
        let (mut bits, mut have, mut acc) = (0u8, 0i32, 0u8);
        for _ in 0..len {
            let mut code = 0usize;
            for _ in 0..fibbits {
                have -= 1;
                if have < 0 {
                    bits = *p.get(src)?;
                    src += 1;
                    have = 7;
                }
                code = code << 1 | (bits >> 7) as usize;
                bits <<= 1;
            }
            acc = acc.wrapping_add(fibtable[code]);
            out.push(acc);
        }
        // the next sample starts at the next byte
        let _ = have;
    }
    if out.len() & 1 != 0 {
        out.push(0);
    }
    let lens = p.get(src..src + 30)?.to_vec();
    out.extend_from_slice(&lens);
    src += 30;
    for k in 0..15 {
        let l = be16(&lens, 2 * k) as usize;
        out.extend_from_slice(p.get(src..src + l)?);
        src += l;
    }
    Some(out)
}

/// The instruments' tables, as `ptrsinit` sets them up from the instrument data.
pub(crate) struct Instruments {
    pub vecs: [u32; INSTNUM],
    /// Length, and three more words (their meanings differ in 4V and 8V).
    pub lens: [[u16; 4]; INSTNUM],
    /// The 4 bytes before the first sample.
    pub defsnd: u32,
    /// LFO (hull) tables 1-15.
    pub lfo: [u32; 16],
}

pub(crate) fn read_instruments(bus: &crate::bus::Bus, a: u32, clear_rest: bool) -> Option<Instruments> {
    let mut a1 = a;
    let mut n = 26usize;
    if bus.r32(a1) == u32::from_be_bytes(*b"SEI1") && bus.r16(a1 + 4) == u16::from_be_bytes(*b"XX") {
        n = bus.r16(a1 + 6) as usize;
        a1 += 8;
    }
    if n > INSTNUM {
        return None;
    }
    let defsnd = a1 + 8 * n as u32;
    let mut s = defsnd + 4;
    let mut ins = Instruments { vecs: [0; INSTNUM], lens: [[0; 4]; INSTNUM], defsnd, lfo: [0; 16] };
    for i in 0..n {
        ins.vecs[i] = s;
        let h = a1 + 8 * i as u32;
        ins.lens[i] = [bus.r16(h), bus.r16(h + 2), bus.r16(h + 4), bus.r16(h + 6)];
        s = s.wrapping_add(ins.lens[i][0] as u32);
    }
    // (the 8-voice ptrsinit clears the lengths of the instruments past n; the
    // 4-voice one leaves them, as they start cleared)
    let _ = clear_rest;
    let lfolens = (s + 1) & !1;
    let mut p = lfolens + 30;
    for k in 0..15 {
        ins.lfo[k + 1] = p;
        p = p.wrapping_add(bus.r16(lfolens + 2 * k as u32) as u32);
    }
    Some(ins)
}

/// A voice's `lastperiods` entry.
#[derive(Default, Clone, Copy)]
pub(crate) struct Last {
    pub per: u16,
    /// The note's index times 2.
    pub note2: u16,
    /// Fade divisor (256: none).
    pub fade: u16,
    /// The hull's (LFO's) table, its position and length in sample bytes.
    pub lfo: u32,
    pub hullcnt: u16,
    pub hulllen: u16,
    pub hull_on: bool,
    pub hull_loop: bool,
    pub looplen: u16,
}

impl Last {
    pub fn reset() -> Self {
        Last { fade: 0x100, ..Default::default() }
    }
}

/// The song: melodies (one per voice) of macro numbers, the macros.
pub(crate) struct Song {
    /// Each voice's melody start and loop position (guest addresses).
    pub melody: Vec<(u32, u32)>,
    pub macrovecs: Vec<u32>,
}

/// `ptrsinit`'s melody and macro search for `voices` voices (enabled in
/// `enabled`), the melodies at `a0`.
pub(crate) fn read_song(bus: &crate::bus::Bus, start: u32, mut a0: u32, voices: usize, enabled: u8) -> Song {
    let d1 = bus.r16(start) as u32;
    let d2 = bus.r16(start + 2) as u32;
    let search = |mut a: u32| {
        // searchmelodystrt (bounded: the original runs on to a 999)
        for _ in 0..0x10000 {
            let w = bus.r16(a);
            a = a.wrapping_add(2);
            if w == 999 {
                break;
            }
        }
        a
    };
    let mut melody = Vec::new();
    for v in 0..voices {
        if enabled & 1 << v != 0 {
            melody.push((a0.wrapping_add(2 * d1), a0.wrapping_add(2 * d2)));
            a0 = search(a0);
        } else {
            melody.push((0, 0));
        }
    }
    // the macros: 3-byte entries ended by $ff, the list by $fe; a lone $ff is an
    // empty macro, which gets the list's start
    let first = a0;
    let mut a1 = a0;
    let mut macrovecs = Vec::with_capacity(MACROVECS);
    while macrovecs.len() < MACROVECS - 1 {
        match bus.r8(a1) {
            0xff => {
                macrovecs.push(first);
                a1 = a1.wrapping_add(1);
            }
            0xfe => break,
            _ => {
                macrovecs.push(a1);
                let mut n = 0;
                loop {
                    a1 = a1.wrapping_add(3);
                    n += 1;
                    if bus.r8(a1) == 0xff || n > 0x10000 {
                        break;
                    }
                }
                a1 = a1.wrapping_add(1);
            }
        }
    }
    while macrovecs.len() < MACROVECS - 1 {
        macrovecs.push(first);
    }
    Song { melody, macrovecs }
}

pub(crate) fn song_name(bus: &crate::bus::Bus, sdata: u32) -> String {
    let mut s = String::new();
    for i in 0..20 {
        let c = bus.r8(sdata + 2 + i);
        if c == 0 {
            break;
        }
        s.push(c as char);
    }
    s.trim().to_string()
}

/// What `handle_channel` hands back: d4 (length, or -1: nothing new, with the
/// instrument and note in the high word), d5 (sample, -1: volume and period
/// only) and d6 (the note's period in the high word, its volume in the low).
pub(crate) struct Row {
    pub d4: u32,
    pub d5: u32,
    pub d6: u32,
}

// Colour clocks between the replayer's writes, fitted to UADE's runs of the
// original (the reference: its CPU is a 68020, so not a 68000's times).
/// From an interrupt to the timer's restart in it (the timer counts from there).
const LATENCY: u64 = 171;
/// The second interrupt's restart of the timer, and the $170's extra E-clock.
const WHATTODO_LATENCY: u64 = 275;
/// From an interrupt to the first voice's row.
const T_START: u64 = 413;
/// From a voice's row to its length and location, volume and period, when a
/// note starts.
const T_NOTE_LEN: u64 = 1294;
const T_NOTE_VOL: u64 = 1646;
const T_NOTE_PER: u64 = 1750;
/// The same without a note.
const T_VOL: u64 = 131;
const T_PER: u64 = 235;
/// From a voice's period to the next voice's row.
const T_NEXT: u64 = 250;
/// From the last period to the DMA on, without a second interrupt; from the
/// last period to the second interrupt's DMA on.
const T_DMAON: u64 = 92;
const T_SECOND: u64 = 2052 + 212;
/// Between two voices' loops.
const T_LOOP: u64 = 118;

/// Voice state.
#[derive(Default, Clone, Copy)]
struct Voice {
    /// COUNTERS: the melody position; the loop position; MACRO_C: the macro entry.
    melody: u32,
    melody_loop: u32,
    macro_ptr: u32,
    /// SCOUNT: ticks left of the entry.
    count: u16,
    /// SOFTMOD: period and volume deltas.
    softmod: [u16; 2],
    /// VIBRATO bytes 0 (>0 tremolo, <0 vibrato) and 1 (the toggle).
    vibrato: [u8; 2],
    /// nextdata: the loop played after the note's sample.
    next: (u32, u16),
    last: Last,
}

pub struct MusicMaker4 {
    name: String,
    ins: Instruments,
    macrovecs: Vec<u32>,
    currlfo: [u32; INSTNUM + 1],
    v: [Voice; 4],
    /// lastvolumes: the voices' volumes times 2, then their base volumes.
    lastvol: [u16; 8],
    volumetable: [u16; 16],
    speed: u16,
    calcspeed: u16,
    playspeed: u16,
    pattlen: u16,
    dmaon: u16,
    dmaoff: u16,
    channelshandle: u8,
    internfinished: u8,
    dtfinished: u8,
    doloudness: u8,
    dofadein: u8,
    dofadeout: u8,
    fadespeedin: u16,
    fadespeedout: u16,
    /// Colour clocks to the next interrupt, as the engine asks before a tick.
    next_period: u64,
    /// When a speed change restarted the timer within this tick.
    fc_at: Option<u64>,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let (s, ext) = song(d)?;
    if ext {
        return None;
    }
    let ia = instruments(d, base, m)?;
    let bus = &m.bus;
    let ins = read_instruments(bus, ia, false)?;
    let sdata = base + s as u32;
    // _newsndresetoneshot
    let mut a2 = sdata + 22;
    let w = bus.r16(a2);
    a2 += 2;
    let pattlen = w & 0xff;
    let extdatasize = if w >> 8 == 0xfe { bus.r16(a2 + 6) as u32 } else { 0 };
    let speed = bus.r16(a2);
    a2 += 2;
    // ptrsinit: the melodies after the start and loop words (and the extra data)
    let song = read_song(bus, a2, a2 + 4 + extdatasize, 4, 0xf);
    let mut v: [Voice; 4] = Default::default();
    for (i, vo) in v.iter_mut().enumerate() {
        let (start, lp) = song.melody[i];
        vo.melody = start;
        vo.melody_loop = lp;
        vo.macro_ptr = song.macrovecs.get(bus.r16(start) as usize).copied().unwrap_or(0);
        vo.count = 1;
        vo.last = Last::reset();
    }
    let mut p = MusicMaker4 {
        name: song_name(bus, sdata),
        ins,
        macrovecs: song.macrovecs,
        currlfo: [0; INSTNUM + 1],
        v,
        lastvol: [0; 8],
        volumetable: STRTVOLUS,
        speed,
        calcspeed: speed,
        playspeed: speed.wrapping_mul(23),
        pattlen,
        dmaon: 0x8000,
        dmaoff: 0,
        channelshandle: 0,
        internfinished: 0,
        dtfinished: 0,
        doloudness: 0,
        dofadein: 0,
        dofadeout: 0,
        fadespeedin: 63,
        fadespeedout: 21,
        next_period: 0,
        fc_at: None,
    };
    p.next_period = p.period();
    // InitPlay's and StartSnd's resets: hardwareinit five times
    let mut hw = Hw { bus: &mut m.bus };
    for _ in 0..5 {
        hardwareinit(&mut hw);
    }
    Some(Box::new(p))
}

pub(crate) fn hardwareinit(hw: &mut Hw) {
    hw.dmacon(0x000f);
    for ch in 0..4 {
        hw.vol(ch, 0);
    }
}

/// `getalegalvolume`: 0 to `max`.
pub(crate) fn legal_vol(v: u16, max: u16) -> u16 {
    if (v as i16) < 0 {
        0
    } else if v as i16 > max as i16 {
        max
    } else {
        v
    }
}

/// `getalegalperiod`: 113 to 856.
pub(crate) fn legal_per(p: u16) -> u16 {
    (p as i16).clamp(113, 856) as u16
}

/// Waits as `waitatimee` does: for the beam's next line, then for bit 4 of its
/// horizontal position.
pub(crate) fn wait_line(hw: &mut Hw) {
    let now = hw.bus.clock;
    let line = (now / LINE + 1) * LINE;
    hw.wait(line + 16 - now);
}

impl Replayer for MusicMaker4 {
    fn format(&self) -> &'static str {
        "MusicMaker 4V"
    }

    fn title(&self) -> String {
        self.name.clone()
    }

    fn tick_clocks(&self) -> u64 {
        self.next_period
    }

    fn songend(&self) -> bool {
        self.dtfinished == 0x0f
    }

    fn tick(&mut self, hw: &mut Hw) {
        let start = hw.bus.clock;
        let scheduled = self.next_period;
        // contsound
        self.dmaoff &= 0xff00;
        self.dmaon &= 0xff00;
        self.channelshandle = 0;
        hw.wait(T_START);
        for n in (0..4).rev() {
            if n != 3 {
                hw.wait(T_NEXT);
            }
            self.channel(n, hw);
        }
        let whattodo = self.channelshandle != 0 && self.dmaon & self.dmaoff != 0;
        // whattodo: the timer set to $170, the rest of the tick after the next
        // interrupt
        hw.wait(if whattodo { T_SECOND } else { T_DMAON });
        // donthavetowait
        hw.dmacon(self.dmaon);
        if self.channelshandle != 0 {
            wait_line(hw);
            let mut first = true;
            for n in (0..4).rev() {
                if self.channelshandle & 1 << n == 0 {
                    continue;
                }
                if !first {
                    hw.wait(T_LOOP);
                }
                first = false;
                let v = &mut self.v[n];
                hw.loc(n, v.next.0);
                hw.len(n, v.next.1);
                if v.last.hull_on && v.next.0 != self.ins.defsnd {
                    v.last.hull_loop = true;
                    v.last.looplen = v.next.1 << 1;
                }
            }
        }
        // the engine asks for the period before a tick: a speed change restarted
        // the timer within this one, so the next period makes up for the time
        // this one is off by
        let mut next = self.period() as i64;
        if whattodo {
            next += WHATTODO_LATENCY as i64;
        }
        if let Some(t) = self.fc_at.take() {
            let truth = (t - start + (self.playspeed as u64 + 1) * 5) as i64;
            next += truth - scheduled as i64;
        }
        self.next_period = next.max(1000) as u64;
    }
}

impl MusicMaker4 {
    fn period(&self) -> u64 {
        (self.playspeed as u64 + 1) * 5 + LATENCY
    }

    /// One voice of `setallchanneldata`.
    fn channel(&mut self, n: usize, hw: &mut Hw) {
        let bit = 1u8 << n;
        let s = hw.bus.clock;
        let r = self.handle_channel(n, hw);
        let (mut d4, d5, mut d6) = (r.d4, r.d5, r.d6);
        let mut d3: u8 = 0;
        let note = (d4 as i32) >= 0 && (d5 as i32) >= 0;
        let wait_to = |hw: &mut Hw, t: u64| {
            let now = hw.bus.clock;
            hw.wait((s + t).saturating_sub(now));
        };
        if (d4 as i32) >= 0 {
            if (d5 as i32) >= 0 {
                self.channelshandle |= bit;
                wait_to(hw, T_NOTE_LEN);
                hw.len(n, d4 as u16);
                hw.loc(n, d5);
                let inst = (d4 >> 24) as usize;
                let lfo = self.currlfo.get(inst).copied().unwrap_or(0);
                let last = &mut self.v[n].last;
                last.hull_on = false;
                last.lfo = lfo;
                if lfo != 0 {
                    last.hull_on = true;
                    last.hullcnt = 0;
                    last.hulllen = (d4 as u16) << 1;
                }
            }
            // pervolonly
            let w = (d6 as u16).rotate_left(8);
            self.lastvol[n + 4] = (self.lastvol[n + 4] & 0xff00) | (w & 0xff);
            self.lastvol[n] = (w >> 8) * 2;
            self.v[n].last.per = (d6 >> 16) as u16;
            self.v[n].last.note2 = (d4 >> 16) as u16 & 0xff;
            d4 = 0xffff_ffff;
        }
        // absnothing
        let last = &mut self.v[n].last;
        if (d4 as i16) >= 0 {
            last.fade = d4 as u16;
        }
        if self.dofadein & bit != 0 {
            if last.fade > 4096 {
                last.fade = 4096;
            }
            if last.fade != 256 {
                if (last.fade as i16) < 256 {
                    last.fade = 256;
                } else {
                    last.fade = last.fade.wrapping_sub(self.fadespeedin);
                }
            }
        } else if self.dofadeout & bit != 0 && last.fade != 8192 {
            if last.fade as i16 > 8192 {
                last.fade = 8192;
            } else {
                last.fade = last.fade.wrapping_add(self.fadespeedout);
            }
        }
        // nomorefade: the volume
        let d7 = self.volumetable[15];
        let v = &mut self.v[n];
        if v.vibrato[0] as i8 > 0 {
            v.vibrato[1] = !v.vibrato[1];
            if v.vibrato[1] == 0 {
                v.softmod[1] = v.softmod[1].wrapping_neg();
            }
        }
        d6 = (self.lastvol[n + 4].wrapping_add(((v.softmod[1] as i16) >> 1) as u16)) as u32;
        self.lastvol[n + 4] = legal_vol(d6 as u16, 64);
        let mut vol = legal_vol(self.lastvol[n].wrapping_add(v.softmod[1]), d7.wrapping_mul(2));
        self.lastvol[n] = vol;
        vol >>= 1;
        let last = &mut v.last;
        if last.hull_on {
            let a2 = last.lfo.wrapping_add(((last.hullcnt / 40) as u32) * 2);
            d3 = hw.bus.r8(a2 + 1);
            let m = hw.bus.r8(a2) as i8;
            if m != 0 {
                let x = (m as i16 as i32 * vol as i16 as i32) as i16 >> 5;
                vol = legal_vol(vol.wrapping_add(x as u16), d7);
            }
        }
        if self.doloudness & 1 << n != 0 {
            let l = LOUDNESS.get(last.note2 as usize / 2).copied().unwrap_or(256);
            let x = ((vol as u32) << 8 & 0xffff) / l as u32;
            vol = if x as u16 == (vol << 8) { 0 } else { x as u16 };
        }
        if last.fade != 0x100 {
            let num = (vol << 8) as u32;
            let q = num / last.fade.max(1) as u32;
            vol = if q > 0xffff || q as u16 == (vol << 8) { 0 } else { q as u16 };
        }
        vol = legal_vol(vol, d7);
        wait_to(hw, if note { T_NOTE_VOL } else { T_VOL });
        hw.vol(n, vol);
        // the period
        if v.vibrato[0] as i8 >= 0 {
        } else {
            v.vibrato[1] = !v.vibrato[1];
            if v.vibrato[1] == 0 {
                v.softmod[0] = v.softmod[0].wrapping_neg();
            }
        }
        let last = &mut v.last;
        let mut per = legal_per(last.per.wrapping_add(v.softmod[0]));
        last.per = per;
        if last.hull_on {
            if d3 != 0 {
                let x = (per as i16 as i32 * d3 as i8 as i32) >> 8;
                per = legal_per(per.wrapping_add(x as u16));
            }
            let step = (self.calcspeed as u32 * 115) / per as u32;
            let (c, carry) = last.hullcnt.overflowing_add(step as u16);
            last.hullcnt = c;
            if carry || c > last.hulllen {
                if last.hull_loop {
                    last.hullcnt = last.hullcnt.wrapping_sub(last.looplen);
                } else {
                    last.hull_on = false;
                }
            }
        }
        wait_to(hw, if note { T_NOTE_PER } else { T_PER });
        hw.per(n, per);
    }

    fn handle_channel(&mut self, n: usize, hw: &mut Hw) -> Row {
        let bit = 1u8 << n;
        let nothing = Row { d4: 0xffff_ffff, d5: 0, d6: 0 };
        self.v[n].count = self.v[n].count.wrapping_sub(1);
        if self.v[n].count != 0 {
            return nothing;
        }
        if self.internfinished & bit != 0 {
            // the melody went back to its loop: the voice is silenced first
            self.dofadein &= !bit;
            self.dofadeout &= !bit;
            hw.vol(n, 0);
            self.lastvol[n] = 0;
            self.lastvol[n + 4] = 0;
            hw.dmacon(bit as u16);
            self.v[n].last = Last::reset();
            self.dtfinished |= bit;
            self.internfinished &= !bit;
        }
        let bus = &*hw.bus;
        let mut a0 = self.v[n].macro_ptr;
        let r8 = |a: u32| bus.r8(a);
        let mut d0 = r8(a0 + 2);
        if r8(a0 + 3) == 0xf2 || r8(a0 + 3) == 0xfb || r8(a0) == 0xf8 {
            d0 = r8(a0 + 5);
        }
        self.v[n].count = (((d0 & 0x3f) as u16) + 1) * 2;
        let mut d5: u32 = 0;
        let mut b0 = r8(a0);
        let mut b1 = r8(a0 + 1);
        let mut d4: u32;
        let mut d6: u32 = 0;
        let mut row = None;
        if b0 > 0xf1 {
            match b0 {
                0xf4 => {
                    self.doloudness &= !bit;
                    if b1 != 0 {
                        self.doloudness |= bit;
                    }
                    row = Some(0xffff_ffff);
                }
                0xfd => row = Some(0xffff_ffff),
                0xf5 => row = Some(0xffff_ffff),
                0xf6 => {
                    self.dofadein &= !bit;
                    self.dofadeout &= !bit;
                    if b1 != 0 {
                        if (b1 as i8) < 0 {
                            self.fadespeedin = (self.fadespeedin & 0xff00) | b1.wrapping_neg() as u16;
                            self.dofadein |= bit;
                        } else {
                            self.fadespeedout = (self.fadespeedout & 0xff00) | b1 as u16;
                            self.dofadeout |= bit;
                        }
                    }
                    row = Some(0xffff_ffff);
                }
                0xf7 => row = Some(if b1 != 0 { 0xffff_0100 } else { 0xffff_2000 }),
                _ => {}
            }
            if row.is_none() {
                if b0 == 0xf8 {
                    d5 = b1 as u32;
                    a0 += 3;
                    b0 = r8(a0);
                    b1 = r8(a0 + 1);
                }
                if b0 == 0xf9 {
                    let d1 = b1 as i8 as i16 as u16;
                    let b2 = r8(a0 + 2);
                    let v = &mut self.v[n];
                    if b2 & 0x80 != 0 {
                        if b2 & 0x40 != 0 {
                            if (d1 as i16) < 0 {
                                if (v.vibrato[0] as i8) < 0 {
                                    v.softmod[0] = 0;
                                }
                                v.softmod[1] = d1.wrapping_neg();
                                v.vibrato = [1, 0];
                            } else {
                                if v.vibrato[0] as i8 > 0 {
                                    v.softmod[1] = 0;
                                }
                                v.softmod[0] = d1;
                                v.vibrato = [0xff, 0];
                            }
                        } else {
                            v.softmod[1] = d1;
                            if v.vibrato[0] as i8 > 0 {
                                v.vibrato[0] = 0;
                            }
                        }
                    } else {
                        v.softmod[0] = d1;
                        if (v.vibrato[0] as i8) < 0 {
                            v.vibrato[0] = 0;
                        }
                    }
                    row = Some(0xffff_ffff);
                } else if b0 == 0xfa {
                    let inst = ((b1 >> 4).wrapping_add(d5 as u8)) as u16;
                    let k = (b1 & 0x0f) as usize;
                    let mut p = self.ins.lfo[k];
                    if p != 0 {
                        if bus.r16(p) == inst {
                            p += 2;
                        } else {
                            p = 0;
                        }
                    }
                    if let Some(c) = self.currlfo.get_mut(inst as usize) {
                        *c = p;
                    }
                    row = Some(0xffff_ffff);
                } else if b0 == 0xfc {
                    let f1 = (b1 >> 4) as u32;
                    let f2 = (b1 & 0x0f) as u32;
                    if f2 != 0 {
                        let mut s = ((f1 * self.speed as u32) / f2) as u16;
                        s = (s as i16).clamp(300, 2800) as u16;
                        self.calcspeed = s;
                        self.playspeed = s.wrapping_mul(23);
                        // settimer: the timer restarts here
                        self.fc_at = Some(hw.bus.clock);
                    }
                    row = Some(0xffff_ffff);
                }
            }
        }
        let d4v = if let Some(d4v) = row {
            d4v
        } else {
            // nocontrolbyte: a note
            let b2 = r8(a0 + 2);
            let v = &mut self.v[n];
            v.softmod = [0, 0];
            v.vibrato = [0, 0];
            if b2 & 0x40 != 0 {
                let led = (b1 & 0x40) >> 5;
                use m68k::AddressBus;
                let pra = hw.bus.read_byte(0xbfe001);
                hw.bus.write_byte(0xbfe001, pra & 0xfd);
                let pra = hw.bus.read_byte(0xbfe001);
                hw.bus.write_byte(0xbfe001, pra | led);
            }
            let bus = &*hw.bus;
            let r8 = |a: u32| bus.r8(a);
            let d1 = ((b1 & 0x3f) as u32) * 2;
            d4 = d1 << 16;
            d6 = (NOTETABLE[d1 as usize / 2] as u32) << 16;
            let vi = (b0 & 0x0f) as usize;
            let novolume = |p: &mut Self, hw: &mut Hw, d6: &mut u32| {
                *d6 &= 0xffff_0000;
                hw.dmacon(bit as u16);
                p.dmaoff |= bit as u16;
                p.v[n].next = (p.ins.defsnd, 1);
                ((INSTNUM as u32) << 24 | 1, p.ins.defsnd)
            };
            if b0 == 0xf1 || vi == 0 {
                let (x4, x5) = novolume(self, hw, &mut d6);
                d4 = x4;
                d5 = x5;
            } else {
                d6 |= (STRTVOLUS[vi] << 8) as u32 | (self.volumetable[vi] & 0xff) as u32;
                if b2 & 0x80 != 0 {
                    // modulate: no new sample, a soft modulation from the next entry
                    d5 = 0xffff_ffff;
                    let next = r8(a0 + 3);
                    if next == 0xf2 || next == 0xfb {
                        let sh = if next == 0xfb { 1 } else { 0 };
                        let x = r8(a0 + 4);
                        let hi = ((x as i8) >> 4) as i16;
                        let lo = (((x << 4) as i8) >> 4) as i16;
                        let v = &mut self.v[n];
                        v.softmod[0] = (lo << sh) as u16;
                        v.softmod[1] = hi as u16;
                        a0 += 3;
                    }
                } else {
                    let inst = (b0 >> 4).wrapping_add(d5 as u8) as usize;
                    d4 = (d4 & 0x00ff_ffff) | (inst as u32) << 24;
                    let (vec, lens) = match (self.ins.vecs.get(inst), self.ins.lens.get(inst)) {
                        (Some(&a), Some(&l)) => (a, l),
                        _ => (0, [0; 4]),
                    };
                    d5 = vec;
                    hw.dmacon(bit as u16);
                    self.dmaoff |= bit as u16;
                    self.dmaon |= bit as u16;
                    if lens[0] == 0 {
                        let (x4, x5) = novolume(self, hw, &mut d6);
                        d4 = x4;
                        d5 = x5;
                    } else {
                        let lw = if b1 & 0x80 != 0 && lens[1] != 0 {
                            self.v[n].next = (d5.wrapping_add(lens[2] as u32), lens[3]);
                            lens[1]
                        } else {
                            self.v[n].next = (self.ins.defsnd, 1);
                            lens[0]
                        };
                        d4 = (d4 & 0xffff_0000) | (lw >> 1) as u32;
                    }
                }
            }
            d4
        };
        // addcounthandle
        let bus = &*hw.bus;
        let mut guard = 0;
        loop {
            a0 = a0.wrapping_add(3);
            if bus.r8(a0) == 0xf3 {
                a0 = a0.wrapping_add(6);
            }
            if bus.r8(a0) != 0xff {
                break;
            }
            let (na0, d1) = self.getnewmacro(n, bus);
            a0 = na0;
            guard += 1;
            if d1 == 0 || bus.r8(a0) != 0xf0 || guard > 1000 {
                break;
            }
            let c = ((bus.r8(a0 + 2) & 0x3f) as u16 + 1) * 2;
            self.v[n].count = self.v[n].count.wrapping_add(c);
        }
        self.v[n].macro_ptr = a0;
        Row { d4: d4v, d5, d6 }
    }

    /// The next macro of voice `n`'s melody (pauses add to its count), and
    /// whether the melody went on (0: it went back to its loop).
    fn getnewmacro(&mut self, n: usize, bus: &crate::bus::Bus) -> (u32, u16) {
        let bit = 1u8 << n;
        let mut d1 = 1u16;
        let v = &mut self.v[n];
        for _ in 0..0x10000 {
            v.melody = v.melody.wrapping_add(2);
            if bus.r16(v.melody) != 0 {
                break;
            }
            v.count = v.count.wrapping_add(self.pattlen.wrapping_mul(2));
        }
        if bus.r16(v.melody) == 999 {
            v.melody = v.melody_loop;
            d1 = 0;
            self.internfinished |= bit;
        }
        let w = bus.r16(v.melody) as usize;
        (self.macrovecs.get(w).copied().unwrap_or(0), d1)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// A chunk (the replayer does not skip pad bytes: MusicMaker's are even).
    fn chunk(id: &[u8; 4], data: Vec<u8>) -> Vec<u8> {
        assert!(data.len() & 1 == 0);
        let mut c = id.to_vec();
        c.extend((data.len() as u32).to_be_bytes());
        c.extend(data);
        c
    }

    /// A FORM MMV8 module: `head` (newformat, pattlen), `extra` song bytes
    /// before the speed, each voice's melody playing macro 0 once, macro 0 one
    /// note (instrument 0 at volume 15, note 12), one 64-byte instrument.
    pub(crate) fn module(head: [u8; 2], voices: usize, extra: &[u8]) -> Vec<u8> {
        let mut sd = b"SE".to_vec();
        sd.extend(b"synthetic");
        sd.extend([0; 11]);
        sd.extend(head);
        sd.extend(extra);
        sd.extend(600u16.to_be_bytes()); // speed
        sd.extend([0, 0, 0, 0]); // start and loop positions
        for _ in 0..voices {
            sd.extend([0, 0, 0x03, 0xe7]); // macro 0, 999
        }
        sd.extend([0x0f, 0x0c, 0x03, 0xff, 0xfe]);
        if sd.len() & 1 != 0 {
            sd.push(0);
        }
        let mut sdat = (sd.len() as u32).to_be_bytes().to_vec();
        sdat.extend(sd);
        let mut inst = b"SEI1XX".to_vec();
        inst.extend(1u16.to_be_bytes());
        inst.extend([0, 64, 0, 0, 0, 0, 0, 0]); // 64 bytes, no loop
        inst.extend([0; 4]); // defsnd
        inst.extend((0..64).map(|i| (i * 4) as u8));
        inst.extend([0; 30]); // no LFOs
        let mut body = b"MMV8".to_vec();
        body.extend(chunk(b"SDAT", sdat));
        body.extend(chunk(b"INST", inst));
        let mut d = b"FORM".to_vec();
        d.extend((body.len() as u32).to_be_bytes());
        d.extend(body);
        d
    }

    #[test]
    fn plays_a_note_on_paula() {
        let d = module([0, 16], 4, &[]);
        assert!(super::detect(&d));
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("mm4.test"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "MusicMaker 4V");
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // the resets, then each voice's DMA off (voice 3 first) and all on
        assert_eq!(&reg(0xDFF096)[..10], &[15, 15, 15, 15, 15, 8, 4, 2, 1, 0x800f]);
        // voice 0: the note's 32 words, then the 4 bytes before the samples as its loop
        assert_eq!(&reg(0xDFF0A4)[..2], &[32, 1]);
        assert_eq!(reg(0xDFF0A6)[0], super::NOTETABLE[12]);
        assert_eq!(reg(0xDFF0A8)[5], 64);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}
