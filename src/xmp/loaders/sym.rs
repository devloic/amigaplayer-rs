//! Digital Symphony modules (`loaders/sym_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::load::*;
use crate::xmp::lzw::*;
use crate::xmp::module::*;

pub fn test(d: &[u8]) -> bool {
    let mut f = Hio::new(d);
    let a = f.read32b();
    let b = f.read32b();
    if a != 0x02011313 || b != 0x1412010B {
        return false; // BASSTRAK
    }
    // v1 files are the same as v0 but may contain strange compression formats
    f.read8() <= 1
}

fn fix_effect(e: &mut Event, parm: i32) {
    match e.fxt {
        // 00 xyz Normal play or Arpeggio + Volume Slide Up
        // 01 xyy Slide Up + Volume Slide Up
        // 02 xyy Slide Down + Volume Slide Up
        0x00 | 0x01 | 0x02 => {
            if parm & 0xff != 0 {
                e.fxp = (parm & 0xff) as u8;
            } else {
                e.fxt = 0;
            }
            if parm >> 8 != 0 {
                e.f2t = FX_VOLSLIDE_UP;
                e.f2p = (parm >> 8) as u8;
            }
        }
        // 03 xyy Tone Portamento, 04 xyz Vibrato, 07 xyz Tremolo
        0x03 | 0x04 | 0x07 => e.fxp = parm as u8,
        // 05 xyz Tone Portamento + Volume Slide, 06 xyz Vibrato + Volume Slide
        0x05 | 0x06 => {
            e.fxp = parm as u8;
            if parm == 0 {
                e.fxt -= 2;
            }
        }
        // 09 xxx Set Sample Offset
        0x09 => {
            e.fxp = (parm >> 1) as u8;
            e.f2t = FX_HIOFFSET;
            e.f2p = (parm >> 9) as u8;
        }
        // 0A xyz Volume Slide + Fine Slide Up
        0x0a => {
            if parm & 0xff != 0 {
                e.fxp = (parm & 0xff) as u8;
            } else {
                e.fxt = 0;
            }
            e.f2t = FX_EXTENDED;
            e.f2p = (EX_F_PORTA_UP << 4) | ((parm & 0xf00) >> 8) as u8;
        }
        // 0B xxx Position Jump, 0C xyy Set Volume
        0x0b | 0x0c => e.fxp = parm as u8,
        // 0D xyy Pattern Break
        0x0d => {
            e.fxt = FX_IT_BREAK;
            e.fxp = if (parm & 0xff) < 0x40 { parm as u8 } else { 0 };
        }
        // 0F xxx Set Speed
        0x0f => {
            if parm != 0 {
                e.fxt = FX_S3M_SPEED;
                e.fxp = parm.min(255) as u8;
            } else {
                e.fxt = 0;
            }
        }
        // 13 xxy Glissando Control
        0x13 => {
            e.fxt = FX_EXTENDED;
            e.fxp = (EX_GLISS << 4) | (parm & 0x0f) as u8;
        }
        // 14 xxy Set Vibrato Waveform
        0x14 => {
            e.fxt = FX_EXTENDED;
            e.fxp = (EX_VIBRATO_WF << 4) | (parm & 0x0f) as u8;
        }
        // 15 xxy Set Fine Tune
        0x15 => {
            e.fxt = FX_EXTENDED;
            e.fxp = (EX_FINETUNE << 4) | (parm & 0x0f) as u8;
        }
        // 16 xxx Jump to Loop
        0x16 => {
            e.fxt = FX_EXTENDED;
            e.fxp = (EX_PATTERN_LOOP << 4) | parm.min(0x0f) as u8;
        }
        // 17 xxy Set Tremolo Waveform
        0x17 => {
            e.fxt = FX_EXTENDED;
            e.fxp = (EX_TREMOLO_WF << 4) | (parm & 0x0f) as u8;
        }
        // 19 xxx Retrig Note
        0x19 => {
            if parm < 0x100 {
                e.fxt = FX_RETRIG;
                e.fxp = (parm & 0xff) as u8;
            } else {
                e.fxt = 0;
            }
        }
        // 11 xyy Fine Slide Up + Fine Volume Slide Up, 12 Fine Slide Down + Fine
        // Volume Slide Up, 1A Fine Slide Up + Fine Volume Slide Down, 1B Fine
        // Slide Down + Fine Volume Slide Down
        0x11 | 0x12 | 0x1a | 0x1b => {
            let pitch_effect = if e.fxt == 0x11 || e.fxt == 0x1a { FX_F_PORTA_UP } else { FX_F_PORTA_DN };
            let vol_effect = if e.fxt == 0x11 || e.fxt == 0x12 { FX_F_VSLIDE_UP } else { FX_F_VSLIDE_DN };
            if parm & 0xff != 0 {
                e.fxt = pitch_effect;
                e.fxp = (parm & 0xff) as u8;
            } else {
                e.fxt = 0;
            }
            if parm >> 8 != 0 {
                e.f2t = vol_effect;
                e.f2p = (parm >> 8) as u8;
            }
        }
        // 1C xxx Note Cut
        0x1c => {
            e.fxt = FX_EXTENDED;
            e.fxp = (EX_CUT << 4) | parm.min(0x0f) as u8;
        }
        // 1D xxx Note Delay
        0x1d => {
            e.fxt = FX_EXTENDED;
            e.fxp = (EX_DELAY << 4) | parm.min(0x0f) as u8;
        }
        // 1E xxx Pattern Delay
        0x1e => {
            e.fxt = FX_EXTENDED;
            e.fxp = (EX_PATT_DELAY << 4) | parm.min(0x0f) as u8;
        }
        // 1F xxy Invert Loop
        0x1f => {
            e.fxt = FX_EXTENDED;
            e.fxp = (EX_INVLOOP << 4) | (parm & 0xf) as u8;
        }
        // 20 xyz Normal play or Arpeggio + Volume Slide Down
        0x20 => {
            e.fxt = FX_ARPEGGIO;
            e.fxp = (parm & 0xff) as u8;
            if parm >> 8 != 0 {
                e.f2t = FX_VOLSLIDE_DN;
                e.f2p = (parm >> 8) as u8;
            }
        }
        // 21 xyy Slide Up + Volume Slide Down, 22 Slide Down + Volume Slide Down
        0x21 | 0x22 => {
            if parm & 0xff != 0 {
                e.fxt = if e.fxt == 0x21 { FX_PORTA_UP } else { FX_PORTA_DN };
                e.fxp = (parm & 0xff) as u8;
            } else {
                e.fxt = 0;
            }
            if parm >> 8 != 0 {
                e.f2t = FX_VOLSLIDE_DN;
                e.f2p = (parm >> 8) as u8;
            }
        }
        // 2A xyz Volume Slide + Fine Slide Down
        0x2a => {
            if parm & 0xff != 0 {
                e.fxt = FX_VOLSLIDE;
                e.fxp = (parm & 0xff) as u8;
            } else {
                e.fxt = 0;
            }
            e.f2t = FX_EXTENDED;
            e.f2p = (EX_F_PORTA_DN << 4) | (parm >> 8) as u8;
        }
        // 2B xyy Line Jump
        0x2b => {
            e.fxt = FX_LINE_JUMP;
            e.fxp = if parm < 0x40 { parm as u8 } else { 0 };
        }
        // 2F xxx Set Tempo
        0x2f => {
            if parm != 0 {
                let p = ((parm + 4) >> 3).clamp(XMP_MIN_BPM, 255); // round to nearest
                e.fxt = FX_S3M_BPM;
                e.fxp = p as u8;
            } else {
                e.fxt = 0;
            }
        }
        // 30 xxy Set Stereo
        0x30 => {
            e.fxt = FX_SETPAN;
            if parm & 7 != 0 {
                // !Tracker-style panning: 1=left, 4=center, 7=right
                if parm & 8 == 0 {
                    e.fxp = (42 * ((parm & 7) - 1) + 2) as u8;
                } else {
                    e.fxt = 0;
                }
            } else {
                let p = parm >> 4;
                if p < 128 {
                    e.fxp = (p + 128) as u8;
                } else if p > 128 {
                    e.fxp = (255 - p) as u8;
                } else {
                    e.fxt = 0;
                }
            }
        }
        // 31 xxx Song Upcall
        0x31 => e.fxt = 0,
        // 32 xxx Unset Sample Repeat
        0x32 => {
            e.fxt = FX_KEYOFF;
            e.fxp = 0;
        }
        _ => e.fxt = 0,
    }
}

fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]).into_owned()
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    f.skip(8); // BASSTRAK
    f.read8(); // version
    m.module.type_ = "Digital Symphony".to_string();

    m.module.chn = f.read8() as usize;
    let pat = f.read16l() as usize;
    m.module.len = pat;
    m.module.pat = pat;

    // sanity check
    if m.module.chn < 1 || m.module.chn > 8 || pat > XMP_MAX_MOD_LENGTH {
        return Err(());
    }

    m.module.trk = f.read16l() as usize; // Symphony patterns are actually tracks
    let infolen = f.read24l() as i32;

    // sanity check: track 0x1000 is used to indicate the empty track
    if m.module.trk > 0x1000 {
        return Err(());
    }

    m.module.ins = 63;
    m.module.smp = 63;
    m.init_instrument();

    let mut sn = [0i32; 64];
    let mut max_sample_size: i32 = 1;
    for i in 0..63 {
        m.module.xxi[i].sub = vec![SubInstrument::default()];
        sn[i] = f.read8() as i32; // sample name length
        if !sn[i] & 0x80 != 0 {
            let len = (f.read24l() << 1) as i32;
            m.module.xxs[i].len = len;
            m.module.xxi[i].nsm = 1;
            // sanity check
            if len > 0x80000 {
                return Err(());
            }
            if max_sample_size < len {
                max_sample_size = len;
            }
        }
    }

    let a = f.read8() as usize; // track name length
    let mut name = [0u8; 64];
    if a > 32 {
        f.read(&mut name[..32]);
        f.skip(a - 32);
    } else {
        f.read(&mut name[..a]);
    }
    m.module.name = cstr(&name);

    let mut allowed_effects = [0u8; 8];
    f.read(&mut allowed_effects);

    m.module.trk += 1; // alloc extra empty track
    let (chn, len, trk) = (m.module.chn, m.module.len, m.module.trk);
    m.module.xxp = vec![Pattern::default(); m.module.pat];
    m.module.xxt = vec![Track::default(); trk];

    // the required size of the temporary buffer
    let sequence_size = len * chn * 2; // uncompressed sequence size
    if sequence_size as i32 > max_sample_size {
        max_sample_size = sequence_size as i32;
    }
    let tracks_size = 64 * (trk - 1) * 4; // uncompressed tracks size
    if tracks_size as i32 > max_sample_size {
        max_sample_size = tracks_size as i32;
    }
    let mut buf = vec![0u8; max_sample_size as usize];

    // sequence
    let a = f.read8();
    if a != 0 && a != 1 {
        return Err(());
    }
    if a != 0 {
        read_lzw(&mut buf[..sequence_size], sequence_size, LZW_FLAGS_SYM, f)?;
    } else if f.read(&mut buf[..sequence_size]) != sequence_size {
        return Err(());
    }

    for i in 0..len {
        // len == pat
        let mut index = vec![0usize; chn];
        for j in 0..chn {
            let idx = 2 * (i * chn + j);
            let mut t = buf[idx] as usize | (buf[idx + 1] as usize) << 8;
            if t == 0x1000 {
                t = trk - 1; // empty trk
            } else if t >= trk - 1 {
                return Err(());
            }
            index[j] = t;
        }
        m.module.xxp[i] = Pattern { rows: 64, index };
        m.module.xxo[i] = i as u8;
    }

    // read and convert patterns: stored in blocks of up to 2000 patterns, read in
    // multiple passes when there are more
    let mut i = 0;
    while i < tracks_size {
        let blk_size = (tracks_size - i).min(4 * 64 * 2000);
        let a = f.read8();
        if a != 0 && a != 1 {
            return Err(());
        }
        if a != 0 {
            read_lzw(&mut buf[i..i + blk_size], blk_size, LZW_FLAGS_SYM, f)?;
        } else if f.read(&mut buf[i..i + blk_size]) != blk_size {
            return Err(());
        }
        i += 4 * 64 * 2000;
    }

    for i in 0..trk - 1 {
        let mut t = Track::new(64);
        for j in 0..64 {
            let event = &mut t.event[j];
            let o = 4 * (i * 64 + j);
            let b = u32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
            event.note = (b & 0x3f) as u8;
            if event.note != 0 {
                event.note += 48;
            }
            event.ins = ((b & 0x1fc0) >> 6) as u8;
            event.fxt = ((b & 0xfc000) >> 14) as u8;
            let parm = ((b & 0xfff00000) >> 20) as i32;
            if allowed_effects[(event.fxt >> 3) as usize] & (1 << (event.fxt & 7)) != 0 {
                fix_effect(event, parm);
            } else {
                event.fxt = 0;
            }
        }
        m.module.xxt[i] = t;
    }

    // extra track
    m.module.xxt[trk - 1] = Track::new(64);

    // load and convert instruments
    for i in 0..63 {
        let mut namebuf = [0u8; 128];
        f.read(&mut namebuf[..(sn[i] & 0x7f) as usize]);
        m.module.xxi[i].name = copy_adjust(&namebuf, 31);

        if !sn[i] & 0x80 != 0 {
            let sus = (f.read24l() << 1) as i32;
            let looplen = (f.read24l() << 1) as i32;
            let sue = sus.wrapping_add(looplen);
            m.xtra[i].sus = sus;
            m.xtra[i].sue = sue;
            let xxs = &mut m.module.xxs[i];
            if sus < xxs.len && sus < sue && sue <= xxs.len && looplen > 2 {
                xxs.flg |= XMP_SAMPLE_SLOOP;
            }
            let xxi = &mut m.module.xxi[i];
            xxi.sub[0].vol = f.read8() as i32;
            xxi.sub[0].pan = XMP_INST_NO_DEFAULT_PAN;
            // finetune adjusted comparing DSym and S3M versions of "inside out"
            xxi.sub[0].fin = (f.read8() << 4) as i8 as i32;
            xxi.sub[0].sid = i as i32;
        }

        if sn[i] & 0x80 != 0 || m.module.xxs[i].len == 0 {
            continue;
        }

        let a = f.read8();
        let size = m.module.xxs[i].len as usize;
        match a {
            // signed 8-bit, logarithmic
            0 => load_sample(m, f, SAMPLE_FLAG_VIDC, i, None)?,
            // LZW compressed signed 8-bit delta, linear
            1 => {
                read_lzw(&mut buf[..size], size, LZW_FLAGS_SYM, f)?;
                load_sample(m, f, SAMPLE_FLAG_NOLOAD | SAMPLE_FLAG_DIFF, i, Some(&buf))?;
            }
            // signed 8-bit, linear
            2 => load_sample(m, f, 0, i, None)?,
            // signed 16-bit, linear
            3 => {
                m.module.xxs[i].flg |= XMP_SAMPLE_16BIT;
                load_sample(m, f, 0, i, None)?;
            }
            // sigma-delta compressed unsigned 8-bit, linear
            4 => {
                read_sigma_delta(&mut buf[..size], size, f)?;
                load_sample(m, f, SAMPLE_FLAG_NOLOAD | SAMPLE_FLAG_UNS, i, Some(&buf))?;
            }
            // sigma-delta compressed signed 8-bit, logarithmic
            5 => {
                read_sigma_delta(&mut buf[..size], size, f)?;
                // this uses a bit packing that isn't either mu-law or normal
                // Archimedes VIDC: convert to the latter
                for b in buf[..size].iter_mut() {
                    let t = if *b < 128 { !*b } else { *b };
                    *b = (*b >> 7) | (t << 1);
                }
                load_sample(m, f, SAMPLE_FLAG_NOLOAD | SAMPLE_FLAG_VIDC, i, Some(&buf))?;
            }
            _ => return Err(()),
        }
    }

    // information text
    if infolen > 0 {
        let a = f.read8(); // packing
        let n = infolen as usize;
        let mut c = vec![0u8; n];
        let ok = if a != 0 {
            read_lzw(&mut c, n, LZW_FLAGS_SYM, f).is_ok()
        } else {
            f.read(&mut c) >= n
        };
        if ok {
            m.comment = cstr(&c);
        }
    }

    for i in 0..chn {
        m.module.xxc[i].pan = m.defpan((((i as i32 + 3) / 2) % 2) * 0xff);
    }

    m.quirk = QUIRK_VIBALL | QUIRK_KEYOFF | QUIRK_INVLOOP;
    Ok(())
}
