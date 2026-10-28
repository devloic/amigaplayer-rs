//! Oktalyzer (`OKTASONG`), ported from NostalgicPlayer's Oktalyzer player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! Oktalyzer has 4 to 8 channels on Paula's 4: a channel is single (one pattern
//! column, played by Paula as it is, with its loop) or mixed (two columns, played
//! together by software, with no loop and 7 bit samples). The Eagleplayer does
//! not mix either, and neither does this port. With no mixed channel it writes
//! Paula's registers itself. With one it asks for the host's 8 Bit Amplifier with
//! 8 AudioStructs, pokes the single channels (`ENPP_Poke*`, here `Hw::voice_*`)
//! and fills the AudioStructs of the mixed ones: voices 0 to 3 are the first
//! column of each Paula channel and 4 to 7 the second of the mixed ones, the
//! voices of Paula's channels 0 and 3 on the left and of 1 and 2 on the right,
//! and the host's mixer (`amplifier.rs`) plays them. The 7 bit samples are not
//! scaled up (half as loud as 8 bit ones), as the original leaves them.
//!
//! Where the Eagleplayer differs from NostalgicPlayer, this follows it (checked
//! against the Eagleplayer's register writes and AudioStructs, tick by tick):
//! - the song's first row comes after `speed` ticks, as a tick's work is done
//!   a tick before the single channels' registers go out (the mixed channels'
//!   AudioStructs change at once, so a mixed note starts a tick before a single
//!   one on the same row);
//! - a sample with no loop is followed by its last word (the loop, a word, at
//!   the sample's end) where it has a loop of one word, else by a word of
//!   silence, and the release effect (R) writes nothing;
//! - the note effects (H, L, D, U) keep the note unclamped and play it within the
//!   table (a mixed voice whose note is above the table stops); with the
//!   amplifier the note stops at the table's bottom, with Paula written directly
//!   it goes below it;
//! - the LED filter is left as the engine has it until an F effect.
//!
//! The mixed channels' notes reach the amplifier's buffers as the original's do
//! only when the ticks have the original's phase against them. That depends on
//! the time the Eagleplayer takes to go through the module's patterns before it
//! starts (fitted, not reproduced), so a note may reach the mixer a buffer early
//! or late; the AudioStructs' contents are the original's.

use super::{be16, be32, Hw, Open, Replayer};
use crate::amplifier::aus;
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Oktalyzer", detect, new };

const PERIODS: [i16; 36] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240,
    226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];
// Colour clocks, as the Eagleplayer's runs take them under the 68000-timed engine.
/// From InitSound's writes to the first tick, with Paula written directly.
const T_FIRST: u64 = 93_793;
/// With the amplifier (some channel mixed): the Eagleplayer starts the amplifier,
/// does InitSound's writes, and the engine restarts the amplifier after them.
/// From the writes to that restart, as measured (the median of 213 modules; the
/// range is 19 000 to 30 000); then the first tick comes a tick later, as the
/// timer starts at the restart, and its writes follow a tick's own work (1 160
/// colour clocks, as measured), which a tick here does not do, so that its
/// AudioStructs' changes reach the mixer in the buffer they reach in the original.
const T_RESTART: u64 = 23_181;
const T_FIRST_AMP: u64 = super::DT_TICK;
/// The time between the amplifier's start and the writes is InitPlayer's, which
/// goes through the patterns: fitted over the 213 modules with a mixed channel
/// (rms 350 000 colour clocks of 1 to 3 700 000), so the phase of the ticks
/// against the amplifier's buffers, which that time sets, is not the original's.
const T_INIT: u64 = 180_000;
const T_INIT_CELL: u64 = 66;
const ARP10: [i8; 16] = [0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0];
const ARP12: [i8; 16] = [0, 1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3];

fn detect(d: &[u8]) -> bool {
    d.len() >= 1368 && d.starts_with(b"OKTASONG")
}

