//! Epic MegaGames MASI 16 (Protracker Studio) modules (`loaders/masi16_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::load::*;
use crate::xmp::module::*;
use crate::xmp::player::c2spd_to_note;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 4 && &d[..4] == b"PSM\xfe"
}

fn translate_effect(e: &mut Event, effect: u8, param: u8, param2: u8, _param3: u8) {
    let ex = |x: u8| (x << 4) | (param & 0x0f);
    match effect {
        1 => (e.fxt, e.fxp) = (FX_F_VSLIDE_UP, param),
        2 => (e.fxt, e.fxp) = (FX_VOLSLIDE_UP, param),
        3 => (e.fxt, e.fxp) = (FX_F_VSLIDE_DN, param),
        4 => (e.fxt, e.fxp) = (FX_VOLSLIDE_DN, param),

        10 => (e.fxt, e.fxp) = (FX_F_PORTA_UP, param),
        11 => (e.fxt, e.fxp) = (FX_PORTA_UP, param),
        12 => (e.fxt, e.fxp) = (FX_F_PORTA_DN, param),
        13 => (e.fxt, e.fxp) = (FX_PORTA_DN, param),
        14 => (e.fxt, e.fxp) = (FX_TONEPORTA, param),
        15 => (e.fxt, e.fxp) = (FX_EXTENDED, ex(EX_GLISS)),
        16 => {
            (e.fxt, e.fxp) = (FX_TONEPORTA, 0);
            (e.f2t, e.f2p) = (FX_VOLSLIDE_UP, param);
        }
        17 => {
            (e.fxt, e.fxp) = (FX_TONEPORTA, 0);
            (e.f2t, e.f2p) = (FX_VOLSLIDE_DN, param);
        }

        20 => (e.fxt, e.fxp) = (FX_VIBRATO, param),
        21 => (e.fxt, e.fxp) = (FX_EXTENDED, ex(EX_VIBRATO_WF)),
        22 => {
            (e.fxt, e.fxp) = (FX_VIBRATO, 0);
            (e.f2t, e.f2p) = (FX_VOLSLIDE_UP, param);
        }
        23 => {
            (e.fxt, e.fxp) = (FX_VIBRATO, 0);
            (e.f2t, e.f2p) = (FX_VOLSLIDE_DN, param);
        }

        30 => (e.fxt, e.fxp) = (FX_TREMOLO, param),
        31 => (e.fxt, e.fxp) = (FX_EXTENDED, ex(EX_TREMOLO_WF)),

        // sample offset: param and param3 are the fine and high offsets (TODO in libxmp)
        40 => (e.fxt, e.fxp) = (FX_OFFSET, param2),
        41 => (e.fxt, e.fxp) = (FX_EXTENDED, ex(EX_RETRIG)),
        42 => (e.fxt, e.fxp) = (FX_EXTENDED, ex(EX_CUT)),
        43 => (e.fxt, e.fxp) = (FX_EXTENDED, ex(EX_DELAY)),

        50 => (e.fxt, e.fxp) = (FX_JUMP, param),
        51 => (e.fxt, e.fxp) = (FX_BREAK, param),
        52 => (e.fxt, e.fxp) = (FX_EXTENDED, ex(EX_PATTERN_LOOP)),
        53 => (e.fxt, e.fxp) = (FX_PATT_DELAY, param),

        60 => (e.fxt, e.fxp) = (FX_S3M_SPEED, param),
        61 => (e.fxt, e.fxp) = (FX_S3M_BPM, param),

        70 => (e.fxt, e.fxp) = (FX_ARPEGGIO, param),
        71 => (e.fxt, e.fxp) = (FX_FINETUNE, param),
        72 => (e.fxt, e.fxp) = (FX_SETPAN, (param & 0x0f) | ((param & 0x0f) << 4)),

        _ => (e.fxt, e.fxp) = (0, 0),
    }
}

