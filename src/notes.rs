//! Software voices for "_note" players. Such players never touch Paula;
//! they fill a NoteStruct with per-channel sample/period/volume requests and
//! call dtg_NotePlayer, and DeliTracker's NotePlayer genie renders them.
//! This module is that renderer.
//!
//! Layout (recovered from SoundFX13_note and the 4Voc-NotePlayer genie):
//!   NoteStruct:  +0 channel list, +4 flags, +8 max frequency, +12 max volume
//!   NoteChannel: +0 next, +11 changed bits, +12 stereo position,
//!                +16 sample start, +20 sample length, +24 repeat start,
//!                +28 repeat length, +32 frequency/period, +36 volume

use crate::bus::Bus;
use crate::paula::PAULA_CLOCK_PAL;

const NSTF_PERIOD: u32 = 1 << 1;
const NSTF_EXTPERIOD: u32 = 1 << 2;
const NSTF_NTSC: u32 = 1 << 3;
const NSTF_EVENLENGTH: u32 = 1 << 4;
const NSTF_ALLREPEATS: u32 = 1 << 5;
const NSTF_REVERSE: u32 = 1 << 8;
const NSTF_UNSIGNED: u32 = 1 << 10;
const NSTF_16BIT: u32 = 1 << 18;

const NCHF_SAMPLE: u8 = 1 << 1;
const NCHF_REPEAT: u8 = 1 << 2;
const NCHF_FREQUENCY: u8 = 1 << 3;
const NCHF_VOLUME: u8 = 1 << 4;

const PAULA_CLOCK_NTSC: f64 = 3_579_545.0;

#[derive(Clone, Default)]
pub struct Voice {
    pub active: bool,
    start: u32,
    end: u32,
    rep_start: u32,
    rep_end: u32,
    next_rep_start: u32,
    next_rep_end: u32,
    /// Position in bytes from `start`. Relative so that the rounding of the fractional
    /// step does not depend on where in memory the sample happens to lie.
    pos: f64,
    pub freq: f64,
    pub vol: f32,
    pub pan: f32, // -1 left .. 1 right
    pub scope: f32,
}

#[derive(Clone, Default)]
pub struct NoteVoices {
    pub voices: Vec<Voice>,
    flags: u32,
    max_vol: f32,
    pub in_use: bool,
}

impl NoteVoices {
    pub fn reset(&mut self) {
        *self = NoteVoices::default();
    }

    pub fn update_from_guest(&mut self, bus: &mut Bus, ns: u32) {
        self.in_use = true;
        self.flags = bus.r32(ns + 4);
        let mv = bus.r16(ns + 12);
        self.max_vol = if mv == 0 { 64.0 } else { mv as f32 };
        let flags = self.flags;
        let mut ch = bus.r32(ns);
        let mut i = 0;
        while ch != 0 && i < 64 {
            if self.voices.len() <= i {
                self.voices.push(Voice::default());
            }
            let changed = bus.r8(ch + 11);
            let pos = bus.r16(ch + 12) as i16;
            if log::log_enabled!(log::Level::Trace) {
                // A hash of the repeat region's bytes too: synth players rewrite the
                // waveform in place every tick without flagging a change.
                let (rs, rl) = (bus.r32(ch + 24), if flags & NSTF_EVENLENGTH != 0 { bus.r16(ch + 28) as u32 * 2 } else { bus.r32(ch + 28) });
                let h = (rs..rs.wrapping_add(rl.min(0x10000))).fold(0xcbf2_9ce4_8422_2325u64, |h, a| {
                    (h ^ bus.r8(a) as u64).wrapping_mul(0x0100_0000_01b3)
                });
                log::trace!(
                    "ns flags={flags:08x} ch{i} hdr={:08x}.{:08x}.{:08x}.{:08x} smp={:08x}/{:08x} rep={:08x}/{:08x} f={:08x} v={:08x} data={h:016x}",
                    bus.r32(ch), bus.r32(ch + 4), bus.r32(ch + 8), bus.r32(ch + 12),
                    bus.r32(ch + 16), bus.r32(ch + 20), rs, bus.r32(ch + 28),
                    bus.r32(ch + 32), bus.r32(ch + 36)
                );
            }
            let v = &mut self.voices[i];
            v.pan = if pos == i16::MIN { 0.0 } else { pos as f32 / 32767.0 };
            let read_len = |bus: &Bus, a: u32| -> u32 {
                if flags & NSTF_EVENLENGTH != 0 {
                    bus.r16(a) as u32 * 2
                } else {
                    bus.r32(a)
                }
            };
            if changed & NCHF_REPEAT != 0 {
                let rs = bus.r32(ch + 24);
                let rl = read_len(bus, ch + 28);
                v.next_rep_start = rs;
                v.next_rep_end = if rl > 2 || flags & NSTF_ALLREPEATS != 0 { rs + rl } else { rs };
                // synth players (AllRepeats) only ever set the repeat part:
                // an idle channel starts on it directly
                if !v.active && changed & NCHF_SAMPLE == 0 && v.next_rep_end > v.next_rep_start {
                    v.start = rs;
                    v.end = v.next_rep_end;
                    v.pos = 0.0;
                    v.rep_start = rs;
                    v.rep_end = v.next_rep_end;
                    v.active = true;
                }
            }
            if changed & NCHF_SAMPLE != 0 {
                let s = bus.r32(ch + 16);
                let l = read_len(bus, ch + 20);
                v.start = s;
                v.end = s + l;
                v.pos = 0.0;
                v.active = l > 0;
                v.rep_start = v.next_rep_start;
                v.rep_end = v.next_rep_end;
            }
            if changed & NCHF_FREQUENCY != 0 {
                v.freq = if flags & (NSTF_PERIOD | NSTF_EXTPERIOD) != 0 {
                    let per = bus.r16(ch + 32) as f64;
                    let clock = if flags & NSTF_NTSC != 0 { PAULA_CLOCK_NTSC } else { PAULA_CLOCK_PAL };
                    let mul = if flags & NSTF_EXTPERIOD != 0 { 4.0 } else { 1.0 };
                    if per > 0.0 { clock * mul / per } else { 0.0 }
                } else {
                    bus.r32(ch + 32) as f64
                };
            }
            if changed & NCHF_VOLUME != 0 {
                v.vol = bus.r16(ch + 36) as f32 / self.max_vol;
            }
            bus.w8(ch + 11, 0);
            ch = bus.r32(ch);
            i += 1;
        }
    }

