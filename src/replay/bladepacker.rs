//! Blade Packer (`uds.*` + `smp.*`), the tracker of Tord "Blade" Jansson, as used in the game
//! Obsession (Unique Development Sweden, 1995).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes of
//! the 68k reference player and from the module data; no original code. A module is a pair: the
//! song file `uds.NAME` (a position list, a pool of 64-row tracks, a dictionary of 4-byte note
//! cells) and the sample file `smp.NAME` (a table of 16-byte headers, then 8-bit samples). The
//! module's own code is not run: the song data is read as the specification says.
//!
//! Each tick the player first runs the output stage (Paula registers from the state the previous
//! sequencer run left, and a logical estimate of where Paula reads in every channel, so that a
//! voice is reprogrammed only when a note starts) and then the sequencer (a ProTracker-like row
//! and command engine with its own set of quirks). The colour-clock position of every write
//! follows the specification's timing formulas.
//!
//! Choices where the specification leaves the behaviour open (all end the voice or ignore the
//! cell, never panic or hang): a note on a channel that never had a sample selected does not start
//! a voice; a sample number beyond the sample table makes the whole cell ignored; a finetune above
//! 15 does not start the note; a command byte that is not a multiple of 4 (or above 60) is a row
//! with no action; a position jump or break past the position list ends the list (song end and
//! restart); every read beyond the song file returns 0.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus as _;
use std::sync::atomic::{AtomicU64, Ordering};

/// The raster phase (colour clocks modulo the line of 227) of the first tick call; the reference
/// host's differs per run. `u64::MAX` keeps the engine's own. A test and measurement knob.
pub static PHASE: AtomicU64 = AtomicU64::new(u64::MAX);

pub const OPEN: Open = Open { name: "Blade Packer", detect, new };

const MAGIC: [u8; 5] = [0x53, 0x8F, 0x4E, 0x47, 0x2E];
const END_POS: u16 = 0xDD48;
const END_TRK: u16 = 0xD8F1;
const TICK: u64 = 70_940;
/// Colour clocks from the DMACON write that ends the init to the first tick's interrupt.
const FIRST_TICK: u64 = 71_385;
const LINE: u64 = 227;

// ---------------------------------------------------------------------------------
// Tables

/// The 16 finetune rows of 36 periods as one linear table (spec 6.2); built from ProTracker's
/// (rows of 37). An index outside it reads 0, except the one in front: the word the original reads.
fn ptab(i: i32) -> i32 {
    if i == -1 {
        return 0x71AE;
    }
    if !(0..576).contains(&i) {
        return 0;
    }
    let i = i as usize;
    crate::protracker::PERIODS[(i / 36) * 37 + i % 36] as i32
}

const SINE_HALF: [u8; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197, 180, 161, 141,
    120, 97, 74, 49, 24,
];

/// The vibrato and tremolo waveforms (spec 6.7): 0 sine, 1 triangle, anything else square.
fn wave(kind: u8, pos: usize) -> u32 {
    let pos = pos & 63;
    match kind {
        0 => SINE_HALF[pos & 31] as u32,
        1 => {
            if pos < 32 {
                8 * pos as u32
            } else if pos == 32 {
                255
            } else {
                256 - 8 * (pos as u32 - 32)
            }
        }
        _ => 255,
    }
}

// ---------------------------------------------------------------------------------
// The song file

#[derive(Clone, Copy, Debug)]
struct Layout {
    /// Bytes per track row (1 or 2) and tracks per position (4 or 8).
    e: usize,
    v: usize,
    restart: usize,
    /// Offsets in the file: the position list, its end, the track pool, the dictionary.
    list: usize,
    list_end: usize,
    pool: usize,
    dict: usize,
    /// Number of cells (read by the tests; the file-size check uses it when the layout is made).
    #[allow(dead_code)]
    cells: usize,
    positions: usize,
}

/// The offset of the first even-offset word `w` at or after `from`.
fn find_word(d: &[u8], from: usize, w: u16) -> Option<usize> {
    let mut p = from;
    while p + 2 <= d.len() {
        if be16(d, p) == w {
            return Some(p);
        }
        p += 2;
    }
    None
}

