//! Megatracker modules (`loaders/mgt_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::load::*;
use crate::xmp::module::*;

pub fn test(d: &[u8]) -> bool {
    // 24-bit "MGT", a version byte, then 0xbd "MCS"
    d.len() >= 8 && &d[..3] == b"MGT" && d[4..8] == [0xbd, b'M', b'C', b'S']
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    f.skip(3);
    let ver = f.read8();
    f.read32b();
    m.module.type_ = format!("Megatracker MGT v{}.{}", msn(ver), lsn(ver));

    m.module.chn = f.read16b() as usize;
    f.read16b();
    m.module.len = f.read16b() as usize;
    m.module.pat = f.read16b() as usize;
    m.module.trk = f.read16b() as usize;
    m.module.ins = f.read16b() as usize;
    m.module.smp = m.module.ins;
    f.read16b();
    f.read32b();

    if m.module.chn > XMP_MAX_CHANNELS || m.module.pat > 256 || m.module.ins > 64 {
        return Err(());
    }

    let sng_ptr = f.read32b() as usize;
    let _ = f.read32b();
    let ins_ptr = f.read32b() as usize;
    let pat_ptr = f.read32b() as usize;
    let trk_ptr = f.read32b() as usize;
    f.read32b();
    f.read32b();
    f.read32b();

    f.seek(sng_ptr);
    let mut name = [0u8; 32];
    f.read(&mut name);
    m.module.name = String::from_utf8_lossy(&name[..name.iter().position(|&b| b == 0).unwrap_or(32)]).into_owned();
    let seq_ptr = f.read32b() as usize;
    m.module.len = f.read16b() as usize;
    m.module.rst = f.read16b() as usize;
    m.module.bpm = f.read8() as i32;
    m.module.spd = f.read8() as i32;
    f.read16b();
    f.read8();
    f.read8();

    if m.module.len > XMP_MAX_MOD_LENGTH || m.module.rst > 255 {
        return Err(());
    }
    for _ in 0..m.module.chn {
        f.read16b();
    }
    m.c4rate = C4_NTSC_RATE;

    // sequence
    f.seek(seq_ptr);
    for i in 0..m.module.len {
        let pos = f.read16b() as usize;
        if pos >= m.module.pat {
            return Err(());
        }
        m.module.xxo[i] = pos as u8;
    }

    // instruments
    m.init_instrument();
    f.seek(ins_ptr);
    let mut sdata = [0usize; 64];
    for i in 0..m.module.ins {
        let mut iname = [0u8; 32];
        f.read(&mut iname);
        sdata[i] = f.read32b() as usize;
        let len = f.read32b();
        if len > MAX_SAMPLE_SIZE as u32 {
            return Err(());
        }
        let lps = f.read32b();
        let lpe = lps.wrapping_add(f.read32b());
        f.read32b();
        f.read32b();
        let c2spd = f.read32b();
        let (xpo, mut fin) = c2spd_to_note(c2spd as i32);
        let vol = (f.read16b() >> 4) as i32;
        f.read8();
        f.read8();
        let flags = f.read8();
        let mut flg = if flags & 0x03 != 0 { XMP_SAMPLE_LOOP } else { 0 };
        if flags & 0x02 != 0 {
            flg |= XMP_SAMPLE_LOOP_BIDIR;
        }
        fin += 0 * f.read8() as i32;
        f.read8();
        f.read8();
        f.read8();
        f.read8();
        f.read16b();
        f.read32b();
        f.read32b();

        let xxs = &mut m.module.xxs[i];
        xxs.len = len as i32;
        xxs.lps = lps as i32;
        xxs.lpe = lpe as i32;
        xxs.flg = flg;
        let xxi = &mut m.module.xxi[i];
        xxi.name = String::from_utf8_lossy(&iname[..iname.iter().position(|&b| b == 0).unwrap_or(32)]).into_owned();
        xxi.sub = vec![SubInstrument { vol, pan: XMP_INST_NO_DEFAULT_PAN, xpo, fin, sid: i as i32, ..Default::default() }];
        xxi.nsm = (len != 0) as i32;
    }

    // tracks
    m.module.xxt = vec![Track::default(); m.module.trk];
    m.module.xxp = vec![Pattern::default(); m.module.pat];
    if trk_ptr >= f.size() {
        return Err(());
    }
    for i in 1..m.module.trk {
        f.seek(trk_ptr + i * 4);
        let offset = f.read32b() as usize;
        f.seek(offset);
        let rows = f.read16b() as usize;
        if rows > 255 || rows == 0 {
            return Err(());
        }
        m.module.xxt[i] = Track::new(rows);

        let mut j = 0usize;
        while j < rows {
            let b = f.read8();
            j += (b & 0x03) as usize;
            if j >= rows {
                return Err(());
            }
            let mut note = 0u8;
            let mut ev = m.module.xxt[i].event[j];
            if b & 0x04 != 0 {
                note = f.read8();
            }
            if b & 0x08 != 0 {
                ev.ins = f.read8();
            }
            if b & 0x10 != 0 {
                ev.vol = f.read8();
            }
            if b & 0x20 != 0 {
                ev.fxt = f.read8();
            }
            if b & 0x40 != 0 {
                ev.fxp = f.read8();
            }
            if b & 0x80 != 0 {
                f.read8();
            }

            if note == 1 {
                ev.note = XMP_KEY_OFF;
            } else if note > 11 {
                // adjusted to play codeine.mgt
                ev.note = note.wrapping_add(1);
            }

            // effects
            if ev.fxt >= 0x10 {
                match ev.fxt {
                    0x13 | 0x14 | 0x15 | 0x17 | 0x1c | 0x1d | 0x1e => {
                        // libxmp assigns FX_EXTENDED first, then builds the
                        // parameter from it
                        ev.fxt = FX_EXTENDED;
                        ev.fxp = ((ev.fxt & 0x0f) << 4) | (ev.fxp & 0x0f);
                    }
                    _ => {
                        ev.fxt = 0;
                        ev.fxp = 0;
                    }
                }
            }

            // volume and volume column effects
            if ev.vol >= 0x10 && ev.vol <= 0x50 {
                ev.vol -= 0x0f;
                m.module.xxt[i].event[j] = ev;
                j += 1;
                continue;
            }
            match ev.vol >> 4 {
                0x06 => {
                    ev.f2t = FX_VOLSLIDE_2;
                    ev.f2p = ev.vol - 0x60;
                }
                0x07 => {
                    ev.f2t = FX_VOLSLIDE_2;
                    ev.f2p = (ev.vol - 0x70) << 4;
                }
                0x08 => {
                    ev.f2t = FX_EXTENDED;
                    ev.f2p = (EX_F_VSLIDE_DN << 4) | (ev.vol - 0x80);
                }
                0x09 => {
                    ev.f2t = FX_EXTENDED;
                    ev.f2p = (EX_F_VSLIDE_UP << 4) | (ev.vol - 0x90);
                }
                0x0a => {
                    ev.f2t = FX_VIBRATO;
                    ev.f2p = (ev.vol - 0xa0) << 4;
                }
                0x0b => {
                    ev.f2t = FX_VIBRATO;
                    ev.f2p = ev.vol - 0xb0;
                }
                0x0c => {
                    ev.f2t = FX_SETPAN;
                    ev.f2p = ((ev.vol - 0xc0) << 4).wrapping_add(8);
                }
                0x0d => {
                    ev.f2t = FX_PANSLIDE;
                    ev.f2p = (ev.vol - 0xd0) << 4;
                }
                0x0e => {
                    ev.f2t = FX_PANSLIDE;
                    ev.f2p = ev.vol - 0xe0;
                }
                0x0f => {
                    ev.f2t = FX_TONEPORTA;
                    ev.f2p = (ev.vol - 0xf0) << 4;
                }
                _ => {}
            }
            ev.vol = 0;
            m.module.xxt[i].event[j] = ev;
            j += 1;
        }
    }

    // extra track
    if m.module.trk > 0 {
        m.module.xxt[0] = Track::new(64);
    }

    // patterns
    f.seek(pat_ptr);
    for i in 0..m.module.pat {
        let rows = f.read16b() as usize;
        if rows > 256 {
            return Err(());
        }
        let mut index = vec![0usize; m.module.chn];
        for ix in index.iter_mut() {
            let track = f.read16b() as i32 - 1;
            if track >= m.module.trk as i32 {
                return Err(());
            }
            // a negative track fails the epilogue's check
            *ix = if track < 0 { usize::MAX } else { track as usize };
        }
        m.module.xxp[i] = Pattern { rows, index };
    }

    // samples
    for i in 0..m.module.ins {
        if m.module.xxi[i].nsm == 0 {
            continue;
        }
        f.seek(sdata[i]);
        load_sample(m, f, 0, i, None)?;
    }
    Ok(())
}
