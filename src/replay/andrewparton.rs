//! Andrew Parton (`bye.*`), the MIDI-like music driver Andrew Parton wrote for
//! MicroProse's B-17 Flying Fortress (1993).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Andrew Parton player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! The files are data only: a `BANK` header of 20 sample slots (length, loop,
//! envelope and vibrato of each), the samples, then a byte stream of MIDI-style
//! events (note on, note off, program change, end) separated by delay bytes counted
//! in 50 Hz ticks. MIDI channel n plays on Paula channel n & 3; every voice has an
//! attack/decay/sustain/release envelope and a delayed vibrato from its program.

use super::{be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Andrew Parton", detect, new };

/// Paula periods of notes 0x30.. (index = note - 0x30).
const PERIODS: [u16; 72] = [
    2032, 1920, 1808, 1712, 1616, 1520, 1440, 1360, 1280, 1200, 1136, 1072, 1016, 960, 904, 856, 808, 760, 720, 680,
    640, 600, 568, 536, 508, 480, 452, 428, 404, 380, 360, 340, 320, 300, 284, 268, 254, 240, 226, 214, 202, 190, 180,
    170, 160, 150, 142, 134, 127, 120, 113, 107, 101, 95, 90, 85, 80, 75, 71, 67, 63, 60, 56, 53, 50, 47, 45, 42, 40,
    37, 35, 33,
];

/// The vibrato wave, 32 steps.
const WAVE: [i16; 32] = [0, 1, 1, 2, 3, 4, 4, 5, 5, 4, 3, 2, 2, 1, 1, 0, 0, 0, -1, -1, -2, -3, -4, -4, -5, -4, -4, -3, -2, -2, -1, -1];

/// Extra data bytes skipped after the first for the status nibbles the driver does not use.
const MIDIBYTES: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 1, 0, 0, 1, 0];

/// Events read in one tick at most (a stream that never delays).
const GUARD: u32 = 4096;

/// Where the samples start: the header of 20 slots of 4 tables and 2 x 4 bytes.
const SAMPLES: usize = 0x1E4;

// Colour clocks between the writes of a note start, as the original's raster waits
// keep its DMA off and on apart (measured in the reference runs).
const WAIT_OFF: u64 = 2061;
const WAIT_ON: u64 = 2084;
const WAIT_LEN_LOC: u64 = 35;
/// DMA off to the sample's address and length, and on to the period.
const WAIT_ADDR: u64 = 27;
const WAIT_REG: u64 = 6;
/// The sequencer's work before the first event of a tick, between two events, and after the last.
const WAIT_FIRST_EVENT: u64 = 330;
/// An event that writes nothing: a release, any other.
const WAIT_RELEASE: u64 = 308;
const WAIT_OTHER: u64 = 154;
const WAIT_EVENT: u64 = 581;
const WAIT_EVENTS_END: u64 = 227;
/// A voice's effect step: period to volume, volume to the next period, period to period.
const WAIT_PER_VOL: u64 = 196;
const WAIT_VOL_PER: u64 = 160;
const WAIT_PER_PER: u64 = 170;
/// Colour clocks from the host's start to the timer's first interrupt beyond one period:
/// the original's init routine runs first (measured in the reference runs).
const INIT_CLOCKS: u64 = 11028;
/// Colour clocks from the song's start to the host's timer start, beyond the first period.
const TIMER_DELAY: u64 = 295;

