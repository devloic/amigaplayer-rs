//! Quadra Composer (IFF EMOD, `FORM....EMODEMIC`), ported from NostalgicPlayer's
//! QuadraComposer player (Copyright (c) 2023 Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer).
//!
//! Where the original player's register writes differ from what NostalgicPlayer
//! asks of its mixer, this follows the original: UADE's QuadraComposer file (V1.01 of
//! 24 Jul 1994, sha1 4276dbed...; it names itself "IFF EMOD"), observed black-box in the
//! engine (write log, DMA log, traces; no player code read). EP206's "IFF EMOD" is another
//! build with a DMA gap of 110 colour clocks (a voice at a period like 538 keeps its old
//! sample) and loop registers of 0 / length 0 after a one-shot sample (Paula then plays
//! 128 KB from address 0), and the DeliPlayer QuadraComposer (V1.4) ticks every 71050
//! colour clocks at tempo 125 instead of 70940: not followed.
//!
//! The samples are played from the module in guest memory (the 8SMP chunk). A row's DMA
//! goes off and on once for all its notes, after the writes of the voices' new samples
//! (the host's wait of ten raster lines after each), and the loop registers of all four
//! voices follow it: a sample without a loop gets a loop of one zeroed word.
//!
//! Every write of the original is reproduced in value, order and colour clock by a cost
//! model of the original's pass over the voices (`cost_row`, `cost_fx`: fitted to the
//! original's instruction counts) except where noted: effect 6 reads its wave from the
//! original's table of effect addresses (a word of which is stood for by one constant),
//! the tremolo is exact only up to a point, a sample number the module lacks is played
//! as volume 0 and one word at address 0 (the original's address is not the module's),
//! and some write times are 1 or 2 colour clocks off.

use super::quadracomposer_tables::{PERIODS, VIBRATO};
use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Quadra Composer", detect, new };

/// What the vibrato of effect 6 reads as its wave: the original's 32 effect addresses, where
/// the reference run has the player ($A69B88 to $A6A238). An address's upper word, and one
/// word that stands for the lower words (the table itself is the player's and is not kept
/// here): with the depths the modules use (1 in the runs) every lower word gives the same step.
const EFFECT_ADDR_HIGH: i16 = 0x00a6;
const EFFECT_ADDR_LOW: i16 = 0x9e00u16 as i16;

#[derive(Clone, Default)]
struct Sample {
    vol: u8,
    /// Bytes.
    length: u32,
    finetune: u8,
    looped: bool,
    loop_start: u32,
    loop_len: u32,
    /// Where the sample data lies in guest memory.
    addr: u32,
}

#[derive(Clone, Default)]
struct Pattern {
    rows: u8,
    /// (rows + 1) rows of four voices' [sample, note, effect, argument].
    cells: Vec<[u8; 4]>,
}

#[derive(Clone, Copy, Default)]
struct Cell {
    sample: u8,
    note: i8,
    effect: u8,
    arg: u8,
}

#[derive(Clone, Default)]
struct Chan {
    cell: Cell,
    loop_start: u32,
    loop_len: u32,
    period: u16,
    volume: u16,
    length: u32,
    start: u32,
    note_nr: i16,
    wanted_period: u16,
    port_up: bool,
    vib_wave: u8,
    gliss: u8,
    vib_cmd: u8,
    vib_pos: u16,
    trem_wave: u8,
    trem_cmd: u8,
    trem_pos: u16,
    sample_offset: u8,
    retrig: u8,
    port_speed: u16,
    finetune: u8,
    addr: u32,
}

pub struct QuadraComposer {
    title: String,
    samples: Vec<Option<Sample>>,
    patterns: Vec<Option<Pattern>>,
    positions: Vec<u8>,
    // global playing info
    position: u16,
    new_position: u16,
    break_row: u16,
    new_row: u16,
    row: u16,
    loop_row: u16,
    pattern_wait: u8,
    tempo: u16,
    speed: u16,
    speed_count: u16,
    new_position_flag: bool,
    jump_break_flag: bool,
    loop_count: u8,
    intro_row: bool,
    set_tempo: bool,
    /// The CIA timer value for the tempo.
    timer: u32,
    ch: [Chan; 4],
    /// Voices whose DMA the row's notes start.
    dma_mask: u16,
    songend: bool,
    visited: Vec<bool>,
    /// The tick's write schedule (see `Cost`).
    sched: Sched,
    /// The EMIC chunk's counts of samples and of patterns, which the original's init time depends on.
    init_counts: (u64, u64),
}

fn detect(d: &[u8]) -> bool {
    d.len() >= 64 && &d[0..4] == b"FORM" && &d[8..16] == b"EMODEMIC" && be16(d, 20) == 1
}

