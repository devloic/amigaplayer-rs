//! Astroidea XMF modules, used by Imperium Galactica (`loaders/xmf_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::load::*;
use crate::xmp::module::*;

const XMF_SAMPLE_ARRAY_SIZE: usize = 16 * 256;

fn mem24l(p: &[u8]) -> u32 {
    p[0] as u32 | (p[1] as u32) << 8 | (p[2] as u32) << 16
}

fn mem16l(p: &[u8]) -> u32 {
    p[0] as u32 | (p[1] as u32) << 8
}

pub fn test(d: &[u8]) -> bool {
    let mut f = Hio::new(d);
    // 0x03 for all Imperium Galactica modules, 0x04 for the demo "Prostate 666"
    // and all other XMFs.
    let xmf_type = f.read8();
    if xmf_type != 0x03 && xmf_type != 0x04 {
        return false;
    }
    let mut buf = vec![0u8; XMF_SAMPLE_ARRAY_SIZE];
    if f.read(&mut buf) < XMF_SAMPLE_ARRAY_SIZE {
        return false;
    }

    let mut samples_length: u32 = 0;
    let mut num_ins = 0;
    for i in 0..256 {
        let pos = &buf[i * 16..];
        let loopstart = mem24l(&pos[0..]);
        let loopend = mem24l(&pos[3..]);
        let datastart = mem24l(&pos[6..]);
        let dataend = mem24l(&pos[9..]);
        let flags = pos[13];
        let srate = mem16l(&pos[14..]);
        let len = dataend.wrapping_sub(datastart);

        if flags & !(0x04 | 0x08 | 0x10) != 0 {
            return false;
        }
        // if ping-pong loop flag is enabled, normal loop flag should be enabled too
        if flags & (0x08 | 0x10) == 0x10 {
            return false;
        }
        // if loop flag is enabled, the loop should have a valid end point
        if flags & 0x08 != 0 && loopend == 0 {
            return false;
        }
        // a 16-bit sample should have an even number of bytes
        if flags & 0x04 != 0 && len & 1 != 0 {
            return false;
        }
        // a valid sample should have a somewhat realistic middle-c frequency
        if len != 0 && srate < 100 {
            return false;
        }
        if datastart > dataend {
            return false;
        }
        samples_length = samples_length.wrapping_add(len);
        // All known XMFs have well-formed loops.
        if loopend != 0 && (loopstart >= len || loopend > len || loopstart > loopend) {
            return false;
        }
        if len > 0 {
            num_ins = i + 1;
        }
    }
    if num_ins > MAX_INSTRUMENTS {
        return false;
    }

    // Get pattern data size.
    let mut b2 = [0u8; 258];
    if f.read(&mut b2) < 258 {
        return false;
    }
    let num_channels = b2[256] as i64 + 1;
    let num_patterns = b2[257] as i64 + 1;
    if num_channels > XMP_MAX_CHANNELS as i64 {
        return false;
    }

    // Test total module length
    let samples_start = 0x1103 + num_channels + num_patterns * num_channels * 64 * 6;
    let length = d.len() as i64;
    if length < samples_start || (length - samples_start) < samples_length as i64 {
        return false;
    }
    true
}

// TODO: command pages would be nice, but no official modules rely on 5xy/6xy.
fn insert_effect(event: &mut Event, fxt: u8, fxp: u8, chn: usize) {
    if chn == 0 {
        event.fxt = fxt;
        event.fxp = fxp;
    } else {
        event.f2t = fxt;
        event.f2p = fxp;
    }
}

