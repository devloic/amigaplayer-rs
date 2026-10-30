//! Digital Sound Studio, ported from NostalgicPlayer's Digital Sound Studio
//! player (Copyright (c) 2023 Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer).
//!
//! The original player here is the DSS Eagleplayer ("Digital Sound Studio
//! player module V1.71", adapted by Buggs/Defect), an early replayer that knows
//! far fewer effects than NostalgicPlayer: arpeggio, slides up and down, set
//! volume, master volume, speed and position jump (0 to 6), and nothing else.
//! It plays from the module in guest memory and writes Paula through
//! ENPP_PokeAdr/Len/Per/Vol and ENPP_DMAMask. Where NostalgicPlayer differs, this
//! follows the original:
//! * the speed is the word at offset 8, the tempo byte and the speed byte
//!   together, so a module with a tempo (byte 8 not 0) would wait that many times
//!   256 ticks for a row; there is no tempo: the ticks are DeliTracker's 50 Hz;
//! * the first row comes on the speed's tick, and a row's tick runs no effects;
//! * a note's period is used as it is (no finetune), a note turns its voice's
//!   DMA off and writes its sample's start, length and period, every row ends
//!   with DMA on for the notes' voices (a mask of 0 too) and then every voice's
//!   loop start and length; a length of 0 or 1 is written as 0, so a sample
//!   without a loop plays itself again from its start, 65536 words long;
//! * the volume is the sample's info word (finetune byte and volume byte) less
//!   the master volume, written on the row of a note only, 7 bits of it; set
//!   volume writes its argument less the master volume and keeps nothing;
//! * period $7FF writes volume 0 and the voice's DMA comes back on;
//! * the arpeggio looks the note up in the original's period table and reads on
//!   past it (its own padding, the sample addresses, the module pointer) until
//!   it finds it, and plays on ticks 1 to 5 of the row only;
//! * a slide's limits are 113 and 1712 for every sample; set master volume
//!   takes 64 less its argument as a byte (an argument over 64 up to 127 is
//!   ignored);
//! * InitSound clears the first long of every sample and the low bit of each
//!   sample's start offset in the module, and set speed writes the module's
//!   speed byte;
//! * a position jump to an earlier position, and the song's wrap to position 0,
//!   are the song end; a jump on a pattern's last row also jumps after the next
//!   row, as its flag stays set.
//!
//! A module with a tempo byte (byte 8 not 0) is from a newer Digital Sound Studio
//! than the Eagleplayer's, which would leave it silent for minutes. It is decided
//! from the header only and played as that player ("player module V1.2") does
//! (`np.rs`, written from a black-box specification of its register writes): the
//! tempo is a BPM tempo (the CIA's 1773375 / BPM), the speed is byte 9,
//! and all 31 effects (finetune, volume and master volume slides, tempo, pitch
//! control, loops, retrigger, note delay and cut, sample offset, tone portamento)
//! are there. Nothing of the above applies to it: no fitted costs, no clearing of
//! the samples' first long. Modules without a tempo play exactly as above, their
//! effects 7 and up ignored.
//!
//! The writes are spaced by the original's code-path costs, fitted by
//! non-negative least squares to the reference's write gaps (`Cost`), and the
//! timer starts when the original's does, after its start's loops over the
//! positions and samples (`start`).

mod np;

use super::{be16, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "Digital Sound Studio", detect, new };

/// The original's period table (its code hunk at $C08), 48 periods, then 113 up to
/// the sample addresses' table; read past by the arpeggio.
const PERIODS: [u16; 48] = [
    1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960, 906, 856, 808, 762, 720, 678, 640, 604, 570, 538,
    508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143,
    135, 127, 120, 113,
];
/// Words of 113 after the table, up to the sample addresses.
const PAD: usize = 30;
/// The mirror of the hunk from the period table on: the periods, the padding,
/// the 31 sample addresses and the module pointer.
const SAMPLES_OFF: u32 = 2 * (PERIODS.len() + PAD) as u32;
const MODULE_OFF: u32 = SAMPLES_OFF + 4 * 31;
const MIRROR_SIZE: u32 = MODULE_OFF + 4;

#[derive(Clone, Copy, Default)]
struct Voice {
    /// The row's four bytes: sample and period, effect, argument.
    note: u32,
    /// The sample info's finetune and volume bytes, as one word.
    vol: u16,
    addr: u32,
    len: u16,
    loop_addr: u32,
    loop_len: u16,
    /// The note's period (the arpeggio's) and the current one (the slides').
    note_per: u16,
    per: u16,
}

