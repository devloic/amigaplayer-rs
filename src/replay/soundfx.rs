//! SoundFX 1.x and 2.0, ported from NostalgicPlayer's SoundFX player and its 1.x
//! converter (Copyright (c) 2023 Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer).
//!
//! NostalgicPlayer plays 2.0 modules (mark `SO31`, 31 samples) and converts 1.x
//! ones (`SONG`, 15 samples) to 2.0 first. This follows the original players
//! instead (checked against the Eagleplayers SoundFX 1.3 and SoundFX 2.0), which
//! differ from it and from each other, so one engine plays both with the
//! differences spelt out. Like the originals it reads the module where it lies in
//! guest memory, and their period tables lie in guest memory too, since 1.3
//! searches its table without an end check.
//!
//! What the originals write and NostalgicPlayer does not ask of its mixer:
//! * every row writes DMACON on (`$8000` and the voices started) and then every
//!   voice's loop location and length, voice 3 first, whether a note started or not;
//!   a sample plays its stored length, and its loop is its stored loop start and
//!   length unchecked (no fixes for lengths, loops or volumes over 64)
//! * the first 4 bytes of every sample are cleared when the song starts
//! * a note on a voice that never had a sample plays address 0, length 0
//! * the first row is played on the sixth interrupt
//! * 1.x: 1.3 has its own period table (856 to 113), and patterns start right
//!   after the orders (no pad long); no `BRK` (`$FFFC` and `$FFFB` play as
//!   periods), `STP` only clears the effect; effects 1, 2, 7 and 8 only; a step
//!   does not stop at its end note but keeps writing it, and starts from the
//!   period word as pitch bends left it (2.0: from the note played); the
//!   arpeggio's search for the note has no end check
//! * 2.0: `STP` switches the voice off, writes volume 0 and switches it on again
//!   (with the loop of the last row); `BRK` and `$FFFB` switch the voice off for
//!   good, and set their sample (bit 12 of `$FFFx` is the sample's bit 4); no
//!   effect 9 (NostalgicPlayer's auto slide, from Flod)
//!
//! The writes are spaced as the originals'. 1.3's come on the Eagleplayer's colour
//! clock: the song starts after the time its start-up takes (`START13` and the
//! module's part, see `new`), and each write after the 68000 cycles of the code
//! paths before it (`F`). Not followed there: a note that is not in the period
//! table (other than 0) under an arpeggio, which the original searches for through
//! all the memory after its table, for many ticks' time.
//!
//! 2.0's writes keep measured gaps (`Timing`), but where the start of its tick lies
//! against the host's interrupt is not followed: the reference's ticks start some
//! 6500 to 8000 colour clocks later than the port's, by the time the host took to
//! run the 68k player's init, which differs from module to module. So the DMA wait
//! after DMA on ends on another line, and where a DMA off to on gap is shorter than
//! a word at the voice's period, whether the voice restarts can come out otherwise
//! than in the reference.

use super::{Hw, Open, Replayer};
use crate::bus::Bus;
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN13: Open = Open { name: "SoundFX1.3", detect: detect13, new: new13 };
pub const OPEN20: Open = Open { name: "SoundFX2.0", detect: detect20, new: new20 };

/// Where the module's parts lie, for each version.
struct Layout {
    samples: usize,
    /// The first sample's info (30 bytes each: name, length, volume, loop start,
    /// loop length), the song length byte, the orders and the patterns.
    info: u32,
    song_len: u32,
    orders: u32,
    patterns: u32,
    delay: u32,
}

const L13: Layout = Layout { samples: 15, info: 0x50, song_len: 0x212, orders: 0x214, patterns: 0x294, delay: 0x40 };
const L20: Layout = Layout { samples: 31, info: 0x90, song_len: 0x432, orders: 0x434, patterns: 0x4b8, delay: 0x80 };

