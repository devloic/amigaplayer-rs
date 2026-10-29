//! DigiBooster Pro modules (`loaders/dbm_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//!
//! Based on DigiBooster_E.guide from the DigiBoosterPro 2.20 package.
//! DigiBooster Pro written by Tomasz & Waldemar Piasta.

use crate::xmp::iff::{self, IffLoader};
use crate::xmp::load::*;
use crate::xmp::module::*;
use crate::xmp::player::c2spd_to_note;

const MAGIC_DBM0: u32 = u32::from_be_bytes(*b"DBM0");

pub fn test(d: &[u8]) -> bool {
    Hio::new(d).read32b() == MAGIC_DBM0
}

const DBM_INSTRUMENT_LOOP: i32 = 1 << 0;
const DBM_INSTRUMENT_LOOP_BIDIR: i32 = 1 << 1;

const DBM_SAMPLE_8BIT: i32 = 1 << 0;
const DBM_SAMPLE_16BIT: i32 = 1 << 1;
const DBM_SAMPLE_32BIT: i32 = 1 << 2;

#[derive(Default)]
struct Local {
    have_info: bool,
    have_song: bool,
    have_patt: bool,
    have_smpl: bool,
    have_inst: bool,
    have_venv: bool,
    have_penv: bool,
    maj_version: i32,
    #[allow(dead_code)]
    min_version: i32,
}

struct DbmEnvelope {
    ins: i32,
    flg: i32,
    npt: i32,
    sus: i32,
    lps: i32,
    lpe: i32,
    nodes: [(u16, i16); 32],
}

fn translate_effect(note: u8, fxt: &mut u8, fxp: &mut u8) {
    match *fxt {
        0x0e => match msn(*fxp) {
            // play from backward
            0x3 => {
                // TODO (libxmp): this is supposed to play the sample in reverse
                // only once, then forward
                if note != 0 {
                    *fxt = FX_REVERSE;
                    *fxp = 1;
                } else {
                    *fxt = 0;
                    *fxp = 0;
                }
            }
            // turn off sound in channel
            0x4 => {
                *fxt = FX_EXTENDED;
                *fxp = EX_CUT << 4;
            }
            // turn on/off channel: tied to the channel mute toggle in the UI
            0x5 => {
                *fxt = FX_TRK_VOL;
                *fxp = if *fxp != 0 { 0x40 } else { 0x00 };
            }
            _ => {}
        },
        // Set Real BPM
        0x1c => *fxt = FX_S3M_BPM,
        _ => {
            if *fxt > 0x1c {
                *fxt = 0;
                *fxp = 0;
            }
        }
    }
}

fn get_info(_id: &[u8], size: u32, m: &mut ModuleData, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    // sanity check
    if d.have_info || size < 10 {
        return Err(());
    }
    d.have_info = true;

    let val = f.read16b() as usize;
    if val > 255 {
        return Err(());
    }
    m.module.ins = val;

    let val = f.read16b() as usize;
    m.module.smp = val;

    f.read16b(); // songs

    let val = f.read16b() as usize;
    if val > 256 {
        m.module.smp = 0;
        m.module.ins = 0;
        return Err(());
    }
    m.module.pat = val;

    let val = f.read16b() as usize;
    if val > XMP_MAX_CHANNELS {
        return Err(());
    }
    m.module.chn = val;

    m.module.trk = m.module.pat * m.module.chn;

    // libxmp_init_instrument refuses more than MAX_SAMPLES samples
    if m.module.smp > MAX_SAMPLES {
        return Err(());
    }
    m.init_instrument();
    Ok(())
}

fn get_song(_id: &[u8], size: u32, m: &mut ModuleData, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    // sanity check
    if d.have_song || size < 46 {
        return Ok(());
    }
    d.have_song = true;

    f.skip(44); // song name

    m.module.len = f.read16b() as usize;

    // sanity check
    if m.module.len > 256 {
        return Err(());
    }

    for i in 0..m.module.len {
        m.module.xxo[i] = f.read16b() as u8;
    }
    Ok(())
}

