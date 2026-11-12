//! Digital Sound Creations, the sample sequencer of Andrew Bailey and David M. Hanlon's game
//! music (`han.*`): the Populous modules (one file with its samples) and the DTRK songs of Fusion
//! and Enlightenment (`han.<name>` with the IFF kit its header names and the 8SVX bank
//! `smp.<name>` beside it). Not Digital Sonix & Chrome (`dsc.rs`), the same authors' other format.
//!
//! Clean-room, written from the behaviour specification `docs/specs/digitalsoundcreations/SPEC.md`
//! and black-box runs; no original code, and no byte of any player. The specification was drawn
//! up from observation only: the audio.device requests and the Paula register writes of the
//! reference (the Wanted Team Eagleplayer "Digital Sound Creations" on this project's 68000
//! engine), when the reference's calls happen, and how they change when bytes of the module, the
//! kit or the bank are changed (built modules). Nothing of the player was disassembled or read.
//!
//! A song is a grid of 64 patterns of four tracks of up to 64 rows (one byte a row: a space, or
//! a note naming one of 104 instrument entries), a sequence of (pattern, count) entries and the
//! pattern lengths. A row's note starts its instrument's sample once, unless the track's sample
//! still plays louder; an instrument of a looped sample plays it over and over. Track k plays on
//! channel k.
//!
//! The reference plays through audio.device: a note is a CheckIO, an AbortIO and a CMD_WRITE of
//! one pass (and for a looped sample a second CMD_WRITE of no end on the same request, which
//! queues behind the first and so makes the whole sample repeat). This port keeps the requests'
//! state as the engine's audio.device does (`os.rs`) and makes the register writes it makes at
//! the same moments: the sample's registers and DMA at the request, period and volume when the
//! engine sees the channel's audio interrupt (after each output sample, vertical blank and tick:
//! the port is called then through `next_tick_at`), the DMA off when a one-pass request has
//! played. The times of the reference's calls within a tick are a model in 68000 cycles (SPEC
//! section 7), exact over the six songs' 42000 call gaps.
//!
//! Choices where the specification is silent (none can panic or hang): a pattern number above
//! 63 plays 64 empty rows; rows past a track's 64 read the next track's (the grid is one block),
//! past the grid nothing; a pattern length 0 plays one row; an instrument rate whose low word is
//! 0 (the original stops with a division by zero) takes the period an overflowing division gives;
//! a kit or entry table shorter than 104 entries reads zeros past the file; the loop request's
//! sample (never played) is the sample from the instrument's loop offset; the time of a
//! loop request on track 2 or 3 is extrapolated from tracks 0 and 1.

use super::{Hw, Open, Replayer, be16, be32};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Digital Sound Creations", detect, new };

/// The grid: 64 patterns of four tracks of 64 rows.
const GRID: usize = 16384;
/// A row's byte `c` is a note when `c - 0x41` (a byte) is below this.
const ENTRIES: usize = 104;
/// Colour clocks between two Populous ticks (50 Hz).
const POP_TICK: u64 = 70940;
/// The constant of a DTRK instrument's period (`K / rate`, the NTSC colour clock).
const NTSC: u32 = 3_579_545;
/// The constant of a DTRK tick: `5 * (DTRK_TEMPO / (tempo + 30) - 176)` colour clocks.
const DTRK_TEMPO: u64 = 7_499_855;

