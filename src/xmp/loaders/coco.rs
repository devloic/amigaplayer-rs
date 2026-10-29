//! Coconizer modules (`loaders/coco_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::load::*;
use crate::xmp::module::*;
use crate::xmp::voltable::ARCH_VOL_TABLE;

pub fn test(d: &[u8]) -> bool {
    let mut f = Hio::new(d);
    let x = f.read8();
    if x != 0x84 && x != 0x88 {
        return false; // number of channels
    }
    let mut buf = [0u8; 20];
    if f.read(&mut buf) != 20 {
        return false; // title
    }
    if !buf.contains(&0x0d) {
        return false;
    }
    let n = f.read8() as i32;
    if n <= 0 || n > 100 {
        return false;
    }
    f.read8(); // sequences
    f.read8(); // patterns
    let y = f.read32l();
    if y < 64 || y > 0x0010_0000 {
        return false;
    }
    let y = f.read32l();
    if y < 64 || y > 0x0010_0000 {
        return false;
    }
    for _ in 0..n {
        let ofs = f.read32l() as i32;
        let len = f.read32l() as i32;
        let vol = f.read32l() as i32;
        let lps = f.read32l() as i32;
        let lsz = f.read32l() as i32;
        if ofs < 64 || ofs > 0x0010_0000 {
            return false;
        }
        if vol < 0 || vol > 0xff {
            return false;
        }
        if len < 0 || lps < 0 || lsz < 0 {
            return false;
        }
        if len > 0x0010_0000 || lps > 0x0010_0000 || lsz > 0x0010_0000 {
            return false;
        }
        if lps > 0 && lps + lsz - 1 > len {
            return false;
        }
        let mut b = [0u8; 11];
        f.read(&mut b);
        f.read8(); // unused
    }
    true
}

fn fix_effect(e: &mut Event) {
    match e.fxt {
        0x00 => e.fxt = FX_ARPEGGIO,
        0x01 | 0x05 => e.fxt = FX_PORTA_UP,
        0x02 | 0x06 => e.fxt = FX_PORTA_DN,
        0x03 => e.fxt = FX_F_VSLIDE_UP,
        0x04 => e.fxt = FX_F_VSLIDE_DN,
        0x07 => {
            // y: stereo position (1-7, ignored). 1=left 4=center 7=right
            if e.fxp > 0 && e.fxp < 8 {
                e.fxt = FX_SETPAN;
                e.fxp = (42 * e.fxp as i32 - 40) as u8;
            } else {
                e.fxt = 0;
                e.fxp = 0;
            }
        }
        0x08..=0x0b => {
            e.fxt = 0;
            e.fxp = 0; // FIXME
        }
        0x0c => {
            e.fxt = FX_VOLSET;
            e.fxp = 0xffu8.wrapping_sub(e.fxp);
        }
        0x0d => e.fxt = FX_BREAK,
        0x0e => e.fxt = FX_JUMP,
        0x0f => e.fxt = FX_SPEED,
        0x10 => {
            e.fxt = 0;
            e.fxp = 0;
        }
        0x11 | 0x12 => {
            e.fxt = 0;
            e.fxp = 0; // FIXME
        }
        0x13 => e.fxt = FX_VOLSLIDE_UP,
        0x14 => e.fxt = FX_VOLSLIDE_DN,
        _ => {
            e.fxt = 0;
            e.fxp = 0;
        }
    }
}

/// `hio_seek(f, pos, SEEK_SET)` with a possibly negative position: an error, and
/// the position left where it was.
fn seek_i(f: &mut Hio, pos: i64) {
    if pos < 0 {
        f.error = true;
    } else {
        f.seek(pos as usize);
    }
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    m.module.chn = (f.read8() & 0x3f) as usize;
    let mut name = [0u8; 20];
    f.read(&mut name);
    m.module.name = copy_adjust(&name, 20);
    m.module.type_ = "Coconizer".into();

    m.module.ins = f.read8() as usize;
    m.module.smp = m.module.ins;
    m.module.len = f.read8() as usize;
    m.module.pat = f.read8() as usize;
    m.module.trk = m.module.pat * m.module.chn;

    let seq_ptr = f.read32l() as i32;
    let pat_ptr = f.read32l() as i32;
    if f.take_error() {
        return Err(());
    }

    m.init_instrument();
    m.vol_table = Some(&ARCH_VOL_TABLE);
    m.volbase = 0xff;

    let mut smp_ptr = [0i32; 100];
    for i in 0..m.module.ins {
        smp_ptr[i] = f.read32l() as i32;
        let len = f.read32l() as i32;
        let vol = 0xffu32.wrapping_sub(f.read32l()) as i32;
        let lps = f.read32l() as i32;
        let mut lpe = lps.wrapping_add(f.read32l() as i32);
        if lpe != 0 {
            lpe = lpe.wrapping_sub(1);
        }
        let mut iname = [0u8; 11];
        f.read(&mut iname);
        f.read8(); // unused
        let xxs = &mut m.module.xxs[i];
        xxs.len = len;
        xxs.lps = lps;
        xxs.lpe = lpe;
        xxs.flg = if lps > 0 { XMP_SAMPLE_LOOP } else { 0 };
        let xxi = &mut m.module.xxi[i];
        xxi.sub = vec![SubInstrument { vol, pan: XMP_INST_NO_DEFAULT_PAN, sid: i as i32, ..Default::default() }];
        xxi.name = copy_adjust(&iname, 11);
        if len > 0 {
            xxi.nsm = 1;
        }
        if f.take_error() {
            return Err(());
        }
    }

    // Sequence
    seek_i(f, seq_ptr as i64);
    let mut i = 0;
    loop {
        let x = f.read8();
        if x == 0xff {
            break;
        }
        if i < m.module.len {
            m.module.xxo[i] = x;
        }
        i += 1;
    }

    // Patterns
    seek_i(f, pat_ptr as i64);
    if f.take_error() {
        return Err(());
    }
    for i in 0..m.module.pat {
        m.module.alloc_pattern_tracks(i, 64);
        for j in 0..64 {
            for k in 0..m.module.chn {
                let mut b = [0u8; 4];
                for x in b.iter_mut() {
                    *x = f.read8();
                }
                let event = m.module.event_mut(i, k, j);
                event.fxp = b[0];
                event.fxt = b[1];
                event.ins = b[2];
                event.note = b[3];
                if event.note != 0 {
                    event.note = event.note.wrapping_add(12);
                }
                if f.take_error() {
                    return Err(());
                }
                fix_effect(event);
            }
        }
    }

    // Samples
    for i in 0..m.module.ins {
        if m.module.xxi[i].nsm == 0 {
            continue;
        }
        seek_i(f, smp_ptr[i] as i64);
        load_sample(m, f, SAMPLE_FLAG_VIDC, i, None)?;
    }

    for i in 0..m.module.chn {
        m.module.xxc[i].pan = m.defpan((((i as i32 + 3) / 2) % 2) * 0xff);
    }
    Ok(())
}
