// Port of NostalgicPlayer's C# port of libsidplayfp (by Simon White, Antti Lankila and
// Leandro Nini): Source/Ports/LibSidPlayFp/PSidDrv.cs and Reloc65.cs, git e183943.
// Reloc65 is "a part of xa65 - 65xx/65816 cross-assembler and utility suite" (André
// Fachat), "o65 file relocator. Trimmed down for our needs".
// Licence: GNU General Public License version 2 (LICENSE-LibSidPlayFp); the upstream
// files carry only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more information."
//
// Embedded data, both copied from PSidDrv.cs:
// - `PSID_DRIVER` (290 bytes): libsidplayfp's own 6502 driver (its psiddrv.a65,
//   assembled to an o65 object), the code that calls a tune's init and play routines.
// - `POWER_ON` (210 bytes): a run-length list of the values a C64 has in $0000-$03FF
//   after power-on ("created by running the kernel reset routine and storing the useful
//   values"): zero page variables, the vectors at $0300-$0333 and, at $0073, the 24 byte
//   CHRGET routine BASIC copies to RAM. Used for RSID tunes only.
//
// Structure changed for Rust: the relocator works on indexes into a copy of the driver
// and gives up (`None`) where the C# code would read past the buffer.

use super::c64::C64;
use super::tune::{Clock, Compatibility, SPEED_VBI, Tune};

/// The CPU status register's interrupt disable bit (Mos6510.SR_INTERRUPT)
const SR_INTERRUPT: u8 = 2;

static PSID_DRIVER: [u8; 290] = [
    0x01, 0x00, 0x6f, 0x36, 0x35, 0x00, 0x00, 0x00, //
    0x00, 0x10, 0xcf, 0x00, 0x00, 0x04, 0x00, 0x00, //
    0x00, 0x40, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x1b, 0x10, 0xc5, 0x10, 0xce, //
    0x10, 0xce, 0x10, 0x8c, 0x10, 0x00, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x6c, 0x0e, 0x10, 0x6c, 0x0c, 0x10, 0x78, 0xa9, //
    0x00, 0x8d, 0x1a, 0xd0, 0xad, 0x19, 0xd0, 0x8d, //
    0x19, 0xd0, 0xa9, 0x7f, 0x8d, 0x0d, 0xdc, 0x8d, //
    0x0d, 0xdd, 0xad, 0x0d, 0xdc, 0xad, 0x0d, 0xdd, //
    0xa9, 0x0f, 0x8d, 0x18, 0xd4, 0xad, 0x12, 0x10, //
    0xf0, 0x07, 0xa2, 0x25, 0xa0, 0x40, 0x4c, 0x4a, //
    0x10, 0xa2, 0x95, 0xa0, 0x42, 0x8e, 0x04, 0xdc, //
    0x8c, 0x05, 0xdc, 0xa2, 0x9b, 0xa0, 0x37, 0x4d, //
    0x13, 0x10, 0x0d, 0x10, 0x10, 0xf0, 0x04, 0xa2, //
    0x1b, 0xa0, 0x00, 0x8e, 0x11, 0xd0, 0x8c, 0x12, //
    0xd0, 0xad, 0x10, 0x10, 0xf0, 0x0a, 0xad, 0x11, //
    0x10, 0xf0, 0x05, 0xa2, 0xb2, 0x8e, 0x14, 0x03, //
    0xad, 0x0b, 0x10, 0xd0, 0x08, 0xa9, 0x81, 0x8d, //
    0x1a, 0xd0, 0x4c, 0x8c, 0x10, 0xa9, 0x81, 0xa2, //
    0x01, 0x8d, 0x0d, 0xdc, 0x8e, 0x0e, 0xdc, 0xad, //
    0x10, 0x10, 0xd0, 0x02, 0xa9, 0x37, 0x85, 0x01, //
    0xad, 0x14, 0x10, 0x48, 0xad, 0x0a, 0x10, 0x28, //
    0x20, 0x18, 0x10, 0xad, 0x10, 0x10, 0xf0, 0x0a, //
    0xad, 0x11, 0x10, 0xf0, 0x04, 0xa9, 0x37, 0x85, //
    0x01, 0x58, 0x4c, 0xaf, 0x10, 0xa5, 0x01, 0x48, //
    0xad, 0x11, 0x10, 0x85, 0x01, 0xa9, 0x00, 0x20, //
    0x15, 0x10, 0x68, 0x85, 0x01, 0xce, 0x19, 0xd0, //
    0xad, 0x0d, 0xdc, 0x68, 0xa8, 0x68, 0xaa, 0x68, //
    0x40, 0x02, 0x00, 0x00, 0x01, 0x82, 0x02, 0x82, //
    0x02, 0x82, 0x02, 0x82, 0x02, 0x82, 0x0e, 0x82, //
    0x03, 0x82, 0x22, 0x82, 0x09, 0x82, 0x11, 0x82, //
    0x03, 0x82, 0x0f, 0x82, 0x05, 0x82, 0x05, 0x22, //
    0x05, 0x82, 0x0a, 0x82, 0x0d, 0x82, 0x09, 0x82, //
    0x04, 0x82, 0x04, 0x82, 0x03, 0x82, 0x05, 0x82, //
    0x0a, 0x82, 0x06, 0x82, 0x07, 0x82, 0x00, 0x00, //
    0x00, 0x00,
];

