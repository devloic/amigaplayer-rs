//! Future Player (`fp.*`; Paul van der Valk, 1988-89, the music of Hybris and others).
//!
//! Clean-room, written from a behavioural specification (drawn up from register-write logs and
//! execution traces of the 68k player on two game modules and three synthetic ones) and checked
//! against those write logs. No original code is used or run: the module is a loadable file
//! with one hunk whose image is pure data (header, song table, song records, track and command
//! streams, sound, waveform and modulator records), interpreted here.
//!
//! Four voices, one Paula channel each, driven by a timer tick whose length the song record
//! gives. A beat comes every S ticks; on a beat each voice's command stream is stepped (notes,
//! ties, calls, loops, jumps, transposes, portamento, sound changes), and every tick each
//! voice runs its sound processing: a retrigger, the choice of waveform (single or a timed list),
//! the period (arpeggio or portamento, two period modulators, detune), the sample address and
//! length (two address modulators for synth waveforms), the volume and the envelope, then a
//! wait for the raster beam before the channel is started. Every register write is placed at the
//! colour clock of the original (static cycle costs summed per step; the raster wait uses the
//! absolute colour clock).
//!
//! Choices where the specification is silent: a period index above 96 is clamped to 96; a command
//! number above 14 is ignored (it takes no parameters and costs as command 14); a song whose
//! voices all lack a track ends at once; the song end is reported at the end of the tick in
//! which it fires and the host's reaction after it is the engine's. The first tick follows the
//! host's start-up by the specification's rule (a fitted constant for the start-up, the pre-run
//! cost and a per-voice-count term), which is accurate to a few colour clocks only.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Future Player", detect, new };

/// The 97-entry period table of the player (the module does not carry one).
const PERIODS: [u16; 97] = [
    7232, 6848, 6464, 6096, 5760, 5424, 5120, 4832, 4560, 4304, 4064, 3840, 3616, 3424, 3232, 3048, 2880, 2712, 2560,
    2416, 2280, 2152, 2032, 1920, 1808, 1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960, 904, 856,
    808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 452, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240,
    226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113, 107, 101, 95, 90, 85, 80, 76, 72, 68, 64, 60,
    57, 53, 50, 47, 45, 42, 40, 38, 36, 34, 32, 30, 28,
];