fn text(d: &[u8], o: usize, n: usize) -> String {
    let s = d.get(o..o + n).unwrap_or(&[]);
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    s[..end].iter().map(|&c| c as char).collect::<String>().trim_end().to_string()
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let mut samples: Vec<Option<Sample>> = vec![None; 256];
    let mut patterns: Vec<Option<Pattern>> = vec![None; 256];
    let mut positions = Vec::new();
    let mut title = String::new();
    let mut start_tempo = 0;
    let mut counts = (0u64, 0u64);
    let mut patt: Option<usize> = None;
    let mut smp: Option<usize> = None;
    let mut fields: Vec<usize> = Vec::new();

    // the chunks, as NostalgicPlayer reads them
    let mut o = 12usize;
    while o + 8 <= d.len() {
        let name = &d[o..o + 4];
        let size = be32(d, o + 4) as usize;
        let body = o + 8;
        if size > d.len() - body {
            return None;
        }
        match name {
            b"EMIC" => {
                let mut p = body + 2;
                title = text(d, p, 20);
                p += 40;
                start_tempo = *d.get(p)?;
                let ns = *d.get(p + 1)? as usize;
                counts.0 = ns as u64;
                p += 2;
                for _ in 0..ns {
                    let e = d.get(p..p + 34)?;
                    let number = e[0] as usize;
                    if number == 0 {
                        return None;
                    }
                    samples[number - 1] = Some(Sample {
                        vol: e[1],
                        length: be16(e, 2) as u32 * 2,
                        looped: e[24] & 1 != 0,
                        finetune: e[25] & 0x0f,
                        loop_start: be16(e, 26) as u32 * 2,
                        loop_len: be16(e, 28) as u32 * 2,
                        addr: 0,
                    });
                    fields.push(p + 30);
                    p += 34;
                }
                p += 1;
                let np = *d.get(p)? as usize;
                counts.1 = np as u64;
                p += 1;
                for _ in 0..np {
                    let e = d.get(p..p + 26)?;
                    patterns[e[0] as usize] = Some(Pattern { rows: e[1], cells: Vec::new() });
                    fields.push(p + 22);
                    p += 26;
                }
                let npos = *d.get(p)? as usize;
                p += 1;
                positions = d.get(p..p + npos)?.to_vec();
            }
            b"PATT" => patt = Some(body),
            b"8SMP" => smp = Some(body),
            _ => {}
        }
        o = body + size;
        // the chunks are not padded in the way IFF asks: NostalgicPlayer takes the next one
        // straight after
    }

    // the patterns follow each other in the PATT chunk, by number
    let mut p = patt?;
    for pat in patterns.iter_mut().flatten() {
        let n = (pat.rows as usize + 1) * 4;
        let b = d.get(p..p + n * 4)?;
        pat.cells = b.chunks(4).map(|c| [c[0], c[1], c[2] & 0x0f, c[3]]).collect();
        p += n * 4;
    }
    // and the samples in the 8SMP chunk
    let mut p = smp?;
    for s in samples.iter_mut().flatten() {
        if s.length == 0 {
            continue;
        }
        if p + s.length as usize > d.len() {
            return None;
        }
        s.addr = base + p as u32;
        p += s.length as usize;
    }
    if positions.is_empty() || positions.iter().any(|&n| patterns[n as usize].is_none()) {
        return None;
    }

    let first = patterns[positions[0] as usize].as_ref()?.rows as u16;
    let n = positions.len();
    let mut q = QuadraComposer {
        title,
        samples,
        patterns,
        positions,
        position: 0,
        new_position: 0,
        break_row: first,
        new_row: 0,
        row: 0,
        loop_row: 0,
        pattern_wait: 0,
        tempo: start_tempo as u16,
        speed: 6,
        speed_count: 6,
        new_position_flag: false,
        jump_break_flag: false,
        loop_count: 0,
        intro_row: true,
        set_tempo: true,
        timer: QuadraComposer::timer_for(start_tempo as u16),
        ch: Default::default(),
        dma_mask: 0,
        songend: false,
        visited: vec![false; n],
        sched: Sched::default(),
        init_counts: counts,
    };
    q.visited[0] = true;
    // the original makes addresses of the offsets in the module's sample and pattern records
    // (measured: the module's memory after its start); a voice that runs on into the memory
    // plays them
    for f in fields {
        m.bus.w32(base + f as u32, be32(d, f).wrapping_add(base));
    }
    // and clears the first word of every sample (measured: the module's memory after its init)
    for s in q.samples.iter().flatten().filter(|s| s.length != 0) {
        m.bus.w16(s.addr, 0);
    }
    // What the original does before its tick timer starts, in colour clocks (traced: DELI_TRACE,
    // 27 modules, exact). Its module check takes 902 plus 9 per sample and pattern entry of the
    // EMIC chunk; the host then sets the volume twice, and each time the volume routine writes 0
    // to the four voices; the init routine asks for the timer (a host call) after (3135 + 49 per
    // sample entry) / 2, sets the LED bit of CIA-A's port 54 later (the filter is off), turns the
    // DMA of the four voices off 272 + 62 per sample entry + 48 per pattern entry after that
    // and writes their volumes 0, and returns 8 after the last. Nothing in it waits for the beam
    // or a timer, so this alone sets the phase of the ticks against the raster. The song starts
    // (the engine aligns Paula's output and line grids there) at the timer's start, after the
    // init, and the timer then runs a plain period.
    let (ns, np) = q.init_counts;
    let mut hw = Hw { bus: &mut m.bus };
    hw.wait(902 + 9 * (ns + np));
    for _ in 0..2 {
        for (i, w) in [157, 156, 168, 162].into_iter().enumerate() {
            hw.wait(w);
            hw.vol(i, 0);
        }
        hw.wait(692 - 643);
    }
    hw.wait((3135 + 49 * ns).div_ceil(2) + 54);
    let pra = hw.bus.ciaa.pra | 2;
    hw.bus.write_byte(0xBFE001, pra);
    hw.wait(272 + 62 * ns + 48 * np);
    hw.dmacon(0x000f);
    for i in 0..4 {
        hw.wait(10);
        hw.vol(i, 0);
    }
    hw.wait(8);
    Some(Box::new(q))
}

impl Replayer for QuadraComposer {
    fn format(&self) -> &'static str {
        "Quadra Composer"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick_clocks(&self) -> u64 {
        (self.timer as u64 + 1) * 5
    }

    fn tick(&mut self, hw: &mut Hw) {
        let t0 = hw.bus.clock;
        self.change_tempo();
        self.dma_mask = 0;
        self.speed_count += 1;
        if self.speed_count < self.speed {
            self.plan(t0, false);
            self.run_tick_effects(hw);
        } else if self.pattern_wait != 0 {
            self.pattern_wait -= 1;
            self.speed_count = 0;
            self.plan(t0, false);
            self.run_tick_effects(hw);
        } else {
            self.get_notes(hw, t0);
        }
        self.start_voices(hw);
    }
}