#[rustfmt::skip]
static POWER_ON: [u8; 210] = [
    /* addr,   off,  rle, values */
    /*$0003*/ 0x83, 0x04, 0xaa, 0xb1, 0x91, 0xb3, 0x22,
    /*$000b*/ 0x03,       0x4c,
    /*$000f*/ 0x03,       0x04,
    /*$0016*/ 0x86, 0x05, 0x19, 0x16, 0x00, 0x0a, 0x76, 0xa3,
    /*$0022*/ 0x86, 0x03, 0x40, 0xa3, 0xb3, 0xbd,
    /*$002b*/ 0x85, 0x01, 0x01, 0x08,
    /*$0034*/ 0x07,       0xa0,
    /*$0038*/ 0x03,       0xa0,
    /*$003a*/ 0x01,       0xff,
    /*$0042*/ 0x07,       0x08,
    /*$0047*/ 0x04,       0x24,
    /*$0053*/ 0x8b, 0x01, 0x03, 0x4c,
    /*$0061*/ 0x0c,       0x8d,
    /*$0063*/ 0x02,       0x10,
    /*$0069*/ 0x84, 0x02, 0x8c, 0xff, 0xa0,
    /*$0071*/ 0x85, 0x1e, 0x0a, 0xa3, 0xe6, 0x7a, 0xd0, 0x02, 0xe6, 0x7b, 0xad, 0x00, 0x08, 0xc9, 0x3a, 0xb0, 0x0a, 0xc9, 0x20, 0xf0, 0xef, 0x38, 0xe9, 0x30, 0x38, 0xe9, 0xd0, 0x60, 0x80, 0x4f, 0xc7, 0x52, 0x58,
    /*$0091*/ 0x01,       0xff,
    /*$009a*/ 0x08,       0x03,
    /*$00b2*/ 0x97, 0x01, 0x3c, 0x03,
    /*$00c2*/ 0x8e, 0x03, 0xa0, 0x30, 0xfd, 0x01,
    /*$00c8*/ 0x82, 0x82, 0x03,
    /*$00cb*/ 0x80, 0x81, 0x01,
    /*$00ce*/ 0x01,       0x20,
    /*$00d1*/ 0x82, 0x01, 0x18, 0x05,
    /*$00d5*/ 0x82, 0x02, 0x27, 0x07, 0x0d,
    /*$00d9*/ 0x81, 0x86, 0x84,
    /*$00e0*/ 0x80, 0x85, 0x85,
    /*$00e6*/ 0x80, 0x86, 0x86,
    /*$00ed*/ 0x80, 0x85, 0x87,
    /*$00f3*/ 0x80, 0x03, 0x18, 0xd9, 0x81, 0xeb,
    /*$0176*/ 0x7f,       0x00,
    /*$01f6*/ 0x7f,       0x00,
    /*$0276*/ 0x7f,       0x00,
    /*$0282*/ 0x8b, 0x0a, 0x08, 0x00, 0xa0, 0x00, 0x0e, 0x00, 0x04, 0x0a, 0x00, 0x04, 0x10,
    /*$028f*/ 0x82, 0x01, 0x48, 0xeb,
    /*$0300*/ 0xef, 0x0b, 0x8b, 0xe3, 0x83, 0xa4, 0x7c, 0xa5, 0x1a, 0xa7, 0xe4, 0xa7, 0x86, 0xae,
    /*$0310*/ 0x84, 0x02, 0x4c, 0x48, 0xb2,
    /*$0314*/ 0x81, 0x1f, 0x31, 0xea, 0x66, 0xfe, 0x47, 0xfe, 0x4a, 0xf3, 0x91, 0xf2, 0x0e, 0xf2, 0x50, 0xf2, 0x33, 0xf3, 0x57, 0xf1, 0xca, 0xf1, 0xed, 0xf6, 0x3e, 0xf1, 0x2f, 0xf3, 0x66, 0xfe, 0xa5, 0xf4, 0xed, 0xf5,
    /*Total 217*/ // (upstream's count; the list is 210 bytes)
];