/// Cycle costs of the steps (the reference machine's static per-instruction costs).
#[allow(dead_code)]
mod k {
    pub const TICK_IN: u64 = 246;
    pub const PR_B: u64 = 20;
    pub const SYNC_A: u64 = 49;
    pub const TICK_OUT: u64 = 168;
    pub const PR_C: u64 = 23;
    pub const A2_A: u64 = 53;
    pub const TICK_A: u64 = 21;
    pub const TR_A: u64 = 24;
    pub const A2_DELAY: u64 = 26;
    pub const FADE_CHK: u64 = 30;
    pub const TR_B: u64 = 85;
    pub const A2_B: u64 = 21;
    pub const FADE_A: u64 = 21;
    pub const LV_SET: u64 = 20;
    pub const A2_C: u64 = 93;
    pub const FADE_STEP: u64 = 28;
    pub const LV_CMP: u64 = 33;
    pub const A2_WRAP: u64 = 29;
    pub const SETMASTER: u64 = 184;
    pub const MI_A_J: [u64; 4] = [25, 25, 25, 25];
    pub const A2_STOP: u64 = 8;
    pub const FADE_END: u64 = 42;
    pub const MI_B_J: [u64; 4] = [36, 36, 36, 36];
    pub const A2_E: u64 = 12;
    pub const TICK_B: u64 = 29;
    pub const TY_A: u64 = 37;
    pub const A3_PRE: u64 = 25;
    pub const BEAT: u64 = 24;
    pub const L_A: u64 = 21;
    pub const A3_A: u64 = 53;
    pub const TICK_C: u64 = 16;
    pub const L_FIRST: u64 = 45;
    pub const A3_DELAY: u64 = 26;
    pub const SEQCALL_I: [u64; 4] = [30, 30, 30, 30];
    pub const L_EMPTY: u64 = 18;
    pub const A3_LOOP: u64 = 78;
    pub const PROCCALL_I: [u64; 4] = [30, 30, 30, 30];
    pub const L_LOAD: u64 = 100;
    pub const A3_B: u64 = 21;
    pub const SEQ_A: u64 = 23;
    pub const L_DEC: u64 = 41;
    pub const A3_C: u64 = 93;
    pub const SEQ_B: u64 = 63;
    pub const L_ADV: u64 = 37;
    pub const A3_WRAP: u64 = 29;
    pub const SEQ_C: u64 = 16;
    pub const L_WRAP: u64 = 16;
    pub const A3_STOP: u64 = 8;
    pub const SEQ_ITEM: u64 = 17;
    pub const L_RD: u64 = 57;
    pub const A3_E: u64 = 12;
    pub const CMD_HDR: u64 = 29;
    pub const L_FLAG: u64 = 40;
    pub const LEN_A: u64 = 12;
    pub const CMD_ABORT: u64 = 36;
    pub const L_END: u64 = 26;
    pub const ADDR_A: u64 = 12;
    pub const CMD_DISP: u64 = 68;
    pub const S_A: u64 = 16;
    pub const ADDR_B: u64 = 18;
    pub const CMD_AFTER: u64 = 10;
    pub const BASE: u64 = 77;
    pub const VOL_A: u64 = 196;
    pub const NOTE_A: u64 = 9;
    pub const ARP_A: u64 = 59;
    pub const VOL_B: u64 = 28;
    pub const NOTE_SET: u64 = 12;
    pub const ARP_WRAP: u64 = 20;
    pub const VOL_PRE_I: [u64; 4] = [156, 179, 202, 225];
    pub const NOTE_B: u64 = 17;
    pub const ARP_B: u64 = 60;
    pub const VOL_POST_I: [u64; 4] = [50, 50, 50, 40];
    pub const DUR_TIE: u64 = 64;
    pub const POR_A: u64 = 55;
    pub const ENV_A: u64 = 21;
    pub const DUR_TRIG: u64 = 68;
    pub const POR_UP: u64 = 21;
    pub const REL_A: u64 = 39;
    pub const RTS: u64 = 16;
    pub const POR_UP_OK: u64 = 13;
    pub const REL_FLOOR: u64 = 26;
    pub const K0_A: u64 = 23;
    pub const POR_UP_CLAMP: u64 = 10;
    pub const ENV_G: u64 = 21;
    pub const K0_POP: u64 = 83;
    pub const POR_CLAMP: u64 = 4;
    pub const PH0_A: u64 = 33;
    pub const END_A: u64 = 49;
    pub const POR_DN: u64 = 21;
    pub const PH0_B: u64 = 25;
    pub const END_B: u64 = 40;
    pub const POR_DN_OK: u64 = 13;
    pub const PH0_C: u64 = 35;
    pub const END_C: u64 = 18;
    pub const POR_END: u64 = 4;
    pub const PH0_TO1: u64 = 46;
    pub const END_D: u64 = 16;
    pub const PER_SAVE: u64 = 53;
    pub const PH2_A: u64 = 17;
    pub const K1: u64 = 364;
    pub const M0_A: u64 = 53;
    pub const PH2_B: u64 = 21;
    pub const K2: u64 = 40;
    pub const M0_DELAY: u64 = 26;
    pub const PH2_UP: u64 = 25;
    pub const K3: u64 = 56;
    pub const M0_B: u64 = 21;
    pub const PH2_UP_OK: u64 = 33;
    pub const K4: u64 = 32;
    pub const M0_C: u64 = 39;
    pub const PH2_UP_END: u64 = 46;
    pub const K5: u64 = 16;
    pub const M0_NEG: u64 = 4;
    pub const PH2_DN: u64 = 33;
    pub const K6_A: u64 = 73;
    pub const M0_D: u64 = 55;
    pub const PH2_DN_OK: u64 = 33;
    pub const K6_INC: u64 = 4;
    pub const M0_WRAP: u64 = 29;
    pub const PH2_DN_END: u64 = 36;
    pub const K6_B: u64 = 74;
    pub const M0_STOP: u64 = 8;
    pub const PH1_A: u64 = 26;
    pub const K7: u64 = 44;
    pub const M0_E: u64 = 12;
    pub const PH3_A: u64 = 26;
    pub const K8: u64 = 76;
    pub const M1_A: u64 = 53;
    pub const PH3_B: u64 = 30;
    pub const K9_A: u64 = 39;
    pub const M1_DELAY: u64 = 26;
    pub const DEC_A: u64 = 37;
    pub const K9_LOOP: u64 = 34;
    pub const M1_B: u64 = 21;
    pub const DEC_OK: u64 = 33;
    pub const K9_EXIT: u64 = 16;
    pub const M1_C: u64 = 39;
    pub const DEC_END: u64 = 46;
    pub const K10_A: u64 = 21;
    pub const M1_NEG: u64 = 4;
    pub const DW_A: u64 = 18;
    pub const K10_JMP: u64 = 46;
    pub const M1_D: u64 = 55;
    pub const DW_B: u64 = 28;
    pub const K11: u64 = 28;
    pub const M1_WRAP: u64 = 29;
    pub const DW_IT: u64 = 16;
    pub const K12: u64 = 28;
    pub const M1_STOP: u64 = 8;
    pub const DW_RD: u64 = 16;
    pub const K13_A: u64 = 25;
    pub const M1_E: u64 = 12;
    pub const DW_CMP: u64 = 9;
    pub const K13_PRE: u64 = 16;
    pub const M1_PRE: u64 = 25;
    pub const DW_IT_END: u64 = 10;
    pub const K14: u64 = 16;
    pub const DET: u64 = 24;
    pub const DW_C: u64 = 44;
    pub const SE_A: u64 = 32;
    pub const PER_POST: u64 = 18;
    pub const AFTER_DW: u64 = 21;
    pub const SE_CMP_I: [u64; 4] = [33, 33, 33, 33];
    pub const SETPER_I: [u64; 4] = [79, 110, 141, 149];
    pub const ON_A: u64 = 24;
    pub const SE_CLR_I: [u64; 4] = [22, 26, 26, 16];
    pub const SETTWO_I: [u64; 4] = [95, 126, 157, 165];
    pub const ON_B: u64 = 32;
    pub const SE_B: u64 = 21;
    pub const W_A: u64 = 49;
    pub const NT_A: u64 = 37;
    pub const SE_FIRE: u64 = 88;
    pub const W_LIST: u64 = 14;
    pub const OS_A: u64 = 41;
    pub const SE_C: u64 = 44;
    pub const W_SINGLE: u64 = 37;
    pub const OS_LEN: u64 = 16;
    pub const PR_A: u64 = 44;
    pub const W_CHK: u64 = 27;
    pub const OS_ADDR: u64 = 20;
    pub const PR_ENDED: u64 = 27;
    pub const GO_A: u64 = 63;
    pub const END_PROC: u64 = 16;
}
use k::*;