/// The 68000 cycles of the reference's tick between its calls (SPEC section 7): a segment between
/// two host calls takes `floor(cycles / 2)` colour clocks.
mod cyc {
    /// A tick without a row, start to end.
    pub const IDLE: u64 = 444;
    /// A row tick's start to track 0.
    pub const ROW: u64 = 681;
    /// An empty track to the next (track 3: to the row's end).
    pub const EMPTY: [u64; 4] = [241, 241, 261, 184];
    /// CheckIO of a skipped note to the next track.
    pub const SKIP: [u64; 4] = [568, 568, 588, 510];
    /// CheckIO to AbortIO: the request had ended, or was still held.
    pub const ABORT_IDLE: u64 = 108;
    pub const ABORT_HELD: u64 = 182;
    /// AbortIO to the CMD_WRITE.
    pub const WRITE: [u64; 4] = [1276, 1366, 1466, 1508];
    /// The CMD_WRITE to the next track.
    pub const AFTER: [u64; 4] = [499, 499, 518, 442];
    /// The first CMD_WRITE of a looped sample to the second (tracks 2 and 3 extrapolated).
    pub const LOOP: [u64; 4] = [1262, 1352, 1452, 1494];
    /// The second CMD_WRITE to the next track (tracks 2 and 3 extrapolated).
    pub const AFTER_LOOP: [u64; 4] = [478, 478, 497, 421];
    /// An instrument without a sample: AbortIO to the next track is this much shorter than
    /// `WRITE + AFTER` (no CMD_WRITE; measured on track 0).
    pub const NO_SAMPLE: u64 = 127;
    /// The row's end: the pattern goes on to the next sequence entry, or repeats.
    pub const NEXT: u64 = 266;
    pub const REPEAT: u64 = 216;
    /// The song's end: to the host's song-end call, and from it to the tick's end.
    pub const END: u64 = 42;
    pub const END_AFTER: u64 = 390;
}

/// What both kinds share once read.
#[derive(Clone, Debug)]
struct Song {
    head: [u16; 64],
    /// (pattern, count) entries.
    seq: Vec<(u16, u16)>,
    grid: Vec<u8>,
    /// Ticks a row.
    speed: u8,
    /// Colour clocks a tick.
    tick: u64,
}

/// A Populous module's layout (SPEC section 2): the offsets of its tables.
struct PopLay {
    entries: usize,
    samples: usize,
    nsamples: usize,
    data: usize,
}

fn pop_layout(d: &[u8]) -> Option<(Song, PopLay)> {
    if d.len() < 0x82 {
        return None;
    }
    let n = be16(d, 0x80) as usize;
    let mut o = 0x82 + 4 * n;
    if be16(d, o) != 0 || be16(d, o + 2) as usize != GRID {
        return None;
    }
    o += 4;
    let grid = d.get(o..o + GRID)?.to_vec();
    o += GRID;
    let ne = be16(d, o) as usize;
    let entries = o + 2;
    o = entries + 8 * ne;
    let ns = be16(d, o) as usize;
    let samples = o + 2;
    o = samples + 8 * ns;
    let data = o + 4;
    let end = data.checked_add(be32(d, o) as usize)?;
    if o + 4 > d.len() || end + 2 != d.len() || ns == 0 {
        return None;
    }
    let head = std::array::from_fn(|i| be16(d, 2 * i));
    let mut seq: Vec<(u16, u16)> = (0..n).map(|i| (be16(d, 0x82 + 4 * i), be16(d, 0x84 + 4 * i))).collect();
    if seq.is_empty() {
        // no entry: pattern 0 once
        seq.push((0, 1));
    }
    let song = Song { head, seq, grid, speed: 10, tick: POP_TICK };
    Some((song, PopLay { entries, samples, nsamples: ns, data }))
}