// ----------------------------------------------------------------------
// Reloc65
// ----------------------------------------------------------------------

/// 16 bit header
const HEADER_SIZE: usize = 8 + (9 * 2);

struct Reloc65 {
    t_base: i32,
    t_diff: i32,
}

fn get_word(buf: &[u8], idx: usize) -> Option<i32> {
    Some(*buf.get(idx)? as i32 | (*buf.get(idx + 1)? as i32) << 8)
}

fn set_word(buf: &mut [u8], idx: usize, value: i32) -> Option<()> {
    *buf.get_mut(idx)? = (value & 0xff) as u8;
    *buf.get_mut(idx + 1)? = ((value >> 8) & 0xff) as u8;
    Some(())
}

impl Reloc65 {
    /// Do the relocation: the text segment's place in `buf` (offset, length).
    fn reloc(&mut self, buf: &mut [u8]) -> Option<(usize, usize)> {
        if buf.get(..5)? != [1, 0, b'o', b'6', b'5'] {
            return None;
        }

        let mode = get_word(buf, 6)?;

        // 32 bit size not supported + pagewise relocation not support
        if mode & 0x2000 != 0 || mode & 0x4000 != 0 {
            return None;
        }

        let h_len = HEADER_SIZE + Self::read_options(buf, HEADER_SIZE)?;

        let t_base = get_word(buf, 8)?;
        let t_len = get_word(buf, 10)? as usize;
        self.t_diff = self.t_base - t_base;

        let d_len = get_word(buf, 14)? as usize;

        let seg_t = h_len; // Text segment
        let seg_d = seg_t + t_len; // Data segment
        let u_tab = seg_d + d_len; // Undefined references list

        let rt_tab = u_tab + Self::read_undef(buf, u_tab)?; // Text relocation table

        let rd_tab = self.reloc_seg(buf, seg_t, rt_tab)?; // Data relocation
        let ex_tab = self.reloc_seg(buf, seg_d, rd_tab)?; // Exported globals list

        self.reloc_globals(buf, ex_tab)?;

        set_word(buf, 8, self.t_base)?;

        buf.get(seg_t..seg_t + t_len)?;
        Some((seg_t, t_len))
    }

    /// Get the size of header options section
    fn read_options(buf: &[u8], at: usize) -> Option<usize> {
        let mut l = 0usize;

        let mut c = *buf.get(at)? as usize;
        while c != 0 {
            l += c;
            c = *buf.get(at + l)? as usize;
        }

        Some(l + 1)
    }

    /// Get the size of undefined references list
    fn read_undef(buf: &[u8], at: usize) -> Option<usize> {
        let mut l = 2usize;

        let mut n = get_word(buf, at)?;
        while n != 0 {
            n -= 1;
            loop {
                let b = *buf.get(at + l)?;
                l += 1;
                if b != 0 {
                    break;
                }
            }
        }

        Some(l)
    }

