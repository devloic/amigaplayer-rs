//! Galaxy Music System 4.0 modules, converted with mod2j2b (`loaders/gal4_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::iff::{self, IffLoader, IFF_LITTLE_ENDIAN};
use crate::xmp::load::*;
use crate::xmp::module::*;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 16 && &d[0..4] == b"RIFF" && &d[8..12] == b"AMFF" && &d[12..16] == b"MAIN"
}

const GAL4_SAMP_16BIT: u8 = 1 << 2;
const GAL4_SAMP_LOOP: u8 = 1 << 3;
const GAL4_SAMP_LOOP_BIDIR: u8 = 1 << 4;
const GAL4_SAMP_SET_PANNING: u8 = 1 << 5;

struct Local {
    snum: usize,
}

fn cstr(b: &[u8]) -> String {
    b.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect()
}

fn get_main(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, _data: &mut Local) -> Result<(), ()> {
    let mut buf = [0u8; 64];
    if f.read(&mut buf) < 64 {
        return Err(());
    }
    m.module.name = cstr(&buf[..63]);
    m.module.type_ = "Galaxy Music System 4.0".to_string();

    let flags = f.read8();
    if flags & 0x01 == 0 {
        m.period_type = PeriodType::Linear;
    }
    m.module.chn = f.read8() as usize;
    m.module.spd = f.read8() as i32;
    m.module.bpm = f.read8() as i32;
    f.read16l(); // unknown - 0x01c5
    f.read16l(); // unknown - 0xff00
    f.read8(); // unknown - 0x80

    // Sanity check
    if m.module.chn > 32 {
        return Err(());
    }
    Ok(())
}

fn get_ordr(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, _data: &mut Local) -> Result<(), ()> {
    m.module.len = f.read8() as usize + 1;
    if f.take_error() {
        return Err(());
    }
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
    f.read8(); // 00
    let i = f.read8() as usize + 1; // instrument number
    if i > MAX_INSTRUMENTS {
        return Err(());
    }
    if i > m.module.ins {
        m.module.ins = i;
    }
    f.skip(28); // skip name
    m.module.smp += f.read8() as usize;
    Ok(())
}