const LINE: u64 = crate::paula::LINE;
/// The file offset of the hunk image.
const IMG: usize = 32;

// ------------------------------------------------------------------ recognition

fn detect(d: &[u8]) -> bool {
    d.len() >= 72
        && be32(d, 0) == 0x0000_03F3
        && d[20] != 0
        && be32(d, 32) == 0x70FF_4E75
        && &d[36..44] == b"F.PLAYER"
        && be32(d, 64) != 0
}

// ------------------------------------------------------------------ state

/// A waveform record's fields (the data address is a guest address).
#[derive(Clone, Copy)]
struct Wf {
    data: u32,
    len: u16,
    flags: u8,
    trans: u8,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    ended: bool,
    /// The sound record (an image offset); `None` is the default sound.
    sound: Option<u32>,
    note: u8,
    gate: u8,
    trigger: bool,
    dur: u8,
    ptr: u32,
    ret: [u32; 8],
    lstart: [u32; 8],
    lcnt: [u8; 8],
    lvl: usize,
    t1: u8,
    t2: u8,
    /// The arpeggio table (an image offset); `None` is the default one.
    arp: Option<u32>,
    aidx: u16,
    port: u16,
    last: u16,
    level: i32,
    phase: u8,
    midx: [u16; 4],
    mdly: [u8; 4],
    started: bool,
    cur: u32,
    rem: u8,
    next: u32,
    lloop: u32,
}

struct FuturePlayer {
    img: Vec<u8>,
    /// The guest address of the image's first byte.
    base: u32,
    silent: u32,
    nsub: usize,
    sub: usize,
    title: String,
    v: [Voice; 4],
    detune: [i8; 4],
    alive: [bool; 4],
    tracks: u64,
    master: u8,
    fade_on: bool,
    fstep: u8,
    speed: u8,
    beat: u8,
    interval: u64,
    songend: bool,
    // the clock of the step being modelled and the cycles into it
    t0: u64,
    cy: u64,
    /// The host's start-up clock this subsong follows (the last start-up write of the reference).
    host: u64,
    /// The pre-run's length in colour clocks, and the first tick's time.
    prerun_cc: u64,
    first: u64,
    /// The cost between the last start-up write and the timer's start (fitted to the reference).
    rcost: u64,
}

/// Clock of the last start-up write of the reference host for the first subsong (four voices,
/// no fade-in): the host's own code, fitted to the reference's logs.
const HOST0: u64 = 8251;

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let words = be32(d, 28) as usize;
    let end = IMG.checked_add(words.checked_mul(4)?)?.min(d.len());
    let img = d.get(IMG..end)?.to_vec();
    let il = |o: usize| img.get(o..o + 4).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let mut nsub = 0;
    while nsub < 4096 && il(32 + 8 * nsub) != 0 {
        nsub += 1;
    }
    if nsub == 0 {
        return None;
    }
    let cstr = |o: u32| -> String {
        img.get(o as usize..).map_or(String::new(), |s| {
            s.iter().take_while(|&&b| (0x20..0x7F).contains(&b)).take(80).map(|&b| b as char).collect()
        })
    };
    let title = cstr(il(12));
    let silent = m.alloc(16);
    if silent == 0 {
        return None;
    }
    let mut s = FuturePlayer {
        img,
        base: base.wrapping_add(IMG as u32),
        silent,
        nsub,
        sub: 0,
        title,
        v: [Voice::default(); 4],
        detune: [0; 4],
        alive: [false; 4],
        tracks: 0,
        master: 0,
        fade_on: false,
        fstep: 0,
        speed: 1,
        beat: 1,
        interval: 0,
        songend: false,
        t0: 0,
        cy: 0,
        host: HOST0,
        prerun_cc: 0,
        first: 0,
        rcost: 0,
    };
    let (voices, fade) = s.shape(0);
    s.host = HOST0 + 10 * (4 - voices) - if fade { 238 } else { 0 };
    let mut hw = Hw { bus: &mut m.bus };
    s.init(&mut hw, 0);
    Some(Box::new(s))
}

impl FuturePlayer {
    // --- image access (out of range reads as zero)

    fn ib(&self, o: u32) -> u8 {
        self.img.get(o as usize).copied().unwrap_or(0)
    }

    fn iw(&self, o: u32) -> u16 {
        be16(&self.img, o as usize)
    }

    fn il(&self, o: u32) -> u32 {
        be32(&self.img, o as usize)
    }

    fn abs(&self, off: u32) -> u32 {
        self.base.wrapping_add(off)
    }

    fn c(&mut self, n: u64) {
        self.cy += n;
    }

    fn at(&self) -> u64 {
        self.t0 + self.cy / 2
    }

    fn dma(&mut self, hw: &mut Hw, v: u16) {
        hw.bus.clock = self.at();
        hw.dmacon(v);
    }

    /// A byte of the sound's parameter block (the default sound's are all zero).
    fn par(&self, snd: Option<u32>, off: u32) -> u8 {
        match snd {
            None => 0,
            Some(s) => {
                let pb = self.il(s.wrapping_add(12));
                self.ib(pb.wrapping_add(off))
            }
        }
    }

    fn par32(&self, snd: Option<u32>, off: u32) -> u32 {
        match snd {
            None => 0,
            Some(s) => {
                let pb = self.il(s.wrapping_add(12));
                self.il(pb.wrapping_add(off))
            }
        }
    }