/// A DTRK song's chunks (SPEC section 3): the THDR body's offset and the song.
fn dtrk_layout(d: &[u8]) -> Option<(Song, usize)> {
    if d.len() < 12 || &d[0..4] != b"FORM" || &d[8..12] != b"DTRK" {
        return None;
    }
    let (mut thdr, mut grid, mut seqn, mut mlen) = (None, None, None, None);
    let mut o = 12;
    // chunks are not padded
    while o + 8 <= d.len() {
        let n = be32(d, o + 4) as usize;
        let body = o + 8;
        let Some(b) = body.checked_add(n).and_then(|e| d.get(body..e)) else { break };
        match &d[o..o + 4] {
            b"THDR" => thdr = Some((body, n)),
            b"BODY" => grid = Some(b),
            b"SEQN" => seqn = Some(b),
            b"MLEN" => mlen = Some(b),
            _ => {}
        }
        o = body + n;
    }
    let (thdr, tn) = thdr?;
    let grid = grid?;
    let (seqn, mlen) = (seqn?, mlen?);
    if tn < 41 || grid.len() != GRID || mlen.len() < 128 || seqn.len() < 3 {
        return None;
    }
    let head = std::array::from_fn(|i| be16(mlen, 2 * i));
    let seq = seqn.chunks_exact(3).map(|e| (be16(e, 0), e[2] as u16)).collect();
    // the tempo: bit 7 of the header's byte 37 is its bit 8, byte 40 its low byte
    let t = if d[thdr + 37] & 0x80 != 0 { 256 } else { 0 } + d[thdr + 40] as u64;
    let latch = (DTRK_TEMPO / (t + 30) - 176) % 65536;
    let tick = 5 * if latch == 0 { 65536 } else { latch };
    Some((Song { head, seq, grid: grid.to_vec(), speed: 4, tick }, thdr))
}

fn detect(d: &[u8]) -> bool {
    pop_layout(d).is_some() || dtrk_layout(d).is_some()
}

/// The NUL-terminated kit name at the start of the THDR body.
fn kit_name(d: &[u8], thdr: usize) -> String {
    let b = &d[thdr..(thdr + 32).min(d.len())];
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..n]).into_owned()
}

/// The bank's name: `smp.` and the song's name less its `han.` prefix.
fn bank_name(path: &str) -> String {
    let file = path.rsplit(['/', ':']).next().unwrap_or(path);
    let rest = if file.len() > 4 && file.is_char_boundary(4) && file[..4].eq_ignore_ascii_case("han.") {
        &file[4..]
    } else {
        file
    };
    format!("smp.{rest}")
}

/// The bank's sample table (SPEC section 3.3): each FORM's BODY data, in file order. Where a
/// FORM's length does not end on the next FORM (Druid_game's eighth sample stops 4 bytes short),
/// the table has an empty slot (a note of it plays nothing) and goes on at the next FORM on an
/// even offset. A FORM without a BODY is an empty slot too.
fn bank_samples(b: &[u8]) -> Vec<Option<usize>> {
    let mut out = Vec::new();
    let mut o = 0;
    while o + 12 <= b.len() && &b[o..o + 4] == b"FORM" && out.len() < 256 {
        let n = be32(b, o + 4) as usize;
        let end = (o + 8).saturating_add(n).min(b.len());
        let mut p = o + 12;
        let mut body = None;
        while p + 8 <= end {
            let cn = be32(b, p + 4) as usize;
            if &b[p..p + 4] == b"BODY" {
                body = Some(p + 8);
                break;
            }
            p = (p + 8).saturating_add(cn).saturating_add(cn & 1);
        }
        out.push(body);
        o = (o + 8).saturating_add(n).saturating_add(n & 1);
        if o + 4 <= b.len() && &b[o..o + 4] != b"FORM" {
            while o + 4 <= b.len() && &b[o..o + 4] != b"FORM" {
                o += 2;
            }
            if o + 12 <= b.len() {
                out.push(None);
            }
        }
    }
    out
}

/// The kit's entry table: the BODY chunk of a FORM DKIT.
fn kit_body(k: &[u8]) -> Option<usize> {
    if k.len() < 12 || &k[0..4] != b"FORM" || &k[8..12] != b"DKIT" {
        return None;
    }
    let mut o = 12;
    while o + 8 <= k.len() {
        if &k[o..o + 4] == b"BODY" {
            return Some(o + 8);
        }
        o = (o + 8).saturating_add(be32(k, o + 4) as usize);
    }
    None
}

/// An instrument's request.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Note {
    /// `None`: no sample, the note only aborts the track's request.
    sample: Option<(u32, u32)>,
    per: u16,
    vol: u8,
    /// The sample repeats: a second request from this offset.
    looped: Option<u32>,
}

