//! Richard Joseph Player (`rjp.*` with a sample companion `smp.*`), the music of
//! James Pond 2, The Chaos Engine, Cannon Fodder, Brutal Football and other Amiga
//! games by Richard Joseph.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour, and this player was written from it alone and checked against the
//! Paula register writes of UADE's Richard Joseph player.
//!
//! A song file (`RJP?SMOD`) is seven length-prefixed blocks: the instrument records,
//! the envelopes, the subsong table, and the offset tables of the position lists and
//! of the patterns, then those lists and patterns themselves. A subsong gives each of
//! the four voices a position list; a voice walks its list of pattern numbers and
//! plays the notes and commands in them. A note starts a sample of the companion
//! file, with an envelope, an optional vibrato (pitch) table and tremolo (volume)
//! table, and a pitch slide; all sound is Paula DMA, started in three steps over
//! three ticks (stop, enable, install the loop). The tick is 50 Hz.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Richard Joseph Player", detect, new };

/// Paula periods of the notes (the note byte is twice the index).
const PERIODS: [u16; 36] = [
    453, 480, 508, 538, 570, 604, 640, 678, 720, 762, 808, 856, 226, 240, 254, 269, 285, 302, 320, 339, 360, 381, 404,
    428, 113, 120, 127, 135, 143, 151, 160, 170, 180, 190, 202, 214,
];

/// Row parsing and list walks that find nothing to play stop the voice after this
/// many steps rather than hang.
const GUARD: u32 = 20000;

/// Recognition by content: `RJP`, any version byte, `SMOD`, and a zero long where
/// the first instrument record (the empty instrument) starts.
fn detect(d: &[u8]) -> bool {
    d.len() >= 16 && d[..3] == *b"RJP" && d[4..8] == *b"SMOD" && be32(d, 12) == 0
}

/// The seven blocks of a song as (offset of the data, length), if they all fit.
fn blocks(d: &[u8]) -> Option<[(usize, usize); 7]> {
    let mut p = 8usize;
    let mut b = [(0, 0); 7];
    for e in b.iter_mut() {
        let len = d.get(p..p + 4).map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]))? as usize;
        *e = (p + 4, len);
        p = p.checked_add(4)?.checked_add(len)?;
    }
    (p <= d.len()).then_some(b)
}

#[derive(Clone, Copy, Default)]
struct Voice {
    /// The pattern read pointer, if the voice has one.
    pat: Option<usize>,
    /// The position list read pointer.
    lst: usize,
    speed: u8,
    repeat: u8,
    cnt: u8,
    rep: u8,
    active: bool,
    /// The current instrument number and the record it selected, if any.
    ins: u16,
    rec: Option<usize>,
    sample: u32,
    loop_start: u16,
    loop_len: u16,
    vib: u32,
    trem: u32,
    vib_restart: u16,
    vib_len: u16,
    trem_restart: u16,
    trem_len: u16,
    ivol: u16,
    base_per: u16,
    per: u16,
    ppos: u16,
    tpos: u16,
    work: i16,
    // the envelope: phase 0 none, 4 attack, 2 decay, 255 release
    phase: u8,
    end: u8,
    delta: i8,
    c: u8,
    d: u8,
    level: u8,
    env: Option<usize>,
    acc: u32,
    rate: u32,
    slide: u8,
    keep: bool,
    dma: bool,
    install: bool,
}

pub struct Rjp {
    d: Vec<u8>,
    blk: [(usize, usize); 7],
    title: String,
    subsongs: u16,
    sub: u16,
    /// The sample area as loaded, and the one being played (the player clears words
    /// in it), mirrored in guest memory at `base`.
    pristine: Vec<u8>,
    area: Vec<u8>,
    base: u32,
    v: [Voice; 4],
    /// Voices that have not yet read their list's end since the last song end.
    flags: u8,
    flags0: u8,
    songend: bool,
    /// The next tick is the first after a change of subsong.
    first: bool,
    /// When that tick is due (colour clocks, absolute): at once.
    tick_at: Option<u64>,
    /// The tick after the start-up tick: when it falls (absolute), and the timer's grid point it is.
    next_at: std::cell::Cell<Option<u64>>,
    grid: u64,
    /// A tick has run since the song was loaded (the host's timer is running).
    ticked: bool,
    /// The tick's start (colour clocks, absolute) and the original's time into it so far, in
    /// half colour clocks (68000 cycles): each write falls at `t0 + h / 2`.
    t0: u64,
    h: u64,
}

/// The name the sample companion has: `smp.NAME` for `rjp.NAME` (the prefix in any
/// case), `NAME.ins` for `NAME.sng`.
fn companion(file: &str) -> Vec<String> {
    let file = file.rsplit(['/', ':']).next().unwrap_or(file);
    let mut v = Vec::new();
    match file.get(..4) {
        Some(p) if p.eq_ignore_ascii_case("rjp.") => v.push(format!("smp.{}", &file[4..])),
        _ => {
            if let Some(s) = file.strip_suffix(".sng").or_else(|| file.strip_suffix(".SNG")) {
                v.push(format!("{s}.ins"));
            }
        }
    }
    v.push("smp.set".to_string());
    v
}