fn layout(d: &[u8]) -> Option<Layout> {
    if d.len() < 12 || d[..5] != MAGIC {
        return None;
    }
    let e = if d[5] == b'B' { 1 } else { 2 };
    let v = if d[6] == b'4' { 4 } else { 8 };
    let restart = if d[7] == 0x7F { 0 } else { d[7] as usize };
    let list_end = find_word(d, 0, END_POS)?;
    if list_end < 8 {
        return None;
    }
    let pool = list_end + 2;
    let pool_end = find_word(d, pool, END_TRK)?;
    let dict = pool_end + 2;
    // 1 + the highest cell index any row refers to (every word of the pool, both bytes for byte rows)
    let mut hi = 0usize;
    let mut p = pool;
    while p + 2 <= pool_end {
        if e == 1 {
            hi = hi.max(d[p] as usize).max(d[p + 1] as usize);
        } else {
            hi = hi.max(be16(d, p) as usize);
        }
        p += 2;
    }
    let cells = hi + 1;
    if dict + 4 * cells > d.len() {
        return None;
    }
    Some(Layout { e, v, restart, list: 8, list_end, pool, dict, cells, positions: (list_end - 8) / v })
}

pub fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

/// The sample file: where its table is and how many headers it has.
fn sample_table(s: &[u8]) -> Option<(usize, usize)> {
    let t = if s.len() >= 8 && s[..4] == *b"SPLS" {
        4
    } else if s.len() >= 4 && be32(s, 0) == 0x1F4 {
        0
    } else {
        return None;
    };
    Some((t, be32(s, t) as usize / 16))
}

// ---------------------------------------------------------------------------------
// The player

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Act {
    #[default]
    None,
    Arp,
    SlideDown,
    SlideUp,
    Tone,
    Vib,
    ToneVol,
    VibVol,
    Trem,
    VolSlide,
    Retrig,
    Cut,
    Delay,
}

/// A music channel's record.
#[derive(Clone, Default)]
struct Ch {
    period: i32,
    slide: i32,
    up: i32,
    down: i32,
    target: i32,
    tone: i32,
    arp: [i32; 3],
    arp_len: usize,
    arp_cur: usize,
    arp_per: i32,
    note: u8,
    vib_pos: usize,
    vib_speed: usize,
    vib_depth: u32,
    /// The vibrato output; `None` when the channel period is used.
    vib_out: Option<i32>,
    trem_pos: usize,
    trem_speed: usize,
    trem_depth: u32,
    trem_base: u8,
    ft: u8,
    sample: u8,
    offpos: u32,
    newnote: bool,
    fresh: bool,
    saved: u32,
    cut: u8,
}

/// A logical output channel: what Paula is to play and the estimate of where it reads.
#[derive(Clone, Copy, Default)]
struct Log {
    per: u32,
    vol: u8,
    pos: u32,
    end: u32,
    lp: u32,
    last: u32,
}

pub struct BladePacker {
    d: Vec<u8>,
    lay: Layout,
    title: String,
    /// The sample file and its place in guest memory.
    smp: Vec<u8>,
    smp_addr: u32,
    tbl: usize,
    nsamp: usize,
    silent: u32,
    /// Subsong start positions: `starts[n]` for subsong n; `nsub` subsongs.
    starts: Vec<usize>,
    nsub: usize,
    sub: usize,
    songend: bool,
    // sequencer
    ppos: usize,
    row_ctr: u8,
    tick_ctr: u8,
    rt: u32,
    speed: u8,
    pdelay: u8,
    repeat: bool,
    jump: Option<usize>,
    rowptr: [usize; 8],
    slot_act: [Act; 4],
    vwave: [u8; 4],
    twave: [u8; 4],
    ch: [Ch; 4],
    // output
    lg: [Log; 4],
    /// The fifth logical channel: 0 not yet started, 1 voice 1 borrowed, 2 done.
    fifth: u8,
    ticks: u64,
    next_at: Option<u64>,
    /// The colour clock of the last init's DMACON write.
    init_at: u64,
    /// Colour clocks added to the first tick to reach the wanted raster phase.
    adj: u64,
}

fn note_of(c: u32) -> u8 {
    (c >> 24) as u8
}
fn samp_of(c: u32) -> u8 {
    (c >> 16) as u8
}
fn par_of(c: u32) -> u8 {
    c as u8
}

/// The subsong start table (spec 2.6): the number of subsongs and the table.
fn scan_subsongs(d: &[u8], lay: &Layout) -> (usize, Vec<usize>) {
    let mut starts = vec![0usize];
    for pos in 0..lay.positions {
        let mut hit = false;
        'p: for k in 0..lay.v {
            let trk = d.get(lay.list + pos * lay.v + k).copied().unwrap_or(0) as usize;
            let base = lay.pool + trk * 64 * lay.e;
            for r in 0..64 {
                let idx = row_index(d, lay, base + r * lay.e);
                let cmd = d.get(lay.dict + 4 * idx + 2).copied().unwrap_or(0);
                if cmd == 0x2C || cmd == 0x20 {
                    hit = true;
                    break 'p;
                }
            }
        }
        if hit {
            starts.push(pos + 1);
        }
    }
    let n = (starts.len() - 1).max(1);
    (n, starts)
}