/// A DTRK instrument's period: the colour clock over the rate's low word (an overflowing
/// division leaves the dividend's low word, as the 68000's).
fn dtrk_period(rate: u32) -> u16 {
    let r = rate & 0xFFFF;
    let q = if r == 0 { u32::MAX } else { NTSC / r };
    if q > 0xFFFF {
        NTSC as u16
    } else {
        q as u16
    }
}

fn pop_notes(d: &[u8], lay: &PopLay, base: u32) -> Vec<Note> {
    // the samples lie one after the other from the data's start, by their lengths
    let mut starts = Vec::with_capacity(lay.nsamples);
    let mut at = base.wrapping_add(lay.data as u32);
    for s in 0..lay.nsamples {
        starts.push(at);
        at = at.wrapping_add(be16(d, lay.samples + 8 * s) as u32);
    }
    (0..ENTRIES)
        .map(|i| {
            let e = lay.entries + 8 * i;
            let (per, len, vol, s, lo) = (be16(d, e), be16(d, e + 2), d.get(e + 4).copied().unwrap_or(0), d.get(e + 5).copied().unwrap_or(0) as usize, be16(d, e + 6));
            if s >= lay.nsamples {
                return Note { sample: None, per, vol, looped: None };
            }
            let se = lay.samples + 8 * s;
            let at = starts[s];
            let looped = (be16(d, se + 2) == 0).then_some(lo as u32);
            Note { sample: Some((at, len as u32)), per, vol, looped }
        })
        .collect()
}

fn dtrk_notes(kit: &[u8], body: usize, samples: &[Option<usize>], bank_addr: u32) -> Vec<Note> {
    (0..ENTRIES)
        .map(|i| {
            let e = body + 44 * i;
            let (rate, len) = (be32(kit, e + 32), be32(kit, e + 36));
            let (vol, smp, lo) = (kit.get(e + 40).copied().unwrap_or(0), kit.get(e + 41).copied().unwrap_or(0), be16(kit, e + 42));
            let per = dtrk_period(rate);
            // sample 0 is the first, as sample 1
            let at = samples.get((smp as usize).saturating_sub(1)).copied().flatten().map(|o| bank_addr.wrapping_add(o as u32));
            Note { sample: at.map(|a| (a, len)), per, vol, looped: (lo != 0).then_some(lo as u32) }
        })
        .collect()
}

/// One track's audio.device request and its channel, as `os.rs` keeps them.
#[derive(Clone, Copy, Default)]
struct Chan {
    /// The request is in the channel's registers (for Paula's next reload).
    latched: bool,
    /// Its pass plays.
    playing: bool,
    /// It waits behind itself (the second request of a looped sample).
    queued: bool,
    passes: u32,
    data: u32,
    len: u32,
    per: u16,
    vol: u8,
    cycles: u16,
}

impl Chan {
    /// CheckIO says pending.
    fn held(&self) -> bool {
        self.latched || self.playing || self.queued
    }
    fn busy(&self) -> bool {
        self.latched || self.playing
    }
}

/// The reference's clock within a tick: the colour clock of its last host call, and the 68000
/// cycles since.
struct Clock {
    at: u64,
    cycles: u64,
}

impl Clock {
    fn add(&mut self, c: u64) {
        self.cycles += c;
    }
    /// A host call: the clock moves on by the cycles' whole colour clocks.
    fn call(&mut self, hw: &mut Hw) {
        self.at += self.cycles / 2;
        self.cycles = 0;
        let now = hw.bus.clock;
        if self.at > now {
            hw.wait(self.at - now);
        }
    }
}

pub struct DigitalSoundCreations {
    song: Song,
    notes: Vec<Note>,
    ch: [Chan; 4],
    /// Sequence entry, row, the play counter (never reset), ticks since the last row.
    pos: usize,
    row: u16,
    count: u32,
    wait: u8,
    songend: bool,
    /// The next music tick, once the first has come.
    music_at: Option<u64>,
    /// When the engine is to call next.
    next: Option<u64>,
}