/// Where the original's writes fall in a tick, in CPU cycles after the voice's start, counted
/// in its executed instructions (a trace of every instruction of the QuadraComposer Eagleplayer on 27
/// modules). Its pass over the four voices spends `v` on a voice and writes the voice's registers
/// at `o[n]`. A tick that starts a row and a tick that only runs the effects are different
/// routines; the cost of a voice depends on its cell, on its sample (a loop, the volume 64, a
/// length of one word), on its place in the pass and on what the effect finds (a slide that
/// reaches its end).
#[derive(Clone, Copy, Default)]
struct Cost {
    v: i64,
    n: usize,
    o: [i64; 6],
    /// A call of the host inside the voice (the tempo effect sets the timer), cycles after its start.
    host: Option<i64>,
}

impl Cost {
    fn new(v: i64, o: &[i64]) -> Cost {
        let mut c = Cost { v, n: o.len().min(6), o: [0; 6], host: None };
        c.o[..c.n].copy_from_slice(&o[..c.n]);
        c
    }

    fn push(&mut self, o: i64) {
        if self.n < 6 {
            self.o[self.n] = o;
            self.n += 1;
        }
    }

    /// `k`'s code and writes after this one's, `skip` cycles of it left out.
    fn then(mut self, k: Cost, skip: i64) -> Cost {
        let at = self.v - skip;
        for &o in &k.o[..k.n] {
            self.push(at + o);
        }
        self.v = at + k.v;
        self
    }
}

#[derive(Default)]
struct Sched {
    t0: u64,
    /// CPU cycles after the tick's start.
    vstart: [i64; 4],
    widx: [u8; 4],
    cls: [Cost; 4],
    end: i64,
    /// The colour clock and the cycle of the tick's start or of the last call of the host (the
    /// tempo effect sets the timer): the engine counts the guest's time from there in whole colour
    /// clocks, so an odd cycle is lost at each.
    base: (u64, i64),
}

/// CPU cycles from the start of a tick to its pass over the voices, in a tick that starts a row
/// and in one that runs the effects, and from the pass's end to the DMA going off.
const ROW_LEAD: i64 = 464;
const FX_LEAD: i64 = 367;
const TAIL: i64 = 17;

/// What a voice's place in the pass adds to the code that fetches a sample, and to the code that
/// writes an effect's volume.
const POS_SAMPLE: [i64; 4] = [0, 54, 108, 106];
const POS_VOL: [i64; 4] = [0, 23, 46, 36];

/// What the cost of a voice depends on besides its cell.
#[derive(Clone, Copy, Default)]
struct Feat {
    /// The sample the cell names (or, without one, the voice's) loops.
    looped: bool,
    /// The cell names a sample number the module does not have.
    missing: bool,
    /// The voice's volume and period, and what its tone portamento will do: nothing (no target),
    /// move, arrive.
    volume: u16,
    period: u16,
    porta: u8,
    /// The tick's number in the row.
    count: u16,
    /// A delayed or retriggered note starts in this tick.
    due: bool,
}

/// The pass over the voices of a tick that starts a row. `v` is the voice's place in the pass.
fn cost_row(c: Cell, f: Feat, v: usize) -> Cost {
    let (e, a) = (c.effect, c.arg);
    let (s, n) = (c.sample != 0, c.note >= 0);
    let lp = f.looped as i64;
    let mut k = Cost::default();
    // the sample's volume
    let vol = if n { 691 } else { 649 } + 4 * lp + 4 * f.missing as i64 + POS_SAMPLE[v];
    if s {
        k.push(vol);
    }
    let mut end = match (s, n) {
        (false, false) => 197,
        (true, false) => 934 + 74 * lp + POS_SAMPLE[v],
        (_, true) => {
            let after = if s { vol } else { 197 };
            // a note that starts nothing now: the glide's target, a delayed note, the finetune
            match (e, a & 0xf0) {
                (3 | 5, _) => return Cost { v: after + 387 + 70 * lp, ..k },
                // a delay of no tick (and a retrigger at once): the sample starts by the effect's
                // own code
                (0xe, 0x90 | 0xd0) if f.due => {
                    let loc = after + 455;
                    k.push(loc);
                    k.push(loc + 20);
                    k.push(loc + 40);
                    return Cost { v: loc + 40 + 131, ..k };
                }
                (0xe, 0xd0) => return Cost { v: after + 449 + 70 * lp, ..k },
                (0xe, 0x50) => return Cost { v: after + 448 + 70 * lp, ..k },
                _ => {}
            }
            // (a note without a sample was seen on the first voice only)
            let loc = if s { vol + 408 + 70 * lp } else { 320 };
            k.push(loc);
            k.push(loc + 20);
            k.push(loc + 40);
            loc + 40 + 131
        }
    };
    match e {
        0 if a != 0 => {
            end += 84;
            k.push(end - 50);
        }
        0 => {}
        1..=7 | 0xa => end -= 23,
        // the sample offset writes the address and the length again
        9 if s && n => {
            // after the note's own start: the writes come later and the end earlier (fitted to
            // 26db4185, fbca0a87, fc79472d; the argument 0 is not in them)
            end += if a != 0 { 172 } else { 160 };
            k.push(end - 69);
            k.push(end - 49);
        }
        9 => {
            end += if a != 0 { 214 } else { 202 };
            k.push(end - 160);
            k.push(end - 140);
        }
        0xc => {
            end += 239 + POS_VOL[v] + if a >= 64 { 4 } else { 0 };
            k.push(end - 78);
        }
        // the position jump (the two seen: 42 cycles)
        0xb => end += 42,
        0xd => end += 57,
        0xf if a >= 0x20 => {
            end += 402;
            k.host = Some(end - 118);
        }
        0xf => end += 59,
        0xe => match a >> 4 {
            1 | 2 => {
                end += 124;
                k.push(end - 50);
            }
            0xa => {
                end += 331 + POS_VOL[v] + if f.volume as i64 + (a & 0x0f) as i64 >= 64 { 4 } else { 0 };
                k.push(end - 78);
            }
            0xb => {
                end += 315 + POS_VOL[v] + if f.volume as i64 - ((a & 0x0f) as i64) < 0 { 8 } else { 0 };
                k.push(end - 78);
            }
            9 => end += 116,
            _ => {}
        },
        _ => {}
    }
    k.v = end;
    k
}