fn row_index(d: &[u8], lay: &Layout, at: usize) -> usize {
    if lay.e == 1 {
        d.get(at).copied().unwrap_or(0) as usize
    } else {
        be16(d, at) as usize
    }
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = layout(d)?;
    let file = m.bus.read_cstr(m.deli.file_buf);
    let name = file.rsplit('/').next().unwrap_or(&file).to_string();
    if name.len() < 4 || !name[..4].eq_ignore_ascii_case("uds.") {
        return None;
    }
    let rest = &name[4..];
    let smp = [format!("smp.{rest}"), format!("SMP.{rest}")].iter().find_map(|n| crate::deli::read_companion(m, n).1.ok())?;
    let (tbl, nsamp) = sample_table(&smp)?;
    let smp_addr = m.alloc((smp.len() as u32 + 8).max(8));
    let silent = m.alloc(8);
    if smp_addr == 0 || silent == 0 {
        return None;
    }
    m.bus.write_bytes(smp_addr, &smp);
    let (nsub, starts) = scan_subsongs(d, &lay);
    let mut p = BladePacker {
        d: d.to_vec(),
        lay,
        title: name,
        smp,
        smp_addr,
        tbl,
        nsamp,
        silent,
        starts,
        nsub,
        sub: 0,
        songend: false,
        ppos: lay.list,
        row_ctr: 0,
        tick_ctr: 0,
        rt: 0,
        speed: 5,
        pdelay: 0,
        repeat: false,
        jump: None,
        rowptr: [0; 8],
        slot_act: [Act::None; 4],
        vwave: [0; 4],
        twave: [0; 4],
        ch: Default::default(),
        lg: [Log::default(); 4],
        fifth: 0,
        ticks: 0,
        next_at: None,
        init_at: 0,
        adj: 0,
    };
    p.init(0);
    let mut hw = Hw { bus: &mut m.bus };
    p.host_init(&mut hw);
    Some(Box::new(p))
}

impl BladePacker {
    /// The song-level init (spec 9.2) and the subsong part of InitSound (9.3).
    fn init(&mut self, n: usize) {
        let n = n.min(self.nsub - 1);
        self.sub = n;
        self.songend = false;
        self.vwave = [0; 4];
        self.twave = [0; 4];
        self.slot_act = [Act::None; 4];
        self.row_ctr = 0;
        self.tick_ctr = 0;
        self.rt = 0;
        self.speed = 5;
        self.pdelay = 0;
        self.repeat = false;
        self.ppos = self.lay.list;
        self.rowptr = [0; 8];
        self.ch = Default::default();
        self.jump = Some(self.starts.get(n).copied().unwrap_or(0));
        // the logical channels (8.1); the last programmed addresses and the fifth channel are kept
        for l in self.lg.iter_mut() {
            l.per = 0x11C;
            l.vol = 0;
            l.pos = self.silent;
            l.end = self.silent.wrapping_add(2);
            l.lp = 0;
        }
    }

    /// InitSound's Paula writes (spec 9.3) and the colour clock of the first tick.
    fn host_init(&mut self, hw: &mut Hw) {
        let t = hw.bus.clock;
        for (i, off) in [0u64, 10, 20, 30].iter().enumerate() {
            hw.bus.clock = t + off;
            hw.vol(i, 0);
        }
        hw.bus.clock = t + 42;
        let x = hw.bus.read_byte(0xBFE001);
        hw.bus.write_byte(0xBFE001, x | 2);
        hw.bus.clock = t + 52;
        hw.dmacon(0x000F);
        self.init_at = hw.bus.clock;
        // the first tick on the wanted raster phase, when one is set
        let due = hw.bus.clock + FIRST_TICK;
        self.adj = match PHASE.load(Ordering::Relaxed) {
            u64::MAX => 0,
            p => (p % LINE + LINE - due % LINE) % LINE,
        };
        self.ticks = 0;
    }