impl DigitalSoundCreations {
    fn build(song: Song, notes: Vec<Note>) -> Self {
        DigitalSoundCreations {
            song,
            notes,
            ch: [Chan::default(); 4],
            pos: 0,
            row: 0,
            count: 0,
            wait: 0,
            songend: false,
            music_at: None,
            next: None,
        }
    }

    fn latch(&mut self, k: usize, hw: &mut Hw) {
        let c = &self.ch[k];
        let r = 0xDFF0A0 + 16 * k as u32;
        hw.bus.write_word(r, (c.data >> 16) as u16);
        hw.bus.write_word(r + 2, c.data as u16);
        hw.bus.write_word(r + 4, (c.len / 2) as u16);
        self.ch[k].latched = true;
    }

    /// CMD_WRITE on track `k`'s request.
    fn write(&mut self, k: usize, (data, len): (u32, u32), per: u16, vol: u8, cycles: u16, hw: &mut Hw) {
        let busy = self.ch[k].busy();
        let latched = self.ch[k].latched;
        let c = &mut self.ch[k];
        (c.data, c.len, c.per, c.vol, c.cycles) = (data, len, per, vol, cycles);
        if !busy {
            self.latch(k, hw);
            hw.dmacon(1 << k);
            hw.dmacon(0x8000 | 1 << k);
        } else if !latched {
            self.latch(k, hw);
        } else {
            self.ch[k].queued = true;
        }
    }

    /// AbortIO on track `k`'s request.
    fn abort(&mut self, k: usize, hw: &mut Hw) {
        let c = &mut self.ch[k];
        if !c.held() {
            return;
        }
        c.queued = false;
        c.latched = false;
        if c.playing {
            c.playing = false;
            hw.dmacon(1 << k);
        }
    }

    /// The engine's audio.device service: the channels whose audio interrupt is requested.
    fn service(&mut self, hw: &mut Hw) {
        for k in 0..4 {
            let bit = 0x80u16 << k;
            if hw.bus.paula.intreq & bit == 0 || !self.ch[k].busy() {
                continue;
            }
            hw.bus.paula.write_intreq(bit);
            if self.ch[k].latched {
                if !self.ch[k].playing {
                    let (per, vol) = (self.ch[k].per, self.ch[k].vol);
                    hw.per(k, per);
                    hw.vol(k, vol as u16);
                    self.ch[k].playing = true;
                    self.ch[k].passes = 0;
                }
                let c = &mut self.ch[k];
                c.passes += 1;
                if c.cycles != 0 && c.passes >= c.cycles as u32 {
                    c.latched = false;
                    if c.queued {
                        c.queued = false;
                        self.latch(k, hw);
                    }
                }
            } else {
                hw.dmacon(1 << k);
                hw.bus.paula.write_intreq(bit);
                self.ch[k].playing = false;
            }
        }
    }

    /// The pattern of the current sequence entry (above 63: none).
    fn pattern(&self) -> (Option<usize>, u16) {
        let p = self.song.seq[self.pos].0 as usize;
        if p < 64 {
            (Some(p), self.song.head[p])
        } else {
            (None, 64)
        }
    }