    /// The interpolated sample `off` bytes past `base`.
    fn sample(&self, ram: &[u8], base: u32, off: f64) -> f32 {
        let wide = self.flags & NSTF_16BIT != 0;
        let unsigned = self.flags & NSTF_UNSIGNED != 0;
        let get = |p: u32| -> f32 {
            if wide {
                let p = (p & !1) as usize;
                let (b0, b1) = (ram.get(p).copied().unwrap_or(0), ram.get(p + 1).copied().unwrap_or(0));
                let v = if self.flags & NSTF_REVERSE != 0 { u16::from_le_bytes([b0, b1]) } else { u16::from_be_bytes([b0, b1]) };
                let v = if unsigned { v.wrapping_sub(0x8000) } else { v } as i16;
                v as f32 / 32768.0
            } else {
                let b = ram.get(p as usize).copied().unwrap_or(0);
                let b = if unsigned { b.wrapping_sub(0x80) } else { b };
                b as i8 as f32 / 128.0
            }
        };
        let step = if wide { 2 } else { 1 };
        let whole = off as u32;
        let i = base.wrapping_add(whole);
        let i = if wide { i & !1 } else { i };
        // (base + whole - i) is 0 or 1: exact, whatever the magnitude of the address
        let frac = ((base.wrapping_add(whole).wrapping_sub(i) as f64 + (off - whole as f64)) / step as f64) as f32;
        let s0 = get(i);
        let s1 = get(i + step);
        s0 + (s1 - s0) * frac
    }

    /// Renders one output frame (left, right) at `rate` Hz.
    pub fn render(&mut self, ram: &[u8], rate: f64) -> (f32, f32) {
        let (mut l, mut r) = (0.0f32, 0.0f32);
        let wide = self.flags & NSTF_16BIT != 0;
        for vi in 0..self.voices.len() {
            if !self.voices[vi].active || self.voices[vi].freq <= 0.0 {
                self.voices[vi].scope = 0.0;
                continue;
            }
            let (start, pos) = (self.voices[vi].start, self.voices[vi].pos);
            let s = self.sample(ram, start, pos) * self.voices[vi].vol;
            let v = &mut self.voices[vi];
            v.scope = s;
            let pan = v.pan.clamp(-1.0, 1.0);
            l += s * (1.0 - pan) * 0.5;
            r += s * (1.0 + pan) * 0.5;
            v.pos += v.freq / rate * if wide { 2.0 } else { 1.0 };
            let len = v.end.wrapping_sub(v.start) as f64;
            if v.pos >= len {
                if v.rep_end > v.rep_start {
                    let rlen = (v.rep_end - v.rep_start) as f64;
                    let over = (v.pos - len) % rlen;
                    v.start = v.rep_start;
                    v.end = v.rep_end;
                    v.pos = over;
                    // a new repeat latched while looping takes effect now
                    v.rep_start = v.next_rep_start;
                    v.rep_end = v.next_rep_end;
                } else {
                    v.active = false;
                }
            }
        }
        (l, r)
    }
}

