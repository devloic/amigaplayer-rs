//! Archimedes Tracker modules (`loaders/arch_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::voltable::ARCH_VOL_TABLE;
use crate::xmp::iff::{self, IffLoader, IFF_LITTLE_ENDIAN};
use crate::xmp::load::*;
use crate::xmp::module::*;

const MAGIC_MUSX: u32 = u32::from_be_bytes(*b"MUSX");
const MAGIC_MNAM: u32 = u32::from_be_bytes(*b"MNAM");
const MAGIC_SNAM: u32 = u32::from_be_bytes(*b"SNAM");
const MAGIC_SVOL: u32 = u32::from_be_bytes(*b"SVOL");
const MAGIC_SLEN: u32 = u32::from_be_bytes(*b"SLEN");
const MAGIC_ROFS: u32 = u32::from_be_bytes(*b"ROFS");
const MAGIC_RLEN: u32 = u32::from_be_bytes(*b"RLEN");
const MAGIC_SDAT: u32 = u32::from_be_bytes(*b"SDAT");

/// Linear (0 to 0x40) to logarithmic volume conversion. This is only used for the
/// Protracker-compatible "linear volume" effect in Andy Southgate's StasisMod. In
/// this implementation linear and logarithmic volumes can be freely intermixed.
static LIN_TABLE: [u8; 65] = [
    0x00, 0x48, 0x64, 0x74, 0x82, 0x8a, 0x92, 0x9a, 0xa2, 0xa6, 0xaa, 0xae, 0xb2, 0xb6, 0xea, 0xbe, 0xc2, 0xc4, 0xc6, 0xc8, 0xca, 0xcc,
    0xce, 0xd0, 0xd2, 0xd4, 0xd6, 0xd8, 0xda, 0xdc, 0xde, 0xe0, 0xe2, 0xe2, 0xe4, 0xe4, 0xe6, 0xe6, 0xe8, 0xe8, 0xea, 0xea, 0xec, 0xec,
    0xee, 0xee, 0xf0, 0xf0, 0xf2, 0xf2, 0xf4, 0xf4, 0xf6, 0xf6, 0xf8, 0xf8, 0xfa, 0xfa, 0xfc, 0xfc, 0xfe, 0xfe, 0xfe, 0xfe, 0xfe,
];

pub fn test(d: &[u8]) -> bool {
    let mut f = Hio::new(d);
    if f.read32b() != MAGIC_MUSX {
        return false;
    }
    f.read32l();
    // The oracle (and libxmp's usual `xmp_load_module`) reads from a file, where
    // hio_eof is only set by a read that hit the end. The loop then never ends
    // normally: a file with no MNAM chunk before its end is refused by the
    // 0xffffffff length that the read past the end returns.
    loop {
        let id = f.read32b();
        let len = f.read32l();
        // sanity check
        if len > 0x100000 {
            return false;
        }
        if id == MAGIC_MNAM {
            return true;
        }
        f.seek_cur(len as i64);
    }
}

struct Local {
    pflag: bool,
    sflag: bool,
    max_ins: usize,
    max_pat: usize,
    has_mvox: bool,
    has_pnum: bool,
    ster: [u8; 8],
    rows: [u8; 64],
}

impl Default for Local {
    fn default() -> Self {
        Local { pflag: false, sflag: false, max_ins: 0, max_pat: 0, has_mvox: false, has_pnum: false, ster: [0; 8], rows: [0; 64] }
    }
}