#[derive(Clone, Default)]
struct Sample {
    length: u32,
    rep_start: u32,
    rep_len: u32,
    volume: u8,
    mode: u16,
    /// Offset of its data in the module (chunk size bytes), if it has any.
    data: Option<(usize, usize)>,
}

#[derive(Clone, Copy, Default)]
struct Line {
    note: u8,
    sample: u8,
    effect: u8,
    arg: u8,
}

struct Pattern {
    lines: usize,
    data: Vec<Line>,
}

struct Song {
    mixed: [bool; 4],
    channels: usize,
    start_speed: u16,
    song_len: u16,
    /// Lines of all patterns times channels.
    cells: u64,
    table: Vec<u8>,
    samples: Vec<Sample>,
    patterns: Vec<Pattern>,
}

fn load(d: &[u8]) -> Option<Song> {
    let mut mixed = None;
    let mut channels = 0;
    let mut samples: Option<Vec<Sample>> = None;
    let (mut start_speed, mut patt_num, mut song_len, mut table) = (6u16, 0u16, 0u16, None::<Vec<u8>>);
    let mut patterns: Option<Vec<Pattern>> = None;
    let (mut read_patt, mut read_samp) = (0usize, 0usize);
    let mut p = 8;
    while p + 8 <= d.len() {
        let (name, size) = (&d[p..p + 4], be32(d, p + 4) as usize);
        p += 8;
        if p >= d.len() {
            break;
        }
        match name {
            b"CMOD" => {
                if size != 8 {
                    return None;
                }
                let mut m = [false; 4];
                channels = 4;
                for (i, f) in m.iter_mut().enumerate() {
                    *f = be16(d, p + 2 * i) != 0;
                    channels += *f as usize;
                }
                mixed = Some(m);
            }
            b"SAMP" => {
                let n = size / 32;
                let mut v = Vec::with_capacity(n);
                for i in 0..n {
                    let o = p + 32 * i;
                    if o + 32 > d.len() {
                        return None;
                    }
                    let s = Sample {
                        length: be32(d, o + 20),
                        rep_start: be16(d, o + 24) as u32 * 2,
                        rep_len: be16(d, o + 26) as u32 * 2,
                        volume: d[o + 29],
                        mode: be16(d, o + 30),
                        data: None,
                    };
                    v.push(s);
                }
                samples = Some(v);
            }
            b"SPEE" => {
                if size != 2 {
                    return None;
                }
                start_speed = be16(d, p);
            }
            b"SLEN" => {
                if size != 2 {
                    return None;
                }
                patt_num = be16(d, p);
                patterns = Some(Vec::new());
            }
            b"PLEN" => {
                if size != 2 {
                    return None;
                }
                song_len = be16(d, p);
            }
            b"PATT" => {
                if size != 128 || p + 128 > d.len() {
                    return None;
                }
                table = Some(d[p..p + 128].to_vec());
            }
            b"PBOD" => {
                if let (true, Some(pats)) = (read_patt < patt_num as usize, patterns.as_mut()) {
                    let lines = be16(d, p) as usize;
                    let ch = channels;
                    let end = p + 2 + lines * ch * 4;
                    if end > d.len() {
                        return None;
                    }
                    let data = (0..lines * ch)
                        .map(|i| {
                            let o = p + 2 + 4 * i;
                            Line { note: d[o], sample: d[o + 1], effect: d[o + 2], arg: d[o + 3] }
                        })
                        .collect();
                    pats.push(Pattern { lines, data });
                    read_patt += 1;
                }
            }
            b"SBOD" => {
                if let (true, Some(s)) = (read_samp < samples.as_ref().map_or(0, |s| s.len()), samples.as_mut()) {
                    while s[read_samp].length == 0 {
                        read_samp += 1;
                        if read_samp >= s.len() {
                            break;
                        }
                    }
                    if read_samp < s.len() {
                        let size = size.min(d.len() - p);
                        s[read_samp].data = Some((p, size));
                    }
                    read_samp += 1;
                }
            }
            // an unknown chunk, such as padding past the last sample: the
            // Eagleplayer stops there (NostalgicPlayer refuses a module that has
            // chunks for fewer samples than it declares; the Eagleplayer does not)
            _ => break,
        }
        p = p.saturating_add(size);
    }
    let (mixed, samples, patterns, table) = (mixed?, samples?, patterns?, table?);
    if patterns.len() < patt_num as usize || channels == 0 {
        return None;
    }
    let cells = patterns.iter().map(|p| (p.lines * channels) as u64).sum();
    Some(Song { mixed, channels, start_speed, song_len, cells, table, samples, patterns })
}