pub struct DigitalSoundStudio {
    base: u32,
    mem: u32,
    voices: [Voice; 4],
    counter: u16,
    position: u16,
    /// The row's offset in its pattern (16 bytes a row).
    row: u32,
    dma: u16,
    master: u16,
    jump: bool,
    next_position: u16,
    songend: bool,
    /// Colour clocks the original spends since its last write (`Cost`).
    spent: f64,
    fit: Option<std::io::BufWriter<std::fs::File>>,
    counts: Vec<(u8, u32)>,
    /// The start's code paths (`start`) and the colour clocks they took.
    init: [(Cost, u32); 5],
    init_wait: u64,
    /// The newer Digital Sound Studio's player, for a module with a tempo.
    np: Option<np::Np>,
}

/// The original's check (EP_Check5): the mark and a file longer than the song.
fn detect(d: &[u8]) -> bool {
    if d.len() < 0x61e || !d.starts_with(b"MMU2") {
        return false;
    }
    (max_pattern(d, |o| d[o]) as usize + 1) * 1024 + 0x61e < d.len()
}

/// The highest pattern of the position list (its dbra loop: 65536 positions for
/// a count of 0), read through `r` from the module's start.
fn max_pattern(d: &[u8], r: impl Fn(usize) -> u8) -> u8 {
    let n = be16(d, 0x59c).wrapping_sub(1) as usize + 1;
    (0..n).map(|k| if 0x59e + k < d.len() { r(0x59e + k) } else { 0 }).max().unwrap_or(0)
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    // a module with a tempo byte is from a newer Digital Sound Studio than the
    // Eagleplayer's, and is played as NostalgicPlayer does (np.rs)
    let tempo = d[8] != 0;
    let mem = m.alloc(MIRROR_SIZE + if tempo { 4 } else { 0 });
    if mem == 0 {
        return None;
    }
    if tempo {
        m.bus.w32(mem + MIRROR_SIZE, 0);
    }
    for (k, &p) in PERIODS.iter().chain(std::iter::repeat(&113).take(PAD)).enumerate() {
        m.bus.w16(mem + 2 * k as u32, p);
    }
    m.bus.w32(mem + MODULE_OFF, base);
    let fit = std::env::var_os("DSS_FIT").and_then(|p| std::fs::File::create(p).ok()).map(std::io::BufWriter::new);
    let mut r = DigitalSoundStudio {
        base,
        mem,
        voices: [Voice::default(); 4],
        counter: 0,
        position: 0,
        row: 0,
        dma: 0,
        master: 0,
        jump: false,
        next_position: 0,
        songend: false,
        spent: 0.0,
        fit,
        counts: Vec::new(),
        init: start(d),
        init_wait: 0,
        np: None,
    };
    // DeliTracker's timer starts after the Eagleplayer's check, EP_Get_ModuleInfo
    // and InitSound, whose loops over the positions and samples take their time
    if tempo {
        r.init_sound(d, &mut m.bus, false);
        r.np = Some(np::Np::new(d, base, mem, &mut m.bus));
        return Some(Box::new(r));
    }
    let c: f64 = r.init.iter().map(|&(c, n)| COSTS[c as usize] * n as f64).sum();
    r.init_wait = c.max(0.0).round() as u64;
    m.bus.clock += r.init_wait;
    r.init_sound(d, &mut m.bus, true);
    Some(Box::new(r))
}

/// What the start's loops count: once, every position, every position that
/// raises the highest pattern so far, every sample with a length, and every one
/// of those whose loop is not 1 word (each loop runs three times: the check,
/// EP_Get_ModuleInfo and InitSound).
fn start(d: &[u8]) -> [(Cost, u32); 5] {
    let n = be16(d, 0x59c).wrapping_sub(1) as usize + 1;
    let (mut max, mut raised) = (0u8, 0);
    for k in 0..n {
        let p = d.get(0x59e + k).copied().unwrap_or(0);
        if p >= max {
            max = p;
            raised += 1;
        }
    }
    let (mut samples, mut loops) = (0, 0);
    for k in 0..31 {
        let o = 0xa + 0x2e * k;
        if be16(d, o + 0x22) != 0 {
            samples += 1;
            if be16(d, o + 0x28) != 1 {
                loops += 1;
            }
        }
    }
    [
        (Cost::InitBase, 1),
        (Cost::InitPos, n as u32),
        (Cost::InitMax, raised),
        (Cost::InitSample, samples),
        (Cost::InitLoop, loops),
    ]
}