/// The volume slide of a tick that runs the effects: up or down, and the end of the range.
fn cost_slide(c: Cell, f: Feat, v: usize) -> Cost {
    let up = (c.arg >> 4) as i64;
    let d = if up != 0 {
        if f.volume as i64 + up >= 64 { 8 } else { -8 }
    } else if f.volume as i64 - ((c.arg & 0x0f) as i64) <= 0 {
        8
    } else {
        0
    };
    let end = 431 + POS_VOL[v] + d;
    Cost::new(end, &[end - 78])
}

/// The pass of a tick that only runs the effects.
fn cost_fx(c: Cell, f: Feat, v: usize) -> Cost {
    let (e, a) = (c.effect, c.arg);
    let k = Cost::new;
    let porta = match f.porta {
        0 => k(135, &[]),
        1 => k(291, &[241]),
        _ => k(288, &[210]),
    };
    // a note that starts in this tick: the delayed one, the retriggered one
    let start = match (a >> 4 == 9, f.due) {
        (false, true) => k(319, &[229, 249, 269]),
        (false, false) => k(223, &[]),
        (true, true) => k(347, &[257, 277, 297]),
        (true, false) => k(235, &[]),
    };
    // a slide that runs into the end of the period range
    let stop = if e == 1 { (f.period as i64 - a as i64) < 113 } else { f.period as i64 + a as i64 > 856 };
    // the vibrato takes each nibble of its argument that is not 0
    let vib = 437 + if a != 0 { 42 + if a & 0x0f != 0 { 36 } else { 0 } + if a & 0xf0 != 0 { 36 } else { 0 } } else { 0 };
    match e {
        0 if a != 0 => match f.count % 3 {
            1 => k(272, &[222]),
            2 => k(273, &[223]),
            _ => k(219, &[169]),
        },
        0 => k(135, &[]),
        1 | 2 if stop => k(207, &[157]),
        1 | 2 => k(191, &[141]),
        3 => porta,
        4 => k(vib, &[vib - 50]),
        // (the tremolo: 246 cycles more than the vibrato, measured with arguments with both nibbles)
        7 => k(vib + 246, &[vib + 246 - 78]),
        // the slide after the glide, and after the period write of the vibrato that has no depth
        5 => porta.then(cost_slide(c, f, v), 135 - 23),
        6 => k(425, &[375]).then(cost_slide(c, f, v), 135),
        0xa => cost_slide(c, f, v),
        0xe => match a >> 4 {
            9 | 0xd => start,
            0xc if f.due => k(166, &[88]),
            _ => k(176, &[]),
        },
        _ => k(112, &[]),
    }
}

impl QuadraComposer {
    fn timer_for(tempo: u16) -> u32 {
        if tempo == 0 { crate::deli::DEFAULT_TIMER as u32 } else { 1773447 / tempo as u32 }
    }

    fn change_tempo(&mut self) {
        if self.set_tempo {
            self.set_tempo = false;
            // the CIA timer of a tempo: 1773447 / tempo E-clock ticks, a PAL machine's constant
            // (UADE's own run: 70940 colour clocks a tick at tempo 125, measured at tempos 125 and
            // 160). The engine's run of the original uses the NTSC 1789773 / tempo (71595 at tempo
            // 125, 0.9 % slower) whatever the host sets, so the port is a TEMPO row in refcmp
            // against it, though equal in every write otherwise.
            self.timer = Self::timer_for(self.tempo);
        }
    }

    fn cell(&self, ch: usize) -> Cell {
        let pat = self.patterns[self.positions[self.position as usize] as usize].as_ref().unwrap();
        let c = pat.cells.get(self.row as usize * 4 + ch).copied().unwrap_or_default();
        Cell { sample: c[0], note: c[1] as i8, effect: c[2], arg: c[3] }
    }

    fn get_notes(&mut self, hw: &mut Hw, t0: u64) {
        if !self.intro_row {
            self.change_tempo();
            if self.new_position_flag {
                self.new_position_flag = false;
                self.position = self.new_position;
                self.init_position();
            } else if self.jump_break_flag {
                self.jump_break_flag = false;
                if self.loop_row <= self.break_row {
                    self.row = self.loop_row;
                }
            } else {
                self.row += 1;
                if self.row > self.break_row {
                    self.position += 1;
                    self.init_position();
                }
            }
        }
        self.intro_row = false;
        self.speed_count = 0;
        self.plan(t0, true);
        for i in 0..4 {
            let c = self.cell(i);
            self.play_note(i, c, hw);
        }
    }

    /// The DMA of the voices whose samples this tick started goes off and on together
    /// (the original does that once it has been over the four voices), and the loop registers
    /// of all four voices follow after the routine has waited ten raster lines.
    fn start_voices(&mut self, hw: &mut Hw) {
        if self.dma_mask != 0 {
            let m = self.dma_mask;
            let end = self.sched.end;
            self.until(end, hw);
            hw.dmacon(m);
            // the host's dtg_WaitAudioDMA (10 raster lines from the start of the line) after the
            // DMA goes off and after it goes on
            hw.wait(24);
            hw.wait_lines(10, 0);
            hw.wait(32);
            hw.dmacon(0x8000 | m);
            hw.wait(24);
            hw.wait_lines(10, 0);
            hw.wait(40);
            for i in 0..4 {
                self.write_loop(i, hw);
            }
        }
    }