/// A write to a single channel's voice, queued by the tick that computed it and
/// made by the next one.
#[derive(Clone, Copy)]
enum Op {
    Off(usize),
    Loc(usize, u32),
    Len(usize, u16),
    Per(usize, u16),
    Vol(usize, u16),
    /// A mixed voice's AudioStruct volume.
    MixVol(u32, u16),
    /// The channel's DMA goes on at the start of the tick after.
    Start(usize, u32, u16),
}

#[derive(Clone, Copy, Default)]
struct Chan {
    /// The note, which the effects move past the ends of the table (it is played
    /// within them).
    note: i16,
    period: i16,
    rel_start: u32,
    rel_len: u32,
    /// The sample playing, and its loop.
    adr: u32,
    loop_start: u32,
    loop_len: u32,
}

pub struct Oktalyzer {
    song: Song,
    base: u32,
    /// The AudioStructs (0: none, Paula is written directly).
    structs: u32,
    /// The player's word of silence.
    silence: u32,
    /// Paula channel and voice of each pattern column.
    index: [usize; 8],
    voice: [usize; 8],
    chan: [Chan; 8],
    cur: [Line; 8],
    vol: [i8; 8],
    speed: u16,
    counter: i32,
    song_pos: i16,
    new_pos: i16,
    patt_pos: i16,
    visited: [bool; 128],
    songend: bool,
    first: std::cell::Cell<bool>,
    /// Single channels whose DMA goes on at the next tick.
    pending: u8,
    /// The loop each pending channel plays once its DMA is on.
    pend_loop: [(u32, u16); 4],
    q: Vec<Op>,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let song = load(d)?;
    let silence = m.alloc(4);
    let mixed_any = song.mixed.iter().any(|&f| f);
    let mut structs = 0;
    if silence == 0 {
        return None;
    }
    if mixed_any {
        structs = m.alloc(8 * aus::SIZE);
        if structs == 0 {
            return None;
        }
    }
    // the columns: a mixed channel's first on voice p, its second on p + 4
    let (mut index, mut voice) = ([0; 8], [0; 8]);
    let mut col = 0;
    for p in 0..4 {
        index[col] = p;
        voice[col] = p;
        col += 1;
        if song.mixed[p] {
            index[col] = p;
            voice[col] = p + 4;
            col += 1;
        }
    }
    if mixed_any {
        for v in 0..8usize {
            // left for Paula's channels 0 and 3, right for 1 and 2; a voice with
            // no channel is not played
            let p = v % 4;
            let has = v < 4 || song.mixed[p];
            let pan: u16 = if !has {
                0
            } else if p == 0 || p == 3 {
                1
            } else {
                0xFFFF
            };
            m.bus.w16(structs + v as u32 * aus::SIZE + aus::PAN, pan);
        }
        use crate::eagle::epamb;
        if !crate::amplifier::start(m, 8, structs, epamb::BIT8 | epamb::CHIP_RAM) {
            return None;
        }
    }
    let mut o = Oktalyzer {
        base,
        structs,
        silence,
        index,
        voice,
        chan: [Chan::default(); 8],
        cur: [Line::default(); 8],
        vol: [64; 8],
        speed: song.start_speed,
        counter: 0,
        song_pos: 0,
        new_pos: -1,
        patt_pos: -1,
        visited: [false; 128],
        songend: false,
        first: std::cell::Cell::new(true),
        pending: 0,
        pend_loop: [(0, 0); 4],
        q: Vec::new(),
        song,
    };
    o.visited[0] = true;
    let mut hw = Hw { bus: &mut m.bus };
    if o.structs != 0 {
        hw.wait(T_INIT + T_INIT_CELL * o.song.cells);
    }
    o.init_sound(&mut hw);
    o.set_volumes();
    if o.structs != 0 {
        hw.wait(T_RESTART);
        crate::amplifier::restart(hw.bus);
    }
    Some(Box::new(o))
}

