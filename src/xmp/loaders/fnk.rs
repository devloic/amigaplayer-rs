//! Funktracker modules (`loaders/fnk_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::load::*;
use crate::xmp::module::*;

pub fn test(d: &[u8]) -> bool {
    if d.len() < 12 || &d[..4] != b"Funk" {
        return false;
    }
    let (a, b) = (d[5], d[6]);
    if (a >> 1) < 10 {
        return false; // creation year (-1980)
    }
    if msn(b) > 7 || lsn(b) > 9 {
        return false; // CPU and card
    }
    let size = u32::from_le_bytes([d[8], d[9], d[10], d[11]]) as i32;
    if size < 1024 {
        return false;
    }
    d.len() as i64 == size as i64
}

#[derive(Clone, Copy, Default)]
struct FnkInstrument {
    name: [u8; 19],
    loop_start: u32,
    length: u32,
    volume: u8,
    pan: u8,
}

fn translate_event(event: &mut Event, ev: &[u8; 3]) {
    let note = (ev[0] >> 2) as i32;
    // 0x3f - "null slot": load command only
    // 0x3e - "sample-only slot": load instrument and command only
    // 0x3d - "reload sample attrs": load instrument and command only
    if note < 0x3d {
        event.note = (37 + note) as u8;
    }
    if note < 0x3f {
        event.ins = (1 + msn(ev[1]) as i32 + (((ev[0] & 0x03) as i32) << 4)) as u8;
    }
    match lsn(ev[1]) {
        0x00 => (event.fxt, event.fxp) = (FX_PER_PORTA_UP, ev[2]),
        0x01 => (event.fxt, event.fxp) = (FX_PER_PORTA_DN, ev[2]),
        0x02 => (event.fxt, event.fxp) = (FX_PER_TPORTA, ev[2]),
        0x03 => (event.fxt, event.fxp) = (FX_PER_VIBRATO, ev[2]),
        0x06 => (event.fxt, event.fxp) = (FX_PER_VSLD_UP, ev[2] << 1),
        0x07 => (event.fxt, event.fxp) = (FX_PER_VSLD_DN, ev[2] << 1),
        0x0b => (event.fxt, event.fxp) = (FX_ARPEGGIO, ev[2]),
        0x0d => (event.fxt, event.fxp) = (FX_VOLSET, ev[2]),
        0x0e => {
            if ev[2] == 0x0a || ev[2] == 0x0b || ev[2] == 0x0c {
                event.fxt = FX_PER_CANCEL;
                return;
            }
            match msn(ev[2]) {
                0x1 => (event.fxt, event.fxp) = (FX_EXTENDED, (EX_CUT << 4) | lsn(ev[2])),
                0x2 => (event.fxt, event.fxp) = (FX_EXTENDED, (EX_DELAY << 4) | lsn(ev[2])),
                0xd => (event.fxt, event.fxp) = (FX_EXTENDED, (EX_RETRIG << 4) | lsn(ev[2])),
                0xe => (event.fxt, event.fxp) = (FX_SETPAN, 8u8.wrapping_add(lsn(ev[2]) << 4)),
                0xf => (event.fxt, event.fxp) = (FX_SPEED, lsn(ev[2])),
                _ => {}
            }
        }
        _ => {}
    }
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let mut allow_panning = true;

    let mut marker = [0u8; 4];
    f.read(&mut marker);
    let mut info = [0u8; 4];
    f.read(&mut info);
    let filesize = f.read32l();
    let mut fmt = [0u8; 4];
    f.read(&mut fmt);
    let _loop = f.read8();
    let mut order = [0u8; 256];
    f.read(&mut order);
    let mut pbrk = [0u8; 128];
    f.read(&mut pbrk);
    if pbrk.iter().any(|&b| b >= 64) {
        return Err(());
    }

    let mut fih = [FnkInstrument::default(); 64];
    for ins in fih.iter_mut() {
        f.read(&mut ins.name);
        ins.loop_start = f.read32l();
        ins.length = f.read32l();
        ins.volume = f.read8();
        ins.pan = f.read8();
        f.read8(); // shifter
        f.read8(); // waveform
        f.read8(); // retrig
        if ins.length >= filesize {
            return Err(());
        }
    }

    m.module.smp = 64;
    m.module.ins = 64;

    let mut pat = 0usize;
    let mut i = 0;
    while i < 256 && order[i] != 0xff {
        if order[i] as usize > pat {
            pat = order[i] as usize;
        }
        i += 1;
    }
    pat += 1;
    m.module.pat = pat;
    if pat > 128 {
        return Err(());
    }
    m.module.len = i;
    m.module.xxo[..i].copy_from_slice(&order[..i]);

    m.module.spd = 4;
    m.module.bpm = 125;
    m.module.chn = 0;

    // If an R1 fmt (funktype = Fk** or Fv**), then ignore byte 3.
    if fmt[0] == b'F' && fmt[1] == b'2' {
        if (((info[3] as i8) as i32) >> 1) & 0x40 != 0 {
            m.module.bpm -= ((info[3] >> 1) & 0x3f) as i32;
        } else {
            m.module.bpm += ((info[3] >> 1) & 0x3f) as i32;
        }
        m.module.type_ = "FunktrackerGOLD".into();
    } else if fmt[0] == b'F' && (fmt[1] == b'v' || fmt[1] == b'k') {
        m.module.type_ = "Funktracker".into();
        if fmt[1] == b'k' {
            allow_panning = false;
        }
    } else {
        m.module.chn = 8;
        allow_panning = false;
        m.module.type_ = "Funktracker DOS32".into();
    }

    if m.module.chn == 0 {
        m.module.chn = if !(b'0'..=b'9').contains(&fmt[2]) || !(b'0'..=b'9').contains(&fmt[3]) {
            8
        } else {
            (fmt[2] - b'0') as usize * 10 + (fmt[3] - b'0') as usize
        };
        if m.module.chn == 0 || m.module.chn > XMP_MAX_CHANNELS {
            return Err(());
        }
    }

    m.module.bpm = 4 * m.module.bpm / 5;
    m.module.trk = m.module.chn * m.module.pat;

    // FNK allows mode per instrument but we don't, so use linear for all
    m.period_type = PeriodType::Linear;

    m.init_instrument();
    for (i, h) in fih.iter().enumerate() {
        let xxs = &mut m.module.xxs[i];
        xxs.len = h.length as i32;
        xxs.lps = h.loop_start as i32;
        if xxs.lps == -1 {
            xxs.lps = 0;
        }
        xxs.lpe = h.length as i32;
        xxs.flg = if h.loop_start != 0xffff_ffff { XMP_SAMPLE_LOOP } else { 0 };
        let has = xxs.len > 0;
        let xxi = &mut m.module.xxi[i];
        xxi.sub = vec![SubInstrument { vol: h.volume as i32, pan: h.pan as i32, sid: i as i32, ..Default::default() }];
        if has {
            xxi.nsm = 1;
        }
        xxi.name = copy_adjust(&h.name, 19);
    }

    for i in 0..m.module.pat {
        m.module.alloc_pattern_tracks(i, 64);
        m.module.event_mut(i, 0, pbrk[i] as usize).f2t = FX_BREAK;
        for j in 0..64 {
            for k in 0..m.module.chn {
                let mut ev = [0u8; 3];
                if f.read(&mut ev) < 3 {
                    return Err(());
                }
                translate_event(m.module.event_mut(i, k, j), &ev);
            }
        }
    }

    for i in 0..m.module.ins {
        if m.module.xxs[i].len <= 2 {
            continue;
        }
        load_sample(m, f, 0, i, None)?;
    }

    for i in 0..m.module.chn {
        m.module.xxc[i].pan = if allow_panning { 0x80 } else { m.defpan(((i % 2) * 0xff) as i32) };
    }

    m.volbase = 0xff;
    m.quirk = QUIRK_VSALL;
    Ok(())
}
