//! Magnetic Fields Packer modules (`loaders/mfp_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//!
//! A module packer by Shaun Southern. The samples are in a separate file: `mfp.name`
//! holds the song and `smp.name` the samples (`smp.name.set` for the songs named
//! `mfp.name-N`). Without that file (`ModuleData::basename` and `dirname` name
//! where to look) the module loads without samples, as in libxmp.

use crate::xmp::load::*;
use crate::xmp::module::*;
use std::path::PathBuf;

fn be16(b: &[u8]) -> i32 {
    u16::from_be_bytes([b[0], b[1]]) as i32
}

pub fn test(d: &[u8]) -> bool {
    if d.len() < 384 {
        return false;
    }
    let buf = &d[..384];

    // check restart byte
    if buf[249] != 0x7f {
        return false;
    }

    for i in 0..31 {
        // check size
        let len = be16(&buf[i * 8..]);
        if len > 0x7fff {
            return false;
        }

        // check finetune
        if buf[i * 8 + 2] & 0xf0 != 0 {
            return false;
        }

        // check volume
        if buf[i * 8 + 3] > 0x40 {
            return false;
        }

        // check loop start
        let lps = be16(&buf[i * 8 + 4..]);
        if lps > len {
            return false;
        }

        // check loop size
        let lsz = be16(&buf[i * 8 + 6..]);
        if lps + lsz - 1 > len {
            return false;
        }

        if len > 0 && lsz == 0 {
            return false;
        }
    }

    if buf[248] as i32 != be16(&buf[378..]) {
        return false;
    }

    be16(&buf[378..]) == be16(&buf[380..])
}

#[derive(Clone, Copy)]
struct MfpMap {
    offset: u16,
    ord: u8,
    chn: u8,
}

/// The sample file of the song: `smp` for the first three letters of its name,
/// or `smp.name.set` for a `-N` song name.
fn open_samples(m: &ModuleData) -> Option<Vec<u8>> {
    let b = m.basename.as_bytes();
    if b.len() < 5 || b[3] != b'.' {
        return None;
    }
    let name = format!("smp{}", &m.basename[3..]);
    let mut path: PathBuf = [m.dirname.as_str(), name.as_str()].iter().collect();
    if let Ok(d) = std::fs::read(&path) {
        return Some(d);
    }
    // handle .set filenames like in Kid Chaos
    if m.basename.contains('-') {
        let s = path.to_string_lossy().into_owned();
        let p = s.rfind('-')?;
        path = PathBuf::from(format!("{}.set", &s[..p]));
    }
    std::fs::read(&path).ok()
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    m.module.type_ = "Magnetic Fields Packer".to_string();

    m.module.chn = 4;
    m.module.ins = 31;
    m.module.smp = 31;

    m.init_instrument();

    for i in 0..31 {
        let len = 2 * f.read16b() as i32;
        let fin = (f.read8() << 4) as i8 as i32;
        let vol = f.read8() as i32;
        let lps = 2 * f.read16b() as i32;
        let loop_size = f.read16b() as i32;

        let xxs = &mut m.module.xxs[i];
        xxs.len = len;
        xxs.lps = lps;
        xxs.lpe = lps + 2 * loop_size;
        xxs.flg = if loop_size > 1 { XMP_SAMPLE_LOOP } else { 0 };

        let xxi = &mut m.module.xxi[i];
        xxi.sub = vec![SubInstrument { fin, vol, pan: XMP_INST_NO_DEFAULT_PAN, sid: i as i32, ..Default::default() }];
        xxi.rls = 0xfff;
        if len > 0 {
            xxi.nsm = 1;
        }
    }

    m.module.len = f.read8() as usize;
    m.module.pat = m.module.len;
    f.read8(); // restart

    f.seek_cur(128);
    for i in 0..m.module.len {
        m.module.xxo[i] = i as u8;
    }

    // Read and convert patterns

    let size1 = f.read16b() as usize;
    f.read16b();
    if size1 > m.module.len {
        return Err(());
    }

    // What follows is a list of order (not pattern) track offsets. To avoid
    // duplicating tracks, merge the track offsets.
    let mut mapping = Vec::with_capacity(size1 * 4);
    for i in 0..size1 {
        for j in 0..4 {
            mapping.push(MfpMap { offset: f.read16b(), ord: i as u8, chn: j });
        }
    }

    mapping.sort_by_key(|e| e.offset);

    let mut track_offs: Vec<u16> = Vec::new();
    let mut i = 0;
    while i < size1 * 4 {
        let offset = mapping[i].offset;
        track_offs.push(offset);
        let trk = (track_offs.len() - 1) as u16;
        while i < size1 * 4 && mapping[i].offset == offset {
            // write the track number to the offset to save an extra field
            mapping[i].offset = trk;
            i += 1;
        }
    }
    m.module.trk = track_offs.len();
    if m.module.trk == 0 {
        return Err(());
    }

    m.module.xxp = vec![Pattern { rows: 64, index: vec![0; 4] }; m.module.len];
    for e in &mapping {
        // offset was replaced with the track number
        m.module.xxp[e.ord as usize].index[e.chn as usize] = e.offset as usize;
    }

    let pat_addr = f.tell();
    m.module.xxt = vec![Track::new(64); m.module.trk];

    let mut buf = [0u8; 255 * 2 + 4];
    for i in 0..m.module.trk {
        f.seek(pat_addr + track_offs[i] as usize);

        let len = if i < m.module.trk - 1 { track_offs[i + 1] as usize - track_offs[i] as usize } else { buf.len() };

        let want = len.min(buf.len());
        let len = f.read(&mut buf[..want]);

        let mut at = 0;
        for k in 0..4 {
            for x in 0..4 {
                for y in 0..4 {
                    if k >= len {
                        return Err(());
                    }
                    let a = buf[k] as usize + x;
                    if a >= len {
                        return Err(());
                    }
                    let b = buf[a] as usize + y;
                    if b >= len {
                        return Err(());
                    }
                    let c = buf[b] as usize * 2;
                    if c + 4 > len {
                        return Err(());
                    }
                    decode_protracker_event(&mut m.module.xxt[i].event[at], &buf[c..c + 4]);
                    at += 1;
                }
            }
        }
    }

    // Read samples
    let samples = open_samples(m);
    match samples {
        Some(d) => {
            let mut s = Hio::new(&d);
            for i in 0..m.module.ins {
                let sid = m.module.xxi[i].sub[0].sid as usize;
                load_sample(m, &mut s, SAMPLE_FLAG_FULLREP, sid, None)?;
            }
            m.period_type = PeriodType::ModRng;
        }
        None => {
            // no sample file: the module loads silent
            for i in 0..m.module.ins {
                m.module.xxi[i].nsm = 0;
                m.module.xxs[i] = Sample::default();
            }
        }
    }

    Ok(())
}
