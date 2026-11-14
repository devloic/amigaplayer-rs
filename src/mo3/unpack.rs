//! MO3's own compression: the LZ coding of the music data and the delta (and
//! delta-prediction) coding of samples, as libopenmpt 0.8.7's `Load_mo3.cpp` decodes them
//! (`MO3FileReaderBuffer`, `UnpackMO3DeltaSample`, `UnpackMO3DeltaPredictionSample`), down to
//! what they do on a truncated stream: where libopenmpt's `READ_CTRL_BIT` cannot read a byte
//! it leaves the innermost loop, and so does `Bits::ctrl` returning false here.
//!
//! Ported from libopenmpt 0.8.7 (BSD-3-Clause, OpenMPT Project Developers and Contributors;
//! see LICENSE-openmpt). Its decompression code is based on UNMO3 by Laurent Clévy and was
//! relicensed to the BSD licence with his permission (libopenmpt's note).

/// The control-bit stream shared by the LZ and the delta decoders (`READ_CTRL_BIT`).
pub(crate) struct Bits<'a> {
    src: &'a [u8],
    pub(crate) pos: usize,
    data: u16,
    pub(crate) carry: bool,
}

impl<'a> Bits<'a> {
    pub(crate) fn new(src: &'a [u8], pos: usize) -> Self {
        Bits { src, pos, data: 0, carry: false }
    }