    /// EndSound (spec 9.4), as the host runs it after an init that is replaced by the next
    /// subsong's: right behind the init when no tick has run since, else now. The next init
    /// follows a host stepping gap later (9.5).
    fn host_end(&mut self, hw: &mut Hw) {
        let at = if self.ticks == 0 { self.init_at + 71 } else { hw.bus.clock };
        hw.bus.clock = hw.bus.clock.max(at);
        hw.dmacon(0x000F);
        for (i, off) in [8u64, 6, 6, 6].iter().enumerate() {
            hw.bus.clock += off;
            hw.vol(i, 0);
        }
        hw.bus.clock += 131_854;
    }

    // --- the sample file

    fn hdr(&self, s: u8) -> usize {
        self.tbl + 16 * (s as usize - 1)
    }
    fn sb(&self, o: usize) -> u8 {
        self.smp.get(o).copied().unwrap_or(0)
    }
    /// The guest address of a sample-file offset (spec 3.1).
    fn addr(&self, o: u32) -> u32 {
        self.smp_addr.wrapping_add(self.tbl as u32).wrapping_add(o).wrapping_sub(4)
    }

    // --- the song file

    fn cell(&self, idx: usize) -> u32 {
        be32(&self.d, self.lay.dict + 4 * idx)
    }

    fn song_end(&mut self) {
        self.songend = true;
    }

    // --- the sequencer

    /// Note start (spec 6.1).
    fn ns(&mut self, c: usize, cell: u32) {
        let (n, s) = (note_of(cell), samp_of(cell));
        if s as usize > self.nsamp {
            return; // undefined: the cell is ignored
        }
        self.ch[c].vib_out = None;
        self.ch[c].fresh = true;
        if n != 0 {
            self.ch[c].note = n;
        }
        if s != 0 {
            let h = self.hdr(s);
            let (off, vol, ft) = (be32(&self.smp, h), self.sb(h + 12), self.sb(h + 13));
            let ch = &mut self.ch[c];
            ch.sample = s;
            ch.offpos = off;
            ch.ft = ft;
            self.lg[c].vol = vol;
        }
        if n != 0 {
            self.ch[c].newnote = true;
            let (s, ft) = (self.ch[c].sample, self.ch[c].ft);
            if s == 0 || ft > 15 {
                return; // undefined: no voice
            }
            let h = self.hdr(s);
            let (end, lp) = (be32(&self.smp, h + 4), be32(&self.smp, h + 8));
            let start = self.addr(self.ch[c].offpos);
            let end = self.addr(end);
            self.lg[c].pos = start;
            self.lg[c].end = end;
            self.lg[c].lp = lp;
            let note = self.ch[c].note as i32;
            self.ch[c].period = ptab(ft as i32 * 36 + note / 2 - 1);
        }
    }

    fn vol(&self, c: usize) -> i32 {
        self.lg[c].vol as i32
    }

    fn load_position(&mut self) {
        let lay = self.lay;
        if self.ppos >= lay.list_end {
            self.ppos = lay.list + lay.restart * lay.v;
            self.song_end();
            // a restart past the end ends the list too: start from its beginning
            if self.ppos >= lay.list_end {
                self.ppos = lay.list;
            }
        }
        for k in 0..lay.v {
            let trk = self.d.get(self.ppos + k).copied().unwrap_or(0) as usize;
            self.rowptr[k] = lay.pool + trk * 64 * lay.e;
        }
        self.ppos += lay.v;
        self.row_ctr = 63;
    }

    fn row_tick(&mut self) {
        let lay = self.lay;
        if self.pdelay != 0 {
            if !self.repeat {
                for p in self.rowptr.iter_mut() {
                    *p = p.saturating_sub(lay.e);
                }
                self.repeat = true;
            }
            self.pdelay -= 1;
        } else {
            if self.repeat {
                for p in self.rowptr.iter_mut() {
                    *p += lay.e;
                }
                self.repeat = false;
            }
            if self.row_ctr != 0 {
                self.row_ctr -= 1;
            } else {
                self.load_position();
            }
        }
        self.tick_ctr = self.speed;
        self.rt = 0;
        let mut slot = 0usize;
        for c in 0..4 {
            let idx = row_index(&self.d, &lay, self.rowptr[c]);
            if !self.repeat {
                self.rowptr[c] += lay.e;
            }
            let mut cell = self.cell(idx);
            if self.repeat {
                cell &= 0x0000_FFFF;
            }
            let advance = self.row_action(c, cell, slot);
            if advance {
                slot += 1;
            }
        }
    }