    fn wf(&self, rec: Option<u32>) -> Wf {
        match rec {
            None => Wf { data: self.silent, len: 2, flags: 0, trans: 0 },
            Some(r) => Wf {
                data: self.abs(self.il(r.wrapping_add(8))),
                len: self.iw(r.wrapping_add(12)),
                flags: self.ib(r.wrapping_add(16)),
                trans: self.ib(r.wrapping_add(17)),
            },
        }
    }

    fn arp_byte(&self, tab: Option<u32>, idx: u16) -> u8 {
        match tab {
            None => [0u8, 0x80].get(idx as usize).copied().unwrap_or(0),
            Some(t) => self.ib(t.wrapping_add(8).wrapping_add(idx as u32)),
        }
    }

    /// The number of voices with a track and whether the fade-in is on, of subsong `n`.
    fn shape(&self, n: usize) -> (u64, bool) {
        let ent = 32 + 8 * n as u32;
        let song = self.il(ent);
        let voices = (0..4).filter(|&i| self.il(song.wrapping_add(8 + 4 * i)) != 0).count() as u64;
        (voices, (self.il(ent + 4) >> 16) & 0xFF != 0)
    }

    // --- start of a subsong (specification section 3)

    fn init(&mut self, hw: &mut Hw, n: usize) {
        self.sub = n;
        self.songend = false;
        let ent = 32 + 8 * n as u32;
        let song = self.il(ent);
        let param = self.il(ent + 4);
        let pre = (param & 0xFFFF) as u64;
        self.fstep = (param >> 16) as u8;
        let sp = self.ib(song.wrapping_add(24)) & 7;
        self.speed = if sp == 0 { 8 } else { sp };
        let t = self.ib(song.wrapping_add(25)) as u64;
        self.interval = 5 * (256 * t + 1);
        self.fade_on = self.fstep != 0;
        self.master = 0;
        self.detune = [0; 4];
        let mut tracks = 0;
        for i in 0..4 {
            let mut v = Voice { ended: true, dur: 1, ..Voice::default() };
            let track = self.il(song.wrapping_add(8 + 4 * i as u32));
            self.alive[i] = track != 0;
            if track != 0 {
                tracks += 1;
                v.ended = false;
                v.ptr = self.il(track.wrapping_add(8));
                v.ret[0] = v.ptr;
            }
            self.v[i] = v;
        }
        self.beat = 1;
        // the clock: the host's start-up, then the pre-run's cost
        self.cy = 0;
        self.t0 = hw.bus.clock.max(self.host);
        self.host = self.t0;
        self.host_silence(hw);
        hw.bus.clock = self.t0;
        self.tracks = tracks;
        if tracks == 0 {
            // nothing can end: report the end at once rather than play silence for ever
            self.songend = true;
        }
        if pre > 0 && !self.songend {
            self.c(20 + 9 + 20);
            'rounds: for _ in 0..pre {
                for i in 0..4 {
                    self.c(SEQCALL_I[i]);
                    self.seq_step(hw, i, true);
                    if self.songend {
                        break 'rounds;
                    }
                }
                self.c(28 + 9);
            }
        }
        if !self.fade_on {
            self.master = 255;
        }
        self.prerun_cc = self.cy / 2;
        // the first tick: the rule of the specification for the host's start-up
        let r: u64 = match tracks {
            4 => 528,
            3 => 472,
            2 => 416,
            _ => 364,
        };
        let v = tracks.clamp(1, 4);
        let r = r.saturating_sub(if self.fade_on { 103 + (4 - v) * 4 / 3 } else { 0 });
        let r = r.saturating_sub(match (pre > 0, self.fade_on) {
            (false, _) => 0,
            (true, false) => 14,
            (true, true) => 15 - (v - 1) * 4 / 3,
        });
        self.rcost = r;
        self.first = self.host + self.prerun_cc + self.interval + r;
        // the init returns with the timer about to start (the engine restarts Paula's output
        // grid here, as it does when the original's init returns)
        hw.bus.clock = self.host + self.prerun_cc + r;
    }

    /// The host's start-up: three rounds that stop the channels, point them at the silent buffer
    /// and set period 1 and volume 0 (the last ends at the host's last start-up write), so that
    /// Paula is in the state the first tick finds it in. Rounds start 1082 and 580 colour clocks
    /// apart (measured); a round is the DMA write, then per channel the address, length, period
    /// and volume, 62 clocks apart.
    fn host_silence(&mut self, hw: &mut Hw) {
        let last = self.host.saturating_sub(240);
        for start in [last.saturating_sub(1082 + 580), last.saturating_sub(1082), last] {
            if hw.bus.clock > start {
                continue;
            }
            hw.bus.clock = start;
            hw.dmacon(0x000F);
            for c in 0..4 {
                let t = start + 62 * c as u64;
                hw.bus.clock = t + 32;
                hw.loc(c, self.silent);
                hw.bus.clock = t + 41;
                hw.len(c, 2);
                hw.bus.clock = t + 54;
                hw.per(c, 1);
                hw.vol(c, 0);
            }
        }
    }

    // --- the sequencer (section 5)

    fn voice_end(&mut self, hw: &mut Hw, i: usize) {
        self.c(END_A);
        {
            self.c(END_B);
            self.v[i].ended = true;
            self.dma(hw, 1 << i);
            self.c(END_C);
            // the song-end test (section 7)
            self.c(SE_A + SE_CMP_I[i] * (i as u64 + 1) + SE_CLR_I[i] + SE_B);
            self.alive[i] = false;
            if self.alive.iter().all(|&a| !a) {
                self.alive = [true; 4];
                self.songend = true;
                self.c(SE_FIRE);
            }
            self.c(SE_C);
        }
        self.v[i].ended = true;
        self.c(END_D);
    }

    fn seq_step(&mut self, hw: &mut Hw, i: usize, prerun: bool) {
        self.c(SEQ_A);
        if self.v[i].ended {
            self.c(RTS);
            return;
        }
        self.c(SEQ_B);
        self.v[i].trigger = false;
        let mut cnt: u8 = 0;
        self.v[i].dur = self.v[i].dur.wrapping_sub(1);
        if self.v[i].dur != 0 {
            self.c(RTS);
            return;
        }
        self.c(SEQ_C);
        let mut p = self.v[i].ptr;
        loop {
            self.c(SEQ_ITEM);
            let b0 = self.ib(p);
            if b0 & 0x80 != 0 {
                self.c(CMD_HDR);
                cnt = cnt.wrapping_sub(1);
                if cnt == 0 {
                    self.c(CMD_ABORT);
                    return;
                }
                let cmd = b0 & 0x3F;
                let op = self.ib(p.wrapping_add(1));
                let mut q = p.wrapping_add(2);
                self.c(CMD_DISP);
                let lv = self.v[i].lvl;
                match cmd {
                    0 => {
                        self.c(K0_A);
                        if lv > 0 {
                            self.v[i].lvl = lv - 1;
                            q = self.v[i].ret[lv - 1];
                            self.c(K0_POP);
                        } else {
                            self.voice_end(hw, i);
                            return;
                        }
                    }
                    1 => {
                        let s = self.il(q);
                        q = q.wrapping_add(4);
                        self.v[i].sound = Some(s);
                        for j in 0..4u32 {
                            self.detune[j as usize] = self.par(Some(s), 14 + j) as i8;
                        }
                        self.v[i].midx = [0; 4];
                        self.v[i].mdly = [0; 4];
                        self.v[i].started = false;
                        self.c(K1);
                    }
                    2 => {
                        self.v[i].arp = Some(self.il(q));
                        q = q.wrapping_add(4);
                        self.c(K2);
                    }
                    3 => {
                        self.v[i].arp = None;
                        self.v[i].aidx = 0;
                        self.c(K3);
                    }
                    4 => {
                        self.v[i].port = self.iw(q);
                        q = q.wrapping_add(2);
                        self.c(K4);
                    }
                    5 => self.c(K5),
                    6 => {
                        let t = self.il(q);
                        let after = q.wrapping_add(4);
                        self.c(K6_A);
                        self.v[i].ret[lv] = after;
                        let mut nl = lv;
                        if lv < 7 {
                            nl = lv + 1;
                            self.c(K6_INC);
                        }
                        self.v[i].lvl = nl;
                        self.v[i].lcnt[nl] = 1;
                        q = self.il(t.wrapping_add(8));
                        self.c(K6_B);
                    }
                    7 => {
                        let t = self.il(q);
                        q = self.il(t.wrapping_add(8));
                        self.c(K7);
                    }
                    8 => {
                        self.v[i].lstart[lv] = q;
                        self.v[i].lcnt[lv] = op;
                        self.c(K8);
                    }
                    9 => {
                        self.c(K9_A);
                        let n = self.v[i].lcnt[lv].wrapping_sub(1);
                        self.v[i].lcnt[lv] = n;
                        if n != 0 {
                            q = self.v[i].lstart[lv];
                            self.c(K9_LOOP);
                        } else {
                            self.c(K9_EXIT);
                        }
                    }
                    10 => {
                        self.c(K10_A);
                        q = self.v[i].lstart[lv];
                        self.c(K10_JMP);
                    }
                    11 => {
                        self.v[i].t1 = op;
                        self.c(K11);
                    }
                    12 => {
                        self.v[i].t2 = op;
                        self.c(K12);
                    }
                    13 => {
                        self.c(K13_A);
                        if prerun {
                            self.c(K13_PRE);
                        } else {
                            self.voice_end(hw, i);
                            return;
                        }
                    }
                    // 14, and the numbers the specification leaves open
                    _ => self.c(K14),
                }
                self.c(CMD_AFTER);
                p = q;
            } else {
                let d = self.ib(p.wrapping_add(1));
                self.c(NOTE_A);
                if b0 != 0 {
                    self.c(NOTE_SET);
                    self.v[i].note = b0;
                }
                self.v[i].gate = b0;
                self.c(NOTE_B);
                if d & 0x80 != 0 {
                    self.v[i].dur = d & 0x7F;
                    self.c(DUR_TIE);
                } else {
                    self.v[i].dur = d;
                    if b0 != 0 {
                        self.v[i].trigger = true;
                    }
                    self.c(DUR_TRIG);
                }
                self.v[i].ptr = p.wrapping_add(2);
                return;
            }
        }
    }

    // --- sound processing of a voice (section 6)

    fn raster_wait(&mut self) {
        self.c(DW_A);
        self.c(DW_B);
        for _ in 0..9 {
            self.c(DW_IT);
            let l0 = self.at() / LINE;
            for _ in 0..4096 {
                self.c(DW_RD);
                let l1 = self.at() / LINE;
                self.c(DW_CMP);
                if l1 != l0 {
                    break;
                }
            }
            self.c(DW_IT_END);
        }
        self.c(DW_C);
    }

    /// A period modulator (index 0 or 1).
    fn period_mod(&mut self, i: usize, m: usize, snd: Option<u32>, p: &mut u16) {
        let (a, dl, b, cc, neg, d, wrap, stop, e) = if m == 0 {
            (M0_A, M0_DELAY, M0_B, M0_C, M0_NEG, M0_D, M0_WRAP, M0_STOP, M0_E)
        } else {
            (M1_A, M1_DELAY, M1_B, M1_C, M1_NEG, M1_D, M1_WRAP, M1_STOP, M1_E)
        };
        let base = if m == 0 { 26 } else { 34 };
        let tab = self.par32(snd, base);
        if m == 1 {
            self.c(M1_PRE);
        }
        if tab == 0 {
            return;
        }
        self.c(a);
        if self.v[i].mdly[m] != 0 {
            self.v[i].mdly[m] -= 1;
            self.c(dl);
            return;
        }
        self.c(b);
        let idx = self.v[i].midx[m];
        if idx & 0x8000 != 0 {
            return;
        }
        let data = self.il(tab.wrapping_add(8));
        let cnt = self.iw(tab.wrapping_add(12));
        let mut x = self.ib(data.wrapping_add(idx as u32)) as i8 as i32;
        self.c(cc);
        if self.par(snd, base + 7) != 0 {
            x = -x;
            self.c(neg);
        }
        self.c(d);
        let sh = (self.par(snd, base + 4) & 63) as u32;
        let x = if sh >= 16 { 0 } else { (x << sh) as i16 };
        *p = p.wrapping_add(x as u16);
        let mut ni = idx.wrapping_add(1);
        if ni >= cnt {
            self.c(wrap);
            ni = 0;
            if self.par(snd, base + 6) & 0x80 != 0 {
                ni = 0xFFFF;
                self.c(stop);
            }
        }
        self.v[i].midx[m] = ni;
        self.c(e);
    }

    /// An address modulator (index 2 or 3); adds to `a`.
    fn addr_mod(&mut self, i: usize, m: usize, snd: Option<u32>, a: &mut u32) {
        let base = if m == 2 { 42 } else { 50 };
        let tab = self.par32(snd, base);
        let shift = self.par(snd, base + 4) as u32;
        let flag = self.par(snd, base + 6);
        let (ca, cdel, cb, cc, cwrap, cstop, ce) = if m == 2 {
            (A2_A, A2_DELAY, A2_B, A2_C, A2_WRAP, A2_STOP, A2_E)
        } else {
            (A3_A, A3_DELAY, A3_B, A3_C, A3_WRAP, A3_STOP, A3_E)
        };
        if m == 3 {
            self.c(A3_PRE);
        }
        if tab == 0 {
            return;
        }
        self.c(ca);
        if m == 2 {
            if self.v[i].mdly[m] != 0 {
                self.v[i].mdly[m] -= 1;
                self.c(cdel);
                return;
            }
        } else {
            while self.v[i].mdly[m] != 0 {
                self.v[i].mdly[m] -= 1;
                self.c(cdel);
                self.c(A3_LOOP);
            }
        }
        self.c(cb);
        let idx = self.v[i].midx[m];
        if idx & 0x8000 != 0 {
            return;
        }
        self.c(cc);
        let data = self.il(tab.wrapping_add(8));
        let cnt = self.iw(tab.wrapping_add(12));
        let v = self.ib(data.wrapping_add(idx as u32)) as u32;
        let sh = (shift + 1) & 63;
        let v = if sh >= 16 { 0 } else { (v << sh) & 0xFFFF };
        *a = a.wrapping_add(v);
        let mut ni = idx.wrapping_add(1);
        if ni >= cnt {
            self.c(cwrap);
            ni = 0;
            if flag & 0x80 != 0 {
                ni = 0xFFFF;
                self.c(cstop);
            }
        }
        self.v[i].midx[m] = ni;
        self.c(ce);
    }

    fn process(&mut self, hw: &mut Hw, i: usize) {
        self.c(PR_A);
        if self.v[i].ended {
            self.c(PR_ENDED);
            return;
        }
        self.c(PR_B);
        self.c(PR_C);
        let snd = self.v[i].sound;
        let trig = self.v[i].trigger;
        // 6.2 a new note
        if trig {
            self.c(TR_A);
            self.dma(hw, 1 << i);
            self.c(TR_B);
            self.v[i].aidx = 0;
            self.v[i].started = false;
            self.v[i].phase = 0;
            let p18 = self.par(snd, 18) as i32;
            if self.par(snd, 25) != 0 {
                self.v[i].level = p18;
                self.c(LV_SET);
            } else {
                self.c(LV_CMP);
                if self.v[i].level <= p18 {
                    self.v[i].level = p18;
                    self.c(LV_SET);
                }
            }
            for j in 0..4 {
                self.c(MI_A_J[j]);
                let flag = self.par(snd, [32, 40, 48, 56][j]);
                if flag != 1 {
                    self.v[i].midx[j] = 0;
                    self.v[i].mdly[j] = self.par(snd, [31, 39, 47, 55][j]);
                    self.c(MI_B_J[j]);
                }
            }
        }
        // 6.3 the waveform record
        self.c(TY_A);
        let ws = match snd {
            Some(s) => self.il(s.wrapping_add(8)),
            None => 0,
        };
        let listmode = snd.is_some() && self.ib(ws.wrapping_add(8)) != 0;
        let mut wrec: Option<u32> = None;
        if snd.is_some() && !listmode {
            wrec = Some(self.il(ws.wrapping_add(10)));
        }
        if listmode {
            self.c(L_A);
            let mut empty = false;
            if !self.v[i].started {
                self.c(L_FIRST);
                let e0 = self.il(ws.wrapping_add(10)).wrapping_add(8);
                if self.il(e0) == 0 {
                    empty = true;
                    self.c(L_EMPTY);
                } else {
                    let v = &mut self.v[i];
                    v.lloop = e0;
                    v.started = true;
                    let (cur, rem) = (self.il(e0), self.ib(e0.wrapping_add(4)));
                    let v = &mut self.v[i];
                    v.cur = cur;
                    v.rem = rem;
                    v.next = e0.wrapping_add(6);
                    self.c(L_LOAD);
                }
            }
            if !empty {
                wrec = Some(self.v[i].cur);
                self.c(L_DEC);
                self.v[i].rem = self.v[i].rem.wrapping_sub(1);
                if self.v[i].rem == 0 {
                    self.c(L_ADV);
                    let mut e = self.v[i].next;
                    if self.il(e) == 0 {
                        e = self.v[i].lloop;
                        self.c(L_WRAP);
                    }
                    self.c(L_RD);
                    let (cur, rem, fl) = (self.il(e), self.ib(e.wrapping_add(4)), self.ib(e.wrapping_add(5)));
                    let v = &mut self.v[i];
                    v.cur = cur;
                    v.rem = rem;
                    v.next = e.wrapping_add(6);
                    if fl != 0 {
                        v.lloop = e;
                        self.c(L_FLAG);
                    }
                    self.c(L_END);
                }
            }
        } else {
            self.c(S_A);
        }
        let w = self.wf(wrec);
        // 6.4 the period
        self.c(BASE);
        let b = w.trans.wrapping_add(self.v[i].note).wrapping_add(self.v[i].t1).wrapping_add(self.v[i].t2);
        let mut per: u16;
        if self.v[i].port == 0 {
            self.c(ARP_A);
            let tab = self.v[i].arp;
            let mut d = self.arp_byte(tab, self.v[i].aidx);
            if d == 0x80 {
                d = self.arp_byte(tab, 0);
                self.v[i].aidx = 0xFFFF;
                self.c(ARP_WRAP);
            }
            self.v[i].aidx = self.v[i].aidx.wrapping_add(1);
            self.c(ARP_B);
            per = PERIODS[(((b.wrapping_add(d)) & 0x7F) as usize).min(96)];
        } else {
            self.c(POR_A);
            let t = PERIODS[((b & 0x7F) as usize).min(96)] as i32;
            let c0 = self.v[i].last as i32;
            let sp = self.v[i].port as i32;
            let mut x;
            if t >= c0 {
                self.c(POR_UP);
                x = c0 + sp;
                if x > 65535 {
                    x = t;
                    self.c(POR_CLAMP);
                } else {
                    self.c(POR_UP_OK);
                    if x >= t {
                        x = t;
                        self.c(POR_UP_CLAMP);
                        self.c(POR_CLAMP);
                    }
                }
            } else {
                self.c(POR_DN);
                x = c0 - sp;
                if x < 0 {
                    x = t;
                    self.c(POR_CLAMP);
                } else {
                    self.c(POR_DN_OK);
                    if x <= t {
                        x = t;
                        self.c(POR_CLAMP);
                    }
                }
            }
            self.c(POR_END);
            per = x as u16;
        }
        self.v[i].last = per;
        self.c(PER_SAVE);
        for m in 0..2 {
            self.period_mod(i, m, snd, &mut per);
        }
        self.c(DET);
        per = per.wrapping_add(self.detune[i] as i16 as u16);
        hw.bus.clock = self.at();
        hw.per(i, per);
        self.c(PER_POST);
        self.c(SETPER_I[i]);
        self.c(W_A);
        // 6.5 the waveform address and length
        let w2 = if listmode { w } else { self.wf(wrec) };
        let update = if listmode {
            self.c(W_LIST);
            true
        } else {
            self.c(W_SINGLE);
            if trig {
                true
            } else {
                self.c(W_CHK);
                w2.flags & 1 != 0
            }
        };
        if update {
            self.c(GO_A);
            let mut a = w2.data;
            let mut l = w2.len / 2;
            if w2.flags & 1 != 0 {
                l /= 2;
                self.c(SYNC_A);
                self.addr_mod(i, 2, snd, &mut a);
                self.addr_mod(i, 3, snd, &mut a);
            }
            self.c(LEN_A);
            hw.bus.clock = self.at();
            hw.len(i, l);
            self.c(ADDR_A);
            hw.bus.clock = self.at();
            hw.loc(i, a);
            self.c(ADDR_B);
            self.c(SETTWO_I[i]);
        }
        // 6.6 the volume
        self.c(VOL_A);
        self.c(VOL_B);
        self.c(VOL_PRE_I[i]);
        let p8 = self.par(snd, 8) as u64;
        let vol = ((((self.v[i].level as u64) * p8 * self.master as u64) >> 16) & 0xFF) >> 2;
        hw.bus.clock = self.at();
        hw.vol(i, vol as u16);
        self.c(VOL_POST_I[i]);
        // 6.7 the envelope
        self.envelope(i, snd);
        // 6.8 the wait for the beam and the start
        self.raster_wait();
        self.c(AFTER_DW);
        if self.v[i].trigger {
            self.c(ON_A);
            self.dma(hw, 0x8000 | 1 << i);
            self.v[i].trigger = false;
            self.c(ON_B);
        } else {
            self.c(NT_A);
            if !listmode {
                self.c(OS_A);
                if w2.flags & 2 != 0 {
                    self.c(OS_LEN);
                    hw.bus.clock = self.at();
                    hw.len(i, 2);
                    self.c(OS_ADDR);
                    hw.bus.clock = self.at();
                    hw.loc(i, self.silent);
                }
            }
            self.c(END_PROC);
        }
    }

    fn envelope(&mut self, i: usize, snd: Option<u32>) {
        let (b18, b19, b20, b21, b22, b23, b24) = (
            self.par(snd, 18) as i32,
            self.par(snd, 19) as i32,
            self.par(snd, 20) as i32,
            self.par(snd, 21) as i32,
            self.par(snd, 22) as i32,
            self.par(snd, 23) as i32,
            self.par(snd, 24) as i32,
        );
        self.c(ENV_A);
        if self.v[i].gate == 0 {
            self.c(REL_A);
            self.v[i].level -= b24;
            if self.v[i].level < 0 {
                self.v[i].level = 0;
                self.c(REL_FLOOR);
            }
            return;
        }
        self.c(ENV_G);
        match self.v[i].phase {
            0 => {
                self.c(PH0_A);
                if b18 == b19 {
                    self.decay(i, b20, b21);
                } else {
                    self.c(PH0_B);
                    self.v[i].level += b18;
                    if self.v[i].level > 255 {
                        self.v[i].level = b19;
                        self.v[i].phase = 1;
                        self.c(PH0_TO1);
                    } else {
                        self.c(PH0_C);
                        if self.v[i].level >= b19 {
                            self.v[i].level = b19;
                            self.v[i].phase = 1;
                            self.c(PH0_TO1);
                        }
                    }
                }
            }
            1 => {
                self.c(PH1_A);
                self.decay(i, b20, b21);
            }
            2 => {
                self.c(PH2_A);
                self.c(PH2_B);
                if b22 < 0x80 {
                    self.c(PH2_UP);
                    self.v[i].level += b22;
                    if self.v[i].level > 255 {
                        self.v[i].level = b23;
                        self.v[i].phase = 3;
                        self.c(PH2_UP_END);
                    } else {
                        self.c(PH2_UP_OK);
                        if self.v[i].level >= b23 {
                            self.v[i].level = b23;
                            self.v[i].phase = 3;
                            self.c(PH2_UP_END);
                        }
                    }
                } else {
                    self.c(PH2_DN);
                    self.v[i].level -= b22 & 0x7F;
                    if self.v[i].level < 0 {
                        self.v[i].level = b23;
                        self.v[i].phase = 3;
                        self.c(PH2_DN_END);
                    } else {
                        self.c(PH2_DN_OK);
                        if self.v[i].level <= b23 {
                            self.v[i].level = b23;
                            self.v[i].phase = 3;
                            self.c(PH2_DN_END);
                        }
                    }
                }
            }
            _ => {
                self.c(PH3_A);
                self.c(PH3_B);
                self.v[i].level = b23;
            }
        }
    }

    fn decay(&mut self, i: usize, b20: i32, b21: i32) {
        self.c(DEC_A);
        self.v[i].level -= b20;
        if self.v[i].level < 0 {
            self.v[i].level = b21;
            self.v[i].phase = 2;
            self.c(DEC_END);
        } else {
            self.c(DEC_OK);
            if self.v[i].level <= b21 {
                self.v[i].level = b21;
                self.v[i].phase = 2;
                self.c(DEC_END);
            }
        }
    }

    // --- one timer tick (section 4)

    fn tick_body(&mut self, hw: &mut Hw) {
        self.t0 = hw.bus.clock;
        self.cy = 0;
        self.c(TICK_IN);
        self.c(TICK_A);
        if self.fade_on {
            self.c(FADE_CHK);
            self.c(FADE_A);
            if self.fstep as u32 + self.master as u32 > 255 {
                self.c(FADE_END);
                self.c(SETMASTER);
                self.master = 255;
                self.fade_on = false;
            } else {
                self.c(FADE_STEP);
                self.c(SETMASTER);
                self.master += self.fstep;
            }
        }
        self.c(TICK_B);
        self.beat = self.beat.wrapping_sub(1);
        if self.beat == 0 {
            self.c(BEAT);
            self.beat = self.speed;
            for i in 0..4 {
                self.c(SEQCALL_I[i]);
                self.seq_step(hw, i, false);
            }
        }
        for i in 0..4 {
            self.c(PROCCALL_I[i]);
            self.process(hw, i);
        }
        self.c(TICK_C);
        self.c(TICK_OUT);
        hw.bus.clock = self.at();
    }
}

impl Replayer for FuturePlayer {
    fn format(&self) -> &'static str {
        "Future Player"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick_clocks(&self) -> u64 {
        self.interval
    }

    /// The first tick follows the init by one interval (the engine adds it to its clock).
    fn timer_delay(&self) -> u64 {
        0
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.nsub as u16 - 1, self.sub as u16)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        let n = (n as usize).min(self.nsub - 1);
        // the host initialises the earlier subsongs as stubs: the previous subsong's start and
        // pre-run, a stop, and the next start-up (fitted to the reference's logs)
        let (voices, fade) = self.shape(n);
        self.host += self.rcost + self.prerun_cc + 7736 + 10 * (4 - voices) - if fade { 120 } else { 0 };
        self.init(hw, n);
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        Some(self.first)
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.tick_body(hw);
    }
}

#[cfg(test)]
#[path = "futureplayer_tests.rs"]
mod tests;