    fn byte(&mut self) -> Option<u8> {
        let b = *self.src.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    /// One control bit into `carry`; false where libopenmpt's macro `break`s (no byte left).
    #[inline]
    pub(crate) fn ctrl(&mut self) -> bool {
        self.data <<= 1;
        self.carry = self.data > 0xFF;
        self.data &= 0xFF;
        if self.data == 0 {
            let Some(b) = self.byte() else { return false };
            self.data = (b as u16) << 1;
            self.data += 1;
            self.carry = self.data > 0xFF;
            self.data &= 0xFF;
        }
        true
    }

    #[inline]
    fn c(&self) -> i32 {
        self.carry as i32
    }

    /// `DECODE_CTRL_BITS`.
    fn decode_len(&mut self, len: &mut i32) {
        *len = len.wrapping_add(1);
        loop {
            if !self.ctrl() {
                break;
            }
            *len = len.wrapping_shl(1).wrapping_add(self.c());
            if !self.ctrl() {
                break;
            }
            if !self.carry {
                break;
            }
        }
    }
}

/// The music data: `size` bytes unpacked from `src` at `pos`. Returns the data (shorter than
/// `size` when the stream is broken or ends early), whether it unpacked completely, and the
/// position in `src` after the packed data.
pub fn unpack_music(src: &[u8], pos: usize, size: u32) -> (Vec<u8>, bool, usize) {
    let size = size as usize;
    let mut out: Vec<u8> = Vec::with_capacity(size.min(src.len().saturating_mul(32)).min(1 << 24));
    let mut r = Bits::new(src, pos);
    if size == 0 {
        return (out, true, r.pos);
    }
    // the first byte is always verbatim (a missing one reads as 0, as FileReader::ReadUint8)
    out.push(r.byte().unwrap_or(0));
    let mut remain = size - 1;
    let mut str_offset: i32 = 0;
    while remain > 0 {
        if !r.ctrl() {
            break;
        }
        if !r.carry {
            // a 0 control bit: one literal byte
            let Some(b) = r.byte() else { break };
            out.push(b);
            remain -= 1;
            continue;
        }
        // a 1 control bit: a copy of earlier bytes
        let mut adjust = 0i32;
        let mut len = 0i32;
        r.decode_len(&mut len);
        len -= 3;
        if len < 0 {
            // the same offset as the previous copy
            len += 1;
        } else {
            let Some(b) = r.byte() else { break };
            str_offset = len.wrapping_shl(8) | b as i32;
            len = 0;
            str_offset = !str_offset;
            if str_offset < -1280 {
                adjust += 1;
            }
            adjust += 1;
            if str_offset < -32000 {
                adjust += 1;
            }
        }
        if str_offset >= 0 || -(out.len() as i64) > str_offset as i64 {
            break;
        }
        if !r.ctrl() {
            break;
        }
        len = len.wrapping_shl(1).wrapping_add(r.c());
        if !r.ctrl() {
            break;
        }
        len = len.wrapping_shl(1).wrapping_add(r.c());
        if len == 0 {
            r.decode_len(&mut len);
            len += 2;
        }
        len = len.wrapping_add(adjust);
        if len <= 0 || remain < len as usize {
            break;
        }
        let from = out.len() - (-(str_offset as i64)) as usize;
        for k in 0..len as usize {
            let b = out[from + k];
            out.push(b);
        }
        remain -= len as usize;
    }
    (out, remain == 0, r.pos)
}

/// `UnpackMO3DeltaSample` for 8-bit samples (interleaved, `length` frames of `channels`).
pub fn delta8(src: &[u8], length: usize, channels: usize) -> Vec<i8> {
    let mut out = vec![0i8; length * channels];
    let mut r = Bits::new(src, 0);
    let mut dh: u8 = 4;
    let mut previous: i8 = 0;
    for chn in 0..channels {
        let mut p = chn;
        while p < length * channels {
            let mut val: u8 = 0;
            decode8(&mut r, &mut val);
            let mut cl = dh;
            while cl > 0 {
                if !r.ctrl() {
                    break;
                }
                val = (val << 1).wrapping_add(r.carry as u8);
                cl -= 1;
            }
            cl = 1;
            if val >= 4 {
                cl = 7;
                while (1u32 << cl) & val as u32 == 0 && cl > 1 {
                    cl -= 1;
                }
            }
            dh = (dh + cl) >> 1;
            let positive = val & 1 != 0;
            val >>= 1;
            if !positive {
                val = !val;
            }
            val = val.wrapping_add(previous as u8);
            out[p] = val as i8;
            p += channels;
            previous = val as i8;
        }
    }
    out
}

/// `UnpackMO3DeltaSample` for 16-bit samples.
pub fn delta16(src: &[u8], length: usize, channels: usize) -> Vec<i16> {
    let mut out = vec![0i16; length * channels];
    let mut r = Bits::new(src, 0);
    let mut dh: u8 = 8;
    let mut previous: i16 = 0;
    for chn in 0..channels {
        let mut p = chn;
        while p < length * channels {
            let mut val: u16 = 0;
            decode16(&mut r, dh, &mut val);
            let mut cl = dh;
            while cl > 0 {
                if !r.ctrl() {
                    break;
                }
                val = (val << 1).wrapping_add(r.carry as u16);
                cl -= 1;
            }
            cl = 1;
            if val >= 4 {
                cl = 15;
                while (1u32 << cl) & val as u32 == 0 && cl > 1 {
                    cl -= 1;
                }
            }
            dh = (dh + cl) >> 1;
            let positive = val & 1 != 0;
            val >>= 1;
            if !positive {
                val = !val;
            }
            val = val.wrapping_add(previous as u16);
            out[p] = val as i16;
            p += channels;
            previous = val as i16;
        }
    }
    out
}

/// `UnpackMO3DeltaPredictionSample` for 8-bit samples.
pub fn prediction8(src: &[u8], length: usize, channels: usize) -> Vec<i8> {
    let mut out = vec![0i8; length * channels];
    let mut r = Bits::new(src, 0);
    let mut dh: u8 = 4;
    let mut next: i32 = 0;
    let mut previous: i8 = 0;
    for chn in 0..channels {
        let mut p = chn;
        while p < length * channels {
            let mut val: u8 = 0;
            decode8(&mut r, &mut val);
            let mut cl = dh;
            while cl > 0 {
                if !r.ctrl() {
                    break;
                }
                val = (val << 1).wrapping_add(r.carry as u8);
                cl -= 1;
            }
            cl = 1;
            if val >= 4 {
                cl = 7;
                while (1u32 << cl) & val as u32 == 0 && cl > 1 {
                    cl -= 1;
                }
            }
            dh = (dh + cl) >> 1;
            let positive = val & 1 != 0;
            val >>= 1;
            if !positive {
                val = !val;
            }
            let delta = val as i8;
            val = val.wrapping_add(next as u8);
            out[p] = val as i8;
            p += channels;
            let sval = val as i8;
            next = (sval as i32 * 2 + (delta as i32 >> 1) - previous as i32).clamp(i8::MIN as i32, i8::MAX as i32);
            previous = sval;
        }
    }
    out
}

/// `UnpackMO3DeltaPredictionSample` for 16-bit samples.
pub fn prediction16(src: &[u8], length: usize, channels: usize) -> Vec<i16> {
    let mut out = vec![0i16; length * channels];
    let mut r = Bits::new(src, 0);
    let mut dh: u8 = 8;
    let mut next: i32 = 0;
    let mut previous: i16 = 0;
    for chn in 0..channels {
        let mut p = chn;
        while p < length * channels {
            let mut val: u16 = 0;
            decode16(&mut r, dh, &mut val);
            let mut cl = dh;
            while cl > 0 {
                if !r.ctrl() {
                    break;
                }
                val = (val << 1).wrapping_add(r.carry as u16);
                cl -= 1;
            }
            cl = 1;
            if val >= 4 {
                cl = 15;
                while (1u32 << cl) & val as u32 == 0 && cl > 1 {
                    cl -= 1;
                }
            }
            dh = (dh + cl) >> 1;
            let positive = val & 1 != 0;
            val >>= 1;
            if !positive {
                val = !val;
            }
            let delta = val as i16;
            val = val.wrapping_add(next as u16);
            out[p] = val as i16;
            p += channels;
            let sval = val as i16;
            next = (sval as i32 * 2 + (delta as i32 >> 1) - previous as i32).clamp(i16::MIN as i32, i16::MAX as i32);
            previous = sval;
        }
    }
    out
}

/// `MO3Delta8BitParams::Decode`.
#[inline]
fn decode8(r: &mut Bits, val: &mut u8) {
    loop {
        if !r.ctrl() {
            break;
        }
        *val = (*val << 1).wrapping_add(r.carry as u8);
        if !r.ctrl() {
            break;
        }
        if !r.carry {
            break;
        }
    }
}

/// `MO3Delta16BitParams::Decode`.
#[inline]
fn decode16(r: &mut Bits, dh: u8, val: &mut u16) {
    if dh < 5 {
        loop {
            if !r.ctrl() {
                break;
            }
            *val = (*val << 1).wrapping_add(r.carry as u16);
            if !r.ctrl() {
                break;
            }
            *val = (*val << 1).wrapping_add(r.carry as u16);
            if !r.ctrl() {
                break;
            }
            if !r.carry {
                break;
            }
        }
    } else {
        loop {
            if !r.ctrl() {
                break;
            }
            *val = (*val << 1).wrapping_add(r.carry as u16);
            if !r.ctrl() {
                break;
            }
            if !r.carry {
                break;
            }
        }
    }
}