impl Replayer for Oktalyzer {
    fn format(&self) -> &'static str {
        "Oktalyzer"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick_clocks(&self) -> u64 {
        // asked for when the timer is set up, and then at each interrupt
        if self.first.replace(false) {
            if self.structs == 0 { T_FIRST } else { T_FIRST_AMP }
        } else {
            super::DT_TICK
        }
    }

    fn tick(&mut self, hw: &mut Hw) {
        // The single channels' registers are written a tick after the tick
        // that works them out, the mixed channels' AudioStructs at once.
        self.dma_on(hw);
        for op in std::mem::take(&mut self.q) {
            self.exec(hw, op);
        }
        self.counter += 1;
        if self.counter >= self.speed as i32 {
            self.counter = 0;
            self.find_next_line();
            self.play_line(hw);
        }
        self.effects(hw);
        self.set_volumes();
    }
}

impl Oktalyzer {
    fn single(&self, p: usize) -> bool {
        !self.song.mixed[p]
    }

    // The voices: Paula's registers when no channel is mixed, else the
    // amplifier's (pokes for the single channels).
    fn exec(&mut self, hw: &mut Hw, op: Op) {
        let paula = self.structs == 0;
        match op {
            Op::Off(p) if paula => hw.dmacon(1 << p),
            Op::Off(p) => hw.voice_dma(1 << p, false),
            Op::Loc(p, a) if paula => hw.loc(p, a),
            Op::Loc(p, a) => hw.voice_loc(p, a),
            Op::Len(p, w) if paula => hw.len(p, w),
            Op::Len(p, w) => hw.voice_len(p, w),
            Op::Per(p, v) if paula => hw.per(p, v),
            Op::Per(p, v) => hw.voice_per(p, v),
            Op::Vol(p, v) if paula => hw.vol(p, v),
            Op::Vol(p, v) => hw.voice_vol(p, v),
            Op::MixVol(s, v) => {
                hw.bus.w16(s + aus::VOL, v);
                hw.bus.w16(s + aus::VOL + 2, v);
            }
            Op::Start(p, a, w) => {
                self.pending |= 1 << p;
                self.pend_loop[p] = (a, w);
            }
        }
    }

    fn v_vol(&self, hw: &mut Hw, p: usize, vol: u16) {
        self.exec_now(hw, Op::Vol(p, vol));
    }

    fn exec_now(&self, hw: &mut Hw, op: Op) {
        match op {
            Op::Vol(p, v) if self.structs == 0 => hw.vol(p, v),
            Op::Vol(p, v) => hw.voice_vol(p, v),
            _ => {}
        }
    }

    fn init_sound(&mut self, hw: &mut Hw) {
        if self.structs == 0 {
            hw.dmacon(0x000f);
        } else {
            hw.voice_dma(0xf, false);
        }
        for p in 0..4 {
            self.v_vol(hw, p, 0);
        }
    }

    fn dma_on(&mut self, hw: &mut Hw) {
        if self.pending == 0 {
            return;
        }
        let m = self.pending as u32;
        if self.structs == 0 {
            hw.dmacon(0x8000 | m as u16);
        } else {
            hw.voice_dma(m, true);
        }
        for p in 0..4 {
            if m & 1 << p == 0 {
                continue;
            }
            let (a, w) = self.pend_loop[p];
            if self.structs == 0 {
                hw.loc(p, a);
                hw.len(p, w);
            } else {
                hw.voice_loc(p, a);
                hw.voice_len(p, w);
            }
        }
        self.pending = 0;
    }

