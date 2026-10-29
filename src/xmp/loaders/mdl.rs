//! Digitrakker modules (`loaders/mdl_load.c`), with the sample unpackers.
//!
//! Note: envelope switching (effect 9) and sample status change (effect 8)
//! are not supported.
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::iff::{self, IffLoader, IFF_LITTLE_ENDIAN};
use crate::xmp::load::*;
use crate::xmp::module::*;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 4 && &d[..4] == b"DMDL"
}

const MDL_NOTE_FOLLOWS: u8 = 0x04;
const MDL_INSTRUMENT_FOLLOWS: u8 = 0x08;
const MDL_VOLUME_FOLLOWS: u8 = 0x10;
const MDL_EFFECT_FOLLOWS: u8 = 0x20;
const MDL_PARAMETER1_FOLLOWS: u8 = 0x40;
const MDL_PARAMETER2_FOLLOWS: u8 = 0x80;

#[derive(Clone, Copy, Default)]
struct MdlEnvelope {
    num: u8,
    data: [u8; 30],
    sus: u8,
    loop_: u8,
}

#[derive(Default)]
struct Local {
    i_index: Vec<i32>,
    s_index: Vec<i32>,
    v_index: Vec<i32>, // volume envelope
    p_index: Vec<i32>, // pan envelope
    f_index: Vec<i32>, // pitch envelope
    packinfo: Option<Vec<i32>>,
    has_in: bool,
    has_pa: bool,
    has_tr: bool,
    has_ii: bool,
    has_is: bool,
    has_sa: bool,
    v_env: Option<Vec<MdlEnvelope>>,
    p_env: Option<Vec<MdlEnvelope>>,
    f_env: Option<Vec<MdlEnvelope>>,
    v_envnum: usize,
    p_envnum: usize,
    f_envnum: usize,
}

fn cstr(b: &[u8]) -> String {
    b.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect()
}

fn fix_env(i: usize, ei: &mut Envelope, env: &[MdlEnvelope], idx: &[i32], envnum: usize) {
    if idx[i] >= 0 {
        ei.flg = XMP_ENVELOPE_ON;
        ei.npt = 15;

        for e in env.iter().take(envnum) {
            if idx[i] == e.num as i32 {
                if e.sus & 0x10 != 0 {
                    ei.flg |= XMP_ENVELOPE_SUS;
                }
                if e.sus & 0x20 != 0 {
                    ei.flg |= XMP_ENVELOPE_LOOP;
                }
                ei.sus = (e.sus & 0x0f) as i32;
                ei.lps = (e.loop_ & 0x0f) as i32;
                ei.lpe = (e.loop_ & 0xf0) as i32;

                let mut lastx = -1i32;
                let mut k = 0;
                while k < ei.npt as usize {
                    let x = e.data[k * 2] as i32;
                    if x == 0 {
                        break;
                    }
                    ei.data[k * 2] = (lastx + x) as i16;
                    ei.data[k * 2 + 1] = e.data[k * 2 + 1] as i16;
                    lastx = ei.data[k * 2] as i32;
                    k += 1;
                }
                ei.npt = k as i32;
                break;
            }
        }
    }
}

/// Effects 1-6 (note effects) can only be entered in the first effect
/// column, G-L (volume-effects) only in the second column.
fn xlat_fx_common(t: &mut u8, p: &mut u8) {
    match *t {
        0x07 => *t = FX_S3M_BPM, // 7 - Set BPM
        0x08 | 0x09 | 0x0a => {
            // 8 - Set pan, 9 - Set envelope (not supported), A - Not used
            *t = 0;
            *p = 0;
        }
        0x0b..=0x0d => {} // B - Position jump, C - Set volume, D - Pattern break: like protracker
        0x0e => match msn(*p) {
            0x0 | 0x3 | 0x8 => {
                // E0, E3 not used; E8 set sample status -- unsupported
                *t = 0;
                *p = 0;
            }
            0x1 => {
                // Pan slide left
                *t = FX_PANSLIDE;
                *p <<= 4;
            }
            0x2 => {
                // Pan slide right
                *t = FX_PANSLIDE;
                *p &= 0x0f;
            }
            _ => {}
        },
        0x0f => *t = FX_S3M_SPEED,
        _ => {}
    }
}