/// The period tables as they lie in the players' data: 1.3's 856 to 113, after
/// its song length, play flag and delay words and 12 more 856s (which a step up
/// from the first note reads), followed by the last word of its hunk and the two
/// words after it; 2.0's is NostalgicPlayer's. `*_START` is the word where the search for a note starts.
const T13_START: u32 = 15;
const T20_START: u32 = 20;
/// Room past 1.3's table for its arpeggio search, which stops only at the note
/// (on the original, in whatever follows its hunk); the port gives up there.
const T13_SEARCH: u32 = 0x1000;

/// The time 2.0's code takes between its writes, in colour clocks (measured on its
/// write logs; 1.3's times are `F`'s). Paula stops a playing voice only at the end
/// of its word and restarts it only once stopped, so the time between a voice's
/// DMA off and the DMA on decides whether its note is heard.
struct Timing {
    /// A row: before the first voice (not measured, see above); a voice up to its
    /// sample (with one, up to its volume write); with no note, the rest of the
    /// voice.
    row_pre: u64,
    smp: u64,
    no_smp: u64,
    no_note: u64,
    /// A note: to its DMA off, then to the address, length, period and the voice's
    /// end.
    note_dma: u64,
    dma_loc: u64,
    /// 2.0's STP: DMA off to the volume.
    stp_vol: u64,
    loc_len: u64,
    len_per: u64,
    note_post: u64,
    /// After the last voice to the DMA on; after the DMA wait to the first loop,
    /// between a loop's two writes and from one loop to the next.
    dma_on: u64,
    loops: u64,
    loop_mid: u64,
    loop_next: u64,
    /// The effects: before the first voice (not measured), for a voice without a
    /// write, and before and after a voice's period write.
    fx_pre: u64,
    fx_none: u64,
    fx_per: u64,
    fx_post: u64,
    /// For each word the arpeggio's and the step's searches pass.
    arp_entry: u64,
    step_entry: u64,
}

const T20: Timing = Timing {
    row_pre: 200,
    smp: 364,
    no_smp: 92,
    no_note: 61,
    note_dma: 153,
    dma_loc: 142,
    stp_vol: 109,
    loc_len: 105,
    len_per: 97,
    note_post: 60,
    dma_on: 105,
    loops: 92,
    loop_mid: 105,
    loop_next: 108,
    fx_pre: 200,
    fx_none: 114,
    fx_per: 129,
    fx_post: 100,
    arp_entry: 32,
    step_entry: 27,
};

/// 1.3: colour clocks from the open replayer's start to the song's (where the timer and
/// the output begin) but for the module's part: the Eagleplayer's check, its start-up
/// calls and InitSound (their lengths on the 147 modules of the corpus; see `new`).
const START13: u64 = 32 + 4543 + 733 + 11;

/// One voice: the fields of the original's voice structure.
#[derive(Default)]
struct Voice {
    /// The row: period word, sample and effect, effect parameter.
    pattern: u32,
    sample: u32,
    len: u16,
    loop_addr: u32,
    loop_len: u16,
    /// The note the arpeggio (and 2.0's step) works from.
    note: u16,
    volume: u16,
    step: u16,
    step_note: u16,
    step_end: u16,
}

pub struct SoundFx {
    v20: bool,
    module: u32,
    /// The period table in guest memory, and the sample addresses.
    table: u32,
    samples: [u32; 31],
    song_len: u16,
    delay: u16,
    voices: [Voice; 4],
    timer: u16,
    track_pos: u32,
    pos: u32,
    /// DMACON bits of the voices started in this row.
    dma: u16,
    brk: bool,
    songend: bool,
    /// 1.3: the 68000 cycles of the paths taken since the last host call (`F`).
    cy: i64,
}

fn detect13(d: &[u8]) -> bool {
    d.len() >= 0x294 && d[60..64] == *b"SONG"
}

fn detect20(d: &[u8]) -> bool {
    d.len() >= 0x4b8 && d[124..128] == *b"SO31"
}

fn new13(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    new(d, base, m, false)
}

fn new20(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    new(d, base, m, true)
}