    fn pattern(&self, pos: i16) -> Option<&Pattern> {
        self.song.patterns.get(*self.song.table.get(pos as usize).unwrap_or(&0) as usize)
    }

    fn find_next_line(&mut self) {
        let songlen = self.song.song_len as i16;
        self.patt_pos += 1;
        let lines = self.pattern(self.song_pos).map_or(0, |p| p.lines) as i16;
        if self.patt_pos >= lines || self.new_pos != -1 {
            self.patt_pos = 0;
            if self.new_pos != -1 {
                self.song_pos = self.new_pos;
                self.new_pos = -1;
            } else {
                self.song_pos += 1;
            }
            if self.song_pos == songlen || self.song_pos < 0 || self.song_pos >= 128 {
                self.song_pos = 0;
            }
            if self.visited[self.song_pos as usize] {
                self.songend = true;
            }
            self.visited[self.song_pos as usize] = true;
        }
        let ch = self.song.channels;
        let at = self.patt_pos as usize * ch;
        if let Some(p) = self.pattern(self.song_pos) {
            let mut cur = self.cur;
            for (i, l) in cur.iter_mut().enumerate().take(ch) {
                *l = p.data.get(at + i).copied().unwrap_or_default();
            }
            self.cur = cur;
        }
    }

    fn play_line(&mut self, hw: &mut Hw) {
        let mut mask = 0u32;
        for c in 0..self.song.channels {
            self.play_channel(c, hw, &mut mask);
        }
    }

    fn play_channel(&mut self, c: usize, hw: &mut Hw, _mask: &mut u32) {
        let line = self.cur[c];
        if line.note == 0 {
            return;
        }
        let note = (line.note - 1) as usize;
        let Some(samp) = self.song.samples.get(line.sample as usize).cloned() else { return };
        if samp.data.is_none() || samp.length == 0 || note >= PERIODS.len() {
            return;
        }
        let p = self.index[c];
        let (off, _) = samp.data.unwrap();
        let adr = self.base + off as u32;
        if self.song.mixed[p] {
            if samp.mode == 1 {
                return;
            }
            self.chan[c].rel_start = 0;
            self.chan[c].rel_len = 0;
            self.mixed_start(hw, c, adr, samp.length);
        } else {
            if samp.mode == 0 {
                return;
            }
            self.vol[p] = samp.volume as i8;
            self.chan[c].note = note as i16;
            self.chan[c].period = PERIODS[note];
            self.chan[c].adr = adr;
            self.chan[c].loop_start = samp.rep_start;
            self.chan[c].loop_len = samp.rep_len;
            self.q.push(Op::Off(p));
            self.q.push(Op::Loc(p, adr));
            self.set_period(hw, c);
            let (first, loop_at) = if samp.rep_len == 0 {
                self.chan[c].rel_start = 0;
                self.chan[c].rel_len = 0;
                (samp.length, None)
            } else {
                self.chan[c].rel_start = samp.rep_start + samp.rep_len;
                self.chan[c].rel_len = samp.length.wrapping_sub(self.chan[c].rel_start);
                (samp.rep_start + samp.rep_len, Some((adr + samp.rep_start, (samp.rep_len / 2) as u16)))
            };
            self.q.push(Op::Len(p, (first / 2) as u16));
            let (la, lw) = loop_at.unwrap_or((self.silence, 1));
            self.q.push(Op::Start(p, la, lw));
            return;
        }
        self.chan[c].note = note as i16;
        self.chan[c].period = PERIODS[note];
        self.set_period(hw, c);
    }

    /// The AudioStruct of the voice a pattern column plays on.
    fn aus(&self, c: usize) -> u32 {
        self.structs + self.voice[c] as u32 * aus::SIZE
    }