fn xlat_fx1(t: &mut u8, p: &mut u8) {
    match *t {
        0x00 => *p = 0,           // - - No effect
        0x05 => *t = FX_ARPEGGIO, // 5 - Arpeggio
        0x06 => {
            // 6 - Not used
            *t = 0;
            *p = 0;
        }
        _ => {}
    }
    xlat_fx_common(t, p);
}

fn xlat_fx2(t: &mut u8, p: &mut u8) {
    match *t {
        0x00 => *p = 0,                  // - - No effect
        0x01 => *t = FX_VOLSLIDE_UP,     // G - Volume slide up
        0x02 => *t = FX_VOLSLIDE_DN,     // H - Volume slide down
        0x03 => *t = FX_MULTI_RETRIG,    // I - Multi-retrig
        0x04 => *t = FX_TREMOLO,         // J - Tremolo
        0x05 => *t = FX_TREMOR,          // K - Tremor
        0x06 => {
            // L - Not used
            *t = 0;
            *p = 0;
        }
        _ => {}
    }
    xlat_fx_common(t, p);
}

struct Bits {
    b: u32,
    n: u32,
}

/// The bit reader of the sample unpackers: `buf` is the packed data followed by
/// four zero bytes, `pos` the read pointer and `len` the bytes left.
struct Unpack<'a> {
    buf: &'a [u8],
    pos: usize,
    len: i32,
    bits: Bits,
}

impl Unpack<'_> {
    fn readmem32l(&self, p: usize) -> u32 {
        let g = |i: usize| self.buf.get(p + i).copied().unwrap_or(0) as u32;
        g(0) | g(1) << 8 | g(2) << 16 | g(3) << 24
    }

    fn get_bits(&mut self, i: u32) -> u32 {
        if i == 0 {
            self.bits.b = self.readmem32l(self.pos);
            self.pos += 4;
            self.len -= 4;
            self.bits.n = 32;
        }

        let x = self.bits.b & ((1u32 << i) - 1); // get i bits
        self.bits.b >>= i;
        self.bits.n = self.bits.n.wrapping_sub(i);
        if self.bits.n <= 24 {
            if self.len <= 0 {
                // FIXME: last few bits can't be consumed
                return x;
            }
            self.bits.b |= self.readmem32l(self.pos).wrapping_shl(self.bits.n);
            self.pos += 1;
            self.bits.n += 8;
            self.len -= 1;
        }
        x
    }

    /// The `b` of one packed byte (see the Digitrakker docs below).
    fn huff_byte(&mut self) -> Result<u8, ()> {
        let s = self.get_bits(1);
        let mut b: u8;
        if self.get_bits(1) != 0 {
            b = self.get_bits(3) as u8;
        } else {
            b = 8;
            while self.len >= 0 && self.get_bits(1) == 0 {
                // Sanity check
                if b >= 240 {
                    return Err(());
                }
                b = b.wrapping_add(16);
            }
            b = b.wrapping_add(self.get_bits(4) as u8);
        }
        if s != 0 {
            b ^= 0xff;
        }
        Ok(b)
    }
}

/* From the Digitrakker docs:
 *
 * The description of the sample-packmethode (1) [8bit packing]:...
 * ----------------------------------------------------------------
 *
 * The method is based on the Huffman algorithm. It's easy and very fast
 * and effective on samples. The packed sample is a bit stream:
 *
 *	     Byte 0    Byte 1    Byte 2    Byte 3
 *	Bit 76543210  fedcba98  nmlkjihg  ....rqpo
 *
 * A packed byte is stored in the following form:
 *
 *	xxxx10..0s => byte = <xxxx> + (number of <0> bits between
 *		s and 1) * 16 - 8;
 *	if s==1 then byte = byte xor 255
 *
 * If there are no <0> bits between the first bit (sign) and the <1> bit,
 * you have the following form:
 *
 *	xxx1s => byte = <xxx>; if s=1 then byte = byte xor 255
 */