fn new(d: &[u8], module: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let blk = blocks(d)?;
    let mut d = d.to_vec();
    // the two known-bad files are repaired, as the reference player does, by the
    // size of the song
    let size = blk[6].0 + blk[6].1;
    let mut repair = None;
    if size == 9506 && be16(&d, 2650) == 2 {
        repair = Some(2650..2652);
    }
    if size == 2989 && be32(&d, 1192) == 0x0805_0706 {
        repair = Some(1192..1196);
    }
    if let Some(r) = repair.clone() {
        d[r.clone()].fill(0);
        // in memory too
        m.bus.write_bytes(module + r.start as u32, &d[r]);
    }
    let file = m.bus.read_cstr(m.deli.file_buf);
    let mut sample = None;
    for name in companion(&file) {
        if let Ok(s) = crate::deli::read_companion(m, &name).1 {
            sample = Some(s);
            break;
        }
    }
    let sample = sample?;
    // the sample area starts after the first four bytes of the file
    let mut pristine = sample.get(4..).unwrap_or(&[]).to_vec();
    // records may reach past the file's end: such reads give zero
    let mut reach = pristine.len();
    for i in 0..blk[0].1 / 32 {
        let r = blk[0].0 + 32 * i;
        let o = be32(&d, r) as usize;
        let words = be16(&d, r + 0x10).max(be16(&d, r + 0x14)) as usize + be16(&d, r + 0x12).max(be16(&d, r + 0x16)) as usize;
        reach = reach.max((o + 2 * words).min(pristine.len() + 0x40000));
    }
    pristine.resize(reach, 0);
    // the sample file lies whole in memory, its area after its four header bytes
    let at = m.alloc(pristine.len() as u32 + 4);
    if at == 0 {
        return None;
    }
    let base = at + 4;
    m.bus.write_bytes(at, sample.get(..4).unwrap_or(&[]));
    m.bus.write_bytes(base, &pristine);
    relocate(&d, &blk, module, base, m);
    let subsongs = ((blk[2].1 / 4) as u16).max(1);
    let mut r = Rjp {
        d,
        blk,
        title: String::new(),
        subsongs,
        sub: 0,
        area: pristine.clone(),
        pristine,
        base,
        v: Default::default(),
        flags: 0,
        flags0: 0,
        songend: false,
        first: false,
        tick_at: None,
        next_at: std::cell::Cell::new(None),
        grid: 0,
        ticked: false,
        t0: 0,
        h: 0,
    };
    r.title = file;
    let mut hw = Hw { bus: &mut m.bus };
    hw.dmacon(0x000F);
    r.start(0, &mut hw, false);
    Some(Box::new(r))
}

/// The song in memory as the original leaves it after its set-up: the tag reads `MODS`, and each
/// instrument record holds the address of its sample (the area's address added to its offset, and to
/// its vibrato and tremolo table offsets when they are set) and its first segment's start in bytes
/// (measured on the memory of the reference runs: a voice whose sample lies in the song plays these
/// bytes).
fn relocate(d: &[u8], blk: &[(usize, usize); 7], module: u32, base: u32, m: &mut Machine) {
    m.bus.write_bytes(module + 4, b"MODS");
    for i in 0..blk[0].1 / 32 {
        let r = blk[0].0 + 32 * i;
        let a = module + r as u32;
        m.bus.w32(a, be32(d, r).wrapping_add(base));
        for o in [4, 8] {
            let v = be32(d, r + o);
            if v != 0 {
                m.bus.w32(a + o as u32, v.wrapping_add(base));
            }
        }
        m.bus.w16(a + 0x10, be16(d, r + 0x10).wrapping_mul(2));
    }
}

impl Rjp {
    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    /// The offset of record `n` of instrument block, or of the empty one if `n` is
    /// past the block.
    fn record(&self, n: u16) -> usize {
        let (o, len) = self.blk[0];
        if 32 * n as usize >= len {
            o
        } else {
            o + 32 * n as usize
        }
    }

    /// The offset of position list `n`.
    fn list(&self, n: u8) -> usize {
        self.blk[5].0.wrapping_add(be32(&self.d, self.blk[3].0 + 4 * n as usize) as usize)
    }

    fn pattern(&self, n: u8) -> usize {
        self.blk[6].0.wrapping_add(be32(&self.d, self.blk[4].0 + 4 * n as usize) as usize)
    }

    /// The start of subsong `n` (the original writes no stop of DMA here: the load stops it once,
    /// a change in its start-up routine).
    fn start(&mut self, n: u16, hw: &mut Hw, change: bool) {
        self.sub = n;
        self.songend = false;
        self.first = change;
        self.tick_at = change.then_some(hw.bus.clock);
        // a new start finds the samples as loaded
        self.area.copy_from_slice(&self.pristine);
        hw.bus.write_bytes(self.base, &self.pristine);
        self.v = Default::default();
        self.flags = 0;
        let row = self.blk[2].0 + 4 * (n & 63) as usize;
        for ch in 0..4 {
            let l = self.rb(row + ch);
            if l == 0 {
                continue;
            }
            self.flags |= 1 << ch;
            let p = self.list(l);
            let first = self.rb(p);
            self.v[ch] = Voice {
                pat: Some(self.pattern(first)),
                lst: p + 1,
                speed: 6,
                cnt: 1,
                rep: 1,
                active: true,
                ..Default::default()
            };
        }
        self.flags0 = self.flags;
    }