    fn music(&mut self, hw: &mut Hw) {
        let mut t = Clock { at: hw.bus.clock, cycles: 0 };
        self.wait += 1;
        if self.songend || self.wait < self.song.speed {
            t.add(cyc::IDLE);
            t.call(hw);
            return;
        }
        self.wait = 0;
        t.add(cyc::ROW);
        let (p, len) = self.pattern();
        for k in 0..4 {
            let c = p.map_or(0x20, |p| self.song.grid.get(p * 256 + k * 64 + self.row as usize).copied().unwrap_or(0x20));
            let i = c.wrapping_sub(0x41) as usize;
            if i >= ENTRIES {
                t.add(cyc::EMPTY[k]);
                continue;
            }
            t.call(hw); // CheckIO
            let n = self.notes[i];
            let held = self.ch[k].held();
            if held && n.vol < self.ch[k].vol {
                t.add(cyc::SKIP[k]);
                continue;
            }
            t.add(if held { cyc::ABORT_HELD } else { cyc::ABORT_IDLE });
            t.call(hw);
            self.abort(k, hw);
            let Some((data, slen)) = n.sample else {
                t.add(cyc::WRITE[k] + cyc::AFTER[k] - cyc::NO_SAMPLE);
                continue;
            };
            t.add(cyc::WRITE[k]);
            t.call(hw);
            self.write(k, (data, slen), n.per, n.vol, 1, hw);
            match n.looped {
                Some(lo) => {
                    t.add(cyc::LOOP[k]);
                    t.call(hw);
                    self.write(k, (data.wrapping_add(lo), slen.wrapping_sub(lo)), n.per, n.vol, 0, hw);
                    t.add(cyc::AFTER_LOOP[k]);
                }
                None => t.add(cyc::AFTER[k]),
            }
        }
        // the row's end (SPEC section 5)
        if self.row as u32 + 1 >= len as u32 {
            self.count = self.count.wrapping_add(1);
            self.row = 0;
            if self.count < self.song.seq[self.pos].1 as u32 {
                t.add(cyc::REPEAT);
            } else if self.pos + 1 >= self.song.seq.len() {
                t.add(cyc::END);
                t.call(hw);
                self.songend = true;
                t.add(cyc::END_AFTER);
            } else {
                t.add(cyc::NEXT);
                self.pos += 1;
            }
        } else {
            self.row += 1;
        }
        t.call(hw);
    }
}

impl Replayer for DigitalSoundCreations {
    fn format(&self) -> &'static str {
        "Digital Sound Creations"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        self.song.tick
    }

    fn next_tick_at(&self) -> Option<u64> {
        self.next
    }

    fn songend(&self) -> bool {
        self.songend
    }

    /// Called at each music tick and, while a request is held, at each moment the engine's
    /// audio.device would look at the channels (SPEC section 6).
    fn tick(&mut self, hw: &mut Hw) {
        let now = hw.bus.clock;
        let due = *self.music_at.get_or_insert(now);
        if now >= due {
            self.music(hw);
            self.music_at = Some(due + self.song.tick);
        }
        self.service(hw);
        let now = hw.bus.clock;
        hw.bus.paula.sync(now, &hw.bus.ram);
        let mut next = self.music_at.unwrap_or(now);
        if self.ch.iter().any(|c| c.busy()) {
            let sample = hw.bus.paula.now() + hw.bus.paula.out.until_sample() as u64;
            // the engine's vertical blanks: the first a frame after its start, then each frame
            let f = hw.bus.frame.max(1);
            let first = crate::bus::FRAME;
            let vblank = if now < first { first } else { first + ((now - first) / f + 1) * f };
            next = next.min(sample).min(vblank);
        }
        self.next = Some(next);
    }
}

pub(super) fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if let Some((song, lay)) = pop_layout(d) {
        let notes = pop_notes(d, &lay, base);
        return Some(Box::new(DigitalSoundCreations::build(song, notes)));
    }
    let (song, thdr) = dtrk_layout(d)?;
    let file = m.bus.read_cstr(m.deli.file_buf);
    let kit = crate::deli::read_companion(m, &kit_name(d, thdr)).1.ok()?;
    let body = kit_body(&kit)?;
    let bank = crate::deli::read_companion(m, &bank_name(&file)).1.ok()?;
    let samples = bank_samples(&bank);
    if samples.iter().all(|s| s.is_none()) {
        return None;
    }
    let bank_addr = m.alloc(bank.len() as u32 + 2);
    if bank_addr == 0 {
        return None;
    }
    m.bus.write_bytes(bank_addr, &bank);
    let notes = dtrk_notes(&kit, body, &samples, bank_addr);
    Some(Box::new(DigitalSoundCreations::build(song, notes)))
}

#[cfg(test)]
#[path = "digitalsoundcreations_tests.rs"]
mod tests;
