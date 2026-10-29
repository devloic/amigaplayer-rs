//! Images Music System modules (`loaders/ims_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::load::*;
use crate::xmp::module::*;

#[derive(Clone, Copy, Default)]
struct ImsInstrument {
    name: [u8; 22],
    size: u16,
    finetune: u8,
    volume: u8,
    loop_start: u16,
    loop_size: u16,
}

/// `libxmp_test_name` with no flags.
fn test_name(s: &[u8]) -> bool {
    s.iter().all(|&b| b <= 0x7f && !(b > 0 && b < 32 && b != 0x08 && b != 0x0e))
}

pub fn test(d: &[u8]) -> bool {
    let mut f = Hio::new(d);
    let mut title = [0u8; 20];
    f.read(&mut title);
    let mut smp_size: i32 = 0;
    for _ in 0..31 {
        let mut ins = ImsInstrument::default();
        if f.read(&mut ins.name) < 22 {
            return false;
        }
        ins.size = f.read16b();
        ins.finetune = f.read8();
        ins.volume = f.read8();
        ins.loop_start = f.read16b();
        ins.loop_size = f.read16b();

        smp_size += ins.size as i32 * 2;

        if !test_name(&ins.name) {
            return false;
        }
        // no support for finetune in this format
        if ins.finetune != 0 {
            return false;
        }
        if ins.volume > 0x40 {
            return false;
        }
        if ins.size > 0x8000 {
            return false;
        }
        if ins.loop_start > ins.size {
            return false;
        }
        if ins.size != 0 && ins.loop_size > 2 * ins.size {
            return false;
        }
    }
    if smp_size < 8 {
        return false;
    }
    let len = f.read8();
    let restart = f.read8();
    let mut orders = [0u8; 128];
    if f.read(&mut orders) < 128 {
        return false;
    }
    let sample_offset = f.read32b();

    if restart >= len {
        return false;
    }
    if len > 0x7f {
        return false;
    }
    // Modules may contain patterns after the end of the module.
    let pat = orders.iter().copied().max().unwrap_or(0) as u32 + 1;
    if pat > 0x7f || len == 0 || len > 0x7f {
        return false;
    }
    // Sample offset should always be the first byte after the pattern data.
    pat * 0x300 + 0x43c == sample_offset
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let xpo = 21; // Tuned against UADE

    m.module.chn = 4;
    m.module.ins = 31;
    m.module.smp = 31;

    let mut title = [0u8; 20];
    f.read(&mut title);
    let mut ih = [ImsInstrument::default(); 31];
    for ins in ih.iter_mut() {
        f.read(&mut ins.name);
        ins.size = f.read16b();
        ins.finetune = f.read8();
        ins.volume = f.read8();
        ins.loop_start = f.read16b();
        ins.loop_size = f.read16b();
    }
    let len = f.read8();
    if len > 128 {
        return Err(());
    }
    let restart = f.read8();
    let mut orders = [0u8; 128];
    f.read(&mut orders);
    let _sample_offset = f.read32b();

    m.module.rst = restart as usize;
    m.module.len = len as usize;
    m.module.xxo[..128].copy_from_slice(&orders);
    m.module.pat = orders.iter().copied().max().unwrap_or(0) as usize + 1;
    m.module.trk = m.module.chn * m.module.pat;

    m.module.name = copy_adjust(&title, 20);
    m.module.type_ = "Images Music System".into();

    m.init_instrument();
    for (i, ins) in ih.iter().enumerate() {
        let xxs = &mut m.module.xxs[i];
        xxs.len = 2 * ins.size as i32;
        xxs.lps = 2 * ins.loop_start as i32;
        xxs.lpe = xxs.lps + 2 * ins.loop_size as i32;
        xxs.flg = if ins.loop_size > 1 { XMP_SAMPLE_LOOP } else { 0 };
        let has = xxs.len > 0;
        let xxi = &mut m.module.xxi[i];
        xxi.sub = vec![SubInstrument {
            fin: ((ins.finetune as i8) as i32) << 4,
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

    // Load and convert patterns
    for i in 0..m.module.pat {
        m.module.alloc_pattern_tracks(i, 64);
        for j in 0..0x100 {
            let event = m.module.event_mut(i, j & 3, j >> 2);
            let mut e = [0u8; 3];
            f.read(&mut e);

            // 0x3f is a blank note; 0x00 is "a valid period two octaves below
            // middle-C".
            event.note = e[0] & 0x3f;
            if event.note != 0x3f {
                event.note += xpo + 12;
            } else {
                event.note = 0;
            }
            event.ins = ((e[0] & 0x40) >> 2) | msn(e[1]);
            event.fxt = lsn(e[1]);
            event.fxp = e[2];
            disable_continue_fx(event);

            // Pattern break (D**) uses HEX in UNIC, so convert.
            if event.fxt == 0x0d {
                event.fxp = ((event.fxp / 10) << 4) | (event.fxp % 10);
            }
        }
    }

    m.period_type = PeriodType::ModRng;

    for i in 0..m.module.smp {
        if m.module.xxs[i].len == 0 {
            continue;
        }
        load_sample(m, f, 0, i, None)?;
    }
    Ok(())
}
