//! What libxmp does around a loader: the file reader (`hio.c` over memory), sample
//! loading (`loaders/sample.c`), the epilogue that checks the module
//! (`load.c`, `load_helpers.c`) and the scan preparation.
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use super::module::*;

pub const SAMPLE_FLAG_DIFF: i32 = 0x0001;
pub const SAMPLE_FLAG_UNS: i32 = 0x0002;
pub const SAMPLE_FLAG_8BDIFF: i32 = 0x0004;
pub const SAMPLE_FLAG_7BIT: i32 = 0x0008;
pub const SAMPLE_FLAG_NOLOAD: i32 = 0x0010;
pub const SAMPLE_FLAG_BIGEND: i32 = 0x0040;
pub const SAMPLE_FLAG_VIDC: i32 = 0x0080;
pub const SAMPLE_FLAG_INTERLEAVED: i32 = 0x0100;
pub const SAMPLE_FLAG_FULLREP: i32 = 0x0200;

/// `HIO_HANDLE` over a module in memory: reads past the end give 0 and set the
/// error flag, as hio's memory reader does.
pub struct Hio<'a> {
    pub data: &'a [u8],
    pub pos: usize,
    pub error: bool,
}

impl<'a> Hio<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Hio { data, pos: 0, error: false }
    }

    pub fn size(&self) -> usize {
        self.data.len()
    }

    pub fn tell(&self) -> usize {
        self.pos
    }

    pub fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub fn skip(&mut self, n: usize) {
        self.pos = self.pos.saturating_add(n);
    }

    pub fn read8(&mut self) -> u8 {
        match self.data.get(self.pos) {
            Some(&b) => {
                self.pos += 1;
                b
            }
            None => {
                self.error = true;
                0
            }
        }
    }

    /// Reads up to `out.len()` bytes; returns how many were read (`hio_read`).
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        let avail = self.data.len().saturating_sub(self.pos);
        let n = out.len().min(avail);
        out[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        if n < out.len() {
            self.error = true;
        }
        n
    }

    pub fn read_vec(&mut self, n: usize) -> Vec<u8> {
        let mut v = vec![0; n];
        self.read(&mut v);
        v
    }

    fn bytes<const N: usize>(&mut self) -> [u8; N] {
        let mut b = [0u8; N];
        if self.data.len().saturating_sub(self.pos) < N {
            self.error = true;
            self.pos = self.data.len();
            return [0; N];
        }
        b.copy_from_slice(&self.data[self.pos..self.pos + N]);
        self.pos += N;
        b
    }

    pub fn read16l(&mut self) -> u16 {
        u16::from_le_bytes(self.bytes())
    }

    pub fn read16b(&mut self) -> u16 {
        u16::from_be_bytes(self.bytes())
    }

    pub fn read32l(&mut self) -> u32 {
        u32::from_le_bytes(self.bytes())
    }

    pub fn read32b(&mut self) -> u32 {
        u32::from_be_bytes(self.bytes())
    }
}

