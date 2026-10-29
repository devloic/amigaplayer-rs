//! Ultra Tracker modules (`loaders/ult_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::load::*;
use crate::xmp::module::*;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 15 && &d[..14] == b"MAS_UTrack_V00" && (b'1'..=b'4').contains(&d[14])
}

fn translate_effect(fxt: &mut u8, fxp: &mut u8) {
    match *fxt {
        0x03 => *fxt = FX_ULT_TPORTA,
        0x05 | 0x06 => {
            *fxt = 0;
            *fxp = 0;
        }
        0x0b => {
            *fxt = FX_SETPAN;
            *fxp <<= 4;
        }
        0x09 => *fxp <<= 2,
        0x0f => *fxt = FX_ULT_TEMPO,
        _ => {}
    }
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let mut magic = [0u8; 15];
    f.read(&mut magic);
    let mut name = [0u8; 32];
    f.read(&mut name);
    let msgsize = f.read8() as usize;
    let ver = (magic[14] - b'0') as usize;
    m.module.name = String::from_utf8_lossy(&name[..name.iter().position(|&b| b == 0).unwrap_or(32)]).into_owned();
    let verstr = ["< 1.4", "1.4", "1.5", "1.6"];
    m.module.type_ = format!("Ultra Tracker {} ULT V{:03}", verstr[ver - 1], ver);
    m.c4rate = C4_NTSC_RATE;

    if msgsize > 0 {
        let mut c = Vec::new();
        for _ in 0..msgsize {
            let mut l = [0u8; 32];
            if f.read(&mut l) < 32 {
                return Err(());
            }
            c.extend_from_slice(&l);
            c.push(b'\n');
        }
        c.pop();
        let end = c.iter().position(|&b| b == 0).unwrap_or(c.len());
        m.comment = String::from_utf8_lossy(&c[..end]).into_owned();
    }

    m.module.ins = f.read8() as usize;
    m.module.smp = m.module.ins;
    m.init_instrument();

    for i in 0..m.module.ins {
        let mut iname = [0u8; 32];
        f.read(&mut iname);
        f.skip(12);
        let loop_start = f.read32l();
        let loopend = f.read32l();
        let sizestart = f.read32l();
        let sizeend = f.read32l();
        let volume = f.read8();
        let bidiloop = f.read8();
        let c2spd = if ver >= 4 { f.read16l() } else { 0 };
        let _finetune = f.read16l();
        if f.error {
            return Err(());
        }
        if sizestart > sizeend || sizeend > (16 << 20) || sizeend - sizestart > (256 << 10) {
            return Err(());
        }
        let xxs = &mut m.module.xxs[i];
        xxs.len = (sizeend - sizestart) as i32;
        xxs.lps = loop_start as i32;
        xxs.lpe = loopend as i32;
        let has = xxs.len > 0;
        xxs.flg = match bidiloop {
            20 | 4 => XMP_SAMPLE_16BIT,
            8 => XMP_SAMPLE_LOOP,
            12 => XMP_SAMPLE_16BIT | XMP_SAMPLE_LOOP,
            24 => XMP_SAMPLE_LOOP | XMP_SAMPLE_LOOP_REVERSE,
            28 => XMP_SAMPLE_16BIT | XMP_SAMPLE_LOOP | XMP_SAMPLE_LOOP_REVERSE,
            _ => xxs.flg,
        };
        let (xpo, fin) = if ver > 3 { c2spd_to_note(c2spd as i32) } else { (0, 0) };
        let xxi = &mut m.module.xxi[i];
        if has {
            xxi.nsm = 1;
        }
        xxi.sub = vec![SubInstrument { vol: volume as i32, pan: XMP_INST_NO_DEFAULT_PAN, sid: i as i32, xpo, fin, ..Default::default() }];
        xxi.name = copy_adjust(&iname, 24);
    }

    let mut order = [0u8; 256];
    f.read(&mut order);
    let channels = f.read8() as usize;
    let patterns = f.read8() as usize;
    if f.error {
        return Err(());
    }
    let len = order.iter().position(|&o| o == 0xff).unwrap_or(256);
    m.module.xxo = [0; XMP_MAX_MOD_LENGTH];
    m.module.xxo[..len].copy_from_slice(&order[..len]);
    m.module.len = len;
    m.module.chn = channels + 1;
    m.module.pat = patterns + 1;
    m.module.spd = 6;
    m.module.bpm = 125;
    m.module.trk = m.module.chn * m.module.pat;
    if m.module.chn > XMP_MAX_CHANNELS {
        return Err(());
    }

    for i in 0..m.module.chn {
        m.module.xxc[i].pan = if ver >= 3 { 255 * f.read8() as i32 / 15 } else { m.defpan((((i as i32 + 1) / 2) % 2) * 0xff) };
    }

    for i in 0..m.module.pat {
        m.module.alloc_pattern_tracks(i, 64);
    }

    // events are stored by channel
    let total = 64 * m.module.pat;
    for i in 0..m.module.chn {
        let mut j = 0;
        while j < total {
            let mut cnt = 1;
            let mut x8 = f.read8();
            if x8 == 0xfc {
                cnt = f.read8() as usize;
                x8 = f.read8();
            }
            let mut ue = [0u8; 4];
            if f.read(&mut ue) < 4 {
                return Err(());
            }
            if cnt == 0 {
                cnt += 1;
            }
            if j + cnt > total {
                return Err(());
            }
            for _ in 0..cnt {
                let e = m.module.event_mut(j >> 6, i, j & 0x3f);
                *e = Event::default();
                if x8 != 0 {
                    e.note = x8.wrapping_add(36);
                }
                e.ins = ue[0];
                e.fxt = msn(ue[1]);
                e.f2t = lsn(ue[1]);
                e.fxp = ue[3];
                e.f2p = ue[2];
                translate_effect(&mut e.fxt, &mut e.fxp);
                translate_effect(&mut e.f2t, &mut e.f2p);
                j += 1;
            }
        }
    }

    for i in 0..m.module.ins {
        if m.module.xxs[i].len == 0 {
            continue;
        }
        load_sample(m, f, 0, i, None)?;
    }
    m.volbase = 0x100;
    Ok(())
}