fn unpack_sample8(t: &mut [u8], f: &[u8], len: i32, l: usize) -> Result<(), ()> {
    let mut u = Unpack { buf: f, pos: 0, len, bits: Bits { b: 0, n: 0 } };
    u.get_bits(0);

    let mut d = 0u8;
    for i in 0..l {
        // Sanity check
        if u.len < 0 {
            return Err(());
        }
        let b = u.huff_byte()?;
        d = d.wrapping_add(b);
        t[i] = d;
    }
    Ok(())
}

/*
 * The description of the sample-packmethode (2) [16bit packing]:...
 * ----------------------------------------------------------------
 *
 * It works as methode (1) but it only crunches every 2nd byte (the high-
 * bytes of 16 bit samples). So when you depack 16 bit samples, you have to
 * read 8 bits from the data-stream first. They present the lowbyte of the
 * sample-word. Then depack the highbyte in the descripted way (methode [1]).
 * Only the highbytes are delta-values. So take the lowbytes as they are.
 * Go on this way for the whole sample!
 */
fn unpack_sample16(t: &mut [u8], f: &[u8], len: i32, l: usize) -> Result<(), ()> {
    let mut u = Unpack { buf: f, pos: 0, len, bits: Bits { b: 0, n: 0 } };
    u.get_bits(0);

    let mut d = 0u8;
    for i in 0..l {
        // Sanity check
        if u.len < 0 {
            return Err(());
        }
        let lo = u.get_bits(8);
        let b = u.huff_byte()?;
        d = d.wrapping_add(b);
        t[i * 2] = lo as u8;
        t[i * 2 + 1] = d;
    }
    Ok(())
}

/*
 * IFF chunk handlers
 */

fn get_chunk_in(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    // Sanity check
    if data.has_in {
        return Err(());
    }
    data.has_in = true;

    let mut name = [0u8; 32];
    f.read(&mut name);
    m.module.name = cstr(&name);
    f.seek_cur(20);

    m.module.len = f.read16l() as usize;
    m.module.rst = f.read16l() as usize;
    f.read8(); // gvol
    m.module.spd = f.read8() as i32;
    m.module.bpm = f.read8() as i32;

    // Sanity check
    if m.module.len > 256 || m.module.rst > 255 {
        return Err(());
    }

    let mut i = 0;
    while i < 32 {
        let chinfo = f.read8();
        if chinfo & 0x80 != 0 {
            break;
        }
        m.module.xxc[i].pan = (chinfo as i32) << 1;
        i += 1;
    }
    m.module.chn = i;
    f.seek_cur(32 - i as i64 - 1);

    let len = m.module.len;
    if f.read(&mut m.module.xxo[..len]) != len {
        return Err(());
    }
    Ok(())
}

fn get_chunk_pa(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    // Sanity check
    if data.has_pa || !data.has_in {
        return Err(());
    }
    data.has_pa = true;

    m.module.pat = f.read8() as usize;
    m.module.xxp = Vec::with_capacity(m.module.pat);

    for _ in 0..m.module.pat {
        let mut p = Pattern { rows: 0, index: vec![0; m.module.chn] };
        let chn = f.read8() as usize;
        p.rows = f.read8() as usize + 1;

        f.skip(16); // Skip pattern name
        for j in 0..chn {
            let x = f.read16l() as usize;
            if j < m.module.chn {
                p.index[j] = x;
            }
        }
        m.module.xxp.push(p);
    }
    Ok(())
}

fn get_chunk_p0(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    // Sanity check
    if data.has_pa || !data.has_in {
        return Err(());
    }
    data.has_pa = true;

    m.module.pat = f.read8() as usize;
    m.module.xxp = Vec::with_capacity(m.module.pat);

    for _ in 0..m.module.pat {
        let mut p = Pattern { rows: 64, index: vec![0; m.module.chn] };
        for j in 0..32 {
            let x = f.read16l() as usize;
            if j < m.module.chn {
                p.index[j] = x;
            }
        }
        m.module.xxp.push(p);
    }
    Ok(())
}