/// `libxmp_load_sample` for sample `i` of `m`, its data read from `f` (or taken
/// from `buffer` with `SAMPLE_FLAG_NOLOAD`).
pub fn load_sample(m: &mut ModuleData, f: &mut Hio, flags: i32, i: usize, buffer: Option<&[u8]>) -> Result<(), ()> {
    let smpctl = m.smpctl;
    let xxs = &mut m.module.xxs[i];
    if xxs.len <= 0 {
        return Ok(());
    }
    if xxs.len > MAX_SAMPLE_SIZE || smpctl & 1 != 0 {
        if flags & SAMPLE_FLAG_NOLOAD == 0 {
            f.skip(xxs.len as usize);
        }
        return Ok(());
    }

    let mut bytelen = xxs.len as i64;
    let mut framelen = 1i64;
    let mut extralen = 4i64;
    let mut channels = 1;
    if xxs.flg & XMP_SAMPLE_16BIT != 0 {
        bytelen *= 2;
        extralen *= 2;
        framelen *= 2;
    }
    if xxs.flg & XMP_SAMPLE_STEREO != 0 {
        bytelen *= 2;
        extralen *= 2;
        framelen *= 2;
        channels = 2;
    }

    if flags & SAMPLE_FLAG_NOLOAD == 0 {
        let file_pos = f.tell() as i64;
        let file_len = f.size() as i64;
        if file_pos >= file_len {
            return Ok(());
        }
        let remaining = file_len - file_pos;
        if bytelen > remaining {
            bytelen = remaining;
            bytelen -= bytelen & (framelen - 1);
            xxs.len = bytelen as i32;
            if xxs.flg & XMP_SAMPLE_16BIT != 0 {
                xxs.len >>= 1;
            }
            if xxs.flg & XMP_SAMPLE_STEREO != 0 {
                xxs.len >>= 1;
            }
        }
    }

    // loop parameters sanity check
    if xxs.lps < 0 {
        xxs.lps = 0;
    }
    if xxs.lpe > xxs.len {
        xxs.lpe = xxs.len;
    }
    if xxs.lps >= xxs.len || xxs.lps >= xxs.lpe {
        xxs.lps = 0;
        xxs.lpe = 0;
        xxs.flg &= !(XMP_SAMPLE_LOOP | XMP_SAMPLE_LOOP_BIDIR);
    }
    if xxs.flg & XMP_SAMPLE_LOOP_BIDIR != 0 && xxs.flg & XMP_SAMPLE_LOOP == 0 {
        xxs.flg &= !XMP_SAMPLE_LOOP_BIDIR;
    }
    if xxs.flg & XMP_SAMPLE_SLOOP_BIDIR != 0 && xxs.flg & XMP_SAMPLE_SLOOP == 0 {
        xxs.flg &= !XMP_SAMPLE_SLOOP_BIDIR;
    }

    let (bytelen, extralen, framelen) = (bytelen as usize, extralen as usize, framelen as usize);
    let g = SAMPLE_GUARD;
    let mut data = vec![0u8; g + bytelen + extralen];
    let interleave = xxs.flg & XMP_SAMPLE_STEREO != 0 && flags & SAMPLE_FLAG_INTERLEAVED == 0;
    let mut tmp = vec![0u8; if interleave { bytelen } else { 0 }];
    {
        let dest: &mut [u8] = if interleave { &mut tmp[..] } else { &mut data[g..g + bytelen] };
        if flags & SAMPLE_FLAG_NOLOAD != 0 {
            let b = buffer.unwrap_or(&[]);
            let n = b.len().min(bytelen);
            dest[..n].copy_from_slice(&b[..n]);
        } else {
            // a short read leaves zeros
            let avail = f.size().saturating_sub(f.tell()).min(bytelen);
            let p = f.tell();
            dest[..avail].copy_from_slice(&f.data[p..p + avail]);
            f.skip(bytelen);
        }
        let n = xxs.len as usize * channels;
        if flags & SAMPLE_FLAG_7BIT != 0 {
            let dl = dest.len();
            for b in dest[..n.min(dl)].iter_mut() {
                *b <<= 1;
            }
        }
        if xxs.flg & XMP_SAMPLE_16BIT != 0 && flags & SAMPLE_FLAG_BIGEND != 0 {
            for w in dest.chunks_exact_mut(2).take(n) {
                w.swap(0, 1);
            }
        }
        if flags & SAMPLE_FLAG_DIFF != 0 {
            convert_delta(dest, xxs.len as usize, xxs.flg & XMP_SAMPLE_16BIT != 0, channels);
        } else if flags & SAMPLE_FLAG_8BDIFF != 0 {
            let mut len = xxs.len as usize;
            if xxs.flg & XMP_SAMPLE_16BIT != 0 {
                len *= 2;
            }
            convert_delta(dest, len, false, channels);
        }
        if flags & SAMPLE_FLAG_UNS != 0 {
            if xxs.flg & XMP_SAMPLE_16BIT != 0 {
                for w in dest.chunks_exact_mut(2).take(n) {
                    let v = u16::from_le_bytes([w[0], w[1]]).wrapping_add(0x8000);
                    w.copy_from_slice(&v.to_le_bytes());
                }
            } else {
                for b in dest.iter_mut().take(n) {
                    *b = b.wrapping_add(0x80);
                }
            }
        }
        if flags & SAMPLE_FLAG_VIDC != 0 {
            for b in dest.iter_mut().take(n) {
                let x = *b;
                let amp = VIDC_TABLE[(x >> 1) as usize];
                *b = if x & 1 != 0 { (-amp) as u8 } else { amp as u8 };
            }
        }
    }
    if interleave {
        let frames = xxs.len as usize;
        let out = &mut data[g..g + bytelen];
        if xxs.flg & XMP_SAMPLE_16BIT != 0 {
            for k in 0..frames {
                out[4 * k..4 * k + 2].copy_from_slice(&tmp[2 * k..2 * k + 2]);
                out[4 * k + 2..4 * k + 4].copy_from_slice(&tmp[2 * (frames + k)..2 * (frames + k) + 2]);
            }
        } else {
            for k in 0..frames {
                out[2 * k] = tmp[k];
                out[2 * k + 1] = tmp[frames + k];
            }
        }
    }
    if flags & SAMPLE_FLAG_FULLREP != 0 && xxs.lps == 0 && xxs.len > xxs.lpe {
        xxs.flg |= XMP_SAMPLE_LOOP_FULL;
    }
    // extra samples at the end and at the start
    for k in 0..extralen {
        data[g + bytelen + k] = data[g + bytelen + k - framelen];
    }
    for k in 1..=4usize {
        data[g - k] = data[g + framelen - k];
    }
    xxs.data = Some(data);
    Ok(())
}