    fn rel_diff(&self, s: u8) -> i32 {
        if s == 2 { self.t_diff } else { 0 }
    }

    /// Relocate segment: the index after its relocation table
    fn reloc_seg(&self, buf: &mut [u8], seg: usize, mut r_tab: usize) -> Option<usize> {
        let mut adr: i64 = -1;

        while *buf.get(r_tab)? != 0 {
            if *buf.get(r_tab)? == 255 {
                adr += 254;
                r_tab += 1;
            } else {
                adr += *buf.get(r_tab)? as i64;
                r_tab += 1;
                let typ = *buf.get(r_tab)? & 0xe0;
                let sg = *buf.get(r_tab)? & 0x07;
                r_tab += 1;
                let at = usize::try_from(seg as i64 + adr).ok()?;

                match typ {
                    0x80 => {
                        let old_val = get_word(buf, at)?;
                        let new_val = old_val + self.rel_diff(sg);
                        set_word(buf, at, new_val)?;
                    }
                    0x40 => {
                        let old_val = *buf.get(at)? as i32 + 256 * *buf.get(r_tab)? as i32;
                        let new_val = old_val + self.rel_diff(sg);
                        *buf.get_mut(at)? = ((new_val >> 8) & 255) as u8;
                        *buf.get_mut(r_tab)? = (new_val & 255) as u8;
                        r_tab += 1;
                    }
                    0x20 => {
                        let old_val = *buf.get(at)? as i32;
                        let new_val = old_val + self.rel_diff(sg);
                        *buf.get_mut(at)? = (new_val & 255) as u8;
                    }
                    _ => {}
                }

                if sg == 0 {
                    r_tab += 2;
                }
            }
        }

        Some(r_tab + 1)
    }

    /// Relocate exported globals list
    fn reloc_globals(&self, buf: &mut [u8], mut at: usize) -> Option<usize> {
        let mut n = get_word(buf, at)?;
        at += 2;

        while n != 0 {
            while *buf.get(at)? != 0 {
                at += 1;
            }

            at += 1;
            let seg = *buf.get(at)?;
            let old_val = get_word(buf, at + 1)?;
            let new_val = old_val + self.rel_diff(seg);

            set_word(buf, at + 1, new_val)?;
            at += 3;
            n -= 1;
        }

        Some(at)
    }
}

// ----------------------------------------------------------------------
// PSidDrv: the code here is use to support the PSID version 2NG
// (proposal B) file format for player relocation support
// ----------------------------------------------------------------------

pub struct PsidDrv {
    /// the relocated driver's text segment (its first 10 bytes are initialization data)
    reloc_driver: Vec<u8>,
    pub driver_addr: u16,
    pub driver_length: u16,
}

impl PsidDrv {
    /// Relocate the driver
    pub fn drv_reloc(tune: &Tune) -> Result<PsidDrv, String> {
        let start_lp = (tune.load_addr >> 8) as i32;
        let end_lp = ((tune.load_addr as u32 + (tune.c64_data_len.wrapping_sub(1))) >> 8) as i32;

        let mut reloc_start_page = tune.reloc_start_page;
        let mut reloc_pages = tune.reloc_pages;

        if tune.compatibility == Compatibility::Basic {
            // The psiddrv is only used for initialization and to
            // autorun basic tunes as running the kernel falls
            // into a manual load/run mode
            reloc_start_page = 0x04;
            reloc_pages = 0x03;
        }

        // Check for free space in tune
        if reloc_start_page == 0xff {
            reloc_pages = 0;
        } else if reloc_start_page == 0 {
            // Check if we need to find the reloc addr
            reloc_pages = 0;

            // Find area where to dump the driver in.
            // It's only 1 block long, so any free block we can find
            // between $0400 and $d000 will do
            for i in 4..0xd0 {
                if i >= start_lp && i <= end_lp {
                    continue;
                }

                if (0xa0..=0xbf).contains(&i) {
                    continue;
                }

                reloc_start_page = i as u8;
                reloc_pages = 1;
                break;
            }
        }

        if reloc_pages < 1 {
            return Err("ERROR: No space to install psid driver in C64 ram".into());
        }

        // Place PSID driver into ram
        let reloc_addr = (reloc_start_page as u16) << 8;

        let mut psid_driver = PSID_DRIVER.to_vec();

        let mut relocator = Reloc65 { t_base: reloc_addr as i32 - 10, t_diff: 0 };
        let Some((seg_t, t_len)) = relocator.reloc(&mut psid_driver) else {
            return Err("ERROR: Failed whilst relocating psid driver".into());
        };
        if t_len < 20 {
            return Err("ERROR: Failed whilst relocating psid driver".into());
        }

        // Adjust size to not include initialization data
        let reloc_size = t_len - 10;

        // Round length to end of page
        let driver_length = ((reloc_size as u16).wrapping_add(0xff)) & 0xff00;

        Ok(PsidDrv { reloc_driver: psid_driver[seg_t..seg_t + t_len].to_vec(), driver_addr: reloc_addr, driver_length })
    }