fn new(_d: &[u8], base: u32, m: &mut Machine, v20: bool) -> Option<Box<dyn Replayer>> {
    let l = if v20 { &L20 } else { &L13 };
    // away from the module, past whose end the original clears a long for each
    // sample of size 0 at the end
    let top = std::mem::replace(&mut m.alloc_top, true);
    let table = m.alloc(2 * if v20 { NOTE_TABLE.len() as u32 } else { T13_START + T13_SEARCH });
    m.alloc_top = top;
    if table == 0 {
        return None;
    }
    let b = &mut m.bus;
    let song_len = b.r8(base + l.song_len) as u16;
    let delay = b.r16(base + l.delay);
    if v20 {
        for (i, &p) in NOTE_TABLE.iter().enumerate() {
            b.w16(table + 2 * i as u32, p);
        }
    } else {
        let mut t = vec![song_len, 0xffff, delay];
        t.extend([856; 12]);
        t.extend(&NOTE_TABLE[24..60]);
        t.extend([113; 12]);
        t.extend([0xffff, 0xc400]);
        // what the reference has after the hunk: the arpeggio of a voice without a note
        // yet finds its 0 there, and its first step up is the word after
        t.extend([0, 0x110]);
        for (i, &p) in t.iter().enumerate() {
            b.w16(table + 2 * i as u32, p);
        }
    }

    // as the original's InitSound: the highest pattern of the song (a song length
    // of 0 is 65536 orders), the samples after the patterns, and the first long
    // of each cleared
    let mut max = 0;
    for i in 0..if song_len == 0 { 0x10000 } else { song_len as u32 } {
        max = max.max(b.r8(base + l.orders + i));
    }
    let mut samples = [0; 31];
    let mut a = base.wrapping_add(l.patterns + (max as u32 + 1) * 0x400);
    for (i, s) in samples.iter_mut().take(l.samples).enumerate() {
        *s = a;
        a = a.wrapping_add(b.r32(base + 4 * i as u32));
    }
    for &s in &samples[..l.samples] {
        b.w32(s, 0);
    }
    if !v20 {
        // the original's start-up before the timer runs: a loop over the samples with a
        // length and InitSound's scan of the orders (a pass for each, and more for one not
        // below the highest so far)
        let sized = (0..15).filter(|&i| b.r32(base + 4 * i) != 0).count() as u64;
        let (mut top, mut tops) = (0, 0u64);
        let orders = if song_len == 0 { 0x10000 } else { song_len as u64 };
        for i in 0..orders as u32 {
            let o = b.r8(base + l.orders + i);
            if o >= top {
                top = o;
                tops += 1;
            }
        }
        b.clock += START13 + 6 * sized + (2348 + 31 * orders + 4 * tops) / 2;
    }

    Some(Box::new(SoundFx {
        v20,
        module: base,
        table: table + 2 * if v20 { T20_START } else { T13_START },
        samples,
        song_len,
        delay,
        voices: Default::default(),
        timer: 0,
        track_pos: 0,
        pos: 0,
        dma: 0,
        brk: false,
        songend: false,
        cy: 0,
    }))
}

impl Replayer for SoundFx {
    fn format(&self) -> &'static str {
        if self.v20 { "SoundFX2.0" } else { "SoundFX1.3" }
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    /// The module's delay is the CIA timer value.
    fn tick_clocks(&self) -> u64 {
        let t = if self.delay == 0 { crate::deli::DEFAULT_TIMER } else { self.delay };
        (t as u64 + 1) * 5
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.cy = 0;
        self.timer += 1;
        if self.timer == 6 {
            self.timer = 0;
            self.row(hw);
        } else {
            self.wait20(hw, T20.fx_pre);
            self.c(F::Fx);
            for i in 0..4 {
                self.effects(i, hw);
            }
        }
    }
}

macro_rules! paths {
    ($($name:ident = $c:expr,)*) => {
        /// 1.3's code paths between its calls to the host (one for every register write),
        /// each with the 68000 cycles it takes as the emulator counts them. A write comes
        /// half the cycles of the paths since the last host call later (an odd cycle is
        /// lost there, as the emulator's clock follows the CPU in whole colour clocks).
        /// The figures are a whole-number solution of the Eagleplayer's gaps between host
        /// calls (half-cycle write logs of 147 modules and of 60 made-up ones that take
        /// the rarer paths); only sums along a tick's way mean something.
        #[derive(Clone, Copy)]
        enum F { $($name,)* }
        const COST: &[i64] = &[$($c,)*];
    };
}