    /// The voice's loop: a sample that does not loop goes on with the word at address 0, one
    /// word long.
    fn write_loop(&mut self, i: usize, hw: &mut Hw) {
        let c = &self.ch[i];
        let (loc, len) = if c.loop_len != 0 { (c.addr + c.loop_start, (c.loop_len / 2) as u16) } else { (0, 1) };
        if i > 0 {
            hw.wait(14);
        }
        hw.loc(i, loc);
        hw.wait(10);
        hw.len(i, len);
    }

    /// What voice `i`'s cost depends on, for the cell `c` it is about to play or run.
    fn feat(&self, i: usize, c: Cell, new: bool) -> Feat {
        let ch = &self.ch[i];
        let s = if new && c.sample != 0 { self.samples.get(c.sample as usize - 1).and_then(|s| s.as_ref()) } else { None };
        let missing = new && c.sample != 0 && s.is_none();
        let looped = match s {
            Some(s) => s.looped,
            None => ch.loop_len != 0,
        };
        // what the tone portamento of this tick will do (`actual_tone_portamento`)
        let speed = if c.effect == 3 && c.arg != 0 { c.arg as u16 } else { ch.port_speed };
        let porta = if ch.wanted_period == 0 {
            0
        } else if ch.port_up {
            1 + (ch.period.wrapping_add(speed) >= ch.wanted_period) as u8
        } else {
            1 + (ch.period.wrapping_sub(speed) as i16 <= ch.wanted_period as i16) as u8
        };
        let due = c.effect == 0xe
            && match c.arg & 0xf0 {
                // (a row starts the count again)
                0x90 => (if new { 1 } else { ch.retrig.wrapping_add(1) }) >= c.arg & 0x0f,
                0xc0 => (c.arg & 0x0f) as u16 <= self.speed_count,
                0xd0 => ch.note_nr >= 0 && (c.arg & 0x0f) as u16 == self.speed_count,
                _ => false,
            };
        Feat { looped, missing, volume: ch.volume, period: ch.period, porta, count: self.speed_count, due }
    }

    /// Plans the tick's writes: where the original's pass over the voices is when it gets to each
    /// (`Cost`).
    fn plan(&mut self, t0: u64, new: bool) {
        let mut t = if new { ROW_LEAD } else { FX_LEAD };
        self.sched.t0 = t0;
        self.sched.base = (t0, 0);
        for i in 0..4 {
            let k = if new {
                let c = self.cell(i);
                cost_row(c, self.feat(i, c, true), i)
            } else {
                let c = self.ch[i].cell;
                cost_fx(c, self.feat(i, c, false), i)
            };
            self.sched.vstart[i] = t;
            self.sched.widx[i] = 0;
            self.sched.cls[i] = k;
            t += k.v;
        }
        // the DMA goes off when the pass is over
        self.sched.end = t + TAIL;
    }

    /// Lets the time pass to cycle `cy` of the tick, where the original calls the host (a register
    /// write does): the colour clocks are whole from one call to the next.
    fn until(&mut self, cy: i64, hw: &mut Hw) {
        // the calls of the host up to there
        for j in 0..4 {
            if let Some(h) = self.sched.cls[j].host {
                let h = self.sched.vstart[j] + h;
                if h > self.sched.base.1 && h <= cy {
                    self.sched.base = (self.sched.base.0 + ((h - self.sched.base.1) / 2) as u64, h);
                }
            }
        }
        let (clock, at) = self.sched.base;
        let t = clock + ((cy - at).max(0) / 2) as u64;
        if hw.bus.clock < t {
            hw.bus.clock = t;
        }
    }

    /// Lets the time pass to voice `i`'s next write of this tick.
    fn at(&mut self, i: usize, hw: &mut Hw) {
        let k = self.sched.cls[i];
        let w = self.sched.widx[i] as usize;
        self.sched.widx[i] = self.sched.widx[i].saturating_add(1);
        // a write the measured runs did not show: 100 colour clocks after the last one known
        let o = if w < k.n { k.o[w] } else if k.n > 0 { k.o[k.n - 1] + 200 * (w + 1 - k.n) as i64 } else { 200 * (w + 1) as i64 };
        let cy = self.sched.vstart[i] + o;
        self.until(cy, hw);
    }

    fn init_position(&mut self) {
        if self.position as usize >= self.positions.len() {
            self.position = 0;
        }
        if self.visited[self.position as usize] {
            self.songend = true;
        }
        self.visited[self.position as usize] = true;
        let pat = self.patterns[self.positions[self.position as usize] as usize].as_ref().unwrap();
        self.break_row = pat.rows as u16;
        self.row = self.new_row;
        self.new_row = 0;
        if self.break_row < self.row {
            self.row = self.break_row;
        }
    }

    fn period(&self, ft: u8, note: i16) -> u16 {
        PERIODS[(ft & 15) as usize].get(note as usize).copied().unwrap_or(0)
    }

    /// Starts the voice's sample from `start`, its registers; DMA is the caller's.
    fn sample_regs(&mut self, i: usize, hw: &mut Hw) {
        let c = &self.ch[i];
        let (a, words) = (c.addr + c.start, (c.length.wrapping_sub(c.start) / 2) as u16);
        self.at(i, hw);
        hw.loc(i, a);
        self.at(i, hw);
        hw.len(i, words);
    }

    /// Plays the voice's sample again, at its period.
    fn retrigger(&mut self, i: usize, hw: &mut Hw) {
        self.sample_regs(i, hw);
        let p = self.ch[i].period;
        self.set_period(i, hw, p);
        self.dma_mask |= 1 << i;
    }

    fn set_period(&mut self, i: usize, hw: &mut Hw, p: u16) {
        self.at(i, hw);
        hw.per(i, p);
    }

    fn set_vol(&mut self, i: usize, hw: &mut Hw, v: u16) {
        self.at(i, hw);
        hw.vol(i, v);
    }