    fn word(&self, at: usize) -> u16 {
        u16::from_le_bytes([self.reloc_driver[at], self.reloc_driver[at + 1]])
    }

    /// Install the driver.
    /// Must be called after the tune has been placed in memory
    pub fn install(&self, mem: &mut C64, tune: &Tune, video: u8) {
        mem.fill_ram_value(0, 0, 0x3ff);

        if tune.compatibility >= Compatibility::R64 {
            copy_poweron_pattern(mem);
        }

        // Set PAL/NTSC switch
        mem.write_mem_byte(0x02a6, video);

        mem.install_reset_hook(self.word(0));

        // If not a basic tune then the psiddrv must install
        // interrupt hooks and trap programs trying to restart basic
        if tune.compatibility == Compatibility::Basic {
            // Install hook to set subtune number for basic
            mem.set_basic_sub_tune((tune.current_song.wrapping_sub(1)) as u8);
            mem.install_basic_trap(0xbf53);
        } else {
            // Only install IRQ handle for RSID tunes
            let n = if tune.compatibility == Compatibility::R64 { 2 } else { 6 };
            mem.fill_ram(0x0314, &self.reloc_driver[2..2 + n]);

            // Experimental restart basic trap
            let addr = self.word(8);
            mem.install_basic_trap(0xffe1);
            mem.write_mem_word(0x0328, addr);
        }

        let mut pos = self.driver_addr;

        // Install driver to ram
        mem.fill_ram(pos, &self.reloc_driver[10..]);

        // Set song number
        mem.write_mem_byte(pos, (tune.current_song.wrapping_sub(1)) as u8);
        pos = pos.wrapping_add(1);

        // Set tunes speed (VIC/CIA)
        mem.write_mem_byte(pos, if tune.song_speed == SPEED_VBI { 0 } else { 1 });
        pos = pos.wrapping_add(1);

        // Set init address
        mem.write_mem_word(pos, if tune.compatibility == Compatibility::Basic { 0xbf55 } else { tune.init_addr });
        pos = pos.wrapping_add(2);

        // Set play address
        mem.write_mem_word(pos, tune.play_addr);
        pos = pos.wrapping_add(2);

        // Set init address IO bank value
        mem.write_mem_byte(pos, io_map(tune, tune.init_addr));
        pos = pos.wrapping_add(1);

        // Set play address IO bank value
        mem.write_mem_byte(pos, io_map(tune, tune.play_addr));
        pos = pos.wrapping_add(1);

        // Set PAL/NTSC flag
        mem.write_mem_byte(pos, video);
        pos = pos.wrapping_add(1);

        // Set the required tune clock speed
        let clock_speed = match tune.clock_speed {
            Clock::Pal => 1,
            Clock::Ntsc => 0,
            // UNKNOWN or ANY
            _ => video,
        };

        mem.write_mem_byte(pos, clock_speed);
        pos = pos.wrapping_add(1);

        // Set default processor register flags on calling init
        mem.write_mem_byte(pos, if tune.compatibility >= Compatibility::R64 { 0 } else { 1 << SR_INTERRUPT });
    }
}