fn le16(b: &[u8]) -> i32 {
    u16::from_le_bytes([b[0], b[1]]) as i32
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let mut buf = [0u8; 1024];
    let mut p_smp = [0u32; 256];
    let mut sample_map = [0u8; 256];

    f.read32b();

    f.read(&mut buf[..60]);
    let n = buf[..59].iter().position(|&b| b == 0).unwrap_or(59);
    m.module.name = String::from_utf8_lossy(&buf[..n]).into_owned();

    let type_ = f.read8(); // song type
    let ver = f.read8(); // song version
    f.read8(); // pattern version

    if type_ & 0x01 != 0 {
        return Err(()); // song mode not supported
    }

    m.module.type_ = format!("Epic MegaGames MASI 16 PSM {}.{:02}", msn(ver), lsn(ver));

    m.module.spd = f.read8() as i32;
    m.module.bpm = f.read8() as i32;
    f.read8(); // master volume
    f.read16l(); // song length
    m.module.len = f.read16l() as usize;
    m.module.pat = f.read16l() as usize;
    let stored_ins = f.read16l() as usize;
    f.read16l(); // channels to play
    m.module.chn = f.read16l() as usize; // channels to proceed

    // Sanity check
    if m.module.len > 256 || m.module.pat > 256 || stored_ins > 255 || m.module.chn > XMP_MAX_CHANNELS {
        return Err(());
    }
    m.module.trk = m.module.pat * m.module.chn;

    let p_ord = f.read32l() as usize;
    let p_chn = f.read32l() as usize;
    let p_pat = f.read32l() as usize;
    let p_ins = f.read32l() as usize;

    m.c4rate = C4_NTSC_RATE;

    f.seek(p_ord);
    let mut ord = vec![0u8; m.module.len];
    f.read(&mut ord);
    m.module.xxo[..ord.len()].copy_from_slice(&ord);

    buf[..m.module.chn].fill(0);
    f.seek(p_chn);
    f.read(&mut buf[..16]);

    for i in 0..m.module.chn {
        if buf[i] < 16 {
            m.module.xxc[i].pan = (buf[i] | (buf[i] << 4)) as i32;
        }
    }

    // Get the actual instruments count...
    m.module.ins = 0;
    for i in 0..stored_ins {
        f.seek(p_ins + 64 * i + 45);
        sample_map[i] = f.read16l().wrapping_sub(1) as u8;
        m.module.ins = m.module.ins.max(sample_map[i] as usize + 1);
    }
    if m.module.ins > 255 || f.take_error() {
        return Err(());
    }

    m.module.smp = m.module.ins;
    m.init_instrument();

    f.seek(p_ins);
    for i in 0..stored_ins {
        let num = sample_map[i] as usize;

        if f.read(&mut buf[..64]) < 64 {
            return Err(());
        }

        // Don't load duplicate instruments
        if !m.module.xxi[num].sub.is_empty() {
            continue;
        }

        p_smp[i] = le32(&buf[37..]);
        let flags = buf[47];
        let finetune = buf[60];
        let c2spd = le16(&buf[62..]);

        let xxs = &mut m.module.xxs[num];
        xxs.len = le32(&buf[48..]) as i32;
        xxs.lps = le32(&buf[52..]) as i32;
        xxs.lpe = le32(&buf[56..]) as i32;
        xxs.flg = if flags & 0x80 != 0 { XMP_SAMPLE_LOOP } else { 0 };
        xxs.flg |= if flags & 0x20 != 0 { XMP_SAMPLE_LOOP_BIDIR } else { 0 };

        let (mut xpo, mut fin) = c2spd_to_note(c2spd);
        fin += ((finetune & 0x0f) << 4) as i8 as i32;
        xpo += (finetune >> 4) as i32 - 7;

        // The documentation claims samples shouldn't exceed 64k. Later
        // Protracker Studio Modules (MASI) allow up to 1MB.
        if xxs.len as u32 > 64 * 1024 {
            return Err(());
        }
        let has = xxs.len > 0;

        let xxi = &mut m.module.xxi[num];
        xxi.name = {
            let n = buf[13..37].iter().position(|&b| b == 0).unwrap_or(24);
            String::from_utf8_lossy(&buf[13..13 + n]).into_owned()
        };
        xxi.sub = vec![SubInstrument {
            vol: buf[61] as i32,
            pan: XMP_INST_NO_DEFAULT_PAN,
            sid: num as i32,
            xpo,
            fin,
            ..Default::default()
        }];
        if has {
            xxi.nsm = 1;
        }
    }

    m.module.xxt = vec![Track::default(); m.module.trk];
    m.module.xxp = vec![Pattern::default(); m.module.pat];

    f.seek(p_pat);
    for i in 0..m.module.pat {
        let mut len = f.read16l() as i32 - 4;
        let rows = f.read8() as usize;
        if rows > 64 {
            return Err(());
        }
        let chan = f.read8();
        if chan > 32 {
            return Err(());
        }

        if rows == 0 {
            return Err(());
        }
        m.module.alloc_pattern_tracks(i, rows);

        for r in 0..rows {
            while len > 0 {
                let b = f.read8();
                len -= 1;

                if b == 0 {
                    break;
                }

                let c = (b & 0x0f) as usize;
                if c >= m.module.chn {
                    return Err(());
                }
                let event = m.module.event_mut(i, c, r);

                if b & 0x80 != 0 {
                    event.note = f.read8().wrapping_add(36);
                    event.ins = f.read8();
                    len -= 2;
                }

                if b & 0x40 != 0 {
                    event.vol = f.read8().wrapping_add(1);
                    len -= 1;
                }

                if b & 0x20 != 0 {
                    let effect = f.read8();
                    let param = f.read8();
                    let mut param2 = 0;
                    let mut param3 = 0;

                    if effect == 40 {
                        // Sample Offset
                        param2 = f.read8();
                        param3 = f.read8();
                    }
                    translate_effect(event, effect, param, param2, param3);
                    len -= 2;
                }
            }
        }

        if len > 0 {
            f.seek_cur(len as i64);
        }
    }

    // Read samples
    for i in 0..stored_ins {
        let num = sample_map[i] as usize;

        // Don't load duplicate sample data
        if m.module.xxs[num].data.is_some() {
            continue;
        }

        f.seek(p_smp[i] as usize);
        load_sample(m, f, SAMPLE_FLAG_DIFF, num, None)?;
    }

    Ok(())
}
