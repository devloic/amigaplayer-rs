//! Frantisek Fuka's AY music ("Amadeus", the `ZXAYAMAD` type of the ZXAY container; modland's
//! "AY Amadeus", `.amad`): three voices, each a small byte-code program of notes, calls, loops
//! and transpositions, with volume envelopes and vibrato tables, played once per 50 Hz
//! interrupt. This file turns a tune into the AY registers of each interrupt; `ay.rs` plays
//! them on its AY-3-8912.
//!
//! Original implementation, written for this project (GPL-2.0-or-later). Sources:
//! - the format's description by its author, `MUSIC_FORMAT.md` of Frantisek Fuka's fxmasm
//!   (github.com/fuxoft/fxmasm, read 2026-11-11): the directives and their meaning, the note,
//!   ENV and VIB data, LEG+, the NOISE+ mask of the early songs; no code of that repository
//!   (its assembler `fxmasm.lua`) was read;
//! - the byte value of each directive, found by laying the song sources of that repository
//!   (`songs/*.fxmsrc`, text) beside the binary data of the same songs in the AMAD files;
//! - the AMAD song header (load address, noise mask, length and fade) from the 33 AMAD files
//!   of Patrik Rak's DeliAYsongs (Aminet mods/misc) and their EMUL rips' lengths;
//! - black-box observation of the original Z80 play routines: the EMUL rips of the same songs
//!   (modland "AY Emul/Frantisek Fuka") run on this project's Z80 (`ay_z80.rs`), the AY
//!   registers compared after each interrupt (the timing of notes, ENV and VIB, the mixer bits
//!   of TYPE). The period of each note is an equal-tempered formula fitted to the periods those
//!   routines write (it gives the same 84 values). No player code was read: neither the Z80
//!   routines nor the 68000 AYPlayer "ZXAYAMAD" of PlayAY.
//!
//! The song data structure (big-endian words, after the container's song entry):
//! +0 load address of the data, +2 the AND mask of NOISE+ (15 or 31), +3 a byte that is 1 or 2
//! (meaning not known, not used), +4 length in 1/50 s, +6 signed fade offset (the fade starts
//! at length + offset), +8 fade length, +10 four Amiga channel numbers (not used: the AY's
//! voices are fixed), +14 the data, loaded at the load address: three little-endian voice
//! pointers, then the voices' byte code.
//!
//! Bounds: a voice runs at most `MAX_OPS` directives per interrupt (a loop without a note ends
//! the interrupt for that voice), its call/loop stack holds `STACK` entries (a deeper push
//! drops the oldest), all addresses wrap in the 64 KB of the data.

/// Directives one voice may execute in one interrupt before it gives up until the next.
const MAX_OPS: u32 = 256;
/// Entries of a voice's call and loop stack.
const STACK: usize = 32;

/// The song data structure of an AMAD song.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub load: u16,
    pub noise_mask: u8,
    pub speed: u8,
    pub length: u16,
    pub fade_offset: i16,
    pub fade_len: u16,
}

impl Header {
    pub const LEN: usize = 14;

    pub fn parse(d: &[u8], o: usize) -> Option<Header> {
        let h = d.get(o..o + Header::LEN)?;
        let w = |i: usize| u16::from_be_bytes([h[i], h[i + 1]]);
        Some(Header {
            load: w(0),
            noise_mask: h[2],
            speed: h[3],
            length: w(4),
            fade_offset: w(6) as i16,
            fade_len: w(8),
        })
    }

    /// The frame where the fade starts and its length, from the length and the fade offset.
    pub fn fade(&self) -> (u16, u16) {
        let start = (self.length as i32 + self.fade_offset as i32).clamp(0, self.length as i32);
        (start as u16, self.fade_len)
    }
}

/// The period of note `n` (1..), an equal-tempered scale fitted to the play routine's writes.
pub fn period(n: i32) -> u16 {
    let x = 4030.58 * 2f64.powf(-(n - 1) as f64 / 12.0);
    (x + 0.48).floor().clamp(1.0, 4095.0) as u16
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Call(u16),
    Loop(u16, u8),
}