    /// Runs the row action of command `cell` on channel `c` with handler slot `slot`; false when
    /// the slot index is not to advance (command 8).
    fn row_action(&mut self, c: usize, cell: u32, slot: usize) -> bool {
        let raw = (cell >> 8) as u8;
        if raw & 3 != 0 || raw >= 64 {
            // undefined: a row with no action
            if let Some(a) = self.slot_act.get_mut(slot) {
                *a = Act::None;
            }
            return true;
        }
        let cmd = raw / 4;
        let p = par_of(cell);
        let (x, y) = ((p >> 4) as i32, (p & 15) as i32);
        let slot = slot.min(3);
        let mut act = Act::None;
        match cmd {
            0 => {
                if p == 0 {
                    self.ns(c, cell);
                } else {
                    self.ns(c, cell);
                    act = Act::Arp;
                    self.build_arp(c, p);
                }
            }
            1 => {
                self.ns(c, cell);
                self.ch[c].slide = p as i32;
                act = Act::SlideDown;
            }
            2 => {
                self.ns(c, cell);
                self.ch[c].slide = p as i32;
                act = Act::SlideUp;
            }
            3 => {
                self.tone_row(c, cell);
                act = Act::Tone;
            }
            4 => {
                self.vib_row(c, cell, true);
                act = Act::Vib;
            }
            5 => {
                self.ch[c].down = y;
                self.ch[c].up = x;
                self.ns(c, cell);
                act = Act::ToneVol;
            }
            6 => {
                self.ch[c].down = y;
                self.ch[c].up = x;
                self.vib_row(c, cell, false);
                act = Act::VibVol;
            }
            7 => {
                let fresh = self.ch[c].fresh;
                if !fresh {
                    self.ns(c, cell);
                    if note_of(cell) != 0 {
                        self.ch[c].trem_pos = 0;
                    }
                    if samp_of(cell) != 0 {
                        self.ch[c].trem_base = self.lg[c].vol;
                    }
                } else {
                    self.ch[c].trem_pos = 0;
                    self.ns(c, cell);
                    self.ch[c].trem_base = self.lg[c].vol;
                }
                self.ch[c].fresh = false;
                if p != 0 {
                    self.ch[c].trem_speed = x as usize;
                    self.ch[c].trem_depth = y as u32;
                }
                act = Act::Trem;
            }
            8 => {
                self.song_end();
                self.jump = Some(self.starts.get(self.sub).copied().unwrap_or(0));
                self.ns(c, cell);
                return false;
            }
            9 => {
                self.ns(c, cell);
                let s = self.ch[c].sample;
                if s != 0 {
                    let h = self.hdr(s);
                    let (start, end) = (be32(&self.smp, h), be32(&self.smp, h + 4));
                    let mut pos = (p as u32 * 256).wrapping_add(start);
                    if pos >= end {
                        pos = start;
                    }
                    self.ch[c].offpos = pos;
                    if note_of(cell) != 0 {
                        self.lg[c].pos = self.addr(pos);
                    }
                }
            }
            10 => {
                self.ch[c].down = y;
                self.ch[c].up = x;
                self.ns(c, cell);
                act = Act::VolSlide;
            }
            11 => {
                self.row_ctr = 0;
                self.ppos = self.lay.list + p as usize * self.lay.v;
                self.song_end();
                self.ns(c, cell);
            }
            12 => {
                self.ns(c, cell);
                self.lg[c].vol = p;
            }
            13 => {
                if p == 0 {
                    self.row_ctr = 0;
                    self.ns(c, cell);
                } else {
                    self.row_ctr = 63u8.wrapping_sub(p);
                    let lay = self.lay;
                    if self.ppos >= lay.list_end {
                        self.ppos = lay.list + lay.restart * lay.v;
                        self.song_end();
                        if self.ppos >= lay.list_end {
                            self.ppos = lay.list;
                        }
                    }
                    for k in 0..lay.v {
                        let trk = self.d.get(self.ppos + k).copied().unwrap_or(0) as usize;
                        self.rowptr[k] = lay.pool + trk * 64 * lay.e + p as usize * lay.e;
                    }
                    self.ppos += lay.v;
                    self.ns(c, cell);
                }
            }
            14 => act = self.e_row(c, cell, slot),
            _ => {
                // F: speed
                self.speed = p.wrapping_sub(1);
                self.tick_ctr = self.speed;
                self.ns(c, cell);
            }
        }
        self.slot_act[slot] = act;
        true
    }