paths! {
    // a row: its start; a voice's start; its sample up to the volume write (more with a
    // loop offset, the volume effects 5 and 6 and their limits) and on from there; a
    // voice without a sample (but for PIC and STP, which take none) and a STP
    Row = 255,
    Voice = 244,
    Smp = 547,
    SmpLoop = 10,
    Vol5 = 24,
    Vol5Limit = 8,
    Vol6 = 35,
    Vol6Limit = 14,
    AfterVol = 27,
    NoSmp = 63,
    Stp = 2,
    // a note: to its DMA off, location, length and period
    Note = 276,
    NoteLoc = 239,
    NoteLen = 210,
    NotePer = 186,
    // the DMA on, and after the host's DMA wait the loops
    RowDma = 298,
    Loops = 230,
    LoopLen = 210,
    LoopNext = 236,
    // the effects: a tick's start, a voice's start, a running step that reached its end
    // (up, down), a voice without a step
    Fx = 71,
    FxVoice = 203,
    StepUpEnd = 4,
    StepDownEnd = 14,
    NoStep = 14,
    // the arpeggio (more on ticks 1 and 5, 2 and 4, and for each word its search passes),
    // the pitch bend
    Arp = 55,
    ArpHigh = 97,
    ArpLow = 91,
    ArpEntry = 49,
    Bend = 50,
    BendUp = 13,
    BendDown = 52,
    // a step's start (effect 7 up, 8 down): a word its search passes, the note found
    SetUp = 143,
    UpEntry = 56,
    UpFound = 52,
    SetDown = 175,
    DownEntry = 56,
    DownFound = 48,
    // the period write; the way on from a voice but after a step's write
    FxPer = 166,
    FxOn = 12,
}

fn cost(f: F, n: u64) -> i64 {
    COST[f as usize] * n as i64
}