fn fix_effect(e: &mut Event) {
    match e.fxt {
        // 00 xy Normal play or Arpeggio (x: first halfnote to add, y: second
        // halftone to subtract)
        0x00 => e.fxt = FX_ARPEGGIO,
        // 01 xx Slide Up
        0x01 => e.fxt = FX_PORTA_UP,
        // 02 xx Slide Down
        0x02 => e.fxt = FX_PORTA_DN,
        // 03 xx Tone Portamento
        0x03 => e.fxt = FX_TONEPORTA,
        // 0B xx Break Pattern
        0x0b => e.fxt = FX_BREAK,
        // set linear volume
        0x0c => {
            if e.fxp <= 64 {
                e.fxt = FX_VOLSET;
                e.fxp = LIN_TABLE[e.fxp as usize];
            } else {
                e.fxp = 0;
                e.fxt = 0;
            }
        }
        // 0E xy Set Stereo (y: stereo position 1-7, 1=left 4=center 7=right), and
        // StasisMod's non-standard set panning effect
        0x0e | 0x19 => {
            if e.fxp > 0 && e.fxp < 8 {
                e.fxt = FX_SETPAN;
                e.fxp = (42 * e.fxp as i32 - 40) as u8;
            } else {
                e.fxt = 0;
                e.fxp = 0;
            }
        }
        // 10 xx Volume Slide Up
        0x10 => e.fxt = FX_VOLSLIDE_UP,
        // 11 xx Volume Slide Down
        0x11 => e.fxt = FX_VOLSLIDE_DN,
        // 13 xx Position Jump
        0x13 => e.fxt = FX_JUMP,
        // 15 xy Line Jump (not in manual): jump to line 10*x+y in the same
        // pattern (10*x+y>63 ignored)
        0x15 => {
            let line = msn(e.fxp) as i32 * 10 + lsn(e.fxp) as i32;
            if line < 64 {
                e.fxt = FX_LINE_JUMP;
                e.fxp = line as u8;
            } else {
                e.fxt = 0;
                e.fxp = 0;
            }
        }
        // 1C xy Set Speed
        0x1c => e.fxt = FX_SPEED,
        // 1F xx Set Volume: all volumes are logarithmic
        0x1f => e.fxt = FX_VOLSET,
        _ => {
            e.fxt = 0;
            e.fxp = 0;
        }
    }
}

fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]).into_owned()
}

fn get_tinf(_id: &[u8], _size: u32, _m: &mut ModuleData, f: &mut Hio, _d: &mut Local) -> Result<(), ()> {
    // the date: not used
    for _ in 0..4 {
        f.read8();
    }
    Ok(())
}

fn get_mvox(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    let chn = f.read32l();
    // sanity check
    if chn < 1 || chn > 8 || d.has_mvox {
        return Err(());
    }
    m.module.chn = chn as usize;
    d.has_mvox = true;
    Ok(())
}

fn get_ster(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    if f.read(&mut d.ster) != 8 {
        return Err(());
    }
    for i in 0..m.module.chn {
        if d.ster[i] > 0 && d.ster[i] < 8 {
            m.module.xxc[i].pan = 42 * d.ster[i] as i32 - 40;
        }
    }
    Ok(())
}

fn get_mnam(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, _d: &mut Local) -> Result<(), ()> {
    let mut name = [0u8; 32];
    if f.read(&mut name) != 32 {
        return Err(());
    }
    m.module.name = cstr(&name);
    Ok(())
}

fn get_anam(_id: &[u8], _size: u32, _m: &mut ModuleData, _f: &mut Hio, _d: &mut Local) -> Result<(), ()> {
    Ok(())
}

fn get_mlen(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, _d: &mut Local) -> Result<(), ()> {
    let len = f.read32l();
    // sanity check
    if len > 0xff {
        return Err(());
    }
    m.module.len = len as usize;
    Ok(())
}

fn get_pnum(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    let pat = f.read32l();
    // sanity check
    if pat < 1 || pat > 64 || d.has_pnum {
        return Err(());
    }
    m.module.pat = pat as usize;
    d.has_pnum = true;
    Ok(())
}

fn get_plen(_id: &[u8], _size: u32, _m: &mut ModuleData, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    if f.read(&mut d.rows) != 64 {
        return Err(());
    }
    Ok(())
}

fn get_sequ(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, _d: &mut Local) -> Result<(), ()> {
    let mut xxo = [0u8; 128];
    f.read(&mut xxo);
    m.module.xxo[..128].copy_from_slice(&xxo);
    m.module.type_ = "Archimedes Tracker".to_string();
    Ok(())
}

fn get_patt(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    // sanity check
    if !d.has_mvox || !d.has_pnum {
        return Err(());
    }

    if !d.pflag {
        d.pflag = true;
        d.max_pat = 0;
        m.module.trk = m.module.pat * m.module.chn;
        m.module.xxt = vec![Track::default(); m.module.trk];
        m.module.xxp = vec![Pattern::default(); m.module.pat];
    }

    // sanity check
    if d.max_pat >= m.module.pat || d.max_pat >= 64 {
        return Err(());
    }

    let i = d.max_pat;
    let rows = d.rows[i] as usize;
    if rows == 0 {
        return Err(());
    }
    m.module.alloc_pattern_tracks(i, rows);

    for j in 0..rows {
        for k in 0..m.module.chn {
            let event = m.module.event_mut(i, k, j);
            event.fxp = f.read8();
            event.fxt = f.read8();
            event.ins = f.read8();
            event.note = f.read8();
            if event.note != 0 {
                event.note = event.note.wrapping_add(48);
            }
            fix_effect(event);
        }
    }

    d.max_pat += 1;
    Ok(())
}