fn get_inst(_id: &[u8], size: u32, m: &mut ModuleData, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    // sanity check
    if d.have_inst || (size as i64) < 50 * m.module.ins as i64 {
        return Err(());
    }
    d.have_inst = true;

    for i in 0..m.module.ins {
        m.module.xxi[i].nsm = 1;
        m.module.xxi[i].sub = vec![SubInstrument::default()];

        let mut buffer = [0u8; 50];
        if f.read(&mut buffer) < 50 {
            return Err(());
        }

        m.module.xxi[i].name = copy_adjust(&buffer, 30);
        let mut snum = u16::from_be_bytes([buffer[30], buffer[31]]) as usize;
        if snum == 0 || snum > m.module.smp {
            // skip remaining data for this instrument
            continue;
        }
        snum -= 1;
        let rd16 = |o: usize| u16::from_be_bytes([buffer[o], buffer[o + 1]]);
        let rd32 = |o: usize| u32::from_be_bytes([buffer[o], buffer[o + 1], buffer[o + 2], buffer[o + 3]]);

        let mut vol = rd16(32) as i32;
        let c2spd = rd32(34) as i32;
        let lps = rd32(38) as i32;
        let lpe = lps.wrapping_add(rd32(42) as i32);
        // the format documentation incorrectly states pan is 0-255
        let mut pan = 0x80 + rd16(46) as i16 as i32;
        let flags = rd16(48) as i32;

        let xxs = &mut m.module.xxs[snum];
        xxs.lps = lps;
        xxs.lpe = lpe;
        if flags & (DBM_INSTRUMENT_LOOP | DBM_INSTRUMENT_LOOP_BIDIR) != 0 {
            xxs.flg |= XMP_SAMPLE_LOOP;
        }
        if flags & DBM_INSTRUMENT_LOOP_BIDIR != 0 {
            xxs.flg |= XMP_SAMPLE_LOOP_BIDIR;
        }

        vol = vol.clamp(0, 64);
        pan = pan.clamp(0, 255);

        let (xpo, fin) = c2spd_to_note(c2spd);
        let sub = &mut m.module.xxi[i].sub[0];
        sub.sid = snum as i32;
        sub.vol = vol;
        sub.pan = pan;
        sub.xpo = xpo;
        sub.fin = fin;
    }
    Ok(())
}

fn get_patt(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    // sanity check
    if d.have_patt || !d.have_info {
        return Err(());
    }
    d.have_patt = true;

    m.module.xxt = vec![Track::default(); m.module.trk];
    m.module.xxp = vec![Pattern::default(); m.module.pat];

    // note: the channel and flag bytes are inverted in the format description
    // document
    for i in 0..m.module.pat {
        let rows = f.read16b() as usize;
        if f.take_error() {
            return Err(());
        }
        if rows == 0 || rows > 256 {
            return Err(());
        }
        m.module.alloc_pattern_tracks(i, rows);

        let mut sz = f.read32b() as i32;

        let mut r = 0usize;
        while sz > 0 {
            sz -= 1;
            let c = f.read8();
            if f.take_error() {
                return Err(());
            }
            if c == 0 {
                r += 1;
                continue;
            }
            let c = c as usize - 1;

            sz -= 1;
            if sz < 0 {
                break;
            }
            let n = f.read8();

            let mut dummy = Event::default();
            let chn = m.module.chn;
            let event = if c >= chn || r >= rows { &mut dummy } else { m.module.event_mut(i, c, r) };
            *event = Event::default();

            if n & 0x01 != 0 {
                sz -= 1;
                if sz >= 0 {
                    let x = f.read8();
                    event.note = if x == 0x1f { XMP_KEY_OFF } else { 13 + msn(x) * 12 + lsn(x) };
                }
            }
            if n & 0x02 != 0 {
                sz -= 1;
                if sz >= 0 {
                    event.ins = f.read8();
                }
            }
            if n & 0x04 != 0 {
                sz -= 1;
                if sz >= 0 {
                    event.fxt = f.read8();
                }
            }
            if n & 0x08 != 0 {
                sz -= 1;
                if sz >= 0 {
                    event.fxp = f.read8();
                }
            }
            if n & 0x10 != 0 {
                sz -= 1;
                if sz >= 0 {
                    event.f2t = f.read8();
                }
            }
            if n & 0x20 != 0 {
                sz -= 1;
                if sz >= 0 {
                    event.f2p = f.read8();
                }
            }

            let note = event.note;
            translate_effect(note, &mut event.fxt, &mut event.fxp);
            translate_effect(note, &mut event.f2t, &mut event.f2p);
        }
    }
    Ok(())
}

fn get_smpl(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    // sanity check
    if d.have_smpl || !d.have_info {
        return Err(());
    }
    d.have_smpl = true;

    for i in 0..m.module.smp {
        let flags = f.read32b() as i32;
        m.module.xxs[i].len = f.read32b() as i32;

        // this seems to be roughly how DigiBooster 2.21 prioritizes sample
        // formats, with the caveat that it doesn't actually support 32-bit
        // samples. No flags set -> assumes 16-bit, but don't support this (in
        // case 3.0 is different...)
        if flags & DBM_SAMPLE_8BIT != 0 {
            // nop
        } else if flags & DBM_SAMPLE_16BIT != 0 {
            m.module.xxs[i].flg |= XMP_SAMPLE_16BIT;
        } else if flags & DBM_SAMPLE_32BIT != 0 {
            // skip 32-bit samples
            f.skip(((m.module.xxs[i].len as u32) << 2) as usize);
            continue;
        } else {
            return Err(());
        }

        load_sample(m, f, SAMPLE_FLAG_BIGEND, i, None)?;
    }
    Ok(())
}