    /// The set-up of subsong `n` within a change after ticks, in half colour clocks (`SETTLE_*`).
    fn change_settle(&self, n: u16) -> u64 {
        let lists = self.blk[3].1 / 4;
        let len = |l: usize| -> i64 {
            let o = be32(&self.d, self.blk[3].0 + 4 * l);
            let e = if l + 1 == lists { self.blk[5].1 as u32 } else { be32(&self.d, self.blk[3].0 + 4 * l + 4) };
            e.wrapping_sub(o) as i32 as i64
        };
        let n = (n & 63) as usize;
        let mut h = SETTLE_BASE + SETTLE_SUBSONG * n as u64;
        for s in 0..=n {
            let mut longest = 0;
            for ch in 0..4 {
                let l = self.rb(self.blk[2].0 + 4 * s + ch) as usize;
                let x = len(l);
                if x >= longest {
                    longest = x;
                    h += SETTLE_LONGEST;
                }
                if l + 1 == lists {
                    h += SETTLE_LAST_LIST;
                }
                if s == n && l != 0 {
                    h += SETTLE_VOICE[ch];
                }
            }
        }
        h
    }

    /// A voice reached the end of its list.
    fn list_end(&mut self, ch: usize) {
        self.flags &= !(1 << ch);
        if self.flags == 0 {
            self.h += H_SONG_END;
            self.songend = true;
            self.flags = self.flags0;
        }
    }

    /// Selects instrument `n` for the voice; whether it is another than the voice's.
    fn select(&mut self, ch: usize, n: u8) -> bool {
        if self.v[ch].ins == n as u16 && self.v[ch].rec.is_some() {
            return false;
        }
        let r = self.record(n as u16);
        let d = &self.d;
        let w = |o: usize| be16(d, r + o);
        let l = |o: usize| be32(d, r + o);
        let v = &mut self.v[ch];
        v.ins = n as u16;
        v.rec = Some(r);
        v.sample = l(0);
        v.vib = l(4);
        v.trem = l(8);
        v.ivol = w(0x0E);
        v.loop_start = w(0x14);
        v.loop_len = w(0x16);
        v.vib_restart = w(0x18);
        v.vib_len = w(0x1A);
        v.trem_restart = w(0x1C);
        v.trem_len = w(0x1E);
        v.ppos = 0;
        v.tpos = 0;
        true
    }

    /// A note of period `p`: stop the channel and set up its first segment.
    fn note(&mut self, ch: usize, p: u16, hw: &mut Hw) {
        let Some(r) = self.v[ch].rec else { return };
        let env = self.blk[1].0 + be16(&self.d, r + 0x0C) as usize;
        let e: [u8; 5] = std::array::from_fn(|i| self.rb(env + i));
        let start = (be16(&self.d, r + 0x10).wrapping_mul(2)) as u32;
        let len = be16(&self.d, r + 0x12);
        let v = &mut self.v[ch];
        v.base_per = p;
        v.per = p;
        if !v.keep {
            v.acc = 0;
            v.slide = 0;
        }
        v.env = Some(env);
        v.end = e[1];
        v.delta = e[1].wrapping_sub(e[0]) as i8;
        v.c = e[2];
        v.d = e[2];
        v.phase = 4;
        let s = v.sample.wrapping_add(start);
        self.h += H_NOTE;
        self.at(hw);
        hw.dmacon(1 << ch);
        self.h += H_NOTE_DAT;
        self.at(hw);
        hw.bus.write_word(0xDFF0AA + 16 * ch as u32, 0);
        if (s as usize) + 2 <= self.area.len() {
            self.area[s as usize..s as usize + 2].fill(0);
            hw.bus.write_bytes(self.base + s, &[0, 0]);
        }
        self.h += H_NOTE_LOC;
        self.at(hw);
        hw.loc(ch, self.base.wrapping_add(s));
        self.h += H_LEN;
        self.at(hw);
        hw.len(ch, len);
        self.h += H_NOTE_END[ch];
        self.v[ch].dma = true;
    }

    /// The next entry of the voice's position list: a pattern to read from, or the
    /// voice's end.
    fn next_pattern(&mut self, ch: usize) -> Option<usize> {
        for _ in 0..GUARD {
            let p = self.v[ch].lst;
            let n = self.rb(p);
            self.v[ch].lst = p + 1;
            if n != 0 {
                return Some(self.pattern(n));
            }
            self.list_end(ch);
            let c = self.rb(p + 1);
            if c == 0 {
                self.h += H_LIST_STOP[ch];
                return None;
            }
            self.h += H_LIST_JUMP[ch];
            match c {
                0 => return None,
                1..=0x7F => self.v[ch].lst = (p + 1).wrapping_sub(c as usize),
                _ => {
                    let l = self.rb(p + 2);
                    self.v[ch].lst = self.list(l);
                }
            }
        }
        None
    }