/// Copy in power on settings. These were created by running the
/// kernel reset routine and storing the useful values from
/// $0000-$03ff. Format is:
/// - offset byte (bit 7 indicates presence rle byte)
/// - rle count byte (bit 7 indicates compression used)
/// - data (single byte) or quantity represented by uncompressed count
/// all counts and offsets are 1 less than they should be
fn copy_poweron_pattern(mem: &mut C64) {
    let mut addr: u16 = 0;
    let mut i = 0usize;
    let next = |i: &mut usize| {
        let b = POWER_ON.get(*i).copied().unwrap_or(0);
        *i += 1;
        b
    };

    while i < POWER_ON.len() {
        let mut off = next(&mut i);
        let mut count: u8 = 0;
        let mut compressed = false;

        // Determine data count/compression
        if off & 0x80 != 0 {
            // Fixup offset
            off &= 0x7f;
            count = next(&mut i);

            if count & 0x80 != 0 {
                // Fixup count
                count &= 0x7f;
                compressed = true;
            }
        }

        // Fix count off by ones (see format details)
        let count = count as u16 + 1;
        addr = addr.wrapping_add(off as u16);

        if compressed {
            // Extract compressed data
            let data = next(&mut i);

            for _ in 0..count {
                mem.write_mem_byte(addr, data);
                addr = addr.wrapping_add(1);
            }
        } else {
            // Extract uncompressed data
            for _ in 0..count {
                let data = next(&mut i);
                mem.write_mem_byte(addr, data);
                addr = addr.wrapping_add(1);
            }
        }
    }
}

/// Get required I/O map to reach address
fn io_map(tune: &Tune, addr: u16) -> u8 {
    // Force real C64 compatibility
    if tune.compatibility == Compatibility::R64 || tune.compatibility == Compatibility::Basic || addr == 0 {
        // Special case, set to 0x37 by the psid driver
        return 0;
    }

    // $34 for init/play in $d000 - $dfff
    // $35 for init/play in $e000 - $ffff
    // $36 for load end/play in $a000 - $ffff
    // $37 for the rest
    if addr < 0xa000 {
        return 0x37; // Basic-ROM, Kernal-ROM, I/O
    }

    if addr < 0xd000 {
        return 0x36; // Kernal-ROM, I/O
    }

    if addr >= 0xe000 {
        return 0x35; // I/O only
    }

    0x34 // RAM only
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_relocates_to_every_page() {
        for page in 4u16..0xd0 {
            let mut d = PSID_DRIVER.to_vec();
            let mut r = Reloc65 { t_base: (page << 8) as i32 - 10, t_diff: 0 };
            let (seg, len) = r.reloc(&mut d).expect("relocation");
            assert_eq!(len, 0xcf);
            let t = &d[seg..seg + len];
            // the initialization data: the reset entry and the interrupt vectors point
            // into the driver
            for at in [0, 2, 4, 6, 8] {
                assert_eq!(u16::from_le_bytes([t[at], t[at + 1]]) >> 8, page);
            }
            // the driver proper: 11 bytes of parameters, then JMP (play), JMP (init)
            // through the parameters in the driver's own page
            assert_eq!((t[21], t[24]), (0x6c, 0x6c));
            assert_eq!(u16::from_le_bytes([t[22], t[23]]), (page << 8) + 4);
            assert_eq!(u16::from_le_bytes([t[25], t[26]]), (page << 8) + 2);
        }
    }

    #[test]
    fn relocator_rejects_garbage() {
        let mut r = Reloc65 { t_base: 0x1000, t_diff: 0 };
        assert!(r.reloc(&mut [0u8; 4]).is_none());
        let mut d = PSID_DRIVER.to_vec();
        d.truncate(100);
        assert!(r.reloc(&mut d).is_none());
        // a relocation table that runs off the end
        let mut d = PSID_DRIVER.to_vec();
        let n = d.len();
        d[n - 4..].fill(0x7f);
        assert!(r.reloc(&mut d).is_none());
    }
}
