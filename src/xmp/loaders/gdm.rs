//! General Digital Music modules (`loaders/gdm_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::load::*;
use crate::xmp::module::*;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 0x4b && &d[..4] == b"GDM\xfe" && &d[0x47..0x4b] == b"GMFS"
}

fn translate_effect(fxt: &mut u8, fxp: &mut u8) {
    match *fxt {
        0x00 => *fxp = 0,
        0x01..=0x07 | 0x09..=0x0d => {}
        0x08 => *fxt = FX_TREMOR,
        0x0e => {
            // some extended effects become their S3M equivalents
            let h = msn(*fxp);
            let l = lsn(*fxp);
            match h {
                EX_F_PORTA_UP => {
                    *fxt = FX_PORTA_UP;
                    *fxp = l | 0xf0;
                }
                EX_F_PORTA_DN => {
                    *fxt = FX_PORTA_DN;
                    *fxp = l | 0xf0;
                }
                0x8 => {
                    *fxt = FX_PORTA_UP;
                    *fxp = l | 0xe0;
                }
                0x9 => {
                    *fxt = FX_PORTA_DN;
                    *fxp = l | 0xe0;
                }
                EX_F_VSLIDE_UP => {
                    if l != 0 {
                        *fxt = FX_VOLSLIDE;
                        *fxp = (l << 4) | 0xf;
                    }
                }
                EX_F_VSLIDE_DN => {
                    if l != 0 {
                        *fxt = FX_VOLSLIDE;
                        *fxp = l | 0xf0;
                    }
                }
                _ => {}
            }
        }
        0x0f => *fxt = FX_S3M_SPEED,
        0x10 => *fxt = FX_S3M_ARPEGGIO,
        0x11 => {
            *fxt = 0;
            *fxp = 0;
        }
        0x12 => *fxt = FX_MULTI_RETRIG,
        0x13 => *fxt = FX_GLOBALVOL,
        0x14 => *fxt = FX_FINE_VIBRATO,
        0x1e => match msn(*fxp) {
            0x0 => {
                if lsn(*fxp) == 1 {
                    *fxt = FX_SURROUND;
                    *fxp = 1;
                } else {
                    *fxt = 0;
                    *fxp = 0;
                }
            }
            0x8 => *fxt = FX_EXTENDED,
            _ => {
                *fxt = 0;
                *fxp = 0;
            }
        },
        0x1f => *fxt = FX_S3M_BPM,
        _ => {
            *fxt = 0;
            *fxp = 0;
        }
    }
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    f.read32b();
    let mut name = [0u8; 32];
    f.read(&mut name);
    m.module.name = String::from_utf8_lossy(&name[..name.iter().position(|&b| b == 0).unwrap_or(32)]).into_owned();
    f.skip(32);
    f.skip(7);
    let vermaj = f.read8();
    let vermin = f.read8();
    let tracker = f.read16l();
    let tvmaj = f.read8();
    let tvmin = f.read8();
    m.module.type_ = if tracker == 0 {
        format!("GDM {}.{:02} (2GDM {}.{:02})", vermaj, vermin, tvmaj, tvmin)
    } else {
        format!("GDM {}.{:02} (unknown tracker {}.{:02})", vermaj, vermin, tvmaj, tvmin)
    };

    let mut panmap = [0u8; 32];
    if f.read(&mut panmap) == 0 {
        return Err(());
    }
    for i in 0..32 {
        if panmap[i] == 255 {
            panmap[i] = 8;
            m.module.xxc[i].flg |= XMP_CHANNEL_MUTE;
        } else if panmap[i] == 16 {
            panmap[i] = 8;
        }
        m.module.xxc[i].pan = 0x80 + (panmap[i] as i32 - 8) * 16;
    }

    m.module.gvl = f.read8() as i32;
    m.module.spd = f.read8() as i32;
    m.module.bpm = f.read8() as i32;
    f.read16l();
    let ord_ofs = f.read32l() as usize;
    m.module.len = f.read8() as usize + 1;
    let pat_ofs = f.read32l() as usize;
    m.module.pat = f.read8() as usize + 1;
    let ins_ofs = f.read32l() as usize;
    let smp_ofs = f.read32l() as usize;
    m.module.ins = f.read8() as usize + 1;
    m.module.smp = m.module.ins;
    if m.module.ins > 255 {
        return Err(());
    }
    m.c4rate = C4_NTSC_RATE;

    f.seek(ord_ofs);
    for i in 0..m.module.len {
        m.module.xxo[i] = f.read8();
    }

    f.seek(ins_ofs);
    m.init_instrument();
    for i in 0..m.module.ins {
        let mut buffer = [0u8; 32];
        if f.read(&mut buffer) != 32 {
            return Err(());
        }
        let name = copy_adjust(&buffer, 31);
        f.skip(12);
        f.read8();
        let len = f.read32l() as i32;
        let lps = f.read32l() as i32;
        let lpe = f.read32l() as i32;
        let flg = f.read8();
        let c4spd = f.read16l();
        let vol = f.read8();
        let pan = f.read8();
        let (xpo, fin) = c2spd_to_note(c4spd as i32);
        let xxs = &mut m.module.xxs[i];
        xxs.len = len;
        xxs.lps = lps;
        xxs.lpe = lpe;
        xxs.flg = 0;
        if flg & 0x01 != 0 {
            xxs.flg |= XMP_SAMPLE_LOOP;
        }
        if flg & 0x02 != 0 {
            xxs.flg |= XMP_SAMPLE_16BIT;
            xxs.len >>= 1;
            xxs.lps >>= 1;
            xxs.lpe >>= 1;
        }
        let xxi = &mut m.module.xxi[i];
        xxi.name = name;
        xxi.sub = vec![SubInstrument {
            vol: if vol > 0x40 { 0x40 } else { vol as i32 },
            pan: if pan > 15 { XMP_INST_NO_DEFAULT_PAN } else { 0x80 + (pan as i32 - 8) * 16 },
            xpo,
            fin,
            sid: i as i32,
            ..Default::default()
        }];
        // C tests the length as read, before the 16-bit halving
        if len > 0 {
            xxi.nsm = 1;
        }
    }

    // Effects in muted channels are processed, so scan the patterns first to
    // see the real number of channels.
    f.seek(pat_ofs);
    m.module.chn = 0;
    for _ in 0..m.module.pat {
        let mut len = f.read16l() as i32 - 2;
        let mut r = 0;
        while len > 0 {
            let c = f.read8() as usize;
            if f.error {
                return Err(());
            }
            len -= 1;
            if c == 0 {
                r += 1;
                if len == 0 {
                    if r > 64 {
                        return Err(());
                    }
                } else if r >= 64 {
                    return Err(());
                }
                continue;
            }
            if m.module.chn <= (c & 0x1f) {
                m.module.chn = (c & 0x1f) + 1;
            }
            if c & 0x20 != 0 {
                f.read8();
                f.read8();
                len -= 2;
            }
            if c & 0x40 != 0 {
                loop {
                    let k = f.read8();
                    if f.error {
                        return Err(());
                    }
                    len -= 1;
                    if k & 0xc0 != 0xc0 {
                        f.read8();
                        len -= 1;
                    }
                    if k & 0x20 == 0 {
                        break;
                    }
                }
            }
        }
    }
    m.module.trk = m.module.pat * m.module.chn;

    f.seek(pat_ofs);
    for i in 0..m.module.pat {
        m.module.alloc_pattern_tracks(i, 64);
        let mut len = f.read16l() as i32 - 2;
        let mut r = 0;
        while len > 0 {
            let c = f.read8() as usize;
            if f.error {
                return Err(());
            }
            len -= 1;
            if c == 0 {
                r += 1;
                continue;
            }
            if (c & 0x1f) >= m.module.chn || r >= 64 {
                return Err(());
            }
            let ch = c & 0x1f;
            if c & 0x20 != 0 {
                let k = f.read8();
                let ins = f.read8();
                let e = m.module.event_mut(i, ch, r);
                e.note = if k != 0 { 12 + 12 * msn(k & 0x7f) + lsn(k) } else { 0 };
                e.ins = ins;
                len -= 2;
            }
            if c & 0x40 != 0 {
                loop {
                    let k = f.read8();
                    if f.error {
                        return Err(());
                    }
                    len -= 1;
                    match (k & 0xc0) >> 6 {
                        0 => {
                            let mut p = f.read8();
                            len -= 1;
                            let mut t = k & 0x1f;
                            translate_effect(&mut t, &mut p);
                            let e = m.module.event_mut(i, ch, r);
                            e.fxt = t;
                            e.fxp = p;
                        }
                        1 => {
                            let mut p = f.read8();
                            len -= 1;
                            let mut t = k & 0x1f;
                            translate_effect(&mut t, &mut p);
                            let e = m.module.event_mut(i, ch, r);
                            e.f2t = t;
                            e.f2p = p;
                        }
                        2 => {
                            f.read8();
                            len -= 1;
                        }
                        _ => {}
                    }
                    if k & 0x20 == 0 {
                        break;
                    }
                }
            }
        }
    }

    f.seek(smp_ofs);
    for i in 0..m.module.ins {
        load_sample(m, f, SAMPLE_FLAG_UNS, i, None)?;
    }

    m.quirk |= QUIRK_ARPMEM | QUIRK_FINEFX;
    m.flow_mode = FLOW_MODE_ST3_321;
    Ok(())
}
