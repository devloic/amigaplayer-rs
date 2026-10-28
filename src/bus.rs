//! The emulated Amiga address space seen by the 68k: flat RAM, the custom
//! chip registers (only Paula and the beam counter matter to players) and
//! the two CIAs.

use crate::cia::Cia;
use crate::paula::{Paula, LINE};
use m68k::{AddressBus, FastMem};

/// RAM from $000000 up to the CIA/custom-chip area. DMA can reach all of it,
/// so there is no chip/fast distinction.
pub const RAM_SIZE: usize = 0x00B0_0000;

/// Colour clocks per frame: 313 lines (PAL, non-interlaced).
pub const FRAME: u64 = 313 * LINE;
/// 68000 clock cycles per colour clock (7.09 MHz, PAL).
pub(crate) const CPU_CYCLES: u64 = 2;

pub struct Bus {
    pub ram: Vec<u8>,
    pub paula: Paula,
    pub ciaa: Cia,
    pub ciab: Cia,
    /// Time in colour clocks. The owner advances it between guest calls; while
    /// guest code runs, its instructions do (`m68k_rc::Bus::cycles`).
    pub clock: u64,
    /// The clock and the CPU cycle count of the code running at the same moment.
    pub(crate) epoch: (u64, u64),
    /// Colour clocks per frame: `FRAME`, or UADE's 312 lines.
    pub frame: u64,
    pub log_unmapped: bool,
    /// Every custom-chip and CIA write (address, value, colour clock), when enabled: diffing two
    /// runs' logs finds where they part far more precisely than comparing audio.
    pub write_log: Option<Vec<(u32, u16, u64)>>,
    /// The Eagleplayer amplifier, while a player's voices are mixed (it plays
    /// on channels 0 and 1).
    pub amp: Option<Box<crate::amplifier::Amplifier>>,
}

impl Bus {
    pub fn new() -> Self {
        Bus {
            ram: vec![0; RAM_SIZE],
            paula: Paula::new(),
            ciaa: Cia::default(),
            ciab: Cia::default(),
            clock: 0,
            epoch: (0, 0),
            frame: FRAME,
            log_unmapped: false,
            write_log: None,
            amp: None,
        }
    }

    // --- host-side helpers (no side effects, RAM only) ---
    pub fn r8(&self, a: u32) -> u8 {
        self.ram.get((a & 0xFF_FFFF) as usize).copied().unwrap_or(0)
    }
    pub fn r16(&self, a: u32) -> u16 {
        u16::from_be_bytes([self.r8(a), self.r8(a.wrapping_add(1))])
    }
    pub fn r32(&self, a: u32) -> u32 {
        ((self.r16(a) as u32) << 16) | self.r16(a.wrapping_add(2)) as u32
    }
    pub fn w8(&mut self, a: u32, v: u8) {
        if let Some(m) = self.ram.get_mut((a & 0xFF_FFFF) as usize) {
            *m = v;
        }
    }
    pub fn w16(&mut self, a: u32, v: u16) {
        self.w8(a, (v >> 8) as u8);
        self.w8(a.wrapping_add(1), v as u8);
    }
    pub fn w32(&mut self, a: u32, v: u32) {
        self.w16(a, (v >> 16) as u16);
        self.w16(a.wrapping_add(2), v as u16);
    }
    pub fn read_cstr(&self, a: u32) -> String {
        let mut s = Vec::new();
        let mut p = a;
        while s.len() < 4096 {
            let c = self.r8(p);
            if c == 0 {
                break;
            }
            s.push(c);
            p = p.wrapping_add(1);
        }
        s.iter().map(|&c| c as char).collect()
    }
    pub fn write_bytes(&mut self, a: u32, d: &[u8]) {
        let a = (a & 0xFF_FFFF) as usize;
        let end = (a + d.len()).min(self.ram.len());
        if a < end {
            self.ram[a..end].copy_from_slice(&d[..end - a]);
        }
    }
    pub fn write_cstr(&mut self, a: u32, s: &str) {
        self.write_bytes(a, s.as_bytes());
        self.w8(a + s.len() as u32, 0);
    }

    /// The beam's line and horizontal position (colour clocks) now, with the
    /// frame starting at clock 0 (vertical blank every `frame` clocks).
    pub fn beam(&self) -> (u32, u32) {
        let line = (self.clock / LINE) % (self.frame / LINE);
        (line as u32, (self.clock % LINE) as u32)
    }

    fn custom_read(&mut self, off: u32) -> u16 {
        self.paula.sync(self.clock, &self.ram);
        match off & 0x1FE {
            0x002 => self.paula.dmacon,
            0x004 => (self.beam().0 >> 8) as u16 & 1,
            0x006 => {
                let (line, h) = self.beam();
                ((line as u16 & 0xFF) << 8) | h as u16
            }
            0x010 => self.paula.adkcon,
            0x01C => self.paula.intena,
            0x01E => self.paula.intreq,
            _ => 0,
        }
    }