fn get_patt(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, _data: &mut Local) -> Result<(), ()> {
    let i = f.read8() as usize; // pattern number
    let len = f.read32l() as i32;

    // Sanity check
    if i >= m.module.pat || len <= 0 || m.module.xxp[i].rows != 0 {
        return Err(());
    }
    let rows = f.read8() as usize + 1;
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
            let mut fxp = f.read8();
            let mut fxt = f.read8();
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

fn get_inst(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    f.read8(); // 00
    let i = f.read8() as usize; // instrument number

    // Sanity check
    if i >= m.module.ins || m.module.xxi[i].nsm != 0 {
        return Err(());
    }

    let mut name = [0u8; 28];
    f.read(&mut name);
    let nsm = f.read8() as i32;
    {
        let xxi = &mut m.module.xxi[i];
        xxi.name = cstr(&name);
        xxi.nsm = nsm;
        for j in 0..108 {
            xxi.map[j].ins = f.read8();
        }
    }

    f.skip(11); // unknown
    let vwf = f.read8() as i32; // vibrato waveform
    let vsw = f.read8() as i32; // vibrato sweep
    f.read8(); // unknown
    f.read8(); // unknown
    let vde = f.read8() as i32; // vibrato depth
    let vra = f.read16l() as i32 / 16; // vibrato speed
    f.read8(); // unknown

    let xxi = &mut m.module.xxi[i];
    let mut val = f.read8(); // PV envelopes flags
    if lsn(val) & 0x01 != 0 {
        xxi.aei.flg |= XMP_ENVELOPE_ON;
    }
    if lsn(val) & 0x02 != 0 {
        xxi.aei.flg |= XMP_ENVELOPE_SUS;
    }
    if lsn(val) & 0x04 != 0 {
        xxi.aei.flg |= XMP_ENVELOPE_LOOP;
    }
    if msn(val) & 0x01 != 0 {
        xxi.pei.flg |= XMP_ENVELOPE_ON;
    }
    if msn(val) & 0x02 != 0 {
        xxi.pei.flg |= XMP_ENVELOPE_SUS;
    }
    if msn(val) & 0x04 != 0 {
        xxi.pei.flg |= XMP_ENVELOPE_LOOP;
    }

    val = f.read8(); // PV envelopes points
    xxi.aei.npt = lsn(val) as i32 + 1;
    xxi.pei.npt = msn(val) as i32 + 1;

    val = f.read8(); // PV envelopes sustain point
    xxi.aei.sus = lsn(val) as i32;
    xxi.pei.sus = msn(val) as i32;

    val = f.read8(); // PV envelopes loop start
    xxi.aei.lps = lsn(val) as i32;
    xxi.pei.lps = msn(val) as i32;

    f.read8(); // PV envelopes loop end
    xxi.aei.lpe = lsn(val) as i32;
    xxi.pei.lpe = msn(val) as i32;

    let maxpt = 10.min(XMP_MAX_ENV_POINTS) as i32;
    if xxi.aei.npt <= 0 || xxi.aei.npt > maxpt {
        xxi.aei.flg &= !XMP_ENVELOPE_ON;
    }
    if xxi.pei.npt <= 0 || xxi.pei.npt > maxpt {
        xxi.pei.flg &= !XMP_ENVELOPE_ON;
    }

    let mut buf = [0u8; 30];
    if f.read(&mut buf) < 30 {
        // volume envelope points
        return Err(());
    }
    for j in 0..xxi.aei.npt as usize {
        if j >= 10 {
            break;
        }
        xxi.aei.data[j * 2] = (u16::from_le_bytes([buf[j * 3], buf[j * 3 + 1]]) / 16) as i16;
        xxi.aei.data[j * 2 + 1] = buf[j * 3 + 2] as i16;
    }

    if f.read(&mut buf) < 30 {
        // pan envelope points
        return Err(());
    }
    for j in 0..xxi.pei.npt as usize {
        if j >= 10 {
            break;
        }
        xxi.pei.data[j * 2] = (u16::from_le_bytes([buf[j * 3], buf[j * 3 + 1]]) / 16) as i16;
        xxi.pei.data[j * 2 + 1] = buf[j * 3 + 2] as i16;
    }

    f.read8(); // fadeout - 0x80->0x02 0x310->0x0c
    f.read8(); // unknown

    if nsm == 0 {
        return Ok(());
    }
    xxi.sub = vec![SubInstrument::default(); nsm as usize];

    for j in 0..nsm as usize {
        let snum = data.snum;
        if snum >= m.module.xxs.len() {
            return Err(());
        }
        f.read32b(); // SAMP
        f.read32b(); // size

        let mut sname = [0u8; 28];
        f.read(&mut sname);

        let pan = f.read8() as i32;
        let svol = f.read8() as i32;
        let flags = f.read8();
        f.read8(); // unknown - 0x80

        let len = f.read32l() as i32;
        let lps = f.read32l() as i32;
        let lpe = f.read32l() as i32;
        let srate = f.read32l() as i32;
        let (xpo, fin) = c2spd_to_note(srate);
        f.read32l(); // 0x00000000
        f.read32l(); // unknown

        let sub = &mut m.module.xxi[i].sub[j];
        sub.pan = if flags & GAL4_SAMP_SET_PANNING != 0 { (pan * 4).min(255) } else { XMP_INST_NO_DEFAULT_PAN };
        sub.vol = svol;
        sub.vwf = vwf;
        sub.vde = vde;
        sub.vra = vra;
        sub.vsw = vsw;
        sub.sid = snum as i32;
        sub.xpo = xpo;
        sub.fin = fin;

        let xxs = &mut m.module.xxs[snum];
        xxs.name = cstr(&sname);
        xxs.len = len;
        xxs.lps = lps;
        xxs.lpe = lpe;
        xxs.flg = 0;
        if flags & GAL4_SAMP_16BIT != 0 {
            xxs.flg |= XMP_SAMPLE_16BIT;
        }
        if flags & GAL4_SAMP_LOOP != 0 {
            xxs.flg |= XMP_SAMPLE_LOOP;
        }
        if flags & GAL4_SAMP_LOOP_BIDIR != 0 {
            xxs.flg |= XMP_SAMPLE_LOOP_BIDIR;
        }

        if len > 1 {
            load_sample(m, f, 0, snum, None)?;
        }
        data.snum += 1;
    }
    Ok(())
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let mut data = Local { snum: 0 };

    f.read32b(); // Skip RIFF
    f.read32b(); // Skip size
    f.read32b(); // Skip AM

    let offset = f.tell();

    m.module.smp = 0;
    m.module.ins = 0;
    m.c4rate = C4_NTSC_RATE;

    // IFF_CHUNK_TRUNC4 of libxmp's iff.h is not used by its iff.c
    let flags = IFF_LITTLE_ENDIAN;
    let first: &[(&[u8; 4], IffLoader<Local>)] =
        &[(b"MAIN", get_main), (b"ORDR", get_ordr), (b"PATT", get_patt_cnt), (b"INST", get_inst_cnt)];
    iff::load(m, f, &mut data, first, 4, flags)?;

    m.module.trk = m.module.pat * m.module.chn;

    m.init_instrument();
    m.module.xxp = vec![Pattern::default(); m.module.pat];
    m.module.xxt = vec![Track::default(); m.module.trk];

    f.seek(offset);
    data.snum = 0;

    let second: &[(&[u8; 4], IffLoader<Local>)] = &[(b"PATT", get_patt), (b"INST", get_inst)];
    iff::load(m, f, &mut data, second, 4, flags)?;

    // Alloc missing patterns
    for i in 0..m.module.pat {
        if m.module.xxp[i].rows == 0 {
            m.module.alloc_pattern_tracks(i, 64);
        }
    }
    for i in 0..m.module.chn {
        m.module.xxc[i].pan = 0x80;
    }

    // TODO: JJ2 1.20 shareware does not support fine slides from converted XMs.
    m.quirk |= QUIRK_FINEFX;
    m.read_event_type = ReadEvent::Ft2;
    Ok(())
}
