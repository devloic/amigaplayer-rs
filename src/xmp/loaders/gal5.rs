//! Galaxy Music System 5.0 modules, the Jazz Jackrabbit 2 J2B format
//! (`loaders/gal5_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::iff::{self, IffLoader, IFF_CHUNK_ALIGN2, IFF_LITTLE_ENDIAN, IFF_SKIP_EMBEDDED};
use crate::xmp::load::*;
use crate::xmp::module::*;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 16 && &d[0..4] == b"RIFF" && &d[8..12] == b"AM  " && &d[12..16] == b"INIT"
}

struct Local {
    chn_pan: [u8; 64],
}

fn cstr(b: &[u8]) -> String {
    b.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect()
}

fn get_init(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    let mut buf = [0u8; 64];
    if f.read(&mut buf) < 64 {
        return Err(());
    }
    m.module.name = cstr(&buf[..63]);
    m.module.type_ = "Galaxy Music System 5.0".to_string();
    let flags = f.read8(); // bit 0: Amiga period
    if flags & 0x01 == 0 {
        m.period_type = PeriodType::Linear;
    }
    m.module.chn = f.read8() as usize;
    m.module.spd = f.read8() as i32;
    m.module.bpm = f.read8() as i32;
    f.read16l(); // unknown - 0x01c5
    f.read16l(); // unknown - 0xff00
    f.read8(); // unknown - 0x80
    if f.read(&mut data.chn_pan) != 64 {
        return Err(());
    }
    if m.module.chn > XMP_MAX_CHANNELS {
        return Err(());
    }
    Ok(())
}

fn get_ordr(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, _data: &mut Local) -> Result<(), ()> {
    m.module.len = f.read8() as usize + 1;
    // Don't follow Dr.Eggman's specs here
    for i in 0..m.module.len {
        m.module.xxo[i] = f.read8();
    }
    Ok(())
}

fn get_patt_cnt(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, _data: &mut Local) -> Result<(), ()> {
    let i = f.read8() as usize + 1; // pattern number
    if i > m.module.pat {
        m.module.pat = i;
    }
    Ok(())
}

fn get_inst_cnt(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, _data: &mut Local) -> Result<(), ()> {
    f.read32b(); // 42 01 00 00
    f.read8(); // 00
    let i = f.read8() as usize + 1; // instrument number
    if i > MAX_INSTRUMENTS {
        return Err(());
    }
    if i > m.module.ins {
        m.module.ins = i;
    }
    Ok(())
}

fn get_patt(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, _data: &mut Local) -> Result<(), ()> {
    let i = f.read8() as usize; // pattern number
    let len = f.read32l() as i32;
    let rows = f.read8() as usize + 1;

    // Sanity check - don't allow duplicate patterns.
    if i >= m.module.xxp.len() || len < 0 || m.module.xxp[i].rows != 0 {
        return Err(());
    }
    m.module.alloc_pattern_tracks(i, rows);

    let mut dummy = Event::default();
    let mut r = 0;
    while r < rows {
        let flag = f.read8();
        if flag == 0 {
            r += 1;
            continue;
        }
        if f.take_error() {
            return Err(());
        }
        let chan = (flag & 0x1f) as usize;
        let chn = m.module.chn;
        let event = if chan < chn { m.module.event_mut(i, chan, r) } else { &mut dummy };

        if flag & 0x80 != 0 {
            let fxp = f.read8();
            let mut fxt = f.read8();
            let mut fxp = fxp;
            match fxt {
                0x14 => fxt = FX_S3M_SPEED, // speed
                _ => {
                    if fxt > 0x0f {
                        fxt = 0;
                        fxp = 0;
                    }
                }
            }
            event.fxt = fxt;
            event.fxp = fxp;
        }
        if flag & 0x40 != 0 {
            event.ins = f.read8();
            event.note = f.read8();
            if event.note == 128 {
                event.note = XMP_KEY_OFF;
            }
        }
        if flag & 0x20 != 0 {
            event.vol = (1 + f.read8() as i32 / 2) as u8;
        }
    }
    Ok(())
}