    fn custom_write(&mut self, off: u32, v: u16) {
        if let Some(l) = &mut self.write_log {
            l.push((0xDFF000 | off, v, self.clock));
        }
        self.paula.sync(self.clock, &self.ram);
        match off & 0x1FE {
            0x096 => self.paula.write_dmacon(v),
            0x09A => self.paula.write_intena(v),
            0x09C => self.paula.write_intreq(v),
            0x09E => self.paula.write_adkcon(v),
            o @ 0x0A0..=0x0DF => self.paula.write_audio_reg(o - 0x0A0, v),
            _ => {}
        }
    }

    fn cia_read(&mut self, a: u32) -> u8 {
        let reg = (a >> 8) & 0xF;
        if a & 0x1000 == 0 {
            self.ciaa.read(reg)
        } else if a & 0x2000 == 0 {
            self.ciab.read(reg)
        } else {
            0xFF
        }
    }

    fn cia_write(&mut self, a: u32, v: u8) {
        if let Some(l) = &mut self.write_log {
            l.push((a, v as u16, self.clock));
        }
        let reg = (a >> 8) & 0xF;
        if a & 0x1000 == 0 {
            self.ciaa.write(reg, v);
        }
        if a & 0x2000 == 0 {
            self.ciab.write(reg, v);
        }
    }

    /// A poke for the amplifier's voice `v`, now.
    pub fn amp_poke(&mut self, v: usize, p: crate::amplifier::Poke) {
        if let Some(a) = &mut self.amp {
            a.poke(&mut self.write_log, &mut self.ram, self.clock, v, p);
        }
    }

    /// Code whose CPU has counted `cycles` so far starts running now.
    pub fn rebase(&mut self, cycles: u64) {
        self.epoch = (self.clock, cycles);
    }

    /// Amiga LED/audio filter state (CIA-A PRA bit 1, active low).
    pub fn led(&self) -> bool {
        self.ciaa.pra & 2 == 0
    }
}

impl AddressBus for Bus {
    fn read_byte(&mut self, address: u32) -> u8 {
        let a = address & 0xFF_FFFF;
        if (a as usize) < RAM_SIZE {
            return self.ram[a as usize];
        }
        match a {
            0xBF_0000..=0xBF_FFFF => self.cia_read(a),
            0xDF_F000..=0xDF_FFFF => {
                let w = self.custom_read(a & 0x1FE);
                if a & 1 == 0 { (w >> 8) as u8 } else { w as u8 }
            }
            _ => 0,
        }
    }

    fn read_word(&mut self, address: u32) -> u16 {
        let a = address & 0xFF_FFFF;
        if (a as usize) + 1 < RAM_SIZE {
            return u16::from_be_bytes([self.ram[a as usize], self.ram[a as usize + 1]]);
        }
        match a {
            0xDF_F000..=0xDF_FFFF => self.custom_read(a & 0x1FE),
            _ => ((self.read_byte(a) as u16) << 8) | self.read_byte(a + 1) as u16,
        }
    }

    fn read_long(&mut self, address: u32) -> u32 {
        ((self.read_word(address) as u32) << 16) | self.read_word(address.wrapping_add(2)) as u32
    }

    fn write_byte(&mut self, address: u32, value: u8) {
        let a = address & 0xFF_FFFF;
        if (a as usize) < RAM_SIZE {
            self.ram[a as usize] = value;
            return;
        }
        match a {
            0xBF_0000..=0xBF_FFFF => self.cia_write(a, value),
            0xDF_F000..=0xDF_FFFF => {
                // byte writes to custom registers put the byte on both halves
                self.custom_write(a & 0x1FE, (value as u16) << 8 | value as u16)
            }
            _ => {
                if self.log_unmapped {
                    log::debug!("unmapped byte write ${a:06x}={value:02x}");
                }
            }
        }
    }

    fn write_word(&mut self, address: u32, value: u16) {
        let a = address & 0xFF_FFFF;
        if (a as usize) + 1 < RAM_SIZE {
            self.ram[a as usize] = (value >> 8) as u8;
            self.ram[a as usize + 1] = value as u8;
            return;
        }
        match a {
            0xDF_F000..=0xDF_FFFF => self.custom_write(a & 0x1FE, value),
            _ => {
                self.write_byte(a, (value >> 8) as u8);
                self.write_byte(a + 1, value as u8);
            }
        }
    }

    fn write_long(&mut self, address: u32, value: u32) {
        self.write_word(address, (value >> 16) as u16);
        self.write_word(address.wrapping_add(2), value as u16);
    }

    fn fast_mem(&mut self) -> Option<FastMem> {
        Some(FastMem { ptr: self.ram.as_mut_ptr(), base: 0, len: RAM_SIZE as u32 })
    }
}