impl Replayer for DigitalSoundStudio {
    fn format(&self) -> &'static str {
        "Digital Sound Studio"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.np.as_ref().map_or(self.songend, |n| n.songend)
    }

    fn tick_clocks(&self) -> u64 {
        self.np.as_ref().map_or(super::DT_TICK, |n| n.tick_clocks())
    }

    fn tick(&mut self, hw: &mut Hw) {
        if let Some(n) = &mut self.np {
            return n.tick(hw);
        }
        if let Some(f) = &mut self.fit {
            use std::io::Write;
            // the tick as if the start took no time, and the start's counts with
            // the tick's first write (the fit's)
            let _ = writeln!(f, "tick {}", hw.bus.clock - self.init_wait);
        }
        self.spent = 0.0;
        self.counts.clear();
        if self.fit.is_some() {
            self.counts.extend(self.init.iter().map(|&(c, n)| (c as u8, n)));
        }
        self.t(Cost::TickStart);
        self.counter = self.counter.wrapping_add(1);
        if self.counter == hw.bus.r16(self.base + 8) {
            self.counter = 0;
            self.row(hw);
            return;
        }
        self.t(Cost::FxStart);
        for i in 0..4 {
            self.t(Cost::FxVoice);
            let v = self.voices[i];
            if (v.note >> 16) as u16 & 0x7ff == 0x7ff {
                self.t(Cost::Fx7ff);
                continue;
            }
            if v.note as u8 == 0 {
                self.t(Cost::FxNoArg);
                continue;
            }
            self.t(Cost::FxCall);
            self.effect(i, hw);
        }
    }
}

/// The original's register writes, for the costs of its calls to the host.
#[derive(Clone, Copy, PartialEq)]
enum W {
    Loc,
    Len,
    Per,
    Vol,
    Dma,
}

macro_rules! costs {
    ($($name:ident = $c:expr,)*) => {
        /// The original's code paths, each with the colour clocks it takes: fitted
        /// (non-negative least squares) to the gaps between the writes of the
        /// Eagleplayer's write logs of 32 modules (rms 0.16 colour clocks over
        /// 53193 gaps). A write waits what the paths since the last one took.
        #[derive(Clone, Copy)]
        #[allow(dead_code)]
        enum Cost { $($name,)* }
        const COSTS: &[f64] = &[$($c as f64,)*];
        #[allow(dead_code)]
        const COST_NAMES: &[&str] = &[$(stringify!($name),)*];
    };
}

// The whole numbers are paths the modules never took before a write (estimated
// from the code; those after a tick's last write cost nothing). TickStart and
// InitBase are one sum to the fit; TickStart is taken as the interrupt's way to
// the play routine, so that the ticks and the audio come when the original's do.
costs! {
    InitBase = 7219.55,
    InitPos = 46.51,
    InitMax = 5.98,
    InitSample = 143.08,
    InitLoop = 19.81,
    TickStart = 150,
    FxStart = 0.00,
    FxVoice = 42.68,
    Fx7ff = 5,
    FxNoArg = 0.00,
    FxCall = 56.63,
    FxArp = 1.43,
    ArpCmp = 5.45,
    ArpNone = 4,
    ArpBase = 18,
    ArpNibble = 0.00,
    ArpStep = 15.54,
    FxSlideUp = 0.00,
    FxSlideDown = 7.23,
    SlideClamp = 6,
    FxOther = 10,
    RowStart = 60.12,
    VoiceRow = 67.78,
    Sample = 215.81,
    SampleCh0 = 0.00,
    SampleCh1 = 15.48,
    SampleCh2 = 31.12,
    SampleCh3 = 35.00,
    SampleUpsPer0 = 5.68,
    SampleUpsShort = 0.00,
    SampleAddr = 49.92,
    SampleNoLoop = 4.11,
    NotePer = 0.00,
    Note7ff = 0.00,
    Note7ffCh0 = 43,
    Note7ffCh1 = 58.88,
    Note7ffCh2 = 74.35,
    Note7ffCh3 = 77.79,
    NoteAddr = 13.00,
    NoteNoAddr = 10,
    NoteEnd = 0.00,
    RowFx = 67.66,
    VolArg = 0.52,
    VolNote = 0.00,
    VolSkip = 0.00,
    VolClamp = 2,
    VolCh0 = 0.00,
    VolCh1 = 11.29,
    VolCh2 = 23.02,
    VolCh3 = 18.04,
    VolUps = 142.73,
    FxRow0 = 0.00,
    FxMaster = 15,
    FxMasterSet = 10,
    FxSpeed = 43.56,
    FxSpeedSet = 0.00,
    FxJump = 70.37,
    FxJumpFF = 0.00,
    FxJumpBack = 46.42,
    FxRowNone = 25.65,
    VoiceReturn = 0.00,
    AfterVoices = 0.00,
    LoopVoice = 0.00,
    RowEnd = 20,
    RowNext = 20,
    RowJump = 10,
    NewPos = 20,
    Wrap = 20,
    LocPre = 110.00,
    LocPost = 109.00,
    LenPre = 0.00,
    LenPost = 0.00,
    LenShort = 2.00,
    PerPre = 89.00,
    PerPost = 0.53,
    VolPre = 0.00,
    VolPost = 12.88,
    DmaPre = 138.33,
    DmaPost = 7.00,
}