#[derive(Clone, Debug)]
struct Voice {
    pc: u16,
    stack: Vec<Slot>,
    /// Frames left of the current note.
    left: u16,
    tr: i8,
    /// TYPE: bit 0 tone off, bit 3 noise off (the mixer bits of channel A).
    kind: u8,
    env: u16,
    vib: u16,
    leg: bool,
    /// A note has started since LEG+, so the next ones keep the envelope going.
    leg_held: bool,
    // the note
    silent: bool,
    note: i32,
    per: i32,
    tone_mode: bool,
    vib_pc: u16,
    env_pc: u16,
    env_left: u16,
    vol: u8,
}

impl Voice {
    fn new(pc: u16) -> Voice {
        Voice {
            pc,
            stack: Vec::with_capacity(STACK),
            left: 1,
            tr: 0,
            kind: 8,
            env: 0,
            vib: 0,
            leg: false,
            leg_held: false,
            silent: true,
            note: 0,
            per: 0,
            tone_mode: false,
            vib_pc: 0,
            env_pc: 0,
            env_left: 1,
            vol: 0,
        }
    }

    fn push(&mut self, s: Slot) {
        if self.stack.len() == STACK {
            self.stack.remove(0);
        }
        self.stack.push(s);
    }
}

pub struct Amad {
    mem: Box<[u8; 65536]>,
    v: [Voice; 3],
    noise: u8,
    noise_mask: u8,
    pub header: Header,
}

impl Amad {
    /// The song whose data structure is at `o` in the file.
    pub fn new(d: &[u8], o: usize) -> Option<Amad> {
        let header = Header::parse(d, o)?;
        let data = &d[o + Header::LEN..];
        if data.len() < 6 {
            return None;
        }
        let mut mem = Box::new([0u8; 65536]);
        for (i, &b) in data.iter().take(65536).enumerate() {
            mem[(header.load as usize + i) & 0xFFFF] = b;
        }
        let ptr = |i: usize| u16::from_le_bytes([data[2 * i], data[2 * i + 1]]);
        Some(Amad {
            mem,
            v: [Voice::new(ptr(0)), Voice::new(ptr(1)), Voice::new(ptr(2))],
            noise: 0,
            noise_mask: if header.noise_mask == 0 { 31 } else { header.noise_mask & 31 },
            header,
        })
    }

    fn byte(&self, a: u16) -> u8 {
        self.mem[a as usize]
    }

    fn word(&self, a: u16) -> u16 {
        u16::from_le_bytes([self.byte(a), self.byte(a.wrapping_add(1))])
    }

    /// Runs voice `i`'s directives up to its next note and starts that note.
    fn next_note(&mut self, i: usize) {
        for _ in 0..MAX_OPS {
            let pc = self.v[i].pc;
            let op = self.byte(pc);
            let arg = self.byte(pc.wrapping_add(1));
            let addr = self.word(pc.wrapping_add(1));
            if op < 0x80 {
                self.v[i].pc = pc.wrapping_add(2);
                self.start_note(i, op, arg);
                return;
            }
            let v = &mut self.v[i];
            v.pc = pc.wrapping_add(match op {
                0x80 | 0x81 | 0x86 | 0x87 | 0x8C => 3,
                0x83 | 0x89 | 0x8A | 0x8B => 1,
                _ => 2,
            });
            match op {
                0x80 => v.pc = addr,
                0x81 => {
                    let ret = v.pc;
                    v.push(Slot::Call(ret));
                    v.pc = addr;
                }
                0x82 => {
                    let top = v.pc;
                    v.push(Slot::Loop(top, arg));
                }
                0x83 => {
                    if let Some(Slot::Loop(top, n)) = v.stack.last().copied() {
                        if n > 1 {
                            *v.stack.last_mut().unwrap() = Slot::Loop(top, n - 1);
                            v.pc = top;
                        } else {
                            v.stack.pop();
                        }
                    }
                }
                0x84 => self.noise = arg & 31,
                0x85 => v.kind = arg,
                0x86 => v.vib = addr,
                0x87 => v.env = addr,
                0x88 => v.tr = arg as i8,
                0x89 => {
                    // back to the caller (a loop left open in the subroutine is dropped)
                    while let Some(s) = v.stack.pop() {
                        if let Slot::Call(r) = s {
                            v.pc = r;
                            break;
                        }
                    }
                }
                0x8A => {
                    v.leg = true;
                    v.leg_held = false;
                }
                0x8B => v.leg = false,
                0x8C => {} // a call of the game's own Z80 code
                0x8D => self.noise = (self.noise as i8).wrapping_add(arg as i8) as u8 & self.noise_mask,
                0x8E => v.tr = v.tr.wrapping_add(arg as i8),
                _ => {}
            }
        }
        // no note within the bound: a silent frame
        let v = &mut self.v[i];
        v.silent = true;
        v.left = 1;
    }