    fn mixed_start(&mut self, hw: &mut Hw, c: usize, adr: u32, len: u32) {
        let s = self.aus(c);
        let b = &mut *hw.bus;
        b.w32(s + aus::ADR, adr);
        b.w32(s + aus::POS, 0);
        // the fraction the mixer left of the last sample stays
        b.w8(s + aus::STATE, 0);
        b.w32(s + aus::LEN, len.wrapping_sub(2));
        b.w32(s + aus::LOOP_ADR, 0);
        b.w32(s + aus::LOOP_LEN, 0);
        b.w8(s + aus::LOOP, 0);
        b.w8(s + aus::ENDED, 0);
        b.w16(s + aus::ON, 0xFFFF);
    }

    fn set_period(&mut self, hw: &mut Hw, c: usize) {
        let (p, per) = (self.index[c], self.chan[c].period as u16);
        if self.song.mixed[p] {
            let s = self.aus(c);
            hw.bus.w16(s + aus::PER, per << 2);
        } else {
            self.q.push(Op::Per(p, per));
        }
    }

    fn set_volumes(&mut self) {
        for i in 0..4 {
            self.vol[i + 4] = self.vol[i];
        }
        for p in 0..4 {
            if self.single(p) {
                self.q.push(Op::Vol(p, self.vol[p] as u8 as u16));
            }
        }
        for c in 0..self.song.channels {
            if self.song.mixed[self.index[c]] {
                let s = self.aus(c);
                let v = self.vol[self.index[c]] as i16 as u16;
                self.q.push(Op::MixVol(s, v));
            }
        }
    }

    fn effects(&mut self, hw: &mut Hw) {
        for c in 0..self.song.channels {
            self.channel_effect(c, hw);
        }
    }

    fn play_note(&mut self, hw: &mut Hw, c: usize, note: i16) {
        let note = note.clamp(0, 35);
        self.chan[c].period = PERIODS[note as usize];
        self.set_period(hw, c);
    }

    /// The effects that move the note itself: a mixed voice whose note is above
    /// the table has no period (it stops, until a note is back in it), where a
    /// single channel plays the end of the table.
    fn play_moved(&mut self, hw: &mut Hw, c: usize) {
        let n = self.chan[c].note;
        if self.song.mixed[self.index[c]] && !(0..36).contains(&n) {
            let s = self.aus(c);
            hw.bus.w16(s + aus::PER, 0);
        } else {
            self.play_note(hw, c, n);
        }
    }