fn get_chunk_tr(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    // Sanity check
    if data.has_tr || !data.has_pa {
        return Err(());
    }
    data.has_tr = true;

    m.module.trk = f.read16l() as usize + 1;

    // Sanity check
    let mut max_trk = 0;
    for p in &m.module.xxp[..m.module.pat] {
        for j in 0..m.module.chn {
            if max_trk < p.index[j] {
                max_trk = p.index[j];
            }
        }
    }
    if max_trk >= m.module.trk {
        return Err(());
    }

    m.module.xxt = vec![Track::default(); m.module.trk];

    // Empty track 0 is not stored in the file
    m.module.xxt[0] = Track::new(256);

    for i in 1..m.module.trk {
        // Length of the track in bytes
        let mut len = f.read16l() as i32;

        let mut track = [Event::default(); 256];

        let mut row = 0usize;
        while len != 0 {
            // Sanity check
            if row > 255 {
                return Err(());
            }

            let j = f.read8() as usize;

            len -= 1;
            match j & 0x03 {
                0 => row += j >> 2,
                1 => {
                    // Sanity check
                    if row < 1 || row + (j >> 2) > 255 {
                        return Err(());
                    }
                    for k in 0..=(j >> 2) {
                        track[row + k] = track[row - 1];
                    }
                    row += j >> 2;
                }
                2 => {
                    // Sanity check
                    if (j >> 2) == row {
                        return Err(());
                    }
                    track[row] = track[j >> 2];
                }
                _ => {
                    let jj = j as u8;
                    let ev = &mut track[row];
                    if jj & MDL_NOTE_FOLLOWS != 0 {
                        let b = f.read8();
                        len -= 1;
                        ev.note = if b == 0xff { XMP_KEY_OFF } else { b.wrapping_add(12) };
                    }
                    if jj & MDL_INSTRUMENT_FOLLOWS != 0 {
                        len -= 1;
                        ev.ins = f.read8();
                    }
                    if jj & MDL_VOLUME_FOLLOWS != 0 {
                        len -= 1;
                        ev.vol = f.read8();
                    }
                    if jj & MDL_EFFECT_FOLLOWS != 0 {
                        len -= 1;
                        let k = f.read8();
                        ev.fxt = lsn(k);
                        ev.f2t = msn(k);
                    }
                    if jj & MDL_PARAMETER1_FOLLOWS != 0 {
                        len -= 1;
                        ev.fxp = f.read8();
                    }
                    if jj & MDL_PARAMETER2_FOLLOWS != 0 {
                        len -= 1;
                        ev.f2p = f.read8();
                    }
                }
            }

            row += 1;
        }

        let rows = if row <= 64 {
            64
        } else if row <= 128 {
            128
        } else {
            256
        };

        let mut t = Track::new(rows);
        t.event.copy_from_slice(&track[..rows]);

        // Translate effects
        for ev in t.event.iter_mut() {
            xlat_fx1(&mut ev.fxt, &mut ev.fxp);
            xlat_fx2(&mut ev.f2t, &mut ev.f2p);
        }
        m.module.xxt[i] = t;
    }
    Ok(())
}