    fn e_row(&mut self, c: usize, cell: u32, slot: usize) -> Act {
        let p = par_of(cell);
        let (x, y) = (p >> 4, (p & 15) as i32);
        let mut act = Act::None;
        match x {
            1 => {
                self.ns(c, cell);
                self.ch[c].period = (self.ch[c].period + y).min(0x38B);
            }
            2 => {
                self.ns(c, cell);
                self.ch[c].period = (self.ch[c].period - y).max(0x6C);
            }
            4 => {
                self.vwave[slot] = y.min(2) as u8;
                self.ch[c].vib_out = None;
                self.ns(c, cell);
            }
            5 => {
                self.ns(c, cell);
                self.ch[c].ft = y as u8;
                self.ns(c, cell & 0xFF00_0000);
            }
            7 => {
                self.twave[slot] = y.min(2) as u8;
                self.ns(c, cell);
            }
            9 => {
                self.ch[c].saved = cell;
                self.ns(c, cell);
                act = Act::Retrig;
            }
            10 => {
                self.ns(c, cell);
                let v = self.lg[c].vol.wrapping_add(y as u8);
                self.lg[c].vol = if (v as i8) > 64 { 64 } else { v };
            }
            11 => {
                self.ns(c, cell);
                let v = self.lg[c].vol.wrapping_sub(y as u8);
                self.lg[c].vol = if (v as i8) < 0 { 0 } else { v };
            }
            12 => {
                self.ns(c, cell);
                self.ch[c].cut = 0;
                if y == 0 {
                    self.lg[c].vol = 0;
                } else {
                    self.ch[c].cut = y as u8;
                }
                act = Act::Cut;
            }
            13 => {
                self.ch[c].saved = cell;
                // the note field cleared; the sample number takes effect now
                self.ns(c, cell & 0x00FF_FFFF);
                act = Act::Delay;
            }
            14 => {
                self.ns(c, cell);
                if !self.repeat {
                    self.pdelay = y as u8;
                }
            }
            // 0, 3, 6, 8, F
            _ => self.ns(c, cell),
        }
        act
    }

    /// Builds the arpeggio list of channel `c` (spec 6.4, command 0).
    fn build_arp(&mut self, c: usize, p: u8) {
        let s = self.ch[c].sample;
        let r = if s == 0 { 0 } else { self.sb(self.hdr(s) + 17) as i32 };
        let n = self.ch[c].note as i32;
        let i0 = r * 36 + n / 2 - 1;
        let (x, y) = ((p >> 4) as i32, (p & 15) as i32);
        let base = ptab(i0);
        let ch = &mut self.ch[c];
        if (p as i8) >= 0x10 {
            if y != 0 {
                ch.arp = [base, ptab(i0 + x), ptab(i0 + y)];
                ch.arp_len = 3;
            } else {
                ch.arp = [ptab(i0 + x), base, 0];
                ch.arp_len = 2;
            }
        } else {
            ch.arp = [base, ptab(i0 + y), 0];
            ch.arp_len = 2;
        }
        ch.arp_cur = 1;
    }

    fn tone_row(&mut self, c: usize, cell: u32) {
        let (n, s, p) = (note_of(cell), samp_of(cell), par_of(cell));
        if n != 0 {
            self.ch[c].target = ptab(n as i32 / 2 - 1);
        }
        if p != 0 {
            self.ch[c].tone = p as i32;
        }
        if s != 0 && (s as usize) <= self.nsamp {
            self.lg[c].vol = self.sb(self.hdr(s) + 12);
        }
        self.ch[c].vib_out = None;
        self.ch[c].fresh = true;
    }

    fn vib_row(&mut self, c: usize, cell: u32, set: bool) {
        let keep = self.ch[c].vib_out;
        self.ns(c, cell);
        self.ch[c].vib_out = keep;
        if self.ch[c].newnote {
            self.ch[c].vib_pos = 0;
            self.ch[c].vib_out = None;
            self.ch[c].newnote = false;
        }
        let p = par_of(cell);
        if set && p != 0 {
            self.ch[c].vib_speed = (p >> 4) as usize;
            self.ch[c].vib_depth = (p & 15) as u32;
        }
    }

    // --- per-tick actions

    fn tone_step(&mut self, c: usize) {
        let ch = &mut self.ch[c];
        if ch.period < ch.target {
            ch.period = (ch.period + ch.tone).min(ch.target);
        } else {
            ch.period = (ch.period - ch.tone).max(ch.target);
        }
    }

    fn vib_step(&mut self, c: usize, slot: usize) {
        let ch = &mut self.ch[c];
        let off = ((wave(self.vwave[slot], ch.vib_pos) * ch.vib_depth) >> 7) as i32;
        let v = if ch.vib_pos < 31 { ch.period + off } else { ch.period - off };
        ch.vib_out = Some(v.clamp(0x6C, 0x38B));
        ch.vib_pos = (ch.vib_pos + ch.vib_speed) & 63;
    }