impl SoundFx {
    fn layout(&self) -> &'static Layout {
        if self.v20 { &L20 } else { &L13 }
    }

    /// 2.0's time between two writes (`Timing`).
    fn wait20(&self, hw: &mut Hw, clocks: u64) {
        if self.v20 {
            hw.wait(clocks);
        }
    }

    /// 1.3 takes path `f`.
    fn c(&mut self, f: F) {
        self.cy += cost(f, 1);
    }

    /// 1.3 calls the host for a write: the paths' time since the last call passes.
    fn call(&mut self, hw: &mut Hw) {
        if !self.v20 {
            hw.wait(self.cy as u64 / 2);
        }
        self.cy = 0;
    }

    fn row(&mut self, hw: &mut Hw) {
        let (t, l) = (&T20, self.layout());
        let order = hw.bus.r8(self.module.wrapping_add(l.orders).wrapping_add(self.track_pos));
        let row = self.module.wrapping_add(l.patterns + ((order as u32) << 10) + self.pos);
        self.dma = 0;
        self.wait20(hw, t.row_pre);
        self.c(F::Row);
        for i in 0..4 {
            self.play_note(i, row + 4 * i as u32, hw);
        }
        self.wait20(hw, t.dma_on);
        self.c(F::RowDma);
        self.call(hw);
        hw.dmacon(0x8000 | self.dma);
        // the host's ENPP_DMAMask waits for the DMA as dtg_WaitAudioDMA does: to
        // 10 lines past the current one
        let now = hw.bus.clock;
        hw.wait((now / LINE + 10) * LINE - now);
        self.wait20(hw, t.loops);
        self.c(F::Loops);
        for i in (0..4).rev() {
            let (a, n) = (self.voices[i].loop_addr, self.voices[i].loop_len);
            self.call(hw);
            hw.loc(i, a);
            self.wait20(hw, t.loop_mid);
            self.c(F::LoopLen);
            self.call(hw);
            hw.len(i, n);
            self.wait20(hw, t.loop_next);
            self.c(F::LoopNext);
        }

        if self.brk {
            self.brk = false;
            self.pos = 0x3f0;
        }
        self.pos += 16;
        if self.pos >= 0x400 {
            self.pos = 0;
            self.track_pos = self.track_pos.wrapping_add(1);
            if self.track_pos as u16 == self.song_len {
                self.track_pos = 0;
                self.songend = true;
            }
        }
    }

    /// Voice `i`'s part of the row at `a`.
    fn play_note(&mut self, i: usize, a: u32, hw: &mut Hw) {
        let (t, l, v20) = (&T20, self.layout(), self.v20);
        let bit = 1u16 << i;
        self.c(F::Voice);
        let pattern = hw.bus.r32(a);
        self.voices[i].pattern = pattern;
        let w = (pattern >> 16) as u16;
        let mut n = (pattern >> 12) as u8 & 0xf;
        if v20 && w & 0x1000 != 0 {
            n += 16;
        }
        // 1.3's STP only clears the effect
        let quiet = w == 0xfffd || (!v20 && w == 0xfffe);
        // and takes no sample
        let has_sample = n != 0 && !quiet;
        if has_sample {
            let n = n as u32 - 1;
            let e = self.module + l.info + 30 * n + 22;
            let b: &Bus = hw.bus;
            let v = &mut self.voices[i];
            v.sample = self.samples[n as usize];
            v.len = b.r16(e);
            v.volume = b.r16(e + 2);
            let loop_start = b.r16(e + 4);
            v.loop_addr = v.sample.wrapping_add(loop_start as u32);
            v.loop_len = b.r16(e + 6);
            let param = pattern as u8 as u16;
            let mut vol = v.volume;
            self.c(F::Smp);
            if loop_start != 0 {
                self.c(F::SmpLoop);
            }
            match (pattern >> 8) & 0xf {
                5 => {
                    self.c(F::Vol5);
                    vol = vol.wrapping_add(param);
                    if vol as i16 > 0x40 {
                        self.c(F::Vol5Limit);
                        vol = 0x40;
                    }
                }
                6 => {
                    self.c(F::Vol6);
                    vol = vol.wrapping_sub(param);
                    if (vol as i16) < 0 {
                        self.c(F::Vol6Limit);
                        vol = 0;
                    }
                }
                _ => {}
            }
            self.wait20(hw, t.smp);
            self.call(hw);
            hw.vol(i, vol);
            self.c(F::AfterVol);
        } else {
            self.wait20(hw, t.no_smp);
            if !quiet {
                self.c(F::NoSmp);
            }
        }

        if quiet {
            self.voices[i].pattern &= 0xffff0000;
            self.wait20(hw, t.no_note);
            if w == 0xfffe {
                self.c(F::Stp);
            }
            return;
        }
        if w == 0 {
            self.wait20(hw, t.no_note);
            return;
        }
        self.voices[i].step = 0;
        self.voices[i].note = if v20 { w & 0xefff } else { w };
        self.wait20(hw, t.note_dma);
        self.c(F::Note);
        self.call(hw);
        hw.dmacon(bit);
        if v20 {
            match w {
                // STP: silent, and on again with the voice's registers as they are
                0xfffe => {
                    hw.wait(t.stp_vol);
                    hw.vol(i, 0);
                    hw.wait(t.note_post);
                    self.dma |= bit;
                    return;
                }
                // BRK: on to the next position after this row
                0xfffc | 0xfffb => {
                    if w == 0xfffc {
                        self.brk = true;
                    }
                    self.voices[i].pattern &= 0xefffffff;
                    hw.wait(t.note_post);
                    return;
                }
                _ => {}
            }
        }
        let (sample, len, note) = (self.voices[i].sample, self.voices[i].len, self.voices[i].note);
        self.wait20(hw, t.dma_loc);
        self.c(F::NoteLoc);
        self.call(hw);
        hw.loc(i, sample);
        self.wait20(hw, t.loc_len);
        self.c(F::NoteLen);
        self.call(hw);
        hw.len(i, len);
        self.wait20(hw, t.len_per);
        self.c(F::NotePer);
        self.call(hw);
        hw.per(i, note);
        self.wait20(hw, t.note_post);
        self.dma |= bit;
    }

    /// Voice `i`'s effects, between rows.
    fn effects(&mut self, i: usize, hw: &mut Hw) {
        let (t, v20, timer, table) = (&T20, self.v20, self.timer, self.table);
        let v = &mut self.voices[i];
        // 2.0: the time the searches in the period table take; 1.3: the voice's paths (`F`)
        let mut searched = 0;
        let mut cy = cost(F::FxVoice, 1);
        let stepping = v.step != 0;
        let per = if stepping {
            v.step_note = v.step_note.wrapping_add(v.step);
            let (s, end) = (v.step_note, v.step_end);
            let down = (v.step as i16) < 0;
            let reached = if down { end as i16 >= s as i16 } else { end <= s };
            if reached {
                cy += cost(if down { F::StepDownEnd } else { F::StepUpEnd }, 1);
            }
            if v20 {
                // 2.0 stops at the end note; 1.3 keeps stepping and writes it
                v.note = s;
                if reached {
                    v.step = 0;
                    v.note = end;
                    v.step_note = end;
                }
            }
            Some(if reached { end } else { s })
        } else {
            let param = v.pattern as u8;
            let b: &Bus = hw.bus;
            cy += cost(F::NoStep, 1);
            match (v.pattern >> 8) & 0xf {
                // arpeggio
                1 => {
                    cy += cost(F::Arp, 1);
                    let idx = match timer {
                        1 | 5 => {
                            cy += cost(F::ArpHigh, 1);
                            (param >> 4) as u32
                        }
                        2 | 4 => {
                            cy += cost(F::ArpLow, 1);
                            (param & 0xf) as u32
                        }
                        _ => 0x100,
                    };
                    if idx == 0x100 {
                        Some(v.note)
                    } else {
                        let (at, n) = find(b, table, v.note, v20);
                        searched = n * t.arp_entry;
                        cy += cost(F::ArpEntry, n);
                        at.map(|a| b.r16(a + 2 * idx))
                    }
                }
                // pitch bend: the row's period word changes
                2 => {
                    cy += cost(F::Bend, 1);
                    let w = (v.pattern >> 16) as u16;
                    let keep = if v20 { w & 0x1000 } else { 0 };
                    let (up, down) = ((param >> 4) as u16, (param & 0xf) as u16);
                    let p = if up != 0 {
                        cy += cost(F::BendUp, 1);
                        Some((w & !keep).wrapping_add(up))
                    } else if down != 0 {
                        cy += cost(F::BendDown, 1);
                        Some((w & !keep).wrapping_sub(down))
                    } else {
                        None
                    };
                    if let Some(p) = p {
                        v.pattern = (v.pattern & 0xffff) | ((p | keep) as u32) << 16;
                    }
                    p
                }
                // the LED filter (NostalgicPlayer's names: 3 is "LED on")
                3 if v20 => {
                    hw.bus.ciaa.pra |= 2;
                    None
                }
                4 if v20 => {
                    hw.bus.ciaa.pra &= !2;
                    None
                }
                // step up or down to the note `param >> 4` away
                c @ (7 | 8) => {
                    let down = c == 8;
                    let from = if v20 { v.note } else { (v.pattern >> 16) as u16 };
                    let s = (param & 0xf) as u16;
                    v.step = if down { s.wrapping_neg() } else { s };
                    v.step_note = from;
                    cy += cost(if down { F::SetDown } else { F::SetUp }, 1);
                    // 1.3 and 2.0 both check the end mark here
                    let mut a = table;
                    v.step_end = loop {
                        let p = b.r16(a);
                        if p == 0xffff {
                            break from;
                        }
                        if p == from {
                            cy += cost(if down { F::DownFound } else { F::UpFound }, 1);
                            let off = 2 * (param >> 4) as i32;
                            break b.r16(a.wrapping_add(if down { off } else { -off } as u32));
                        }
                        a += 2;
                        searched += t.step_entry;
                        cy += cost(if down { F::DownEntry } else { F::UpEntry }, 1);
                    };
                    None
                }
                _ => None,
            }
        };
        self.cy += cy;
        if let Some(p) = per {
            self.wait20(hw, t.fx_per + searched);
            self.c(F::FxPer);
            self.call(hw);
            hw.per(i, p);
            self.wait20(hw, t.fx_post);
        } else {
            self.wait20(hw, t.fx_none + searched);
        }
        if !stepping {
            self.c(F::FxOn);
        }
    }
}