impl DigitalSoundStudio {
    fn t(&mut self, c: Cost) {
        self.tn(c, 1);
    }

    fn tn(&mut self, c: Cost, n: u32) {
        let k = c as usize;
        self.spent += COSTS[k] * n as f64;
        if self.fit.is_some() && n != 0 {
            match self.counts.iter_mut().find(|e| e.0 as usize == k) {
                Some(e) => e.1 += n,
                None => self.counts.push((k as u8, n)),
            }
        }
    }

    /// Lets the time pass that the original's code takes up to a write of kind
    /// `w`, and counts the host call's own time after it for the next write.
    fn write(&mut self, hw: &mut Hw, w: W) {
        let (pre, post) = match w {
            W::Loc => (Cost::LocPre, Cost::LocPost),
            W::Len => (Cost::LenPre, Cost::LenPost),
            W::Per => (Cost::PerPre, Cost::PerPost),
            W::Vol => (Cost::VolPre, Cost::VolPost),
            W::Dma => (Cost::DmaPre, Cost::DmaPost),
        };
        self.t(pre);
        if let Some(f) = &mut self.fit {
            use std::io::Write;
            let s: Vec<String> = self.counts.iter().map(|(k, n)| format!("{}:{n}", COST_NAMES[*k as usize])).collect();
            let _ = writeln!(f, "w {} {}", hw.bus.clock + self.spent.max(0.0).round() as u64, s.join(" "));
        }
        // the fraction of a colour clock left over is carried to the next write
        let c = self.spent.max(0.0).round();
        hw.wait(c as u64);
        self.spent -= c;
        self.counts.clear();
        self.t(post);
    }

    fn loc(&mut self, hw: &mut Hw, ch: usize, a: u32) {
        self.write(hw, W::Loc);
        hw.loc(ch, a);
    }

    /// The original's length routine: 0 or 1 is written as 0.
    fn len(&mut self, hw: &mut Hw, ch: usize, l: u16) {
        let l = if l <= 1 {
            self.t(Cost::LenShort);
            0
        } else {
            l
        };
        self.write(hw, W::Len);
        hw.len(ch, l);
    }

    fn per(&mut self, hw: &mut Hw, ch: usize, p: u16) {
        self.write(hw, W::Per);
        hw.per(ch, p);
    }

    /// The original's volume routine: 7 bits, and its scope structure's volume
    /// (found by comparing the channel's registers, one after the other).
    fn vol(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.t(Cost::VolUps);
        self.t([Cost::VolCh0, Cost::VolCh1, Cost::VolCh2, Cost::VolCh3][ch]);
        self.write(hw, W::Vol);
        hw.vol(ch, v & 0x7f);
    }

    /// ENPP_DMAMask: on (bit 15) with the host's wait for the DMA, as
    /// dtg_WaitAudioDMA does, to 10 lines past the current one.
    fn dma(&mut self, hw: &mut Hw, v: u16) {
        self.write(hw, W::Dma);
        hw.dmacon(v & 0x8000 | v & 0xf);
        if v & 0x8000 != 0 {
            let now = hw.bus.clock;
            hw.wait((now / LINE + 10) * LINE - now);
        }
    }

