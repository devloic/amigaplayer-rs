//! XPK-NUKE and XPK-DUKE (NUKE followed by delta decoding). Ported from LibAncient's
//! `NukeDecompressor.cs` / `DukeDecompressor.cs` (NostalgicPlayer, Thomas Neumann, MIT), ports
//! of ancient's `NUKEDecompressor.cpp` (Teemu Suutari, BSD-2-Clause); see LICENSE-ancient.
//! The byte stream (backwards) and the bit streams (forwards) are linked as in the C++ code, so
//! they cannot cross.

use crate::decrunch::ancient::{delta_decode, FwdOut, Linked, Lsb, Msb, Vlc};

pub(super) fn decompress(chunk: &[u8], out: &mut [u8], duke: bool) -> Option<()> {
    let mut lk = Linked::new(chunk);
    let mut bit1 = Msb::default();
    let mut bit2 = Msb::default();
    let mut bit4 = Lsb::default();
    let mut bitx = Msb::default();
    macro_rules! bit {
        () => {
            bit1.bits16(&mut lk.fwd(), 1)?
        };
    }
    macro_rules! bits2 {
        () => {
            bit2.bits16(&mut lk.fwd(), 2)?
        };
    }
    macro_rules! bits4 {
        () => {
            bit4.bits32(&mut lk.fwd(), 4)?
        };
    }
    let n = out.len();
    let mut o = FwdOut::new(out, 0, n)?;
    let vlc = Vlc::new(&[4, 6, 8, 9, -4, 7, 9, 11, 13, 14, -5, 7, 9, 11, 13, 14]);
    loop {
        if bit!() == 0 {
            let count = if bit!() != 0 {
                1
            } else {
                let mut count = 0u32;
                loop {
                    let tmp = bits2!();
                    count = count.wrapping_add(if tmp != 0 { 5 - tmp } else { 3 });
                    if tmp != 0 {
                        break;
                    }
                }
                count
            };
            for _ in 0..count {
                let b = lk.back_byte()?;
                o.write(b)?;
            }
        }
        if o.eof() {
            break;
        }
        let index = bits4!();
        let distance = vlc.decode(index, |c| bitx.bits16(&mut lk.fwd(), c))?;
        let count = if index < 4 {
            2
        } else if index < 10 {
            3
        } else {
            let c = bits2!();
            if c == 0 {
                let mut c = 3 + 3u32;
                loop {
                    let tmp = bits4!();
                    c = c.wrapping_add(if tmp != 0 { 16 - tmp } else { 15 });
                    if tmp != 0 {
                        break;
                    }
                }
                c
            } else {
                3 + 4 - c
            }
        };
        o.copy(distance as usize, count as usize)?;
    }
    if duke {
        delta_decode(out);
    }
    Some(())
}
