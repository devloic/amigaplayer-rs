//! Liquid Tracker "NO" modules (`loaders/no_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//!
//! Nir Oren's Liquid Tracker old "NO" format.

use crate::xmp::load::*;
use crate::xmp::module::*;

pub fn test(d: &[u8]) -> bool {
    if d.len() < 4 + 33 || &d[..4] != b"NO\0\0" {
        return false;
    }
    let buf = &d[4..37];
    let nsize = buf[0] as usize;
    if nsize > 29 {
        return false;
    }
    if buf[1..=nsize].contains(&0) {
        return false;
    }
    if buf[30] == 0 {
        return false;
    }
    let chn = buf[32];
    chn != 0 && chn <= 16
}

const FX: [u8; 15] = [
    FX_SPEED,
    FX_VIBRATO,
    FX_BREAK,
    FX_PORTA_DN,
    FX_PORTA_UP,
    0,
    FX_ARPEGGIO,
    FX_SETPAN,
    0,
    FX_JUMP,
    FX_TREMOLO,
    FX_VOLSLIDE,
    0,
    FX_TONEPORTA,
    FX_OFFSET,
];

// All FX_EXTENDED, in a custom order.
const FX_MISC2: [u8; 16] = [
    EX_F_PORTA_UP,
    EX_F_PORTA_DN,
    EX_F_VSLIDE_UP,
    EX_F_VSLIDE_DN,
    EX_VIBRATO_WF,
    EX_TREMOLO_WF,
    EX_RETRIG,
    EX_CUT,
    EX_DELAY,
    0,
    0,
    EX_PATTERN_LOOP,
    EX_PATT_DELAY,
    0,
    0,
    0,
];