fn read_envelope(ins_count: usize, f: &mut Hio) -> Result<DbmEnvelope, ()> {
    let ins = f.read16b() as i32 - 1;
    let flg = (f.read8() & 0x7) as i32;
    let npt = f.read8() as i32 + 1; // DBM counts sections, not points
    let sus = f.read8() as i32;
    let lps = f.read8() as i32;
    let lpe = f.read8() as i32;
    let _sus2 = f.read8();

    // The format document claims there should be a reserved byte here but no
    // DigiBooster Pro module actually has this.

    // sanity check
    if ins < 0 || ins as usize >= ins_count || npt > 32 || sus >= 32 || lps >= 32 || lpe >= 32 {
        return Err(());
    }

    let mut nodes = [(0u16, 0i16); 32];
    for n in nodes.iter_mut() {
        n.0 = f.read16b();
        n.1 = f.read16b() as i16;
    }
    if f.take_error() {
        return Err(());
    }
    Ok(DbmEnvelope { ins, flg, npt, sus, lps, lpe, nodes })
}

fn get_venv(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    // sanity check
    if d.have_venv || !d.have_info {
        return Err(());
    }
    d.have_venv = true;

    let nenv = f.read16b();
    for _ in 0..nenv {
        let env = read_envelope(m.module.ins, f)?;
        let aei = &mut m.module.xxi[env.ins as usize].aei;
        aei.flg = env.flg;
        aei.npt = env.npt;
        aei.sus = env.sus;
        aei.lps = env.lps;
        aei.lpe = env.lpe;
        for j in 0..32 {
            aei.data[j * 2] = env.nodes[j].0 as i16;
            aei.data[j * 2 + 1] = env.nodes[j].1;
        }
    }
    Ok(())
}

fn get_penv(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    // sanity check
    if d.have_penv || !d.have_info {
        return Err(());
    }
    d.have_penv = true;

    let nenv = f.read16b();
    for _ in 0..nenv {
        let mut env = read_envelope(m.module.ins, f)?;
        let pei = &mut m.module.xxi[env.ins as usize].pei;
        pei.flg = env.flg;
        pei.npt = env.npt;
        pei.sus = env.sus;
        pei.lps = env.lps;
        pei.lpe = env.lpe;
        for j in 0..32 {
            // DigiBooster Pro 2 stores the pan value between 0 and 64.
            // DigiBooster 3 stores it from -128 to 128 (Krashan - M2.dbm).
            if d.maj_version >= 3 {
                env.nodes[j].1 = (env.nodes[j].1 as i32 / 4 + 32) as i16;
            }
            pei.data[j * 2] = env.nodes[j].0 as i16;
            pei.data[j * 2 + 1] = env.nodes[j].1;
        }
    }
    Ok(())
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    f.read32b(); // DBM0

    let mut data = Local::default();
    let version = f.read16b();
    data.maj_version = (version >> 8) as i32;
    data.min_version = (version & 0xff) as i32;

    f.skip(10);
    let mut name = [0u8; 44];
    if f.read(&mut name) < 44 {
        return Err(());
    }

    m.c4rate = C4_NTSC_RATE;
    m.quirk |= QUIRK_FINEFX;

    let chunks: &[(&[u8; 4], IffLoader<Local>)] = &[
        (b"INFO", get_info),
        (b"SONG", get_song),
        (b"INST", get_inst),
        (b"PATT", get_patt),
        (b"SMPL", get_smpl),
        (b"VENV", get_venv),
        (b"PENV", get_penv),
    ];

    m.module.name = String::from_utf8_lossy(&name[..name.iter().position(|&b| b == 0).unwrap_or(44)]).into_owned();
    m.module.type_ = format!("DigiBooster Pro {}.{:02x} DBM0", data.maj_version, data.min_version);

    iff::load(m, f, &mut data, chunks, 4, 0)?;
    // no PATT chunk: libxmp's pattern table was never allocated (xmp_load_module
    // refuses a module with a NULL `xxp`)
    if !data.have_patt {
        return Err(());
    }

    for i in 0..m.module.chn {
        m.module.xxc[i].pan = 0x80;
    }
    Ok(())
}