/// The arpeggio's search for `note` in the period table: 2.0 stops at a negative
/// word, 1.3 only at the note. Returns where it is, and the words passed.
fn find(b: &Bus, table: u32, note: u16, v20: bool) -> (Option<u32>, u64) {
    for n in 0..T13_SEARCH {
        let a = table + 2 * n;
        let p = b.r16(a);
        if v20 && (p as i16) < 0 {
            return (None, n as u64);
        }
        if p == note {
            return (Some(a), n as u64);
        }
    }
    (None, T13_SEARCH as u64)
}

include!("soundfx_tables.rs");

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// One position of one pattern whose first row plays period 428 on voice 0 with
    /// a sample of 32 words (volume 48, loop of one word): 1.3's sample 1, or 2.0's
    /// sample 17 (bit 12 of the period word is the sample number's bit 4).
    fn module(v20: bool) -> Vec<u8> {
        let (n, info, song_len, patterns) = if v20 { (16, 0x90, 0x432, 0x4b8) } else { (0, 0x50, 0x212, 0x294) };
        let mut d = vec![0u8; patterns + 0x400 + 64];
        d[4 * n..4 * n + 4].copy_from_slice(&64u32.to_be_bytes());
        d[if v20 { 124..128 } else { 60..64 }].copy_from_slice(if v20 { b"SO31" } else { b"SONG" });
        let e = info + 30 * n + 22;
        d[e..e + 8].copy_from_slice(&[0, 32, 0, 48, 0, 0, 0, 1]);
        d[song_len] = 1;
        let row: u32 = if v20 { 0x11ac_1000 } else { 0x01ac_1000 };
        d[patterns..patterns + 4].copy_from_slice(&row.to_be_bytes());
        for (i, b) in d[patterns + 0x400..].iter_mut().enumerate() {
            *b = 0x40 + i as u8;
        }
        d
    }

    #[test]
    fn plays_a_note_on_paula() {
        for v20 in [false, true] {
            let mut e = Engine::new(48000.0);
            e.open_replayers = true;
            e.m.bus.write_log = Some(Vec::new());
            e.play_data(Path::new("test.sfx"), module(v20)).unwrap();
            let f = if v20 { "SoundFX2.0" } else { "SoundFX1.3" };
            assert_eq!(e.playing.as_ref().unwrap().format, f);
            let mut out = vec![0f32; 2 * 9600];
            e.render(&mut out);
            let log = e.m.bus.write_log.take().unwrap();
            let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
            // the first row, on the sixth tick: volume, DMA off, the sample, DMA on,
            // then every voice's loop (voice 0's is the sample's first word)
            assert_eq!(reg(0xDFF0A8), [48]);
            assert_eq!(&reg(0xDFF096)[..2], &[0x0001, 0x8001]);
            assert_eq!(&reg(0xDFF0A4)[..2], &[32, 1]);
            assert_eq!(reg(0xDFF0A6), [428]);
            assert_eq!(reg(0xDFF0B4)[0], 0);
            let (hi, lo) = (reg(0xDFF0A0), reg(0xDFF0A2));
            assert_eq!((hi[0], lo[0]), (hi[1], lo[1]));
            // the sample's first long was cleared, the rest is as loaded
            let a = (hi[0] as u32) << 16 | lo[0] as u32;
            assert_eq!(e.m.bus.r32(a), 0);
            assert_eq!(e.m.bus.r8(a + 4), 0x44);
            assert!(out.iter().any(|&s| s != 0.0), "no sound");
        }
    }
}