fn translate_effect(e: &mut Event, fxt: u32, fxp: u8) {
    match fxt {
        0x0 | 0x1 | 0x2 | 0x3 | 0x4 | 0x6 | 0x9 | 0xa | 0xb | 0xd | 0xe => {
            e.fxt = FX[fxt as usize];
            e.fxp = fxp;
        }
        0x7 => {
            // decimal value, effective values > 64 are ignored
            let value = msn(fxp) as u32 * 10 + lsn(fxp) as u32;
            if value <= 64 && value != 70 {
                e.fxt = FX_SETPAN;
                e.fxp = (value * 0xff / 64) as u8;
            }
        }
        0x8 => match msn(fxp) {
            0x0 => {
                e.fxt = FX_VIBRA_VSLIDE;
                e.fxp = lsn(fxp) << 4;
            }
            0x1 => {
                e.fxt = FX_VIBRA_VSLIDE;
                e.fxp = lsn(fxp);
            }
            0x2 => {
                e.fxt = FX_TONE_VSLIDE;
                e.fxp = lsn(fxp) << 4;
            }
            0x3 => {
                e.fxt = FX_TONE_VSLIDE;
                e.fxp = lsn(fxp);
            }
            0x4 => {
                e.fxt = FX_TREMOLO;
                e.fxp = 0;
                e.f2t = FX_VOLSLIDE;
                e.f2p = lsn(fxp) << 4;
            }
            0x5 => {
                e.fxt = FX_TREMOLO;
                e.fxp = 0;
                e.f2t = FX_VOLSLIDE;
                e.f2p = lsn(fxp);
            }
            _ => {}
        },
        0xc => {
            let value = msn(fxp);
            let mut p = lsn(fxp);
            match value {
                0x4 | 0x5 | 0x0..=0x3 | 0x6..=0x8 | 0xb | 0xc => {
                    if (value == 0x4 || value == 0x5) && p & 3 == 3 {
                        p -= 1;
                    }
                    e.fxt = FX_EXTENDED;
                    e.fxp = (FX_MISC2[value as usize] << 4) | p;
                }
                _ => {}
            }
        }
        _ => {}
    }
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    f.skip(4);
    m.module.type_ = "Liquid Tracker".to_string();

    let nsize = f.read8() as usize;
    let mut name = [0u8; 29];
    if f.read(&mut name) < 29 {
        return Err(());
    }
    let n = nsize.min(29);
    m.module.name = String::from_utf8_lossy(&name[..name[..n].iter().position(|&b| b == 0).unwrap_or(n)]).into_owned();

    let pat = f.read8() as usize;
    f.read8();
    let chn = f.read8() as usize;
    f.read8();
    f.read16l();
    f.read16l();
    f.read8();
    m.module.pat = pat;
    m.module.chn = chn;
    m.module.trk = pat * chn;
    m.module.ins = 63;
    m.module.smp = 63;

    m.module.xxo = [0; XMP_MAX_MOD_LENGTH];
    let mut i = 0;
    while i < 256 {
        let x = f.read8();
        if x == 0xff {
            break;
        }
        if i < XMP_MAX_MOD_LENGTH {
            m.module.xxo[i] = x;
        }
        i += 1;
    }
    f.seek_cur(255 - i as i64);
    m.module.len = i;

    m.c4rate = C4_NTSC_RATE;

    m.init_instrument();
    for i in 0..63 {
        let mut b = [0u8; 46];
        if f.read(&mut b) < 46 {
            return Err(());
        }
        let nsize = (b[0] as usize).min(30);
        let c2spd = u16::from_le_bytes([b[32], b[33]]) as i32;
        let (xpo, fin) = c2spd_to_note(c2spd);
        let slen = u32::from_le_bytes([b[34], b[35], b[36], b[37]]);
        let lps = u32::from_le_bytes([b[38], b[39], b[40], b[41]]);
        let lpe = u32::from_le_bytes([b[42], b[43], b[44], b[45]]);
        let xxs = &mut m.module.xxs[i];
        xxs.len = slen as i32;
        xxs.lps = lps as i32;
        xxs.lpe = lpe as i32;
        xxs.flg = if xxs.lpe > 0 { XMP_SAMPLE_LOOP } else { 0 };
        let has = xxs.len > 0;
        let xxi = &mut m.module.xxi[i];
        xxi.name = copy_adjust(&b[1..], nsize);
        xxi.sub = vec![SubInstrument {
            vol: b[31] as i32,
            fin,
            xpo,
            pan: XMP_INST_NO_DEFAULT_PAN,
            sid: i as i32,
            ..Default::default()
        }];
        if has {
            xxi.nsm = 1;
        }
    }

    for i in 0..pat {
        m.module.alloc_pattern_tracks(i, 64);
        for j in 0..64 {
            for k in 0..chn {
                let x = f.read32l();
                let note = x & 0x3f;
                let ins = (x & 0x1fc0) >> 6;
                let vol = (x & 0xfe000) >> 13;
                let fxt = (x & 0x00f0_0000) >> 20;
                let fxp = ((x & 0xff00_0000) >> 24) as u8;
                let e = m.module.event_mut(i, k, j);
                if note != 0x3f {
                    e.note = (37 + note) as u8;
                }
                if ins != 0x7f {
                    e.ins = (1 + ins) as u8;
                }
                if vol != 0x7f {
                    e.vol = (1 + vol) as u8;
                }
                if fxt != 0x0f {
                    translate_effect(e, fxt, fxp);
                }
            }
        }
    }

    for i in 0..63 {
        if m.module.xxs[i].len == 0 {
            continue;
        }
        load_sample(m, f, SAMPLE_FLAG_UNS, i, None)?;
    }

    m.quirk |= QUIRK_FINEFX | QUIRK_RTONCE;
    m.flow_mode = FLOW_MODE_LIQUID;
    m.read_event_type = ReadEvent::St3;
    for i in 0..chn {
        m.module.xxc[i].pan = m.defpan(if i & 1 != 0 { 0xff } else { 0 });
    }
    Ok(())
}