    /// InitSound: the samples' addresses (after the patterns, each its length and
    /// loop, unless the loop is 1 word, plus its start offset, whose low bit is
    /// cleared in the module), the first long of each sample cleared, and the
    /// player's variables.
    fn init_sound(&mut self, d: &[u8], b: &mut crate::bus::Bus, clear: bool) {
        let base = self.base;
        let maxp = max_pattern(d, |o| b.r8(base + o as u32)) as u32;
        let mut a = base + 0x61e + ((maxp + 1) << 10);
        for k in 0..31u32 {
            let info = base + 0xa + 0x2e * k;
            let mut entry = 0;
            let len = b.r16(info + 0x22) as u32;
            if len != 0 {
                entry = a;
                let ll = b.r16(info + 0x28);
                let mut size = len + if ll == 1 { 0 } else { ll as u32 };
                size *= 2;
                b.w8(info + 0x21, b.r8(info + 0x21) & !1);
                size = size.wrapping_add(b.r32(info + 0x1e));
                a = a.wrapping_add(size);
            }
            b.w32(self.mem + SAMPLES_OFF + 4 * k, entry);
        }
        for k in 0..31u32 {
            let s = b.r32(self.mem + SAMPLES_OFF + 4 * k);
            if s != 0 && clear {
                b.w32(s, 0);
            }
        }
        self.counter = 0;
        self.position = 0;
        self.row = 0;
        self.dma = 0;
        self.master = 0;
        self.jump = false;
        self.next_position = 0;
    }

    /// A tick's effect on voice `i`, when its row has an argument.
    fn effect(&mut self, i: usize, hw: &mut Hw) {
        let v = self.voices[i];
        match (v.note >> 8) as u8 {
            0 => {
                self.t(Cost::FxArp);
                let c = self.counter;
                self.tn(Cost::ArpCmp, (c.clamp(1, 5)) as u32);
                let arg = v.note as u8;
                let off = match c {
                    1 | 5 => arg >> 4,
                    2 | 4 => arg & 15,
                    3 => {
                        self.t(Cost::ArpBase);
                        return self.per(hw, i, v.note_per);
                    }
                    _ => {
                        self.t(Cost::ArpNone);
                        return;
                    }
                };
                self.t(Cost::ArpNibble);
                // the first word equal to the note's period, and the one `off`
                // words on, reading on past the table in guest memory
                let mut a = self.mem;
                let mut n = 0;
                let p = loop {
                    let p = hw.bus.r16(a.wrapping_add(2 * off as u32));
                    n += 1;
                    if hw.bus.r16(a) == v.note_per || n >= 0x100000 {
                        break p;
                    }
                    a = a.wrapping_add(2);
                };
                self.tn(Cost::ArpStep, n);
                self.per(hw, i, p);
            }
            1 => {
                self.t(Cost::FxSlideUp);
                let vv = &mut self.voices[i];
                vv.per = vv.per.wrapping_sub(v.note as u8 as u16);
                if vv.per.wrapping_sub(0x71) & 0x8000 != 0 {
                    vv.per = 0x71;
                    self.t(Cost::SlideClamp);
                }
                let p = self.voices[i].per;
                self.per(hw, i, p);
            }
            2 => {
                self.t(Cost::FxSlideDown);
                let vv = &mut self.voices[i];
                vv.per = vv.per.wrapping_add(v.note as u8 as u16);
                if vv.per.wrapping_sub(0x6b0) & 0x8000 == 0 {
                    vv.per = 0x6b0;
                    self.t(Cost::SlideClamp);
                }
                let p = self.voices[i].per;
                self.per(hw, i, p);
            }
            _ => self.t(Cost::FxOther),
        }
    }

