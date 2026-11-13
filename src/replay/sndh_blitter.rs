//! The Atari STE's BLiTTER (bit-block transfer processor, $FF8A00) for the SNDH player
//! (`sndh.rs`): some music drivers move sample data with it, or write the YM2149's registers with
//! it (the destination is any address the CPU can write, the sound chip's included).
//!
//! The register map is the public Atari STE hardware documentation's: halftone RAM, 16 words at
//! $FF8A00; source X / Y increments $FF8A20 / $FF8A22; source address $FF8A24; end masks 1, 2, 3
//! $FF8A28 / $FF8A2A / $FF8A2C; destination X / Y increments $FF8A2E / $FF8A30; destination
//! address $FF8A32; X count $FF8A36, Y count $FF8A38; HOP $FF8A3A; logical operation $FF8A3B; line
//! number $FF8A3C (BUSY bit 7, HOG bit 6, SMUDGE bit 5, the halftone line in bits 0-3); skew
//! $FF8A3D (FXSR bit 7, NFSR bit 6, the skew in bits 0-3).
//!
//! The transfer's model is ported from AtariAudio 1.30's `Blitter.cpp` by Arnaud Carre
//! (Copyright (c) 2026 Arnaud Carre, MIT License, see `LICENSE-AtariAudio`), the reference player
//! of this format, so that a tune that uses the blitter plays as there: a transfer starts when the
//! line-number register is written with BUSY set and runs to its end at once (no bus time; HOG
//! makes no difference); a Y count of 0 transfers nothing, an X count of 0 is 65536; per line an
//! extra HOP value with FXSR, then one per word; the HOP value is all ones, the halftone word of
//! the line, the source word, or the source word ANDed with the halftone word, and only for the
//! 12 logical operations that use the source (0 otherwise); the 32-bit buffer of HOP values is
//! shifted right by the skew (not with HOP 1); the source address moves by the X increment after
//! every read but the line's last word's, with NFSR the last word is not read (the previous one is
//! used again and the address steps back by the X increment); the first word of a line is masked
//! by end mask 1, the last by end mask 3, the others by end mask 2; the destination moves by the X
//! increment but after the last word; then both addresses take the Y increments, and the halftone
//! line counts up, or down with a negative destination Y increment. At the end the address
//! registers hold where the transfer stopped, the Y count is 0 and BUSY is cleared (the line
//! number is left as written). SMUDGE, which the reference does not do, is added from the
//! documentation: the halftone word is chosen by the low four bits of the skewed source word.
//!
//! Where the documented hardware may differ (not checked on one): the hardware ANDs the halftone
//! after the skew, and a negative source X increment loads the buffer the other way; here, as in
//! the reference, the halftone is ANDed before it and the buffer is always loaded from below.

use m68k::AddressBus;

#[derive(Clone)]
pub struct Blitter {
    regs: [u8; 0x40],
}

impl Default for Blitter {
    fn default() -> Self {
        Blitter { regs: [0; 0x40] }
    }
}

/// The last register byte.
pub const LAST: usize = 0x3D;

fn add(a: u32, d: i32) -> u32 {
    (a as i32).wrapping_add(d) as u32 & 0xFF_FFFE
}

/// One transfer's constant parameters and moving state.
struct Run {
    sxi: i32,
    nfsr: bool,
    hop: u8,
    uses_src: bool,
    smudge: bool,
    skew: u32,
    halftone: [u16; 16],
    line: usize,
    src: u32,
    /// The buffer of HOP values, and (for SMUDGE) of the source words read.
    buf: u32,
    raw: u32,
}

impl Run {
    /// The next source word; `left` counts the line's words still to write, this one included.
    fn source<B: AddressBus>(&mut self, left: u32, bus: &mut B) -> u16 {
        let w = if self.nfsr && left == 1 {
            // (the last HOP value, the halftone already applied with HOP 3)
            self.src = add(self.src, -self.sxi);
            self.raw = self.raw << 16 | (self.raw & 0xFFFF);
            return self.buf as u16;
        } else {
            let w = bus.read_word(self.src);
            if left > 1 {
                self.src = add(self.src, self.sxi);
            }
            w
        };
        self.raw = self.raw << 16 | w as u32;
        w
    }

    /// One HOP value into the buffer.
    fn fetch<B: AddressBus>(&mut self, left: u32, bus: &mut B) {
        let v = if !self.uses_src {
            0
        } else {
            match self.hop & 3 {
                0 => 0xFFFF,
                1 => self.halftone[self.line],
                2 => self.source(left, bus),
                _ => {
                    let w = self.source(left, bus);
                    let i = if self.smudge { ((self.raw >> self.skew) & 15) as usize } else { self.line };
                    w & self.halftone[i]
                }
            }
        };
        self.buf = self.buf << 16 | v as u32;
    }
}

