//! Asylum Music Format modules (`loaders/asylum_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//! Based on the AMF->MOD converter written by Mr. P / Powersource, 1995.

use crate::xmp::load::*;
use crate::xmp::module::*;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 32 && &d[..32] == b"ASYLUM Music Format V1.0\0\0\0\0\0\0\0\0"
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    f.skip(32);
    let spd = f.read8() as i32;
    let bpm = f.read8() as i32;
    let ins = f.read8() as usize;
    let pat = f.read8() as usize;
    let len = f.read8() as usize;
    let _rst = f.read8();
    if ins > 64 {
        return Err(());
    }
    let md = &mut m.module;
    md.spd = spd;
    md.bpm = bpm;
    md.ins = ins;
    md.pat = pat;
    md.len = len;
    let mut order = vec![0u8; len];
    f.read(&mut order);
    md.xxo = [0; XMP_MAX_MOD_LENGTH];
    md.xxo[..len].copy_from_slice(&order);
    f.seek(294);

    md.chn = 8;
    md.smp = ins;
    md.trk = pat * 8;
    md.type_ = "Asylum Music Format v1.0".to_string();

    m.init_instrument();
    for i in 0..ins {
        let mut b = [0u8; 37];
        if f.read(&mut b) != 37 {
            return Err(());
        }
        let slen = u32::from_le_bytes([b[25], b[26], b[27], b[28]]);
        let lps = u32::from_le_bytes([b[29], b[30], b[31], b[32]]);
        let lpl = u32::from_le_bytes([b[33], b[34], b[35], b[36]]);
        let xxi = &mut m.module.xxi[i];
        xxi.name = copy_adjust(&b, 22);
        xxi.sub = vec![SubInstrument {
            fin: ((b[22] << 4) as i8) as i32,
            vol: b[23] as i32,
            xpo: (b[24] as i8) as i32,
            pan: XMP_INST_NO_DEFAULT_PAN,
            sid: i as i32,
            ..Default::default()
        }];
        let xxs = &mut m.module.xxs[i];
        xxs.len = slen as i32;
        xxs.lps = lps as i32;
        xxs.lpe = lps.wrapping_add(lpl) as i32;
        if slen >= 0x20000 {
            return Err(());
        }
        xxs.flg = if xxs.lpe > 2 { XMP_SAMPLE_LOOP } else { 0 };
    }
    f.seek_cur(37 * (64 - ins as i64));

    for i in 0..pat {
        m.module.alloc_pattern_tracks(i, 64);
        let mut buf = [0u8; 2048];
        if f.read(&mut buf) < 2048 {
            return Err(());
        }
        for j in 0..64 * 8 {
            let e = m.module.event_mut(i, j % 8, j / 8);
            *e = Event::default();
            let p = &buf[j * 4..j * 4 + 4];
            if p[0] != 0 {
                e.note = p[0].wrapping_add(13);
            }
            e.ins = p[1];
            e.fxt = p[2];
            e.fxp = p[3];
            if e.fxt >= 0x10 && e.fxt != FX_MULTI_RETRIG {
                e.fxt = 0;
                e.fxp = 0;
            }
        }
    }

    for i in 0..ins {
        if m.module.xxs[i].len > 1 {
            load_sample(m, f, 0, i, None)?;
            m.module.xxi[i].nsm = 1;
        }
    }
    Ok(())
}