/// The NoteStruct DeliTracker hands a NotePlayer genie. The genie does not see the
/// player's: it gets one in the form its `DTP_NotePlayer` record asks for, which
/// DeliTracker refills from the player's on every `dtg_NotePlayer`. The record is
/// left voices (word), right voices (word), NSTF flags (long), maximum frequency
/// (long, $FFFFFFFF: none), and a genie's channel differs from a player's in
///   +4  the genie's own pointer for the channel (left alone here)
///   +14 the side DeliTracker gave it (<0 left, >0 right, $8000 no voice left)
///   +20, +28 lengths in bytes, +32 the frequency in Hz (unless the genie takes
///   periods), clamped to the genie's maximum
/// (from the 4Voc, 8Voc and Headphone NotePlayer genies).
pub struct GenieNotes {
    pub ns: u32,
    flags: u32,
    max_freq: u32,
    chans: Vec<u32>,
}

pub const GENIE_CHANNELS: usize = 64;
const GENIE_CH_SIZE: u32 = 40;
pub const GENIE_NOTES_SIZE: u32 = 16 + GENIE_CHANNELS as u32 * GENIE_CH_SIZE;

fn player_len(bus: &Bus, flags: u32, a: u32) -> u32 {
    if flags & NSTF_EVENLENGTH != 0 { bus.r16(a) as u32 * 2 } else { bus.r32(a) }
}

impl GenieNotes {
    /// Lays the genie's NoteStruct out at `mem` (GENIE_NOTES_SIZE bytes) after the
    /// player's `pns`, and gives each channel a side, as many per side as the genie
    /// has voices there.
    pub fn new(bus: &mut Bus, mem: u32, record: u32, pns: u32) -> Self {
        let (mut left, mut right) = (bus.r16(record) as u32, bus.r16(record + 2) as u32);
        let mut g = GenieNotes { ns: mem, flags: bus.r32(record + 4), max_freq: bus.r32(record + 8), chans: Vec::new() };
        for a in mem..mem + GENIE_NOTES_SIZE {
            bus.w8(a, 0);
        }
        let mut pch = bus.r32(pns);
        while pch != 0 && g.chans.len() < GENIE_CHANNELS {
            let ch = mem + 16 + g.chans.len() as u32 * GENIE_CH_SIZE;
            if let Some(&prev) = g.chans.last() {
                bus.w32(prev, ch);
            }
            let want = bus.r16(pch + 12) as i16;
            let side: i16 = if want == i16::MIN {
                i16::MIN
            } else if (want < 0 || (want == 0 && left >= right)) && left > 0 {
                left -= 1;
                if want < 0 { want } else { -0x7FFF }
            } else if right > 0 {
                right -= 1;
                if want > 0 { want } else { 0x7FFF }
            } else if left > 0 {
                left -= 1;
                -0x7FFF
            } else {
                i16::MIN
            };
            bus.w16(ch + 12, want as u16);
            bus.w16(ch + 14, side as u16);
            g.chans.push(ch);
            pch = bus.r32(pch);
        }
        bus.w32(mem, if g.chans.is_empty() { 0 } else { mem + 16 });
        bus.w32(mem + 4, g.flags);
        bus.w32(mem + 8, g.max_freq);
        bus.w16(mem + 12, bus.r16(pns + 12));
        g
    }