impl Blitter {
    fn r16(&self, o: usize) -> u16 {
        (self.regs[o] as u16) << 8 | self.regs[o + 1] as u16
    }

    fn w16(&mut self, o: usize, v: u16) {
        self.regs[o] = (v >> 8) as u8;
        self.regs[o + 1] = v as u8;
    }

    pub fn read8(&self, o: usize) -> u8 {
        if o <= LAST { self.regs[o] } else { 0xFF }
    }

    pub fn read16(&self, o: usize) -> u16 {
        let o = o & !1;
        if o < LAST { self.r16(o) } else { 0xFFFF }
    }

    /// A byte write; true when it starts a transfer (the line-number register with BUSY set).
    pub fn write8(&mut self, o: usize, v: u8) -> bool {
        if o > LAST {
            return false;
        }
        self.regs[o] = v;
        o == 0x3C && v & 0x80 != 0
    }

    /// A word write; true when it starts a transfer.
    pub fn write16(&mut self, o: usize, v: u16) -> bool {
        let o = o & !1;
        if o >= LAST {
            return false;
        }
        self.w16(o, v);
        o == 0x3C && v & 0x8000 != 0
    }

    /// Runs the transfer the registers describe, through `bus` (the CPU's view of memory and I/O),
    /// to its end.
    pub fn run<B: AddressBus>(&mut self, bus: &mut B) {
        let mut lines = self.r16(0x38) as u32;
        if self.regs[0x3C] & 0x80 == 0 || lines == 0 {
            self.regs[0x3C] &= 0x7F;
            return;
        }
        let xcount = match self.r16(0x36) {
            0 => 65536u32,
            n => n as u32,
        };
        let inc = |o: usize| (self.r16(o) & !1) as i16 as i32;
        let (syi, dxi, dyi) = (inc(0x22), inc(0x2E), inc(0x30));
        let fxsr = self.regs[0x3D] & 0x80 != 0;
        let hop_reg = self.regs[0x3A];
        let op = self.regs[0x3B] & 15;
        let masks = [self.r16(0x28), self.r16(0x2A), self.r16(0x2C)];
        let mut st = Run {
            sxi: inc(0x20),
            nfsr: self.regs[0x3D] & 0x40 != 0,
            hop: hop_reg,
            uses_src: !matches!(op, 0 | 5 | 10 | 15),
            smudge: self.regs[0x3C] & 0x20 != 0,
            skew: (self.regs[0x3D] & 15) as u32,
            halftone: std::array::from_fn(|i| self.r16(2 * i)),
            line: (self.regs[0x3C] & 15) as usize,
            src: ((self.r16(0x24) as u32) << 16 | self.r16(0x26) as u32) & 0xFF_FFFE,
            buf: 0,
            raw: 0,
        };
        let mut dst = ((self.r16(0x32) as u32) << 16 | self.r16(0x34) as u32) & 0xFF_FFFE;
        while lines > 0 {
            let mut left = xcount;
            if fxsr {
                st.fetch(left, bus);
            }
            let mut mask = 0;
            while left >= 1 {
                st.fetch(left, bus);
                let h = if hop_reg != 1 { (st.buf >> st.skew) as u16 } else { st.buf as u16 };
                let d = bus.read_word(dst);
                let r = match op {
                    0 => 0,
                    1 => h & d,
                    2 => h & !d,
                    3 => h,
                    4 => !h & d,
                    5 => d,
                    6 => h ^ d,
                    7 => h | d,
                    8 => !h & !d,
                    9 => !h ^ d,
                    10 => !d,
                    11 => h | !d,
                    12 => !h,
                    13 => !h | d,
                    14 => !h | !d,
                    _ => 0xFFFF,
                };
                bus.write_word(dst, (r & masks[mask]) | (d & !masks[mask]));
                if left > 1 {
                    dst = add(dst, dxi);
                }
                left -= 1;
                mask = if left == 1 { 2 } else { 1 };
            }
            st.src = add(st.src, syi);
            dst = add(dst, dyi);
            st.line = if dyi >= 0 { (st.line + 1) & 15 } else { (st.line + 15) & 15 };
            lines -= 1;
        }
        self.w16(0x24, (st.src >> 16) as u16);
        self.w16(0x26, st.src as u16);
        self.w16(0x32, (dst >> 16) as u16);
        self.w16(0x34, dst as u16);
        self.w16(0x38, 0);
        self.regs[0x3C] &= 0x7F;
    }
}