    /// Reads one row of the voice's pattern.
    fn row(&mut self, ch: usize, hw: &mut Hw) {
        for _ in 0..GUARD {
            let Some(p) = self.v[ch].pat else {
                self.v[ch].active = false;
                return;
            };
            let b = self.rb(p);
            self.v[ch].pat = Some(p + 1);
            match b {
                0..=0x7F => {
                    let per = PERIODS.get(b as usize / 2).copied().unwrap_or(0);
                    self.note(ch, per, hw);
                    return;
                }
                0x80 => {
                    self.h += H_PAT_END;
                    self.v[ch].repeat = 1;
                    match self.next_pattern(ch) {
                        Some(q) => self.v[ch].pat = Some(q),
                        None => {
                            self.v[ch].pat = None;
                            self.v[ch].active = false;
                            return;
                        }
                    }
                }
                0x81 => {
                    let rel = self.v[ch].env.map(|e| self.rb(e + 5));
                    self.h += if rel.is_some() { H_RELEASE } else { H_RELEASE_NONE };
                    if let Some(rel) = rel {
                        let v = &mut self.v[ch];
                        v.end = 0;
                        v.delta = 0u8.wrapping_sub(v.level) as i8;
                        v.c = rel;
                        v.d = rel;
                        v.phase = 255;
                    }
                    return;
                }
                0x82 => {
                    self.h += H_SPEED;
                    self.v[ch].speed = self.rb(p + 1);
                    self.v[ch].pat = Some(p + 2);
                }
                0x83 => {
                    self.h += H_REPEAT_CMD;
                    self.v[ch].repeat = self.rb(p + 1);
                    self.v[ch].pat = Some(p + 2);
                }
                0x84 => {
                    let n = self.rb(p + 1);
                    self.v[ch].pat = Some(p + 2);
                    self.h += if n == 0 {
                        H_INSTRUMENT_NONE
                    } else if self.select(ch, n) {
                        H_INSTRUMENT_NEW
                    } else {
                        H_INSTRUMENT_SAME
                    };
                }
                0x85 => {
                    self.h += H_VOLUME_CMD;
                    self.v[ch].ivol = self.rb(p + 1) as u16;
                    self.v[ch].pat = Some(p + 3);
                }
                0x86 => {
                    self.h += H_SLIDE_CMD;
                    let v = &mut self.v[ch];
                    v.slide = self.d.get(p + 1).copied().unwrap_or(0);
                    v.rate = be32(&self.d, p + 2);
                    v.acc = 0;
                    v.keep = true;
                    v.pat = Some(p + 6);
                }
                0x87 => {
                    self.h += H_ROW_END;
                    return;
                }
                // undefined commands: skipped
                _ => self.h += H_UNDEFINED,
            }
        }
        self.v[ch].active = false;
    }

    fn sequence(&mut self, ch: usize, hw: &mut Hw) {
        let v = &mut self.v[ch];
        v.cnt = v.cnt.wrapping_sub(1);
        if v.cnt != 0 {
            return;
        }
        v.rep = v.rep.wrapping_sub(1);
        if v.rep != 0 {
            self.h += H_REPEAT;
            let v = &mut self.v[ch];
            v.cnt = v.speed;
            return;
        }
        self.row(ch, hw);
        let v = &mut self.v[ch];
        v.rep = v.repeat;
        v.cnt = v.speed;
    }

    /// The envelope, tremolo and scaling; the volume to write.
    fn volume(&mut self, ch: usize) -> u16 {
        let mut h = H_VOLUME[ch];
        let v = &mut self.v[ch];
        let mut refresh = true;
        if v.phase != 0 {
            h += H_ENVELOPE;
            let q = if v.d == 0 { 0 } else { v.delta as i32 * v.c as i32 / v.d as i32 };
            v.level = (v.end as i32).wrapping_sub(q) as u8;
            v.c = v.c.wrapping_sub(1);
            if v.c == 255 {
                h += match v.phase {
                    4 => H_DECAY,
                    2 => H_SUSTAIN,
                    _ => H_RELEASED,
                };
                match v.phase {
                    4 => {
                        let env = v.env.unwrap_or(0);
                        let e: [u8; 5] = std::array::from_fn(|i| self.d.get(env + i).copied().unwrap_or(0));
                        let v = &mut self.v[ch];
                        v.end = e[3];
                        v.delta = e[3].wrapping_sub(e[1]) as i8;
                        v.c = e[4];
                        v.d = e[4];
                        v.phase = 2;
                    }
                    2 => v.phase = 0,
                    _ => {
                        v.phase = 0;
                        refresh = false;
                    }
                }
            }
        }
        let v = &mut self.v[ch];
        if refresh {
            v.work = v.level as i16;
        }
        if v.trem != 0 {
            h += H_TREMOLO;
            let t = self.area.get(v.trem as usize + v.tpos as usize).copied().unwrap_or(0) as i8 as i32;
            let w = v.work as i32;
            v.work = (w + ((t * w) >> 7)) as i16;
            v.tpos = v.tpos.wrapping_add(1);
            if v.tpos == v.trem_len.wrapping_mul(2) {
                h += H_TABLE_WRAP;
                v.tpos = v.trem_restart.wrapping_mul(2);
            }
        }
        let w = ((v.work as i32 * v.ivol as i32) as i16) >> 6;
        let w = ((w as i32 * 64) as i16) >> 6;
        v.work = w;
        self.h += h;
        w.clamp(0, 64) as u16
    }

    /// The period to write: vibrato, then the slide.
    fn period(&mut self, ch: usize) -> u16 {
        let mut h = H_PERIOD[ch];
        let v = &mut self.v[ch];
        if v.vib != 0 {
            h += H_VIBRATO;
            let t = self.area.get(v.vib as usize + v.ppos as usize).copied().unwrap_or(0) as i8 as i32;
            let x = (t * v.base_per as i32) >> 7;
            let mut y = (-x) as i16;
            if y < 0 {
                h += H_VIBRATO_DOWN;
                y >>= 1;
            }
            v.per = v.base_per.wrapping_add(y as u16);
            v.ppos = v.ppos.wrapping_add(1);
            if v.ppos == v.vib_len.wrapping_mul(2) {
                h += H_TABLE_WRAP;
                v.ppos = v.vib_restart.wrapping_mul(2);
            }
        }
        if v.slide != 0 {
            h += H_SLIDE;
            v.acc = v.acc.wrapping_add(v.rate);
            v.slide -= 1;
        }
        self.h += h;
        ((v.acc >> 16) as u16).wrapping_add(v.per)
    }

