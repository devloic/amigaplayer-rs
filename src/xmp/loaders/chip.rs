//! ChipTracker modules (`loaders/chip_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::load::*;
use crate::xmp::module::*;

pub fn test(d: &[u8]) -> bool {
    // Also RASP?
    d.len() >= 956 && &d[952..956] == b"KRIS"
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let mut tidx = [0u8; 1024];

    let mut name = [0u8; 20];
    f.read(&mut name);
    f.read16b();

    struct Ins {
        name: [u8; 22],
        size: u16,
        finetune: u8,
        volume: u8,
        loop_start: u16,
        loop_size: u16,
    }
    let mut ih = Vec::with_capacity(31);
    for _ in 0..31 {
        let mut name = [0u8; 22];
        f.read(&mut name);
        ih.push(Ins {
            name,
            size: f.read16b(),
            finetune: f.read8(),
            volume: f.read8(),
            loop_start: f.read16b(),
            loop_size: f.read16b(),
        });
    }

    let mut magic = [0u8; 4];
    f.read(&mut magic);
    let len = f.read8() as usize;
    // Sanity check
    if len > 128 {
        return Err(());
    }
    let restart = f.read8();
    f.read(&mut tidx);
    f.read16b();

    m.module.chn = 4;
    m.module.ins = 31;
    m.module.smp = 31;
    m.module.len = len;
    m.module.pat = len;
    m.module.rst = restart as usize;

    let mut tnum = 0;
    for i in 0..len {
        m.module.xxo[i] = i as u8;
        for j in 0..4 {
            let t = tidx[2 * (4 * i + j)] as usize;
            if t > tnum {
                tnum = t;
            }
        }
    }
    m.module.trk = tnum + 1;

    m.module.name = copy_adjust(&name, 20);
    m.module.type_ = "Chiptracker".into();

    m.init_instrument();
    for (i, ins) in ih.iter().enumerate() {
        let xxs = &mut m.module.xxs[i];
        xxs.len = 2 * ins.size as i32;
        xxs.lps = ins.loop_start as i32;
        xxs.lpe = xxs.lps + 2 * ins.loop_size as i32;
        xxs.flg = if ins.loop_size > 1 { XMP_SAMPLE_LOOP } else { 0 };
        let has = xxs.len > 0;
        let xxi = &mut m.module.xxi[i];
        xxi.sub = vec![SubInstrument {
            fin: ((ins.finetune << 4) as i8) as i32,
            vol: ins.volume as i32,
            pan: XMP_INST_NO_DEFAULT_PAN,
            sid: i as i32,
            ..Default::default()
        }];
        if has {
            xxi.nsm = 1;
        }
        xxi.name = copy_adjust(&ins.name, 22);
    }

    m.module.xxp = Vec::with_capacity(len);
    for i in 0..len {
        let index = (0..4).map(|j| tidx[2 * (4 * i + j)] as usize).collect();
        m.module.xxp.push(Pattern { rows: 64, index });
    }

    // Load and convert tracks
    m.module.xxt = Vec::with_capacity(m.module.trk);
    for _ in 0..m.module.trk {
        let mut t = Track::new(64);
        for event in t.event.iter_mut() {
            let mut e = [0u8; 4];
            if f.read(&mut e) < 4 {
                return Err(());
            }
            if e[0] != 0 && e[0] != 0xa8 {
                event.note = 13 + e[0] / 2;
            }
            event.ins = e[1];
            event.fxt = e[2] & 0x0f;
            event.fxp = e[3];
        }
        m.module.xxt.push(t);
    }

    m.period_type = PeriodType::ModRng;

    for i in 0..m.module.smp {
        if m.module.xxs[i].len == 0 {
            continue;
        }
        load_sample(m, f, SAMPLE_FLAG_FULLREP, i, None)?;
    }
    Ok(())
}