    fn vol_step(&mut self, c: usize) {
        let v = self.vol(c) + self.ch[c].up - self.ch[c].down;
        self.lg[c].vol = v.clamp(0, 64) as u8;
    }

    /// The per-tick action of handler slot `slot`, run on channel `slot` (spec 6.3).
    fn mid_action(&mut self, slot: usize) {
        let c = slot;
        let t = self.rt;
        match self.slot_act[slot] {
            Act::None => {}
            Act::Arp => {
                let ch = &mut self.ch[c];
                if ch.arp_len > 0 {
                    ch.arp_per = ch.arp[ch.arp_cur % ch.arp_len];
                    ch.arp_cur = (ch.arp_cur + 1) % ch.arp_len;
                }
            }
            Act::SlideDown => {
                let ch = &mut self.ch[c];
                ch.period = (ch.period - ch.slide).max(0x6C);
            }
            Act::SlideUp => {
                let ch = &mut self.ch[c];
                ch.period = (ch.period + ch.slide).min(0x38B);
            }
            Act::Tone => self.tone_step(c),
            Act::Vib => self.vib_step(c, slot),
            Act::ToneVol => {
                self.tone_step(c);
                self.vol_step(c);
            }
            Act::VibVol => {
                self.vib_step(c, slot);
                self.vol_step(c);
            }
            Act::Trem => {
                let ch = &mut self.ch[c];
                let delta = ((wave(self.twave[slot], ch.trem_pos) * ch.trem_depth) & 0xFFFF) >> 6;
                let v = if ch.trem_pos < 31 { ch.trem_base.wrapping_add(delta as u8) } else { ch.trem_base.wrapping_sub(delta as u8) };
                let v = if (v as i8) > 64 {
                    64
                } else if (v as i8) < 0 {
                    0
                } else {
                    v
                };
                ch.trem_pos = (ch.trem_pos + ch.trem_speed) & 63;
                self.lg[c].vol = v;
            }
            Act::VolSlide => self.vol_step(c),
            Act::Retrig => {
                let cell = self.ch[c].saved;
                let y = (par_of(cell) & 15) as u32;
                if y != 0 && t % y == 0 {
                    self.ns(c, cell);
                }
            }
            Act::Cut => {
                let ch = &mut self.ch[c];
                if ch.cut != 0 {
                    ch.cut -= 1;
                    if ch.cut == 0 {
                        self.lg[c].vol = 0;
                    }
                }
            }
            Act::Delay => {
                let cell = self.ch[c].saved;
                let y = (par_of(cell) & 15) as u32;
                if y != 0 && t == y && note_of(cell) != 0 {
                    self.ns(c, cell);
                }
            }
        }
    }

    fn sequencer(&mut self) {
        if let Some(start) = self.jump.take() {
            for l in self.lg.iter_mut() {
                l.vol = 0;
            }
            self.row_ctr = 0;
            self.tick_ctr = 0;
            self.ppos = self.lay.list + start * self.lay.v;
        }
        let mid = self.tick_ctr != 0;
        if mid {
            self.tick_ctr -= 1;
            self.rt += 1;
            for slot in 0..4 {
                self.mid_action(slot);
            }
        } else {
            self.row_tick();
        }
        // period selection (spec 6.6)
        for c in 0..4 {
            let ch = &self.ch[c];
            let p = if mid && self.slot_act[c] == Act::Arp {
                ch.arp_per
            } else if let Some(v) = ch.vib_out {
                v
            } else {
                ch.period
            };
            self.lg[c].per = p as u32;
        }
    }

    // --- the output stage (spec 8, timing 10)

    fn put(hw: &mut Hw, t2: u64) {
        hw.bus.clock = t2 / 2;
    }