fn get_chunk_ii(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    // Sanity check
    if data.has_ii {
        return Err(());
    }
    data.has_ii = true;

    m.module.ins = f.read8() as usize;
    m.module.xxi = vec![Instrument::default(); m.module.ins];

    for i in 0..m.module.ins {
        data.i_index[i] = f.read8() as i32;
        let nsm = f.read8() as i32;
        let mut buf = [0u8; 32];
        if f.read(&mut buf) < 32 {
            return Err(());
        }
        let xxi = &mut m.module.xxi[i];
        xxi.nsm = nsm;
        xxi.name = copy_adjust(&buf, 31);
        xxi.sub = vec![SubInstrument::default(); nsm as usize];

        for j in 0..XMP_MAX_KEYS {
            xxi.map[j].ins = 0xff;
        }

        let mut last_map = 0usize;
        for j in 0..nsm as usize {
            let sub = &mut xxi.sub[j];

            sub.sid = f.read8() as i32;
            let map = f.read8() as usize + 12;
            sub.vol = f.read8() as i32;
            for k in last_map..=map {
                if k < XMP_MAX_KEYS {
                    xxi.map[k].ins = j as u8;
                }
            }
            last_map = map + 1;

            let mut x = f.read8(); // Volume envelope
            if j == 0 {
                data.v_index[i] = if x & 0x80 != 0 { (x & 0x3f) as i32 } else { -1 };
            }
            if x & 0x40 == 0 {
                sub.vol = 0xff;
            }

            sub.pan = (f.read8() as i32) << 1;

            x = f.read8(); // Pan envelope
            if j == 0 {
                data.p_index[i] = if x & 0x80 != 0 { (x & 0x3f) as i32 } else { -1 };
            }
            if x & 0x40 == 0 {
                sub.pan = XMP_INST_NO_DEFAULT_PAN;
            }

            let x = f.read16l();
            if j == 0 {
                xxi.rls = x as i32;
            }

            sub.vra = f.read8() as i32; // vibrato rate
            sub.vde = (f.read8() as i32) << 1; // vibrato depth
            sub.vsw = f.read8() as i32; // vibrato sweep
            sub.vwf = f.read8() as i32; // vibrato waveform
            f.read8(); // Reserved

            let x = f.read8(); // Pitch envelope
            if j == 0 {
                data.f_index[i] = if x & 0x80 != 0 { (x & 0x3f) as i32 } else { -1 };
            }
        }
    }
    Ok(())
}

fn get_chunk_is(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    // Sanity check
    if data.has_is {
        return Err(());
    }
    data.has_is = true;

    m.module.smp = f.read8() as usize;
    let smp = m.module.smp;
    m.module.xxs = vec![Sample::default(); smp];
    m.xtra = vec![ExtraSample::default(); smp];
    let mut packinfo = vec![0i32; smp];

    for i in 0..smp {
        data.s_index[i] = f.read8() as i32; // Sample number
        let mut buf = [0u8; 32];
        if f.read(&mut buf) < 32 {
            return Err(());
        }
        let xxs = &mut m.module.xxs[i];
        xxs.name = copy_adjust(&buf, 31);

        f.skip(8); // Sample filename

        let c5spd = f.read32l() as i32;

        xxs.len = f.read32l() as i32;
        xxs.lps = f.read32l() as i32;
        xxs.lpe = f.read32l() as i32;

        // Sanity check
        if xxs.len < 0 || xxs.lps < 0 || xxs.lps > xxs.len || xxs.lpe > (xxs.len - xxs.lps) {
            return Err(());
        }

        xxs.flg = if xxs.lpe > 0 { XMP_SAMPLE_LOOP } else { 0 };
        xxs.lpe += xxs.lps;

        m.xtra[i].c5spd = c5spd as f64;

        f.read8(); // Volume in DMDL 0.0
        let x = f.read8();
        if x & 0x01 != 0 {
            xxs.flg |= XMP_SAMPLE_16BIT;
            xxs.len >>= 1;
            xxs.lps >>= 1;
            xxs.lpe >>= 1;
        }
        if x & 0x02 != 0 {
            xxs.flg |= XMP_SAMPLE_LOOP_BIDIR;
        }
        packinfo[i] = ((x & 0x0c) >> 2) as i32;
    }
    data.packinfo = Some(packinfo);
    Ok(())
}