    fn step(&mut self, ch: usize, hw: &mut Hw) {
        self.v[ch].keep = false;
        if self.v[ch].install {
            self.v[ch].install = false;
            let v = &self.v[ch];
            let a = self.base.wrapping_add(v.sample).wrapping_add(2 * v.loop_start as u32);
            let len = v.loop_len;
            self.at(hw);
            hw.loc(ch, a);
            self.h += H_LEN;
            self.at(hw);
            hw.len(ch, len);
            self.h += H_INSTALL_END;
        }
        if self.v[ch].dma {
            self.v[ch].dma = false;
            self.h += H_DMA_ON;
            self.at(hw);
            hw.dmacon(0x8200 | 1 << ch);
            self.h -= H_DMA_ON_BACK;
            self.v[ch].install = true;
        }
        self.sequence(ch, hw);
        let vol = self.volume(ch);
        self.at(hw);
        hw.vol(ch, vol);
        let per = self.period(ch);
        self.at(hw);
        hw.per(ch, per);
        self.h += H_NEXT_VOICE[ch];
    }

    /// The write that follows falls at the original's time into the tick.
    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = hw.bus.clock.max(self.t0 + self.h / 2);
    }
}

// The original's time in a tick, in half colour clocks (68000 cycles), measured on the half-cycle
// write logs of the reference runs of 85 modules (every in-tick gap of 567,000 writes): each piece of
// work the voices do between two writes costs a fixed time by its path, and the voice's channel.

/// From the tick's start to the first voice's work.
const H_START: u64 = 1;
/// A voice without a pattern, skipped.
const H_SKIP: u64 = 177;
/// From a voice's period write to the next voice's work, by the voice's channel.
const H_NEXT_VOICE: [u64; 4] = [409, 440, 471, 0];
/// The install of a sample's loop: from its location write to its length write, and after it.
const H_LEN: u64 = 20;
const H_INSTALL_END: u64 = 72;
/// The DMA start: to its write, and what the work after it takes less (55 - 7).
const H_DMA_ON: u64 = 55;
const H_DMA_ON_BACK: u64 = 7;
/// The sequencer's repeat count running down (its row counter at zero, the repeat not).
const H_REPEAT: u64 = 45;
/// Row commands: speed, repeat, instrument (none, the voice's, another), volume, slide.
const H_SPEED: u64 = 75;
const H_REPEAT_CMD: u64 = 75;
const H_INSTRUMENT_NONE: u64 = 76;
const H_INSTRUMENT_SAME: u64 = 131;
const H_INSTRUMENT_NEW: u64 = 562;
/// Not seen in the runs: as a speed command.
const H_VOLUME_CMD: u64 = 75;
const H_UNDEFINED: u64 = 75;
const H_SLIDE_CMD: u64 = 139;
/// A pattern's end with the next list entry read; a list end with a jump, by channel; a list end
/// that stops the voice (its row's end included), by channel (voice 0's not seen: as voice 1's
/// less the step of the others); the last voice's end (the song's end).
const H_PAT_END: u64 = 188;
const H_LIST_JUMP: [u64; 4] = [235, 268, 297, 316];
const H_LIST_STOP: [u64; 4] = [258, 291, 320, 339];
const H_SONG_END: u64 = 87;
/// A row's end without a note: the release (with an envelope, without), the end-of-row command.
const H_RELEASE: u64 = 309;
const H_RELEASE_NONE: u64 = 213;
const H_ROW_END: u64 = 144;
/// A note: the row to its stop of DMA, then to the data, location and length writes, and from
/// the length write to the volume work, by channel.
const H_NOTE: u64 = 515;
const H_NOTE_DAT: u64 = 44;
const H_NOTE_LOC: u64 = 44;
const H_NOTE_END: [u64; 4] = [175, 220, 265, 281];
/// The volume work to its write, by channel; the envelope step, its segment ends (attack to decay,
/// decay to sustain, release to silence); the tremolo (not seen in the runs) and a table's wrap.
const H_VOLUME: [u64; 4] = [702, 719, 726, 753];
const H_ENVELOPE: u64 = 330;
const H_DECAY: u64 = 202;
const H_SUSTAIN: u64 = 89;
const H_RELEASED: u64 = 13;
const H_TREMOLO: u64 = 210;
const H_TABLE_WRAP: u64 = 24;
/// The period work to its write, by channel; the vibrato (its downward half), the slide.
const H_PERIOD: [u64; 4] = [273, 304, 335, 343];
const H_VIBRATO: u64 = 210;
const H_VIBRATO_DOWN: u64 = 8;
const H_SLIDE: u64 = 56;

/// Colour clocks from the song's start to the host's timer start, beyond the first period (the
/// song starts without a start-up routine: the reference's first tick follows the song start by
/// a period and this much; measured over ten modules, the ticks' grid stays on it).
const TIMER_DELAY: u64 = 376;

/// A change of subsong: the start-up routine's first stop of DMA follows this long after the change.
const CHANGE_FIRST: u64 = 234;

/// A change of subsong after ticks: from the start-up routine's second stop of DMA to the song's
/// start (the host's change returns), in half colour clocks. The set-up walks the voice entries of
/// every subsong from the first to the new one, and in each subsong keeps the longest position list
/// so far (a list's length: the next list's offset, or for the last list the list block's length,
/// less its own; signed, from zero; a list as long as the longest so far counts). Fitted, exact on
/// the 51 subsong changes of the 91 reference runs (and on 85 song starts but six, by a half
/// cycle) and on byte-modified copies: a fixed part, each subsong walked, each entry that is a new
/// longest, each entry of the last list, and each voice of the new subsong (voice 0 more).
const SETTLE_BASE: u64 = 1749;
const SETTLE_SUBSONG: u64 = 602;
const SETTLE_LONGEST: u64 = 52;
const SETTLE_LAST_LIST: u64 = 16;
const SETTLE_VOICE: [u64; 4] = [511, 507, 507, 507];