    fn play_note(&mut self, i: usize, c: Cell, hw: &mut Hw) {
        self.ch[i].cell = c;
        if c.sample != 0 {
            // a number without a sample plays what the original's table holds there: a volume 0
            // and a word of memory (measured: the address is not one the module gives; the
            // port's is 0)
            let s = self.samples[c.sample as usize - 1].clone().unwrap_or(Sample { length: 2, ..Sample::default() });
            let ch = &mut self.ch[i];
            ch.volume = s.vol as u16;
            ch.length = s.length;
            ch.finetune = s.finetune;
            ch.addr = s.addr;
            ch.start = 0;
            if s.looped {
                ch.loop_start = s.loop_start;
                ch.length = s.loop_start + s.loop_len;
                ch.loop_len = s.loop_len;
            } else {
                ch.loop_start = 0;
                ch.loop_len = 0;
            }
            self.set_vol(i, hw, s.vol as u16);
        }
        if c.note >= 0 {
            self.ch[i].note_nr = c.note as i16;
            if c.effect == 0xe && c.arg & 0xf0 == 0x50 {
                self.ch[i].finetune = c.arg & 0x0f;
            } else if c.effect == 3 || c.effect == 5 {
                let ch = &mut self.ch[i];
                ch.wanted_period = PERIODS[(ch.finetune & 15) as usize].get(ch.note_nr as usize).copied().unwrap_or(0);
                ch.port_up = ch.wanted_period > ch.period;
                return;
            }
            let p = self.period(self.ch[i].finetune, self.ch[i].note_nr);
            self.ch[i].period = p;
            if c.effect == 0xe && c.arg & 0xf0 == 0xd0 {
                self.note_delay(i, hw);
                return;
            }
            self.sample_regs(i, hw);
            self.set_period(i, hw, p);
            self.dma_mask |= 1 << i;
        }
        self.after_period_effect(i, hw);
    }

    fn run_tick_effects(&mut self, hw: &mut Hw) {
        for i in 0..4 {
            let c = self.ch[i].cell;
            match c.effect {
                0 => self.arpeggio(i, hw),
                1 => self.slide_up(i, hw),
                2 => self.slide_down(i, hw),
                3 => self.tone_portamento(i, hw),
                4 => self.vibrato(i, hw),
                5 => {
                    if self.ch[i].wanted_period != 0 {
                        self.actual_tone_portamento(i, hw);
                    }
                    self.volume_slide(i, hw);
                }
                6 => {
                    // the original's vibrato here goes on by the vibrato's speed, but takes its
                    // wave 256 bytes below the sine: from the player's table of 32 effect
                    // addresses (measured: the wave address and the words there). An even place
                    // is an address's upper word, which the depth cannot bring to one step; an
                    // odd place is the lower word of the address as the reference run loads the
                    // player, a step or more down (two at depth 1; from depth 2 on the one word
                    // taken here for all of them can be a step off the reference).
                    let ch = &mut self.ch[i];
                    ch.vib_pos = (ch.vib_pos + (ch.vib_cmd >> 4) as u16) & 0x3f;
                    let pos = ch.vib_pos as usize;
                    let v = if pos & 1 == 0 { EFFECT_ADDR_HIGH } else { EFFECT_ADDR_LOW } as i32;
                    let np = (ch.period as i16 as i32 + (((ch.vib_cmd & 0x0f) as i32 * v) >> 14)) as i16;
                    let p = np.clamp(113, 856) as u16;
                    self.set_period(i, hw, p);
                    self.volume_slide(i, hw);
                }
                7 => self.tremolo(i, hw),
                0xa => self.volume_slide(i, hw),
                0xe => match c.arg & 0xf0 {
                    0x90 => self.retrig_note(i, hw),
                    0xc0 => self.note_cut(i, hw),
                    0xd0 => self.note_delay(i, hw),
                    _ => {}
                },
                _ => {}
            }
        }
    }

    fn after_period_effect(&mut self, i: usize, hw: &mut Hw) {
        let c = self.ch[i].cell;
        match c.effect {
            0 => self.arpeggio(i, hw),
            9 => self.set_sample_offset(i, hw),
            0xb => {
                self.new_position = c.arg as u16;
                self.new_position_flag = true;
                self.new_row = 0;
            }
            0xc => {
                let v = (c.arg as u16).min(64);
                self.ch[i].volume = v;
                self.set_vol(i, hw, v);
            }
            0xd => {
                self.new_position = self.position + 1;
                self.new_row = c.arg as u16;
                self.new_position_flag = true;
            }
            0xe => match c.arg & 0xf0 {
                0x10 => {
                    let ch = &mut self.ch[i];
                    ch.period = ch.period.wrapping_sub((c.arg & 0x0f) as u16);
                    if ch.period < 113 {
                        ch.period = 113;
                    }
                    let p = ch.period;
                    self.set_period(i, hw, p);
                }
                0x20 => {
                    let ch = &mut self.ch[i];
                    ch.period = ch.period.wrapping_add((c.arg & 0x0f) as u16);
                    if ch.period > 856 {
                        ch.period = 856;
                    }
                    let p = ch.period;
                    self.set_period(i, hw, p);
                }
                0x30 => self.ch[i].gliss = c.arg & 0x0f,
                0x40 => self.ch[i].vib_wave = c.arg & 0x0f,
                0x50 => self.ch[i].finetune = c.arg & 0x0f,
                0x60 => {
                    let a = c.arg & 0x0f;
                    if a == 0 {
                        self.loop_row = self.row;
                    } else if self.loop_count == 0 {
                        self.loop_count = a;
                        self.jump_break_flag = true;
                    } else {
                        self.loop_count -= 1;
                        if self.loop_count != 0 {
                            self.jump_break_flag = true;
                        }
                    }
                }
                0x70 => self.ch[i].trem_wave = c.arg & 0x0f,
                0x90 => {
                    self.ch[i].retrig = 0;
                    self.retrig_note(i, hw);
                }
                0xa0 => {
                    let v = (self.ch[i].volume + (c.arg & 0x0f) as u16).min(64);
                    self.ch[i].volume = v;
                    self.set_vol(i, hw, v);
                }
                0xb0 => {
                    let v = (self.ch[i].volume as i16 - (c.arg & 0x0f) as i16).max(0) as u16;
                    self.ch[i].volume = v;
                    self.set_vol(i, hw, v);
                }
                0xc0 => self.note_cut(i, hw),
                0xd0 => self.note_delay(i, hw),
                0xe0 => self.pattern_wait = c.arg & 0x0f,
                _ => {}
            },
            0xf => {
                if c.arg > 31 {
                    // the original sets the timer here: the tick under way and the next one keep
                    // the old period (the latch is loaded at an underflow)
                    self.tempo = c.arg as u16;
                    self.timer = Self::timer_for(self.tempo);
                } else {
                    self.speed = (c.arg as u16).max(1);
                    self.speed_count = 0;
                }
            }
            _ => {}
        }
    }