fn get_chunk_i0(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    // Sanity check
    if data.has_ii || data.has_is {
        return Err(());
    }
    data.has_ii = true;
    data.has_is = true;

    m.module.ins = f.read8() as usize;
    m.module.smp = m.module.ins;

    m.init_instrument();

    let n = m.module.ins;
    let mut packinfo = vec![0i32; n];

    for i in 0..n {
        let xxi = &mut m.module.xxi[i];
        let xxs = &mut m.module.xxs[i];

        xxi.nsm = 1;
        xxi.sub = vec![SubInstrument::default()];
        let sub = &mut xxi.sub[0];

        let sid = f.read8() as i32;
        sub.sid = sid;
        data.i_index[i] = sid;
        data.s_index[i] = sid;

        let mut buf = [0u8; 32];
        if f.read(&mut buf) < 32 {
            return Err(());
        }
        f.skip(8); // Sample filename
        xxi.name = copy_adjust(&buf, 31);

        let c5spd = f.read16l() as i32;

        xxs.len = f.read32l() as i32;
        xxs.lps = f.read32l() as i32;
        xxs.lpe = f.read32l() as i32;

        // Sanity check
        if xxs.len < 0 || xxs.lps < 0 || xxs.lps > xxs.len || xxs.lpe > (xxs.len - xxs.lps) {
            return Err(());
        }

        xxs.flg = if xxs.lpe > 0 { XMP_SAMPLE_LOOP } else { 0 };
        xxs.lpe += xxs.lps;

        sub.vol = f.read8() as i32; // Volume
        sub.pan = XMP_INST_NO_DEFAULT_PAN;

        m.xtra[i].c5spd = c5spd as f64;

        let x = f.read8();
        if x & 0x01 != 0 {
            xxs.flg |= XMP_SAMPLE_16BIT;
            xxs.len >>= 1;
            xxs.lps >>= 1;
            xxs.lpe >>= 1;
        }
        if x & 0x02 != 0 {
            xxs.flg |= XMP_SAMPLE_LOOP_BIDIR;
        }
        packinfo[i] = ((x & 0x0c) >> 2) as i32;
    }
    data.packinfo = Some(packinfo);
    Ok(())
}

fn get_chunk_sa(_id: &[u8], size: u32, m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    // Sanity check
    if data.has_sa || !data.has_is || data.packinfo.is_none() {
        return Err(());
    }
    data.has_sa = true;

    let mut left = f.size() as i64 - f.tell() as i64;
    if (size as i64) < left {
        left = size as i64;
    }

    let packinfo = data.packinfo.clone().unwrap();
    for i in 0..m.module.smp {
        let xxs = &m.module.xxs[i];
        let (xlen, x16) = (xxs.len, xxs.flg & XMP_SAMPLE_16BIT != 0);

        let mut len = xlen;
        if x16 {
            len <<= 1;
        }

        // Bound the packed sample data size before trying to allocate RAM for it...
        let size_bound = match packinfo[i] {
            0 => len,
            // See unpack_sample8: each byte packs to 5 bits minimum.
            1 => (len >> 3) * 5,
            // See unpack_sample16: each upper byte packs to 5 bits minimum, lower bytes are not packed.
            2 => (len >> 4) * 13,
            _ => return Err(()),
        };

        // Sanity check
        if left < size_bound as i64 {
            return Err(());
        }

        let mut smpbuf = vec![0u8; len as usize];

        match packinfo[i] {
            0 => {
                if f.read(&mut smpbuf) < len as usize {
                    return Err(());
                }
                left -= len as i64;
            }
            p => {
                let clen = f.read32l() as i32;
                // Sanity check
                if p == 1 {
                    if x16 {
                        return Err(());
                    }
                    if clen <= 0 || clen > 0x80000 {
                        // Max compressed sample size
                        return Err(());
                    }
                } else {
                    if !x16 {
                        return Err(());
                    }
                    if clen <= 0 || clen > MAX_SAMPLE_SIZE {
                        return Err(());
                    }
                }
                // The unpack function may read slightly beyond the end.
                let mut buf = vec![0u8; clen as usize + 4];
                if f.read(&mut buf[..clen as usize]) != clen as usize {
                    return Err(());
                }
                if p == 1 {
                    unpack_sample8(&mut smpbuf, &buf, clen, xlen as usize)?;
                } else {
                    unpack_sample16(&mut smpbuf, &buf, clen, xlen as usize)?;
                }
                left -= clen as i64 + 4;
            }
        }

        load_sample(m, f, SAMPLE_FLAG_NOLOAD, i, Some(&smpbuf))?;
    }
    Ok(())
}

fn read_envelopes(f: &mut Hio) -> (usize, Option<Vec<MdlEnvelope>>) {
    let n = f.read8() as usize;
    if n == 0 {
        return (0, None);
    }
    let mut v = vec![MdlEnvelope::default(); n];
    for e in v.iter_mut() {
        e.num = f.read8();
        f.read(&mut e.data);
        e.sus = f.read8();
        e.loop_ = f.read8();
    }
    (n, Some(v))
}