    fn output(&mut self, hw: &mut Hw) {
        let call = hw.bus.clock;
        // the fifth channel (8.3)
        let mut borrowed = false;
        let mut extra: u64 = 0;
        match self.fifth {
            0 => {
                borrowed = true;
                extra = 120;
                self.fifth = 1;
            }
            1 => {
                self.lg[0].last = 0;
                extra = 30;
                self.fifth = 2;
            }
            _ => {}
        }
        const VOICE: [usize; 4] = [0, 1, 3, 2];
        let mut st = [false; 4];
        let (mut s, mut l) = (0u64, 0u64);
        for h in 0..4 {
            let lc = VOICE[h];
            st[h] = self.lg[lc].pos != self.lg[lc].last || (h == 0 && borrowed);
            if st[h] {
                s += 1;
                if self.lg[lc].lp != 0 {
                    l += 1;
                }
            }
        }
        let mask: u16 = (0..4).filter(|&h| st[h]).fold(0, |a, h| a | 1 << h);
        let first = st.iter().position(|&b| b);
        let last = st.iter().rposition(|&b| b);
        let spread = if s > 0 { (153 * s + 60 * l) / 2 } else { 0 };
        let t0 = call + spread + extra;
        Self::put(hw, 2 * t0);
        hw.dmacon(mask);
        let mut t2 = 2 * t0;
        for (h, (a, b)) in [(56u64, 82u64), (24, 86), (23, 81), (24, 86)].iter().enumerate() {
            let lc = VOICE[h];
            t2 += 2 * a;
            Self::put(hw, t2);
            hw.per(h, self.lg[lc].per as u16);
            t2 += 2 * b;
            Self::put(hw, t2);
            hw.vol(h, self.lg[lc].vol as u16);
        }
        let tvol = t2 / 2;
        let p1 = match first {
            None => 112,
            Some(h) => [63, 78, 93, 108][h],
        };
        let w1 = LINE * (tvol + 1853).div_ceil(LINE) + p1;
        t2 = 2 * w1;
        // address group 1
        let mut prev: Option<usize> = None;
        for h in 0..4 {
            if !st[h] {
                continue;
            }
            if let Some(pv) = prev {
                t2 += 102 + 29 * (h - pv - 1) as u64;
            }
            let lc = VOICE[h];
            let a = self.lg[lc].pos & !1;
            Self::put(hw, t2);
            hw.loc(h, a);
            t2 += 20;
            Self::put(hw, t2);
            hw.len(h, (self.lg[lc].end.wrapping_sub(a) / 2) as u16);
            prev = Some(h);
        }
        match last {
            Some(v) => t2 += 80 + 29 * (3 - v) as u64,
            None => {}
        }
        Self::put(hw, t2);
        hw.dmacon(0x8000 | mask);
        let p2 = match first {
            None => 110,
            Some(h) => [61, 77, 92, 106][h],
        };
        let w2 = LINE * (t2 / 2 + 1847).div_ceil(LINE) + p2;
        t2 = 2 * w2;
        // address group 2
        let mut prev: Option<usize> = None;
        for h in 0..4 {
            if !st[h] {
                continue;
            }
            if let Some(pv) = prev {
                t2 += 45 + 29 * (h - pv - 1) as u64;
            }
            let lc = VOICE[h];
            let lg = self.lg[lc];
            let (a, w) = if lg.lp != 0 { (lg.end.wrapping_sub(lg.lp), ((lg.lp & 0xFFFF) / 2) as u16) } else { (self.silent, 1) };
            Self::put(hw, t2);
            hw.loc(h, a);
            t2 += 20;
            Self::put(hw, t2);
            hw.len(h, w);
            prev = Some(h);
        }
        if let Some(v) = last {
            t2 += 24 + 29 * (3 - v) as u64;
        }
        Self::put(hw, t2);
        hw.dmacon(0x8000 | mask);
        // advance the logical positions (8.4)
        for lg in self.lg.iter_mut() {
            let step = if lg.per == 0 { 70_938 } else { (70_938 / lg.per) & 0xFFFF };
            let pos = lg.pos.wrapping_add(step);
            if pos < lg.end {
                lg.pos = pos;
            } else if lg.lp != 0 {
                let over = (pos - lg.end) / lg.lp + 1;
                lg.pos = pos.wrapping_sub(over.wrapping_mul(lg.lp));
            } else {
                lg.pos = self.silent;
                lg.end = self.silent.wrapping_add(2);
            }
            lg.last = lg.pos;
        }
    }
}

impl Replayer for BladePacker {
    fn format(&self) -> &'static str {
        "Blade Packer"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn timer_delay(&self) -> u64 {
        // the engine counts one period from the clock after `new`
        FIRST_TICK - TICK + self.adj
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.nsub as u16 - 1, self.sub as u16)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.host_end(hw);
        self.init(n as usize);
        self.host_init(hw);
        self.next_at = Some(hw.bus.clock + FIRST_TICK + self.adj);
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.next_at.take()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.output(hw);
        self.sequencer();
        self.ticks += 1;
    }
}

#[cfg(test)]
#[path = "bladepacker_tests.rs"]
mod tests;