    /// Copies the player's changes into the genie's NoteStruct, and clears them.
    pub fn update(&self, bus: &mut Bus, pns: u32) {
        let pf = bus.r32(pns + 4);
        bus.w16(self.ns + 12, bus.r16(pns + 12));
        let mut pch = bus.r32(pns);
        for &ch in &self.chans {
            if pch == 0 {
                break;
            }
            let changed = bus.r8(pch + 11);
            bus.w8(ch + 11, changed);
            bus.w16(ch + 12, bus.r16(pch + 12));
            if changed & NCHF_SAMPLE != 0 {
                bus.w32(ch + 16, bus.r32(pch + 16));
                bus.w32(ch + 20, player_len(bus, pf, pch + 20));
            }
            if changed & NCHF_REPEAT != 0 {
                bus.w32(ch + 24, bus.r32(pch + 24));
                bus.w32(ch + 28, player_len(bus, pf, pch + 28));
            }
            if changed & NCHF_FREQUENCY != 0 {
                let f = if pf & (NSTF_PERIOD | NSTF_EXTPERIOD) != 0 && self.flags & NSTF_PERIOD == 0 {
                    let per = bus.r16(pch + 32) as f64;
                    let clock = if pf & NSTF_NTSC != 0 { PAULA_CLOCK_NTSC } else { PAULA_CLOCK_PAL };
                    let mul = if pf & NSTF_EXTPERIOD != 0 { 4.0 } else { 1.0 };
                    if per > 0.0 { (clock * mul / per).round() as u32 } else { 0 }
                } else {
                    bus.r32(pch + 32)
                };
                bus.w32(ch + 32, f.min(self.max_freq));
            }
            if changed & NCHF_VOLUME != 0 {
                bus.w16(ch + 36, bus.r16(pch + 36));
            }
            bus.w8(pch + 11, 0);
            log::trace!(
                "genie ch {ch:06x}: voice={:08x} side={:04x} changed={changed:02x} smp={:08x}/{:08x} rep={:08x}/{:08x} f={} v={}",
                bus.r32(ch + 4), bus.r16(ch + 14), bus.r32(ch + 16), bus.r32(ch + 20),
                bus.r32(ch + 24), bus.r32(ch + 28), bus.r32(ch + 32), bus.r16(ch + 36)
            );
            pch = bus.r32(pch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A player NoteStruct at $1000 with `pos.len()` channels, as SoundFX lays it out.
    fn player(bus: &mut Bus, pos: &[u16]) -> u32 {
        let ns = 0x1000;
        bus.w32(ns + 4, NSTF_PERIOD | NSTF_EVENLENGTH);
        bus.w16(ns + 12, 64);
        for (i, &p) in pos.iter().enumerate() {
            let ch = 0x1100 + i as u32 * 0x40;
            bus.w32(if i == 0 { ns } else { ch - 0x40 }, ch);
            bus.w16(ch + 12, p);
        }
        ns
    }

    #[test]
    fn genie_gets_sides_hz_and_byte_lengths() {
        let mut bus = Bus::new();
        // L R R L plus a fifth channel; the genie has one voice per side
        let pns = player(&mut bus, &[0x8001, 0x7FFF, 0x7FFF, 0x8001, 0]);
        let rec = 0x2000;
        bus.w16(rec, 1);
        bus.w16(rec + 2, 1);
        bus.w32(rec + 4, 0x0003_0201);
        bus.w32(rec + 8, 28_867);
        let g = GenieNotes::new(&mut bus, 0x3000, rec, pns);
        let ch: Vec<u32> = (0..5).map(|i| 0x3010 + i * GENIE_CH_SIZE).collect();
        assert_eq!(bus.r32(g.ns), ch[0]);
        assert_eq!(bus.r32(ch[3]), ch[4]);
        assert_eq!(bus.r32(ch[4]), 0);
        let sides: Vec<u16> = ch.iter().map(|&c| bus.r16(c + 14)).collect();
        assert_eq!(sides, [0x8001, 0x7FFF, 0x8000, 0x8000, 0x8000]);
        assert_eq!(bus.r16(g.ns + 12), 64);

        let p0 = 0x1100;
        bus.w8(p0 + 11, NCHF_SAMPLE | NCHF_FREQUENCY | NCHF_VOLUME);
        bus.w32(p0 + 16, 0x8000);
        bus.w16(p0 + 20, 0x61F);
        bus.w16(p0 + 32, 570);
        bus.w16(p0 + 36, 63);
        bus.w8(0x1140 + 11, NCHF_FREQUENCY);
        bus.w16(0x1140 + 32, 100); // 35469 Hz: above the genie's maximum
        g.update(&mut bus, pns);
        assert_eq!(bus.r8(ch[0] + 11), NCHF_SAMPLE | NCHF_FREQUENCY | NCHF_VOLUME);
        assert_eq!((bus.r32(ch[0] + 16), bus.r32(ch[0] + 20)), (0x8000, 0xC3E));
        assert_eq!(bus.r32(ch[0] + 32), 6223);
        assert_eq!(bus.r16(ch[0] + 36), 63);
        assert_eq!(bus.r32(ch[1] + 32), 28_867);
        assert_eq!(bus.r8(p0 + 11), 0, "the player's changes are taken");
    }
}
