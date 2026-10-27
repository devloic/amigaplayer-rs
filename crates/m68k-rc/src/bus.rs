//! Memory bus. The 68000 has a 24-bit address bus: implementations mask every address with
//! `0xFF_FFFF` themselves (generated code passes full 32-bit values, like the CPU's internal adder).

pub const ADDR_MASK: u32 = 0x00FF_FFFF;

pub trait Bus {
    fn read8(&mut self, a: u32) -> u8;
    fn read16(&mut self, a: u32) -> u16;
    fn write8(&mut self, a: u32, v: u8);
    fn write16(&mut self, a: u32, v: u16);
    #[inline(always)]
    fn read32(&mut self, a: u32) -> u32 {
        (self.read16(a) as u32) << 16 | self.read16(a.wrapping_add(2)) as u32
    }
    #[inline(always)]
    fn write32(&mut self, a: u32, v: u32) {
        self.write16(a, (v >> 16) as u16);
        self.write16(a.wrapping_add(2), v as u16);
    }
    /// Instruction-stream fetch (only the interpreter fetches; generated code has its operands baked in).
    #[inline(always)]
    fn fetch16(&mut self, a: u32) -> u16 {
        self.read16(a)
    }
    /// The CPU's cycle count (`Cpu::cycles`) before the instruction about to run, plus that
    /// instruction's: code counting cycles reports it, so that a bus with a clock of its own
    /// sees the time an access happens at.
    #[inline(always)]
    fn cycles(&mut self, _total: u64) {}
}

/// 16 MB of flat RAM covering the whole 24-bit space. No address error on odd word accesses
/// (Musashi's default behaviour, which the tests use as reference).
pub struct FlatMem {
    pub mem: Vec<u8>,
}

impl Default for FlatMem {
    fn default() -> Self {
        FlatMem { mem: vec![0; 1 << 24] }
    }
}

impl FlatMem {
    pub fn load(&mut self, addr: u32, bytes: &[u8]) {
        let a = (addr & ADDR_MASK) as usize;
        self.mem[a..a + bytes.len()].copy_from_slice(bytes);
    }
}

impl Bus for FlatMem {
    #[inline(always)]
    fn read8(&mut self, a: u32) -> u8 {
        self.mem[(a & ADDR_MASK) as usize]
    }
    #[inline(always)]
    fn read16(&mut self, a: u32) -> u16 {
        let a = (a & ADDR_MASK) as usize;
        if a < ADDR_MASK as usize {
            u16::from_be_bytes([self.mem[a], self.mem[a + 1]])
        } else {
            u16::from_be_bytes([self.mem[a], self.mem[0]])
        }
    }
    #[inline(always)]
    fn read32(&mut self, a: u32) -> u32 {
        let m = (a & ADDR_MASK) as usize;
        if m + 3 <= ADDR_MASK as usize {
            u32::from_be_bytes([self.mem[m], self.mem[m + 1], self.mem[m + 2], self.mem[m + 3]])
        } else {
            (self.read16(a) as u32) << 16 | self.read16(a.wrapping_add(2)) as u32
        }
    }
    #[inline(always)]
    fn write8(&mut self, a: u32, v: u8) {
        self.mem[(a & ADDR_MASK) as usize] = v;
    }
    #[inline(always)]
    fn write16(&mut self, a: u32, v: u16) {
        let [h, l] = v.to_be_bytes();
        self.mem[(a & ADDR_MASK) as usize] = h;
        self.mem[(a.wrapping_add(1) & ADDR_MASK) as usize] = l;
    }
    #[inline(always)]
    fn write32(&mut self, a: u32, v: u32) {
        let m = (a & ADDR_MASK) as usize;
        if m + 3 <= ADDR_MASK as usize {
            self.mem[m..m + 4].copy_from_slice(&v.to_be_bytes());
        } else {
            self.write16(a, (v >> 16) as u16);
            self.write16(a.wrapping_add(2), v as u16);
        }
    }
}