    fn start_note(&mut self, i: usize, n: u8, len: u8) {
        let v = &mut self.v[i];
        v.left = if len == 0 { 256 } else { len as u16 };
        if n == 0 {
            // a rest: the routine writes period 0 and volume 0
            v.silent = true;
            v.per = 0;
            return;
        }
        v.silent = false;
        v.note = n as i32 + v.tr as i32;
        v.per = period(v.note) as i32;
        v.tone_mode = false;
        v.vib_pc = v.vib;
        if !(v.leg && v.leg_held) {
            v.env_pc = v.env;
            self.env_load(i);
        } else {
            // legato: the envelope goes on as within one note
            self.env_tick(i);
        }
        let v = &mut self.v[i];
        if v.leg {
            v.leg_held = true;
        }
    }

    /// Reads the envelope entry at the envelope pointer (after any JUMP).
    fn env_load(&mut self, i: usize) {
        for _ in 0..8 {
            let pc = self.v[i].env_pc;
            let b = self.byte(pc);
            let l = self.byte(pc.wrapping_add(1));
            let next = self.word(pc.wrapping_add(1));
            let v = &mut self.v[i];
            if b == 0x80 {
                v.env_pc = next;
                continue;
            }
            if (50..=65).contains(&b) {
                v.vol = b - 50;
                v.env_left = 1;
                v.env_pc = pc.wrapping_add(1);
            } else {
                v.vol = b & 15;
                v.env_left = if l == 0 { 256 } else { l as u16 };
                v.env_pc = pc.wrapping_add(2);
            }
            return;
        }
    }

    /// One interrupt of the envelope.
    fn env_tick(&mut self, i: usize) {
        let v = &mut self.v[i];
        v.env_left -= 1;
        if v.env_left == 0 {
            self.env_load(i);
        }
    }

    /// The vibrato of one interrupt after the first of a note.
    fn vib_step(&mut self, i: usize) {
        for _ in 0..8 {
            let pc = self.v[i].vib_pc;
            let b = self.byte(pc);
            let next = self.word(pc.wrapping_add(1));
            let v = &mut self.v[i];
            match b {
                0x80 => {
                    v.vib_pc = next;
                    continue;
                }
                0x82 => v.tone_mode = true,
                0x83 => v.tone_mode = false,
                0x84 => v.kind ^= 9,
                _ => {
                    v.vib_pc = pc.wrapping_add(1);
                    if v.tone_mode {
                        v.note += b as i8 as i32;
                        v.per = period(v.note) as i32;
                    } else {
                        v.per += b as i8 as i32;
                    }
                    return;
                }
            }
            v.vib_pc = pc.wrapping_add(1);
        }
    }

    /// One interrupt: the AY registers 0..=12 it leaves.
    pub fn tick(&mut self) -> [u8; 13] {
        for i in 0..3 {
            self.v[i].left -= 1;
            if self.v[i].left == 0 {
                self.next_note(i);
            } else if !self.v[i].silent {
                self.vib_step(i);
                self.env_tick(i);
            }
        }
        let mut r = [0u8; 13];
        let mut mix = 0u8;
        for (i, v) in self.v.iter().enumerate() {
            let p = (v.per & 0xFFF) as u16;
            r[2 * i] = p as u8;
            r[2 * i + 1] = (p >> 8) as u8;
            r[8 + i] = if v.silent { 0 } else { v.vol };
            // TYPE shifted by the channel: values other than 0, 1, 8, 9 spill over as they do
            // in the original routine (bit 6, the I/O port's direction, included)
            mix |= v.kind << i;
        }
        r[6] = self.noise;
        r[7] = mix;
        r
    }
}