fn convert_delta(p: &mut [u8], frames: usize, is_16bit: bool, channels: usize) {
    if is_16bit {
        let mut k = 0;
        for _ in 0..channels {
            let mut acc = 0u16;
            for _ in 0..frames {
                if k + 2 > p.len() {
                    return;
                }
                acc = u16::from_le_bytes([p[k], p[k + 1]]).wrapping_add(acc);
                p[k..k + 2].copy_from_slice(&acc.to_le_bytes());
                k += 2;
            }
        }
    } else {
        let mut k = 0;
        for _ in 0..channels {
            let mut acc = 0u16;
            for _ in 0..frames {
                if k >= p.len() {
                    return;
                }
                acc = (p[k] as u16).wrapping_add(acc);
                p[k] = acc as u8;
                k += 1;
            }
        }
    }
}

const VIDC_TABLE: [i8; 128] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2,
    3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 6, 6, 6, 6, 7, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 14, 15, 15, 16, 17,
    18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 33, 34, 36, 38, 40, 42, 44, 46, 48, 50, 52, 54, 56, 58, 60, 62, 65, 68,
    72, 77, 80, 84, 91, 95, 98, 103, 109, 114, 120, 126, 127,
];

fn check_envelope(env: &mut Envelope) {
    if env.npt <= 0 || env.npt > XMP_MAX_ENV_POINTS as i32 {
        env.flg &= !XMP_ENVELOPE_ON;
    }
    if env.lps >= env.npt || env.lpe >= env.npt {
        env.flg &= !XMP_ENVELOPE_LOOP;
    }
    if env.sus >= env.npt || env.sue >= env.npt {
        env.flg &= !XMP_ENVELOPE_SUS;
    }
}

/// The checks of `xmp_load_module` after the loader, `libxmp_load_epilogue` and
/// `libxmp_prepare_scan`. Err: the module is broken.
pub fn load_epilogue(m: &mut ModuleData) -> Result<(), ()> {
    let volbase = m.volbase;
    let quirk = m.quirk;
    let md = &mut m.module;
    if md.chn > XMP_MAX_CHANNELS || md.len > XMP_MAX_MOD_LENGTH {
        return Err(());
    }
    for c in &md.xxc[..md.chn] {
        if !(0..=0xff).contains(&c.vol) || !(0..=0xff).contains(&c.pan) {
            return Err(());
        }
    }
    if md.xxp.len() < md.pat || md.xxt.len() < md.trk {
        return Err(());
    }
    for p in &md.xxp[..md.pat] {
        if p.index.len() < md.chn || p.index[..md.chn].iter().any(|&t| t >= md.trk) {
            return Err(());
        }
    }
    // libxmp_adjust_string
    let adjust = |s: &str| -> String {
        s.chars().map(|c| if (' '..='~').contains(&c) { c } else { ' ' }).collect::<String>().trim_end().to_string()
    };
    md.name = adjust(&md.name);
    for i in md.xxi.iter_mut() {
        i.name = adjust(&i.name);
    }

    md.gvl = m.gvol;
    md.len = md.len.min(XMP_MAX_MOD_LENGTH);
    md.pat = md.pat.min(257);
    md.ins = md.ins.min(255);
    md.chn = md.chn.min(XMP_MAX_CHANNELS);
    if md.rst >= md.len {
        md.rst = 0;
    }
    if md.spd <= 0 || md.spd > 255 {
        md.spd = 6;
    }
    md.bpm = md.bpm.clamp(XMP_MIN_BPM, 1000);
    for ins in md.xxi.iter_mut().take(md.ins) {
        if quirk & QUIRK_INSVOL == 0 {
            ins.vol = volbase;
        }
        let nsm = (ins.nsm.max(0) as usize).min(ins.sub.len());
        for s in ins.sub.iter_mut().take(nsm) {
            if quirk & QUIRK_INSVOL == 0 {
                s.gvl = volbase;
            }
        }
        check_envelope(&mut ins.aei);
        check_envelope(&mut ins.fei);
        check_envelope(&mut ins.pei);
        if ins.aei.flg & XMP_ENVELOPE_ON != 0 {
            for k in 0..ins.aei.npt as usize {
                let v = &mut ins.aei.data[k * 2 + 1];
                *v = (*v).clamp(0, volbase as i16);
            }
        }
    }
    for (k, xxs) in md.xxs.iter_mut().enumerate().take(md.smp) {
        let xtra = &mut m.xtra[k];
        if xtra.sus < 0 {
            xtra.sus = 0;
        }
        if xtra.sue > xxs.len {
            xtra.sue = xxs.len;
        }
        if xtra.sus >= xxs.len || xtra.sus >= xtra.sue {
            xtra.sus = 0;
            xtra.sue = 0;
            xxs.flg &= !(XMP_SAMPLE_SLOOP | XMP_SAMPLE_SLOOP_BIDIR);
        }
    }

    // libxmp_prepare_scan
    let mut ord = 0;
    while ord < md.len && md.xxo[ord] as usize >= md.pat {
        ord += 1;
    }
    if ord >= md.len {
        md.len = 0;
        return Ok(());
    }
    m.scan_cnt = (0..md.len)
        .map(|i| {
            let p = md.xxo[i] as usize;
            let rows = if p < md.pat && md.xxp[p].rows > 0 { md.xxp[p].rows } else { 1 };
            vec![0u8; rows]
        })
        .collect();
    Ok(())
}