fn get_inst(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, _data: &mut Local) -> Result<(), ()> {
    f.read32b(); // 42 01 00 00
    f.read8(); // 00
    let i = f.read8() as usize; // instrument number

    // Sanity check - don't allow duplicate instruments.
    if i >= m.module.xxi.len() || m.module.xxi[i].nsm != 0 {
        return Err(());
    }
    let mut name = [0u8; 28];
    f.read(&mut name);
    m.module.xxi[i].name = cstr(&name);
    f.skip(290); // Sample/note map, envelopes
    let nsm = f.read16l() as i32;
    m.module.xxi[i].nsm = nsm;
    if nsm == 0 {
        return Ok(());
    }
    m.module.xxi[i].sub = vec![SubInstrument::default(); nsm as usize];

    // FIXME: Currently reading only the first sample
    f.read32b(); // RIFF
    f.read32b(); // size
    f.read32b(); // AS
    f.read32b(); // SAMP
    f.read32b(); // size
    f.read32b(); // unknown - usually 0x40000000

    let mut sname = [0u8; 28];
    f.read(&mut sname);
    f.read32b(); // unknown - 0x0000
    f.read8(); // unknown - 0x00

    m.module.xxi[i].vol = f.read8() as i32;
    let svol = (f.read16l() as i32 + 1) / 512;
    let flags = f.read16l();
    f.read16l(); // unknown - 0x0080
    let len = f.read32l() as i32;
    let lps = f.read32l() as i32;
    let lpe = f.read32l() as i32;

    let mut flg = 0;
    let mut has_unsigned_sample = false;
    if flags & 0x04 != 0 {
        flg |= XMP_SAMPLE_16BIT;
    }
    if flags & 0x08 != 0 {
        flg |= XMP_SAMPLE_LOOP;
    }
    if flags & 0x10 != 0 {
        flg |= XMP_SAMPLE_LOOP | XMP_SAMPLE_LOOP_BIDIR;
    }
    if flags & 0x80 == 0 {
        has_unsigned_sample = true;
    }
    let srate = f.read32l() as i32;
    let (xpo, fin) = c2spd_to_note(srate);

    f.read32l(); // 0x00000000
    f.read32l(); // unknown

    let sub = &mut m.module.xxi[i].sub[0];
    sub.sid = i as i32;
    sub.pan = XMP_INST_NO_DEFAULT_PAN;
    sub.vol = svol;
    sub.xpo = xpo;
    sub.fin = fin;
    let xxs = &mut m.module.xxs[i];
    xxs.name = cstr(&sname);
    xxs.len = len;
    xxs.lps = lps;
    xxs.lpe = lpe;
    xxs.flg = flg;

    if len > 1 {
        load_sample(m, f, if has_unsigned_sample { SAMPLE_FLAG_UNS } else { 0 }, i, None)?;
    }
    Ok(())
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let mut data = Local { chn_pan: [0; 64] };

    f.read32b(); // Skip RIFF
    f.read32b(); // Skip size
    f.read32b(); // Skip AM

    let offset = f.tell();

    m.module.smp = 0;
    m.module.ins = 0;
    m.c4rate = C4_NTSC_RATE;

    let flags = IFF_LITTLE_ENDIAN | IFF_SKIP_EMBEDDED | IFF_CHUNK_ALIGN2;
    let first: &[(&[u8; 4], IffLoader<Local>)] =
        &[(b"INIT", get_init), (b"ORDR", get_ordr), (b"PATT", get_patt_cnt), (b"INST", get_inst_cnt)];
    iff::load(m, f, &mut data, first, 4, flags)?;

    m.module.trk = m.module.pat * m.module.chn;
    m.module.smp = m.module.ins;

    m.init_instrument();
    m.module.xxp = vec![Pattern::default(); m.module.pat];
    m.module.xxt = vec![Track::default(); m.module.trk];

    f.seek(offset);

    let second: &[(&[u8; 4], IffLoader<Local>)] = &[(b"PATT", get_patt), (b"INST", get_inst)];
    iff::load(m, f, &mut data, second, 4, flags)?;

    // Alloc missing patterns
    for i in 0..m.module.pat {
        if m.module.xxp[i].rows == 0 {
            m.module.alloc_pattern_tracks(i, 64);
        }
    }
    for i in 0..m.module.chn {
        m.module.xxc[i].pan = data.chn_pan[i] as i32 * 2;
    }

    // TODO: JJ2 1.24 Secret Files shareware does not support fine slides
    // from converted XMs.
    m.quirk |= QUIRK_FINEFX;
    m.read_event_type = ReadEvent::Ft2;
    Ok(())
}