/// The start-up routine of a change of subsong, from `CHANGE_FIRST` after it: all voices off,
/// the volumes cleared, the DMA stopped again.
fn startup(hw: &mut Hw) {
    hw.wait(CHANGE_FIRST);
    hw.dmacon(0x000F);
    for ch in 0..4 {
        hw.wait(6);
        hw.vol(ch, 0);
    }
    hw.wait(4648);
    hw.dmacon(0x000F);
}

impl Replayer for Rjp {
    fn format(&self) -> &'static str {
        "Richard Joseph Player"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn timer_delay(&self) -> u64 {
        TIMER_DELAY
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subsongs - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        if self.ticked {
            // the start-up routine runs within the change, before the song's start (in the
            // reference the host's change returns only after it and the voices' set-up: the song
            // starts 1,561-2,431 colour clocks after its second stop of DMA, and the first notes
            // come 990 after that, not 8,257 after the change as when the start-up ran in the tick)
            //
            // the host's timer runs on through the change: a tick that falls due within it runs
            // at the song's start (after the timer's own delay), and the next one comes on the
            // timer's old grid (measured on 43 runs, changes after 1 to 45 ticks)
            let grid = self.t0 + self.tick_clocks();
            self.start(n, hw, true);
            startup(hw);
            hw.wait(self.change_settle(n) / 2);
            if grid <= hw.bus.clock {
                self.tick_at = Some(hw.bus.clock + TIMER_DELAY);
                self.grid = grid + self.tick_clocks();
            } else {
                self.first = false;
                self.tick_at = Some(grid);
            }
        } else {
            // a change before the host's timer has run (a subsong chosen at the start): the
            // start-up routine runs at once, and the first tick comes a period after the song's
            // start as for the first subsong (the reference's first notes follow its last
            // start-up by a period and its timer's start, not 8,000 colour clocks, and their
            // DMA comes on a full tick after their data-register write, when the channel has
            // left the state that write put it in)
            self.start(n, hw, false);
            startup(hw);
        }
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.tick_at.take()
    }