fn translate_effect(event: &mut Event, effect: u8, mut param: u8, chn: usize) {
    // Most effects are Protracker compatible. Only the effects actually
    // implemented by Imperium Galactica are handled here.
    match effect {
        0x00 | 0x01 | 0x02 | 0x0f => {
            if param != 0 {
                insert_effect(event, effect, param, chn);
            }
        }
        0x03 => insert_effect(event, FX_ULT_TPORTA, param, chn),
        0x04 | 0x0c | 0x0d => insert_effect(event, effect, param, chn),
        0x05 | 0x06 | 0x0a => {
            if effect == 0x05 {
                insert_effect(event, FX_ULT_TPORTA, 0, chn ^ 1);
            }
            if effect == 0x06 {
                insert_effect(event, FX_VIBRATO, 0, chn ^ 1);
            }
            if param & 0x0f != 0 {
                // down takes precedence and uses the full param.
                insert_effect(event, FX_VOLSLIDE_DN, param << 2, chn);
            } else if param & 0xf0 != 0 {
                insert_effect(event, FX_VOLSLIDE_UP, param >> 2, chn);
            }
        }
        0x0b => {
            if param < 255 {
                insert_effect(event, FX_JUMP, param + 1, chn);
            }
        }
        0x0e => match param >> 4 {
            0x01 | 0x02 | 0x09 | 0x0c | 0x0d | 0x0e => {
                if param & 0x0f != 0 {
                    insert_effect(event, effect, param, chn);
                }
            }
            0x04 => {
                param &= 3;
                param = if param < 3 { param } else { 2 };
                insert_effect(event, effect, param, chn);
            }
            0x06 => insert_effect(event, effect, param, chn),
            0x0a => {
                if param & 0x0f != 0 {
                    insert_effect(event, FX_F_VSLIDE_UP, (param & 0x0f) << 2, chn);
                }
            }
            0x0b => {
                if param & 0x0f != 0 {
                    insert_effect(event, FX_F_VSLIDE_DN, (param & 0x0f) << 2, chn);
                }
            }
            _ => {}
        },
        0x10 => {
            param &= 0x0f;
            param |= param << 4;
            insert_effect(event, FX_SETPAN, param, chn);
        }
        0x11 => insert_effect(event, FX_EXTENDED, (EX_RETRIG << 4) | (param & 0x0f), chn),
        _ => {}
    }
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let xmf_type = f.read8();
    m.module.type_ = if xmf_type == 0x03 { "Imperium Galactica XMF" } else { "Astroidea XMF" }.into();

    let mut buf = vec![0u8; XMF_SAMPLE_ARRAY_SIZE];
    if f.read(&mut buf) < XMF_SAMPLE_ARRAY_SIZE {
        return Err(());
    }

    // Count instruments
    let mut ins = 0;
    for i in 0..256 {
        let pos = &buf[i * 16..];
        if mem24l(&pos[9..]) > mem24l(&pos[6..]) {
            ins = i;
        }
    }
    ins += 1;
    m.module.ins = ins;
    m.module.smp = ins;
    m.init_instrument();

    // Instruments
    for i in 0..ins {
        let pos = &buf[i * 16..];
        let len = mem24l(&pos[9..]).wrapping_sub(mem24l(&pos[6..])) as i32;
        let xxs = &mut m.module.xxs[i];
        xxs.len = len;
        xxs.lps = mem24l(&pos[0..]) as i32;
        xxs.lpe = mem24l(&pos[3..]) as i32;
        m.xtra[i].c5spd = mem16l(&pos[14..]) as f64;
        // The Sound Blaster driver will only loop if both the loop start and
        // loop end are non-zero; 16-bit samples are loaded as 8-bit by the GUS.
        if pos[13] & 0x04 != 0 {
            xxs.flg |= XMP_SAMPLE_16BIT;
            xxs.len >>= 1;
        }
        if pos[13] & 0x08 != 0 {
            xxs.flg |= XMP_SAMPLE_LOOP;
        }
        if pos[13] & 0x10 != 0 {
            xxs.flg |= XMP_SAMPLE_LOOP_BIDIR;
        }
        let has = xxs.len > 0;
        let xxi = &mut m.module.xxi[i];
        xxi.sub = vec![SubInstrument { vol: pos[12] as i32, pan: XMP_INST_NO_DEFAULT_PAN, sid: i as i32, ..Default::default() }];
        if has {
            xxi.nsm = 1;
        }
    }

    // Sequence
    if f.read(&mut m.module.xxo) < 256 {
        return Err(());
    }
    m.module.chn = f.read8() as usize + 1;
    m.module.pat = f.read8() as usize + 1;
    m.module.trk = m.module.chn * m.module.pat;
    let mut len = 0;
    while len < 256 && m.module.xxo[len] != 0xff {
        len += 1;
    }
    m.module.len = len;

    // Panning table (supported by the Gravis UltraSound driver only)
    let chn = m.module.chn;
    let mut pan = vec![0u8; chn];
    if f.read(&mut pan) < chn {
        return Err(());
    }
    for i in 0..chn {
        let p = 0x80 + (pan[i] as i32 - 7) * 16;
        m.module.xxc[i].pan = p.min(255);
    }

    let pat_sz = chn * 6 * 64;
    let mut pbuf = vec![0u8; pat_sz];
    for i in 0..m.module.pat {
        m.module.alloc_pattern_tracks(i, 64);
        if f.read(&mut pbuf) < pat_sz {
            return Err(());
        }
        let mut p = 0;
        for j in 0..64 {
            for k in 0..chn {
                let pos = &pbuf[p..p + 6];
                let event = m.module.event_mut(i, k, j);
                if pos[0] > 0 {
                    event.note = pos[0].wrapping_add(36);
                }
                event.ins = pos[1];
                translate_effect(event, pos[2], pos[5], 0);
                translate_effect(event, pos[3], pos[4], 1);
                p += 6;
            }
        }
    }

    // Despite the GUS sample start and end pointers saved in the file, the
    // samples are loaded sequentially.
    for i in 0..ins {
        load_sample(m, f, 0, i, None)?;
    }

    // With the Sound Blaster driver, full volume samples have a -0dB mix; use a
    // slightly lower level (x3 mvolbase) for the Imperium Galactica tracks.
    m.volbase = 0xff;
    m.mvolbase = 48;
    m.mvol = if xmf_type == 0x03 { m.mvolbase * 3 } else { m.mvolbase };
    Ok(())
}