    fn arpeggio(&mut self, i: usize, hw: &mut Hw) {
        let c = self.ch[i].cell;
        if c.arg != 0 {
            let p = match self.speed_count % 3 {
                0 => self.ch[i].period,
                n => {
                    let off = if n == 1 { c.arg >> 4 } else { c.arg & 0x0f };
                    let note = (off as i16 + self.ch[i].note_nr).min(35);
                    self.period(self.ch[i].finetune, note)
                }
            };
            self.set_period(i, hw, p);
        }
    }

    fn slide_up(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        ch.period = ch.period.wrapping_sub(ch.cell.arg as u16);
        if ch.period < 113 {
            ch.period = 113;
        }
        let p = ch.period;
        self.set_period(i, hw, p);
    }

    fn slide_down(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        ch.period = ch.period.wrapping_add(ch.cell.arg as u16);
        if ch.period > 856 {
            ch.period = 856;
        }
        let p = ch.period;
        self.set_period(i, hw, p);
    }

    fn tone_portamento(&mut self, i: usize, hw: &mut Hw) {
        if self.ch[i].wanted_period == 0 {
            return;
        }
        if self.ch[i].cell.arg != 0 {
            self.ch[i].port_speed = self.ch[i].cell.arg as u16;
        }
        self.actual_tone_portamento(i, hw);
    }

    fn actual_tone_portamento(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        let speed = ch.port_speed;
        if ch.port_up {
            ch.period = ch.period.wrapping_add(speed);
            if ch.period >= ch.wanted_period {
                ch.period = ch.wanted_period;
                ch.wanted_period = 0;
                let p = ch.period;
                self.set_period(i, hw, p);
                return;
            }
        } else {
            let np = ch.period.wrapping_sub(speed) as i16;
            if np <= ch.wanted_period as i16 {
                ch.period = ch.wanted_period;
                ch.wanted_period = 0;
                let p = ch.period;
                self.set_period(i, hw, p);
                return;
            }
            ch.period = np as u16;
        }
        let (gliss, ft, period) = (ch.gliss, ch.finetune, ch.period);
        if gliss != 0 {
            if let Some(&p) = PERIODS[(ft & 15) as usize].iter().find(|&&p| period >= p) {
                self.set_period(i, hw, p);
            }
        } else {
            self.set_period(i, hw, period);
        }
    }

    fn vibrato(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        let arg = ch.cell.arg;
        if arg != 0 {
            if arg & 0x0f != 0 {
                ch.vib_cmd = (ch.vib_cmd & 0xf0) | (arg & 0x0f);
            }
            if arg & 0xf0 != 0 {
                ch.vib_cmd = (ch.vib_cmd & 0x0f) | (arg & 0xf0);
            }
        }
        self.actual_vibrato(i, hw);
    }

    fn actual_vibrato(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        ch.vib_pos = (ch.vib_pos + (ch.vib_cmd >> 4) as u16) & 0x3f;
        let v = VIBRATO[(ch.vib_wave as usize).min(2)][ch.vib_pos as usize] as i32;
        let np = (ch.period as i16 as i32 + (((ch.vib_cmd & 0x0f) as i32 * v) >> 14)) as i16;
        let p = np.clamp(113, 856) as u16;
        self.set_period(i, hw, p);
    }

    fn tremolo(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        let arg = ch.cell.arg;
        if arg != 0 {
            if arg & 0x0f != 0 {
                ch.trem_cmd = (ch.trem_cmd & 0xf0) | (arg & 0x0f);
            }
            if arg & 0xf0 != 0 {
                ch.trem_cmd = (ch.trem_cmd & 0x0f) | (arg & 0xf0);
            }
        }
        ch.trem_pos = (ch.trem_pos + (ch.trem_cmd >> 4) as u16) & 0x3f;
        let v = VIBRATO[(ch.trem_wave as usize).min(2)][ch.trem_pos as usize] as i32;
        // (the original's tremolo scales by one bit more than its vibrato: measured, depth 3)
        let nv = (ch.volume as i32 + (((ch.trem_cmd & 0x0f) as i32 * v) >> 13)).clamp(0, 64) as u16;
        self.set_vol(i, hw, nv);
    }

    fn set_sample_offset(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        if ch.cell.arg != 0 {
            ch.sample_offset = ch.cell.arg;
        }
        let offset = ch.sample_offset as u32 * 512;
        // (the offset adds to where the voice's sample starts now, measured: a second offset
        // effect without a note moves on from the first one, past the end into the loop)
        if offset < ch.length.saturating_sub(ch.start) {
            ch.start += offset;
        } else {
            ch.start = ch.loop_start;
            ch.length = ch.loop_len;
        }
        if ch.length > 0 {
            self.sample_regs(i, hw);
        } else {
            // the loop is the empty word: the original only writes its length
            self.at(i, hw);
            self.at(i, hw);
            hw.len(i, 1);
        }
    }