fn rb(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

/// The offsets of the 20 samples' data (0: absent slot) and of the music stream.
fn layout(d: &[u8]) -> Option<([u32; 20], usize)> {
    let mut a = SAMPLES;
    let mut s = [0u32; 20];
    for (i, p) in s.iter_mut().enumerate() {
        if be32(d, 4 + 4 * i) != 0 {
            a += 16;
            *p = a as u32;
            a += be32(d, 0x54 + 4 * i) as usize;
        }
    }
    (a < d.len()).then_some((s, a))
}

fn detect(d: &[u8]) -> bool {
    if d.len() < SAMPLES || &d[0..4] != b"BANK" {
        return false;
    }
    if (0..20).any(|i| be32(d, 4 + 4 * i) >= 0x20_0000) || (0..40).any(|i| be32(d, 0x54 + 4 * i) >= 0x1_0000) {
        return false;
    }
    let Some((_, music)) = layout(d) else { return false };
    // the stream's end: the first 0xFF from the music on must be followed by 0x2F
    match d[music..].iter().position(|&b| b == 0xFF) {
        Some(p) => rb(d, music + p + 1) == 0x2F,
        None => false,
    }
}

#[derive(Clone, Copy, Default)]
struct Voice {
    atk: u8,
    dec: u8,
    sus: u8,
    rel: u8,
    phase: u8,
    level: u8,
    active: u8,
    peak: u8,
    base_per: u16,
    vdelay: u8,
    vdepth: u8,
    vidx: u8,
    vspeed: u8,
}

struct Parton {
    d: Vec<u8>,
    base: u32,
    title: String,
    sample: [u32; 20],
    music: usize,
    pos: usize,
    wait: u8,
    playing: bool,
    rs: u8,
    prog: [u8; 16],
    v: [Voice; 4],
    songend: bool,
    /// Notes started in this tick's sequencer pass.
    started_notes: u8,
}

/// Signed byte comparison helper.
fn sx(b: u8) -> i8 {
    b as i8
}

impl Parton {
    fn byte(&mut self) -> u8 {
        let b = rb(&self.d, self.pos);
        self.pos += 1;
        b
    }

    /// The bank table of program `p` (slot tables of 20 longs, then 4-byte records).
    fn long(&self, table: usize, p: usize) -> u32 {
        let o = table + 4 * p;
        if table == 4 && p < 20 {
            // the pointer slots: the sample's address, 0 for an absent slot
            return if self.sample[p] != 0 { self.base + self.sample[p] } else { 0 };
        }
        be32(&self.d, o)
    }

    fn note(&mut self, v: usize, n: u8, vel: u8, p: usize, hw: &mut Hw) {
        let n = n & 0x7F;
        let vel = (vel & 0x7F) >> 1;
        if vel == 0 {
            self.v[v].phase = 0x64;
            return;
        }
        hw.wait(if self.started_notes == 0 { WAIT_FIRST_EVENT } else { WAIT_EVENT });
        self.started_notes += 1;
        let env = 0x144 + 4 * p;
        let vib = 0x194 + 4 * p;
        let d = &self.d;
        let per = n.checked_sub(0x30).map_or(PERIODS[0], |i| PERIODS[(i as usize).min(PERIODS.len() - 1)]);
        let vo = &mut self.v[v];
        vo.peak = vel;
        vo.level = 0;
        vo.phase = 0;
        vo.active = 1;
        vo.atk = rb(d, env);
        vo.dec = rb(d, env + 1);
        vo.rel = rb(d, env + 3);
        vo.sus = ((rb(d, env + 2) as u32 * vel as u32) >> 6) as u8;
        vo.vdelay = rb(d, vib);
        vo.vdepth = rb(d, vib + 1);
        vo.vspeed = rb(d, vib + 2);
        vo.vidx = 0;
        vo.base_per = per;

        let bit = 1u16 << v;
        let start = self.long(4, p);
        let len = self.long(0x54, p);
        let rlen = self.long(0xA4, p);
        let roff = self.long(0xF4, p);
        hw.dmacon(bit);
        hw.wait(WAIT_ADDR);
        hw.loc(v, start);
        hw.wait(WAIT_REG);
        hw.len(v, (len & 0xFFFF) as u16 >> 1);
        hw.wait(WAIT_REG);
        hw.per(v, per);
        hw.wait(WAIT_OFF);
        hw.dmacon(0x8200 | bit);
        hw.wait(WAIT_ON);
        if rlen != 0 {
            hw.len(v, (rlen & 0xFFFF) as u16 >> 1);
            hw.wait(WAIT_LEN_LOC);
            hw.loc(v, start.wrapping_add(roff));
        } else {
            hw.loc(v, self.base);
            hw.wait(WAIT_LEN_LOC);
            hw.len(v, 2);
        }
    }

    /// The sequencer step; false when the stream's end was read.
    fn sequencer(&mut self, hw: &mut Hw) -> bool {
        if !self.playing {
            return true;
        }
        self.wait = self.wait.wrapping_sub(1);
        if self.wait != 0 {
            return true;
        }
        self.started_notes = 0;
        for _ in 0..GUARD {
            let mut b = self.byte();
            if b & 0x80 != 0 {
                self.rs = b;
                b = self.byte();
            }
            let hi = self.rs & 0xF0;
            let ch = (self.rs & 0x0F) as usize;
            let v = (self.rs & 3) as usize;
            if self.rs == 0xFF {
                self.playing = false;
                self.songend = true;
                return false;
            }
            match hi {
                0x90 => {
                    let vel = self.byte();
                    if vel != 0 {
                        let p = (self.prog[ch] & 0x1F) as usize;
                        self.note(v, b, vel, p, hw);
                    } else {
                        self.v[v].phase = 0x64;
                        hw.wait(WAIT_RELEASE);
                    }
                }
                0x80 => {
                    self.pos += 1;
                    self.v[v].phase = 0x64;
                    hw.wait(WAIT_RELEASE);
                }
                0xC0 => {
                    if b == 0x7E {
                        // the restart: the byte after the first status byte, under
                        // the running status the loop event leaves
                        self.pos = self.music + 2;
                        continue;
                    }
                    self.prog[ch] = b;
                    hw.wait(WAIT_OTHER);
                }
                // a controller: two bytes, no effect (the original is broken here)
                0xB0 => hw.wait(WAIT_OTHER),
                _ => {
                    if MIDIBYTES[(self.rs >> 4) as usize] == 1 {
                        self.pos += 1;
                    }
                    hw.wait(WAIT_OTHER);
                }
            }
            let d = self.byte();
            if d != 0 {
                self.wait = d;
                if self.started_notes != 0 {
                    hw.wait(WAIT_EVENTS_END);
                }
                return true;
            }
        }
        true
    }

    fn effects(&mut self, hw: &mut Hw) {
        for i in 0..4 {
            let v = &mut self.v[i];
            let mut wrote_per = false;
            if v.vdelay != 0 {
                v.vdelay -= 1;
            } else {
                wrote_per = true;
                let w = WAVE[v.vidx as usize & 0x1F] as i32;
                let per = (w * v.vdepth as i32 + v.base_per as i32) as u16;
                v.vidx = v.vidx.wrapping_add(v.vspeed) & 0x1F;
                hw.per(i, per);
            }
            let v = &mut self.v[i];
            let write = match v.phase {
                0 => {
                    v.level = v.level.wrapping_add(v.atk);
                    if sx(v.level) > 0x3F {
                        v.level = 0x3F;
                    }
                    if sx(v.peak) <= sx(v.level) {
                        v.phase = 1;
                        v.level = v.peak;
                    }
                    true
                }
                1 => {
                    if v.sus == v.level {
                        v.phase = 2;
                        false
                    } else if sx(v.sus) > sx(v.level) {
                        v.level = v.level.wrapping_add(v.dec);
                        if sx(v.sus) <= sx(v.level) {
                            v.level = v.sus;
                        }
                        true
                    } else {
                        v.level = v.level.wrapping_sub(v.dec);
                        if v.level & 0x80 != 0 {
                            v.level = 0;
                        }
                        if sx(v.sus) >= sx(v.level) {
                            v.level = v.sus;
                        }
                        true
                    }
                }
                0x64 => {
                    v.level = v.level.wrapping_sub(v.rel);
                    if v.level & 0x80 != 0 {
                        v.level = 0;
                    }
                    if v.level == 0 {
                        v.phase = 0xFF;
                        v.active = 0;
                    }
                    true
                }
                _ => false,
            };
            if write {
                hw.wait(if wrote_per { WAIT_PER_VOL } else { WAIT_VOL_PER });
                hw.vol(i, (v.level & 0x3F) as u16);
                hw.wait(WAIT_VOL_PER);
            } else {
                hw.wait(WAIT_PER_PER);
            }
        }
    }
}

impl Replayer for Parton {
    fn format(&self) -> &'static str {
        "Andrew Parton"
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

    fn tick(&mut self, hw: &mut Hw) {
        // the tick that reads the end does nothing more
        if self.sequencer(hw) {
            self.effects(hw);
        }
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let (sample, music) = layout(d)?;
    // one-shot samples loop on the first long of the module, which the player clears
    m.bus.w32(base, 0);
    let title = m.bus.read_cstr(m.deli.file_buf);
    // the init routine sets the LED bit of CIA-A's port: the filter is off
    m.bus.ciaa.pra |= 2;
    m.bus.clock += INIT_CLOCKS;
    Some(Box::new(Parton {
        d: d.to_vec(),
        base,
        title,
        sample,
        music,
        pos: music + 1,
        wait: 1,
        playing: true,
        rs: 0,
        prog: [0; 16],
        v: [Voice::default(); 4],
        songend: false,
        started_notes: 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn put32(d: &mut [u8], o: usize, v: u32) {
        d[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }

    /// Slot 0 present: 8 bytes, a loop of 4 at 2; slot 1: a one-shot of 4 bytes.
    /// Program 0: attack 0x20, decay 4, sustain 32, release 0x10; no vibrato delay,
    /// depth 2, speed 1. The stream plays a note, releases it and ends.
    fn module(stream: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; SAMPLES];
        d[0..4].copy_from_slice(b"BANK");
        put32(&mut d, 4, 1);
        put32(&mut d, 8, 1);
        put32(&mut d, 0x54, 8);
        put32(&mut d, 0x58, 4);
        put32(&mut d, 0xA4, 4);
        put32(&mut d, 0xF4, 2);
        d[0x144..0x148].copy_from_slice(&[0x20, 4, 32, 0x10]);
        d[0x194..0x198].copy_from_slice(&[0, 2, 1, 0]);
        d.extend_from_slice(&[0u8; 16 + 8 + 16 + 4]);
        d.extend_from_slice(stream);
        d
    }

    const SONG: &[u8] = &[0, 0x90, 0x3C, 0x7F, 3, 0x80, 0x3C, 0x40, 2, 0xFF, 0x2F, 0];

    #[test]
    fn recognition() {
        let d = module(SONG);
        assert!(detect(&d));
        assert_eq!(layout(&d).unwrap().1, SAMPLES + 16 + 8 + 16 + 4);
        let mut e = d.clone();
        e[0] = b'b';
        assert!(!detect(&e));
        let mut e = d.clone();
        put32(&mut e, 0x58, 0x10000);
        assert!(!detect(&e));
        // the first 0xFF is not the end event, or there is none
        assert!(!detect(&module(&[0, 0x90, 0x3C, 0x7F, 0xFF, 0x2E])));
        assert!(!detect(&module(&[0, 0x90, 0x3C, 0x7F, 3])));
        assert!(!detect(&d[..0x100]));
    }

    fn run(d: Vec<u8>, ticks: usize) -> (Vec<(u32, u16)>, Engine) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/bye-test/bye.test"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Andrew Parton");
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        (log, e)
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn plays_a_note_with_envelope_vibrato_and_ends() {
        let (log, e) = run(module(SONG), 10);
        // note 0x3C on voice 0: DMA off, the sample, its length and period, DMA on,
        // then the loop (length before address)
        let dma = reg(&log, 0xDFF096);
        assert_eq!(dma[..2], [0x0001, 0x8201]);
        assert_eq!(reg(&log, 0xDFF0A4)[..2], [4, 2]);
        let lc = reg(&log, 0xDFF0A2);
        assert_eq!(lc[1].wrapping_sub(lc[0]), 2);
        let per = reg(&log, 0xDFF0A6);
        assert_eq!(per[0], PERIODS[12]);
        // vibrato: base + wave * depth, the index stepping by the speed
        assert_eq!(per[1..4], [PERIODS[12], PERIODS[12] + 2, PERIODS[12] + 2]);
        // velocity 0x7F >> 1 = 63: attack 0x20 a tick up to the cap 0x3F, then decay
        // towards 63 * 32 >> 6 = 31 by 4, and the release by 0x10 after three ticks
        let vol = reg(&log, 0xDFF0A8);
        assert_eq!(vol[..4], [0x20, 0x3F, 0x3B, 0x2B]);
        assert!(e.songend());
    }

    #[test]
    fn one_shot_loops_on_the_cleared_header() {
        let (log, _) = run(module(&[0, 0xC0, 1, 0, 0x90, 0x48, 0x40, 1, 0xFF, 0x2F]), 3);
        // the one-shot: address before length, the loop on the module's first long
        let lc = reg(&log, 0xDFF0A2);
        let base = lc[1];
        assert_eq!(lc[0].wrapping_sub(base) as usize, SAMPLES + 16 + 8 + 16);
        assert_eq!(reg(&log, 0xDFF0A4)[..2], [2, 2]);
    }
}