fn get_samp(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    if !d.sflag {
        m.module.ins = 36;
        m.module.smp = 36;
        m.init_instrument();
        d.sflag = true;
        d.max_ins = 0;
    }

    // FIXME (libxmp): more than 36 sample slots used. Unfortunately we have no way
    // to handle this without two passes, and it's officially supposed to be 36, so
    // ignore the rest.
    if d.max_ins >= 36 {
        return Ok(());
    }

    let i = d.max_ins;
    m.module.xxi[i].nsm = 1;
    m.module.xxi[i].sub = vec![SubInstrument::default()];

    if f.read32b() != MAGIC_SNAM {
        return Err(());
    }

    // should usually be 0x14 but zero is not unknown
    let name_len = f.read32l() as i32;
    // sanity check
    if name_len < 0 || name_len > 32 {
        return Err(());
    }
    let mut name = [0u8; 32];
    f.read(&mut name[..name_len as usize]);
    m.module.xxi[i].name = cstr(&name);

    if f.read32b() != MAGIC_SVOL {
        return Err(());
    }
    f.read32l();
    m.module.xxi[i].sub[0].vol = (f.read32l() & 0xff) as i32;

    if f.read32b() != MAGIC_SLEN {
        return Err(());
    }
    f.read32l();
    m.module.xxs[i].len = f.read32l() as i32;

    if f.read32b() != MAGIC_ROFS {
        return Err(());
    }
    f.read32l();
    m.module.xxs[i].lps = f.read32l() as i32;

    if f.read32b() != MAGIC_RLEN {
        return Err(());
    }
    f.read32l();
    m.module.xxs[i].lpe = f.read32l() as i32;

    if f.read32b() != MAGIC_SDAT {
        // This is corrupt in the final sample of 5-OverPar.
        m.module.xxs[i].len = 0;
    }
    f.read32l();
    f.read32l(); // 0x00000000

    m.module.xxi[i].sub[0].sid = i as i32;
    m.module.xxi[i].sub[0].pan = XMP_INST_NO_DEFAULT_PAN;

    m.vol_table = Some(&ARCH_VOL_TABLE);
    m.volbase = 0xff;

    // clean bad loops
    let xxs = &mut m.module.xxs[i];
    if xxs.lps < 0 || xxs.lps >= xxs.len {
        xxs.lps = 0;
        xxs.lpe = 0;
    }

    if xxs.lpe > 2 {
        if xxs.lpe > xxs.len.wrapping_sub(xxs.lps) {
            xxs.lpe = xxs.len.wrapping_sub(xxs.lps);
        }
        xxs.flg = XMP_SAMPLE_LOOP;
        xxs.lpe = xxs.lps.wrapping_add(xxs.lpe);
    } else if xxs.lpe == 2 && xxs.lps > 0 {
        // non-zero repeat offset and repeat length of 2 means loop to end of sample
        xxs.flg = XMP_SAMPLE_LOOP;
        xxs.lpe = xxs.len;
    }

    load_sample(m, f, SAMPLE_FLAG_VIDC, i, None)?;

    d.max_ins += 1;
    Ok(())
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    f.read32b(); // MUSX
    f.read32b();

    let mut data = Local::default();

    let chunks: &[(&[u8; 4], IffLoader<Local>)] = &[
        (b"TINF", get_tinf),
        (b"MVOX", get_mvox),
        (b"STER", get_ster),
        (b"MNAM", get_mnam),
        (b"ANAM", get_anam),
        (b"MLEN", get_mlen),
        (b"PNUM", get_pnum),
        (b"PLEN", get_plen),
        (b"SEQU", get_sequ),
        (b"PATT", get_patt),
        (b"SAMP", get_samp),
    ];
    iff::load(m, f, &mut data, chunks, 4, IFF_LITTLE_ENDIAN)?;
    // no PATT chunk: libxmp's pattern table was never allocated (xmp_load_module
    // refuses a module with a NULL `xxp`)
    if !data.pflag {
        return Err(());
    }

    for i in 0..m.module.chn {
        m.module.xxc[i].pan = m.defpan((((i as i32 + 3) / 2) % 2) * 0xff);
    }
    Ok(())
}