    fn next_tick_at(&self) -> Option<u64> {
        self.next_at.take()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.ticked = true;
        self.t0 = hw.bus.clock;
        if self.flags0 == 0 {
            // a subsong without voices ends at every tick
            self.songend = true;
            return;
        }
        self.h = H_START;
        if self.first {
            // the tick that fell due within a change of subsong: the next one is on the timer's grid
            self.first = false;
            self.next_at.set(Some(self.grid));
        }
        for ch in 0..4 {
            if self.v[ch].active {
                self.step(ch, hw);
            } else {
                self.h += H_SKIP;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use crate::replay::DT_TICK;
    use std::path::Path;

    fn block(d: &mut Vec<u8>, b: &[u8]) {
        d.extend_from_slice(&(b.len() as u32).to_be_bytes());
        d.extend_from_slice(b);
    }

    /// One voice (list 1: pattern 1 then stop): instrument 1 with envelope 0, a
    /// note, a rest, a note off; the sample area is 64 bytes of 0x40.
    fn song() -> Vec<u8> {
        song_with(&[[1, 0, 0, 0]])
    }

    /// `song` with these subsongs (the list number of each voice).
    fn song_with(subsongs: &[[u8; 4]]) -> Vec<u8> {
        song_pattern(subsongs, &[0x84, 1, 0x82, 1, 0x83, 1, 38, 0x87, 0x81, 0x80])
    }

    /// `song_with` playing pattern 1 = `pat`; instrument 2 is a copy of instrument 1.
    fn song_pattern(subsongs: &[[u8; 4]], pat: &[u8]) -> Vec<u8> {
        let mut d = b"RJP1SMOD".to_vec();
        let mut ins = vec![0u8; 96];
        // record 0: the empty instrument
        ins[0x12 + 1] = 1;
        ins[0x16 + 1] = 1;
        ins[0x1A + 1] = 1;
        ins[0x1E + 1] = 1;
        // record 1: sample at 0, volume 64, first segment 8 words, loop at 4 of 4 words
        let r = 32;
        ins[r + 0x0E + 1] = 64;
        ins[r + 0x12 + 1] = 8;
        ins[r + 0x14 + 1] = 4;
        ins[r + 0x16 + 1] = 4;
        ins.copy_within(32..64, 64);
        block(&mut d, &ins);
        block(&mut d, &[64, 64, 254, 64, 254, 254]);
        block(&mut d, subsongs.concat().as_slice());
        block(&mut d, &[0, 0, 0, 0, 0, 0, 0, 0]);
        block(&mut d, &[0, 0, 0, 0, 0, 0, 0, 0]);
        block(&mut d, &[1, 0, 0, 0]);
        // pattern 1: instrument 1, speed 1, repeat 1, note index 19 (period 339),
        // rest, note off, end of pattern
        block(&mut d, pat);
        // fix up the tables: list 1 at block 6 offset 0, pattern 1 at offset 0
        let b = blocks(&d).unwrap();
        d[b[3].0 + 4..b[3].0 + 8].copy_from_slice(&0u32.to_be_bytes());
        d[b[4].0 + 4..b[4].0 + 8].copy_from_slice(&0u32.to_be_bytes());
        d
    }

    #[test]
    fn recognises_by_content() {
        let d = song();
        assert!(detect(&d));
        assert!(blocks(&d).is_some());
        let mut e = d.clone();
        e[3] = b'3';
        assert!(detect(&e));
        e[4] = b'X';
        assert!(!detect(&e));
        let mut e = d.clone();
        e[15] = 1;
        assert!(!detect(&e));
        assert!(!detect(&d[..15]));
        assert!(blocks(&d[..d.len() - 1]).is_none());
        assert_eq!(companion("RJP.title"), ["smp.title", "smp.set"]);
        assert_eq!(companion("a.sng"), ["a.ins", "smp.set"]);
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn plays_a_note_in_three_ticks() {
        let dir = Path::new("/rjp-test");
        let mut smp = b"RJP1".to_vec();
        smp.extend_from_slice(&[0x40; 64]);
        let path = dir.join("rjp.test");
        let mut e = Engine::new(48000.0);
        e.m.os.mem_files.insert(dir.join("SMP.test"), smp);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(&path, song()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Richard Joseph Player");
        let mut out = vec![0f32; 2 * 800 * 8];
        e.render(&mut out);
        let log: Vec<(u32, u16)> = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        // tick 0: DMA off, first segment of 8 words, period 339
        assert_eq!(reg(&log, 0xDFF0A4)[..2], [8, 4]);
        assert_eq!(reg(&log, 0xDFF0AA)[0], 0);
        assert_eq!(reg(&log, 0xDFF0A6)[..2], [339, 339]);
        // tick 1 enables, tick 2 installs the loop of 4 words at word 4
        let dma = reg(&log, 0xDFF096);
        assert!(dma.contains(&0x0001) && dma.contains(&0x8201));
        let lc = reg(&log, 0xDFF0A2);
        assert_eq!(lc[1] - lc[0], 8);
        // the start-up routine (volumes cleared first) runs only after a change of subsong, not
        // at the song's start; the envelope is flat at 64
        assert_eq!(reg(&log, 0xDFF0A8)[..2], [64, 64]);
    }

    /// A subsong chosen before the timer has run (`--subsong`): the start-up routine runs at the
    /// change, and the first notes come a period after the song's start, so that their DMA comes on a
    /// full tick after the data-register write, when the channel has left the word that write
    /// started; it then fetches the sample (it fetched from address 0 when the notes came in the
    /// start-up tick, 8,000 colour clocks after the change).
    #[test]
    fn subsong_chosen_at_the_start_fetches_the_sample() {
        let dir = Path::new("/rjp-test");
        let mut smp = b"RJP1".to_vec();
        smp.extend_from_slice(&[0x40; 64]);
        let path = dir.join("rjp.test");
        let mut e = Engine::new(48000.0);
        e.m.os.mem_files.insert(dir.join("SMP.test"), smp);
        e.open_replayers = true;
        // subsong 1 plays on voice 3 alone
        e.play_data(&path, song_with(&[[1, 0, 0, 0], [0, 0, 0, 1]])).unwrap();
        e.m.bus.write_log = Some(Vec::new());
        e.m.bus.paula.dma_log = Some(Vec::new());
        let start = e.m.bus.clock;
        e.change_subsong(1).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().sub, 1);
        let song_start = e.m.bus.clock;
        let mut out = vec![0f32; 2 * 960 * 6];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let at = |a: u32, v: Option<u16>| log.iter().find(|w| w.0 == a && v.is_none_or(|v| w.1 == v)).map(|w| w.2);
        // the note: a period after the song's start, then the DMA a tick later
        let dat = at(0xDFF0DA, None).unwrap();
        let on = at(0xDFF096, Some(0x8208)).unwrap();
        assert!(dat > song_start + DT_TICK, "{dat} {song_start}");
        assert!(on - dat > 65536 + 339, "DMA on {} after the data write", on - dat);
        // every fetch of voice 3 is in its sample
        let loc = log.iter().filter(|w| w.0 == 0xDFF0D0 || w.0 == 0xDFF0D2).map(|w| w.1 as u32).collect::<Vec<_>>();
        let base = loc[0] << 16 | loc[1];
        let dma = e.m.bus.paula.dma_log.take().unwrap();
        assert!(!dma.is_empty());
        assert!(dma.iter().all(|f| f.1 == 3 && f.2 >= base && f.2 < base + 16), "{:x?}", &dma[..4.min(dma.len())]);
        // the start-up routine at the change: DMA off, the volumes cleared, DMA off again
        let vols: Vec<u64> = log.iter().filter(|w| [0xDFF0A8, 0xDFF0B8, 0xDFF0C8, 0xDFF0D8].contains(&w.0) && w.1 == 0).map(|w| w.2).collect();
        assert_eq!(vols.len(), 4);
        assert!(vols.iter().all(|&t| t > start && t <= song_start), "{vols:?} {start} {song_start}");
        // the voices before voice 3 have no pattern: its first write of the tick (the stop of DMA)
        // follows their skips and its row's commands
        let off = at(0xDFF096, Some(0x0008)).unwrap();
        let h = H_START + 3 * H_SKIP + H_INSTRUMENT_NEW + H_SPEED + H_REPEAT_CMD + H_NOTE;
        assert_eq!(off, song_start + DT_TICK + TIMER_DELAY + h / 2);
    }

    /// A note whose instrument command selects another instrument than the voice's has its stop of
    /// DMA later than one whose command repeats the voice's instrument, by the difference of their
    /// costs (`H_INSTRUMENT_NEW`, `H_INSTRUMENT_SAME`, in half colour clocks).
    #[test]
    fn a_new_instrument_delays_the_note() {
        let off = |second: u8| {
            let dir = Path::new("/rjp-test");
            let mut smp = b"RJP1".to_vec();
            smp.extend_from_slice(&[0x40; 64]);
            let mut e = Engine::new(48000.0);
            e.m.os.mem_files.insert(dir.join("SMP.test"), smp);
            e.open_replayers = true;
            e.m.bus.write_log = Some(Vec::new());
            let pat = [0x84, 1, 0x82, 1, 0x83, 1, 38, 0x84, second, 38, 0x87, 0x81, 0x80];
            e.play_data(&dir.join("rjp.test"), song_pattern(&[[1, 0, 0, 0]], &pat)).unwrap();
            let mut out = vec![0f32; 2 * 960 * 6];
            e.render(&mut out);
            let log = e.m.bus.write_log.take().unwrap();
            log.iter().filter(|w| w.0 == 0xDFF096 && w.1 == 0x0001).map(|w| w.2).nth(1).unwrap()
        };
        let d = off(2) - off(1);
        let h = (H_INSTRUMENT_NEW - H_INSTRUMENT_SAME) / 2;
        assert!(d == h || d == h + 1, "{d}");
    }

    /// The empty first subsong is skipped once it has ticked (the reference's change after ticks):
    /// the start-up routine runs within the change, the song starts a voice-dependent time after
    /// its second stop of DMA (`CHANGE_SETTLE`), and the tick that fell due within the change runs
    /// the timer's delay after that (refcmp rows 1676 and 1688: the notes came 7,267 colour clocks
    /// late when the start-up ran in the tick after the song's start).
    #[test]
    fn a_change_after_ticks_runs_the_start_up_before_the_song_start() {
        let dir = Path::new("/rjp-test");
        let mut smp = b"RJP1".to_vec();
        smp.extend_from_slice(&[0x40; 64]);
        let path = dir.join("rjp.test");
        let mut e = Engine::new(48000.0);
        e.m.os.mem_files.insert(dir.join("SMP.test"), smp);
        e.open_replayers = true;
        e.play_data(&path, song_with(&[[0, 0, 0, 0], [1, 0, 0, 0]])).unwrap();
        e.m.bus.write_log = Some(Vec::new());
        // the skip is checked after each render
        let mut out = vec![0f32; 2 * 960 * 10];
        e.render(&mut out);
        assert_eq!(e.playing.as_ref().unwrap().sub, 1);
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let stops: Vec<u64> = log.iter().filter(|w| w.0 == 0xDFF096 && w.1 == 0x000F).map(|w| w.2).collect();
        let second = *stops.last().unwrap();
        let first_dma = log.iter().find(|w| w.0 == 0xDFF096 && w.1 == 0x0001).unwrap().2;
        let h = H_START + H_INSTRUMENT_NEW + H_SPEED + H_REPEAT_CMD + H_NOTE;
        // the set-up walks subsongs 0 and 1: list 0 (length 0) four times, each as long as the
        // longest, then list 1 (the last list, 4 bytes long) on voice 0, a new longest
        let settle = SETTLE_BASE + SETTLE_SUBSONG + 5 * SETTLE_LONGEST + SETTLE_LAST_LIST + SETTLE_VOICE[0];
        assert_eq!(first_dma, second + settle / 2 + TIMER_DELAY + h / 2);
    }

    /// The set-up of a change walks every subsong up to the new one and counts each entry whose list
    /// is as long as the longest so far in its subsong (lengths from the offset table, the last list's
    /// from the list block's length). A fixed time by the number of voices put the first notes of
    /// 13 of the 21 changes after an empty first subsong (refcmp rows 349, 1618, 1676, ...) -70 to
    /// +8 colour clocks off, and those of rows 1631, 1647, 1650 and 1683 +6 to +32.
    #[test]
    fn the_change_set_up_counts_the_longest_lists() {
        let mut d = song_with(&[[0, 0, 0, 0], [1, 1, 0, 1]]);
        let b = blocks(&d).unwrap();
        let settle = |d: &[u8]| {
            let r = Rjp {
                d: d.to_vec(),
                blk: blocks(d).unwrap(),
                title: String::new(),
                subsongs: 2,
                sub: 0,
                pristine: Vec::new(),
                area: Vec::new(),
                base: 0,
                v: Default::default(),
                flags: 0,
                flags0: 0,
                songend: false,
                first: false,
                tick_at: None,
                next_at: std::cell::Cell::new(None),
                grid: 0,
                ticked: false,
                t0: 0,
                h: 0,
            };
            r.change_settle(1)
        };
        let fixed = SETTLE_BASE + SETTLE_SUBSONG + 3 * SETTLE_LAST_LIST + SETTLE_VOICE[0] + SETTLE_VOICE[1] + SETTLE_VOICE[3];
        // two lists at offset 0: list 0 is 0 bytes long, list 1 (the last) the list block's 4; subsong
        // 0's four entries of list 0 are each as long as the longest, and subsong 1's three of list 1
        assert_eq!(settle(&d), fixed + 7 * SETTLE_LONGEST);
        // list 1 moved to offset 8: list 0 is 8 bytes long, list 1 -4 (never the longest)
        d[b[3].0 + 4..b[3].0 + 8].copy_from_slice(&8u32.to_be_bytes());
        assert_eq!(settle(&d), fixed + 5 * SETTLE_LONGEST);
    }
}