    fn row(&mut self, hw: &mut Hw) {
        self.t(Cost::RowStart);
        let b = self.base;
        self.next_position = self.position;
        let pat = hw.bus.r8(b.wrapping_add(0x59e).wrapping_add(self.position as i16 as u32)) as u32;
        let mut off = (pat << 10).wrapping_add(self.row);
        self.dma = 0;
        for i in 0..4 {
            self.t(Cost::VoiceRow);
            self.voice_row(i, b.wrapping_add(0x61e).wrapping_add(off), hw);
            off = off.wrapping_add(4);
        }
        self.t(Cost::AfterVoices);
        let d = self.dma | 0x8000;
        self.dma(hw, d);
        for i in 0..4 {
            self.t(Cost::LoopVoice);
            let v = self.voices[i];
            self.loc(hw, i, v.loop_addr);
            self.len(hw, i, v.loop_len);
        }
        self.t(Cost::RowEnd);
        if self.row != 0x3f0 {
            self.t(Cost::RowNext);
            self.row += 16;
            if !self.jump {
                return;
            }
            self.t(Cost::RowJump);
            self.jump = false;
        }
        self.t(Cost::NewPos);
        self.row = 0;
        self.position = self.next_position.wrapping_add(1);
        if self.position == hw.bus.r16(b + 0x59c) {
            self.t(Cost::Wrap);
            self.position = 0;
            self.songend = true;
        }
    }