fn get_chunk_ve(_id: &[u8], _size: u32, _m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    // Sanity check
    if data.v_env.is_some() {
        return Err(());
    }
    (data.v_envnum, data.v_env) = read_envelopes(f);
    Ok(())
}

fn get_chunk_pe(_id: &[u8], _size: u32, _m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    // Sanity check
    if data.p_env.is_some() {
        return Err(());
    }
    (data.p_envnum, data.p_env) = read_envelopes(f);
    Ok(())
}

fn get_chunk_fe(_id: &[u8], _size: u32, _m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    // Sanity check
    if data.f_env.is_some() {
        return Err(());
    }
    (data.f_envnum, data.f_env) = read_envelopes(f);
    Ok(())
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let mut data = Local::default();

    // Check magic and get version
    f.read32b();
    let mut buf = [0u8; 1];
    if f.read(&mut buf) < 1 {
        return Err(());
    }
    let version = buf[0];

    // IFFoid chunk IDs
    let mut chunks: Vec<(&[u8; 4], IffLoader<Local>)> = vec![
        (b"IN\0\0", get_chunk_in), // Module info
        (b"TR\0\0", get_chunk_tr), // Tracks
        (b"SA\0\0", get_chunk_sa), // Sampled data
        (b"VE\0\0", get_chunk_ve), // Volume envelopes
        (b"PE\0\0", get_chunk_pe), // Pan envelopes
        (b"FE\0\0", get_chunk_fe), // Pitch envelopes
    ];
    if msn(version) != 0 {
        chunks.push((b"II\0\0", get_chunk_ii)); // Instruments
        chunks.push((b"PA\0\0", get_chunk_pa)); // Patterns
        chunks.push((b"IS\0\0", get_chunk_is)); // Sample info
    } else {
        chunks.push((b"PA\0\0", get_chunk_p0)); // Old 0.0 patterns
        chunks.push((b"IS\0\0", get_chunk_i0)); // Old 0.0 Sample info
    }

    m.module.type_ = format!("Digitrakker MDL {}.{}", msn(version), lsn(version));

    m.volbase = 0xff;
    m.c4rate = C4_NTSC_RATE;

    data.s_index = vec![0; 256];
    data.i_index = vec![0; 256];
    data.v_index = vec![-1; 256];
    data.p_index = vec![-1; 256];
    data.f_index = vec![-1; 256];

    // Load IFFoid chunks. MDL uses a IFF-style file format with 16 bit IDs and
    // little endian 32 bit chunk size. There's only one chunk per data type
    // (i.e. one big chunk for all samples).
    let ret = iff::load(m, f, &mut data, &chunks, 2, IFF_LITTLE_ENDIAN);

    if ret.is_ok() {
        // Reindex instruments
        for i in 0..m.module.trk {
            for e in m.module.xxt[i].event.iter_mut() {
                for l in 0..m.module.ins {
                    if e.ins != 0 && e.ins as i32 == data.i_index[l] {
                        e.ins = (l + 1) as u8;
                        break;
                    }
                }
            }
        }

        // Reindex envelopes, etc.
        let (ve, pe, fe) = (
            data.v_env.clone().unwrap_or_default(),
            data.p_env.clone().unwrap_or_default(),
            data.f_env.clone().unwrap_or_default(),
        );
        for i in 0..m.module.ins {
            let xxi = &mut m.module.xxi[i];
            fix_env(i, &mut xxi.aei, &ve, &data.v_index, data.v_envnum);
            fix_env(i, &mut xxi.pei, &pe, &data.p_index, data.p_envnum);
            fix_env(i, &mut xxi.fei, &fe, &data.f_index, data.f_envnum);

            for j in 0..xxi.nsm.max(0) as usize {
                for k in 0..m.module.smp {
                    if xxi.sub[j].sid == data.s_index[k] {
                        xxi.sub[j].sid = k as i32;
                        break;
                    }
                }
            }
        }
    }

    m.quirk |= QUIRK_FINEFX | QUIRK_KEYOFF;
    m.read_event_type = ReadEvent::Ft2;

    ret
}