    fn channel_effect(&mut self, c: usize, hw: &mut Hw) {
        let line = self.cur[c];
        let arg = line.arg;
        let sc = self.counter.max(0) as usize;
        match line.effect {
            // portamento down
            1 => {
                let ch = &mut self.chan[c];
                ch.period = ch.period.wrapping_sub(arg as i16);
                if ch.period < 113 {
                    ch.period = 113;
                }
                self.set_period(hw, c);
            }
            // portamento up
            2 => {
                let ch = &mut self.chan[c];
                ch.period = ch.period.wrapping_add(arg as i16);
                if ch.period > 856 {
                    ch.period = 856;
                }
                self.set_period(hw, c);
            }
            // arpeggio type 1
            10 => {
                let mut n = self.chan[c].note;
                match ARP10[sc & 15] {
                    0 => n -= (arg >> 4) as i16,
                    2 => n += (arg & 15) as i16,
                    _ => {}
                }
                self.play_note(hw, c, n);
            }
            // arpeggio type 2
            11 => {
                let mut n = self.chan[c].note;
                match sc & 3 {
                    1 => n += (arg & 15) as i16,
                    3 => n -= (arg >> 4) as i16,
                    _ => {}
                }
                self.play_note(hw, c, n);
            }
            // arpeggio type 3
            12 => {
                let mut n = self.chan[c].note;
                match ARP12[sc & 15] {
                    0 => return,
                    1 => n -= (arg >> 4) as i16,
                    2 => n += (arg & 15) as i16,
                    _ => {}
                }
                self.play_note(hw, c, n);
            }
            // increase note, once a line (17) or a tick (30)
            17 | 30 => {
                if line.effect == 17 && sc != 0 {
                    return;
                }
                let ch = &mut self.chan[c];
                ch.note += arg as i16;
                self.play_moved(hw, c);
            }
            // decrease note, once a line (21) or a tick (13)
            21 | 13 => {
                if line.effect == 21 && sc != 0 {
                    return;
                }
                let ch = &mut self.chan[c];
                // the note goes past the table's top; past its bottom too with Paula written
                // directly, with the amplifier it stops there
                ch.note -= arg as i16;
                if self.structs != 0 {
                    ch.note = ch.note.max(0);
                }
                self.play_moved(hw, c);
            }
            // filter
            15 => {
                // the LED filter stays as the engine has it (on, at the start) until the song
                // turns it off or on
                if sc == 0 {
                    if arg != 0 {
                        hw.bus.ciaa.pra &= !2;
                    } else {
                        hw.bus.ciaa.pra |= 2;
                    }
                }
            }
            // position jump
            25 => {
                if sc == 0 {
                    let pos = ((arg >> 4) as u16) * 10 + (arg & 15) as u16;
                    if pos < self.song.song_len {
                        self.new_pos = pos as i16;
                    }
                }
            }
            // release the sample
            27 => {
                let ch = self.chan[c];
                // the Eagleplayer's R writes nothing to Paula (checked on a module
                // with 15 of them): the release is not played
                let _ = ch;
            }
            // speed
            28 => {
                if sc == 0 && arg & 15 != 0 {
                    self.speed = (arg & 15) as u16;
                }
            }
            // volume, with retrigger (24)
            24 | 31 => {
                let vi = self.index[c];
                if line.effect == 24 {
                    self.vol[vi] = self.vol[vi + 4];
                }
                let mut a = arg;
                let v = &mut self.vol[vi];
                if a <= 64 {
                    *v = a as i8;
                    return;
                }
                a -= 64;
                if a < 16 {
                    *v = v.wrapping_sub(a as i8);
                    if *v < 0 {
                        *v = 0;
                    }
                    return;
                }
                a -= 16;
                if a < 16 {
                    *v = v.wrapping_add(a as i8);
                    if *v > 64 {
                        *v = 64;
                    }
                    return;
                }
                a -= 16;
                if a < 16 {
                    if sc == 0 {
                        *v = v.wrapping_sub(a as i8);
                        if *v < 0 {
                            *v = 0;
                        }
                    }
                    return;
                }
                a -= 16;
                if a < 16 && sc == 0 {
                    *v = v.wrapping_add(a as i8);
                    if *v > 64 {
                        *v = 64;
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn chunk(d: &mut Vec<u8>, name: &[u8; 4], body: &[u8]) {
        d.extend(name);
        d.extend((body.len() as u32).to_be_bytes());
        d.extend(body);
    }

    /// A module of 4 channels (`cmod` makes some of them two columns), speed 2,
    /// one position with a pattern of 4 lines. Sample 0 is 64 bytes of a ramp
    /// looping from byte 16, of `mode`; row 0 plays it at note 13 on every
    /// column. 36 samples are declared, as Oktalyzer does, so that the module
    /// is as long as the smallest the player takes.
    fn module(cmod: [u16; 4], mode: u16, tail: &[u8]) -> Vec<u8> {
        let mut d = b"OKTASONG".to_vec();
        chunk(&mut d, b"CMOD", &cmod.iter().flat_map(|f| f.to_be_bytes()).collect::<Vec<_>>());
        let mut samp = Vec::new();
        for i in 0..36 {
            samp.extend([0u8; 20]);
            let (len, rs, rl) = if i == 0 { (64u32, 8u16, 24u16) } else { (0, 0, 0) };
            samp.extend(len.to_be_bytes());
            samp.extend(rs.to_be_bytes());
            samp.extend(rl.to_be_bytes());
            samp.extend([0, 64]);
            samp.extend(mode.to_be_bytes());
        }
        chunk(&mut d, b"SAMP", &samp);
        chunk(&mut d, b"SPEE", &[0, 2]);
        chunk(&mut d, b"SLEN", &[0, 1]);
        chunk(&mut d, b"PLEN", &[0, 1]);
        chunk(&mut d, b"PATT", &[0u8; 128]);
        let channels = 4 + cmod.iter().filter(|&&f| f != 0).count();
        let mut pbod = vec![0, 4];
        for row in 0..4 {
            for _ in 0..channels {
                pbod.extend(if row == 0 { [13, 0, 0, 0] } else { [0; 4] });
            }
        }
        chunk(&mut d, b"PBOD", &pbod);
        chunk(&mut d, b"SBOD", &(0..64).map(|i| i as u8).collect::<Vec<_>>());
        d.extend(tail);
        d
    }

    #[test]
    fn detects_and_loads() {
        let d = module([0; 4], 1, &[]);
        assert!(detect(&d));
        assert!(!detect(&d[..1000]));
        assert!(!detect(&[b"OKTASONX".to_vec(), vec![0; 1400]].concat()));
        let s = load(&d).unwrap();
        assert_eq!((s.channels, s.start_speed, s.song_len, s.samples.len()), (4, 2, 1, 36));
        // the loop is kept as the file has it (in bytes), and the data lies in the module
        let s0 = &s.samples[0];
        assert_eq!((s0.length, s0.rep_start, s0.rep_len, s0.volume, s0.mode), (64, 16, 48, 64, 1));
        let (o, n) = s0.data.unwrap();
        assert_eq!((d[o], n), (0, 64));
        assert_eq!(load(&module([1, 0, 1, 1], 0, &[])).unwrap().channels, 7);
        // padding after the last sample is not a chunk the Eagleplayer minds
        assert!(load(&module([0; 4], 1, &[0; 24])).is_some());
        // patterns that are not there are
        let mut cut = module([0; 4], 1, &[]);
        let at = cut.windows(4).position(|w| w == b"PBOD").unwrap();
        cut.truncate(at);
        assert!(load(&cut).is_none());
    }

    /// Four single channels play on Paula itself: the sample up to the end of its
    /// loop first, DMA on a tick after, then the loop.
    #[test]
    fn single_channels_write_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.okt"), module([0; 4], 1, &[])).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Oktalyzer");
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // the start, then the 4 notes' DMA off one by one and on together
        assert_eq!(&reg(0xDFF096)[..6], &[0x000f, 0x0001, 0x0002, 0x0004, 0x0008, 0x800f]);
        // 32 words up to the loop's end, then the loop's 24
        assert_eq!(&reg(0xDFF0A4)[..2], &[32, 24]);
        assert_eq!(reg(0xDFF0A6)[0], PERIODS[12] as u16);
        // the volume goes out every tick; after the note it is the sample's
        assert_eq!(&reg(0xDFF0A8)[..2], &[0, 64]);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }

    /// With a mixed channel the AudioStructs and the host's amplifier play: a
    /// 7 bit sample is left as it is (the original does not scale it up; it is
    /// half as loud as an 8 bit one) and heard.
    #[test]
    fn mixed_channels_play_on_the_amplifier() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.okt"), module([1, 0, 0, 0], 0, &[])).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Oktalyzer");
        let (addr, size) = e.m.deli.list[0];
        let at: Vec<u8> = (0..size).map(|i| e.m.bus.r8(addr + i)).collect();
        let sample = at.windows(4).position(|w| w == [0, 1, 2, 3]).expect("the sample");
        assert_eq!(at[sample + 63], 63);
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
        // the single channels are poked on the amplifier's voices 1 to 3
        let log = e.m.bus.write_log.take().unwrap();
        assert!(log.iter().any(|w| w.0 == (crate::amplifier::LOG_BASE | 0x38)));
    }
}