    /// One voice's part of the row at `a`.
    fn voice_row(&mut self, i: usize, a: u32, hw: &mut Hw) {
        let b = self.base;
        let note = hw.bus.r32(a);
        self.voices[i].note = note;
        let s = (note >> 27) as u32;
        if s != 0 {
            self.t(Cost::Sample);
            let info = b.wrapping_add(0xa).wrapping_add((s - 1) * 0x2e + 0x1e);
            let addr = hw.bus.r32(self.mem + SAMPLES_OFF + 4 * (s - 1)).wrapping_add(hw.bus.r32(info));
            // the scope structure's: the period (the last one if the row has
            // none) and the length (0 if 1 word)
            self.t([Cost::SampleCh0, Cost::SampleCh1, Cost::SampleCh2, Cost::SampleCh3][i]);
            if (note >> 16) & 0x7ff == 0 {
                self.t(Cost::SampleUpsPer0);
            }
            if hw.bus.r16(info + 0xa) <= 1 {
                self.t(Cost::SampleUpsShort);
            }
            let v = &mut self.voices[i];
            v.addr = addr;
            if addr != 0 {
                v.len = hw.bus.r16(info + 4);
                let mut la = hw.bus.r32(info + 6);
                let mut ll = hw.bus.r16(info + 10);
                if ll == 0 {
                    ll = 1;
                    la = 0;
                }
                v.loop_len = ll;
                v.loop_addr = la.wrapping_add(addr);
                v.vol = hw.bus.r16(info + 12);
                self.t(Cost::SampleAddr);
                if ll == 1 && la == 0 {
                    self.t(Cost::SampleNoLoop);
                }
            }
        }
        let per = (note >> 16) as u16 & 0x7ff;
        if per != 0 {
            self.t(Cost::NotePer);
            self.voices[i].note_per = per;
            self.dma(hw, 1 << i);
            if per == 0x7ff {
                self.t(Cost::Note7ff);
                self.vol(hw, i, 0);
                self.t([Cost::Note7ffCh0, Cost::Note7ffCh1, Cost::Note7ffCh2, Cost::Note7ffCh3][i]);
                self.dma |= 1 << i;
                return;
            }
            let v = self.voices[i];
            if v.addr != 0 {
                self.t(Cost::NoteAddr);
                self.loc(hw, i, v.addr);
                self.len(hw, i, v.len);
                self.per(hw, i, per);
                self.voices[i].per = per;
                self.t(Cost::NoteEnd);
                self.dma |= 1 << i;
            } else {
                self.t(Cost::NoteNoAddr);
            }
        }
        // the row's effect: the volume first
        self.t(Cost::RowFx);
        let fx = (note >> 8) as u8;
        let arg = note as u8;
        let v = if fx == 3 {
            self.t(Cost::VolArg);
            Some(arg as u16)
        } else if per == 0 {
            self.t(Cost::VolSkip);
            None
        } else {
            self.t(Cost::VolNote);
            Some(self.voices[i].vol)
        };
        if let Some(v) = v {
            let mut v = v.wrapping_sub(self.master);
            if v & 0x8000 != 0 {
                self.t(Cost::VolClamp);
                v = 0;
            }
            self.vol(hw, i, v);
        }
        match fx {
            0 => self.t(Cost::FxRow0),
            4 => {
                self.t(Cost::FxMaster);
                // sub.b, blt: skipped when the argument is 65 to 127
                if (arg as i8) <= 64 {
                    self.t(Cost::FxMasterSet);
                    self.master = 0x40u8.wrapping_sub(arg) as u16;
                }
            }
            5 => {
                self.t(Cost::FxSpeed);
                if arg != 0 {
                    self.t(Cost::FxSpeedSet);
                    self.counter = 0;
                    hw.bus.w8(b + 9, arg);
                }
            }
            6 => {
                self.t(Cost::FxJump);
                self.jump = true;
                if arg == 0xff {
                    self.t(Cost::FxJumpFF);
                    self.next_position = self.position;
                } else {
                    let p = arg.wrapping_sub(2) as i8 as i16;
                    self.next_position = p as u16;
                    if p < self.position as i16 {
                        self.t(Cost::FxJumpBack);
                        self.songend = true;
                    }
                }
            }
            // effect 7 (the filter) too: the V1.71 player never writes $BFE001;
            // the tempo modules' player (np.rs) does
            _ => self.t(Cost::FxRowNone),
        }
        self.t(Cost::VoiceReturn);
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// One sample of 16 words without a loop, one position, speed 1: row 0
    /// plays it on voice 0 at period 428, row 1 sets voice 0's volume to $20.
    fn module() -> Vec<u8> {
        let mut d = b"MMU2\0\0\0\0".to_vec();
        d.extend([0, 1]); // tempo 0, speed 1: the speed word is 1
        d.extend([0u8; 30]);
        d.extend(0u32.to_be_bytes()); // start offset
        d.extend(16u16.to_be_bytes()); // length in words
        d.extend(0u32.to_be_bytes()); // loop start
        d.extend(1u16.to_be_bytes()); // loop length: none
        d.extend([0, 48]); // finetune, volume
        d.extend(8363u16.to_be_bytes());
        d.resize(0x59c, 0);
        d.extend(1u16.to_be_bytes()); // 1 position: pattern 0
        d.resize(0x61e, 0);
        let mut pattern = vec![0u8; 1024];
        pattern[..4].copy_from_slice(&[0x08 | 0x01, 0xac, 0, 0]);
        pattern[16..20].copy_from_slice(&[0, 0, 3, 0x20]);
        d.extend(pattern);
        d.extend((0..32).map(|i| i as u8 + 1));
        d.extend([0; 2]);
        d
    }

    /// With a tempo byte the module is the newer player's: speed 2 (byte 9) at 250
    /// BPM, so a row every 2 ticks of 1773375 / 250 CIA ticks, and effect 7+ work
    /// (here B, set song tempo, on row 1).
    #[test]
    fn plays_a_module_with_a_tempo() {
        let mut d = module();
        d[8] = 125; // tempo byte
        d[9] = 2; // speed
        d[0x61e + 16..0x61e + 20].copy_from_slice(&[0, 0, 0x0b, 250]);
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.dss"), d).unwrap();
        let mut out = vec![0f32; 2 * 96000];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        assert_eq!(reg(0xDFF0A6)[0], 428);
        // after the start's two clears of the volume
        assert_eq!(&reg(0xDFF0A8)[..3], &[0, 0, 48]);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
        // the pattern's 128 ticks take 2.56 s at 125 BPM, about 1.3 s once row 1 has
        // set 250: the note on row 0 comes again within the 2 s
        assert!(reg(0xDFF0A6).len() >= 2, "{:?}", reg(0xDFF0A6));
    }

    #[test]
    fn plays_a_note_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.dss"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Digital Sound Studio");
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // the note's DMA off and on, then on with no note (a mask of 0) every row
        assert_eq!(&reg(0xDFF096)[..4], &[0x0001, 0x8001, 0x8000, 0x8000]);
        // voice 0: the sample's length, then its loop of 1 word, written as 0
        assert_eq!(&reg(0xDFF0A4)[..3], &[16, 0, 0]);
        assert_eq!(reg(0xDFF0A6), [428]);
        // the sample's volume with the note, then set volume, kept for no row
        assert_eq!(reg(0xDFF0A8), [48, 0x20]);
        // InitSound cleared the sample's first long
        let a = (reg(0xDFF0A0)[0] as u32) << 16 | reg(0xDFF0A2)[0] as u32;
        assert_eq!(e.m.bus.r32(a), 0);
        assert_eq!(e.m.bus.r8(a + 4), 5);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}