    fn volume_slide(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        let mut v = ch.volume as i16;
        let up = (ch.cell.arg >> 4) as i16;
        if up != 0 {
            v = (v + up).min(64);
        } else {
            v = (v - (ch.cell.arg & 0x0f) as i16).max(0);
        }
        ch.volume = v as u16;
        self.set_vol(i, hw, v as u16);
    }

    fn retrig_note(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        ch.retrig = ch.retrig.wrapping_add(1);
        if ch.retrig >= ch.cell.arg & 0x0f {
            ch.retrig = 0;
            self.retrigger(i, hw);
        }
    }

    fn note_cut(&mut self, i: usize, hw: &mut Hw) {
        if (self.ch[i].cell.arg & 0x0f) as u16 <= self.speed_count {
            self.ch[i].volume = 0;
            self.set_vol(i, hw, 0);
        }
    }

    fn note_delay(&mut self, i: usize, hw: &mut Hw) {
        if self.ch[i].note_nr >= 0 && (self.ch[i].cell.arg & 0x0f) as u16 == self.speed_count {
            self.retrigger(i, hw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn chunk(out: &mut Vec<u8>, name: &[u8; 4], body: &[u8]) {
        out.extend_from_slice(name);
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(body);
    }

    /// A module of one position and one pattern of `rows` + 1 rows, two samples (the second
    /// looped from word 8, 8 words long), with `cells` at (row, voice); tempo 125.
    fn module(rows: u8, cells: &[(usize, usize, [u8; 4])]) -> Vec<u8> {
        let mut emic = vec![0, 1];
        emic.extend_from_slice(b"test song\0\0\0\0\0\0\0\0\0\0\0");
        emic.extend_from_slice(b"nobody\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
        emic.extend_from_slice(&[125, 2]);
        for (n, vol, ctl, ls, ll) in [(1u8, 48u8, 0u8, 0u16, 1u16), (2, 40, 1, 8, 8)] {
            let mut e = vec![n, vol];
            e.extend_from_slice(&16u16.to_be_bytes());
            e.extend_from_slice(&[0; 20]);
            e.extend_from_slice(&[ctl, 0]);
            e.extend_from_slice(&ls.to_be_bytes());
            e.extend_from_slice(&ll.to_be_bytes());
            e.extend_from_slice(&[0; 4]);
            emic.extend(e);
        }
        emic.push(0);
        emic.push(1);
        emic.extend_from_slice(&[0, rows]);
        emic.extend_from_slice(&[0; 24]);
        emic.extend_from_slice(&[1, 0]);
        // an empty cell has note -1
        let mut patt: Vec<u8> = (0..(rows as usize + 1) * 4).flat_map(|_| [0, 0xff, 0, 0]).collect();
        for &(row, voice, c) in cells {
            let o = row * 16 + voice * 4;
            patt[o..o + 4].copy_from_slice(&c);
        }
        let mut d = b"FORM\0\0\0\0EMOD".to_vec();
        chunk(&mut d, b"EMIC", &emic);
        chunk(&mut d, b"PATT", &patt);
        chunk(&mut d, b"8SMP", &[0x20; 64]);
        let n = d.len() as u32 - 8;
        d[4..8].copy_from_slice(&n.to_be_bytes());
        d
    }

    /// Sample `s`, note `n`, effect `e`, argument `a`.
    fn cell(s: u8, n: i8, e: u8, a: u8) -> [u8; 4] {
        [s, n as u8, e, a]
    }

    fn play(d: Vec<u8>, ticks: usize) -> (String, Vec<(u32, u16)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.emod"), d).unwrap();
        let format = e.playing.as_ref().unwrap().format.clone();
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        (format, log.iter().map(|w| (w.0, w.1)).collect())
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn identifies_and_rejects() {
        let d = module(3, &[]);
        assert!(detect(&d));
        let mut bad = d.clone();
        bad[8..12].copy_from_slice(b"ILBM");
        assert!(!detect(&bad));
        bad = d.clone();
        bad[21] = 2;
        assert!(!detect(&bad));
        assert!(!detect(&d[..40]));
    }

    #[test]
    fn plays_a_note_and_its_loop() {
        let d = module(3, &[(0, 0, cell(1, 12, 0, 0)), (0, 1, cell(2, 12, 0, 0))]);
        let (format, log) = play(d, 30);
        assert_eq!(format, "Quadra Composer");
        // (the init and each new note write the volume 0 first)
        let vols = |a| reg(&log, a).into_iter().skip_while(|&v| v == 0).take(1).collect::<Vec<_>>();
        assert_eq!(vols(0xDFF0A8), [48]);
        assert_eq!(vols(0xDFF0B8), [40]);
        // voice 0 plays its 16 words once (then the loop of one zeroed word), voice 1 up to the
        // end of its loop
        assert_eq!(&reg(&log, 0xDFF0A4)[..2], &[16, 1]);
        assert_eq!(&reg(&log, 0xDFF0B4)[..2], &[16, 8]);
        // note 12 of fine tune 0
        assert_eq!(&reg(&log, 0xDFF0A6)[..1], &[428]);
        // after the init's DMA off, both DMA bits go off and on together, once
        assert_eq!(&reg(&log, 0xDFF096)[..3], &[0x000f, 0x0003, 0x8003]);
    }

    #[test]
    fn effects_set_volume_and_slide_the_period() {
        let d = module(
            3,
            &[(0, 0, cell(1, 12, 0xc, 0x20)), (1, 0, cell(0, -1, 1, 4)), (1, 1, cell(2, 12, 0xf, 3))],
        );
        let (_, log) = play(d, 40);
        let vol: Vec<u16> = reg(&log, 0xDFF0A8).into_iter().skip_while(|&v| v == 0).take(2).collect();
        assert_eq!(vol, [48, 0x20]);
        // slide up by 4 on each of the row's two other ticks, at speed 3
        let per = reg(&log, 0xDFF0A6);
        assert_eq!(&per[..3], &[428, 424, 420]);
    }
}
