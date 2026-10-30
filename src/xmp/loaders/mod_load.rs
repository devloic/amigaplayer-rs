//! Amiga ProTracker and compatible modules (`loaders/mod_load.c`), with
//! libxmp's tracker detection. Used only by the UMX container: plain `.mod`
//! files stay on the native ProTracker path, so this loader is not in
//! `xmp::LOADERS`.
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::load::*;
use crate::xmp::module::*;

const TRACKER_PROTRACKER: i32 = 0;
const TRACKER_NOISETRACKER: i32 = 1;
const TRACKER_SOUNDTRACKER: i32 = 2;
const TRACKER_FASTTRACKER: i32 = 3;
const TRACKER_FASTTRACKER2: i32 = 4;
const TRACKER_OCTALYSER: i32 = 5;
const TRACKER_TAKETRACKER: i32 = 6;
const TRACKER_DIGITALTRACKER: i32 = 7;
const TRACKER_FLEXTRAX: i32 = 8;
const TRACKER_MODSGRAVE: i32 = 9;
const TRACKER_SCREAMTRACKER3: i32 = 10;
const TRACKER_OPENMPT: i32 = 11;
const TRACKER_SOFTWAREVISIONS: i32 = 12;
const TRACKER_UNKNOWN_CONV: i32 = 95;
const TRACKER_CONVERTEDST: i32 = 96;
const TRACKER_CONVERTED: i32 = 97;
const TRACKER_CLONE: i32 = 98;
const TRACKER_UNKNOWN: i32 = 99;
const TRACKER_PROBABLY_NOISETRACKER: i32 = 20;

/// `mod_magic`: magic, locked-in flag, tracker id, channels.
const MOD_MAGIC: &[(&[u8; 4], bool, i32, usize)] = &[
    (b"M.K.", false, TRACKER_PROTRACKER, 4),
    (b"M!K!", true, TRACKER_PROTRACKER, 4),
    (b"M&K!", true, TRACKER_NOISETRACKER, 4),
    (b"N.T.", true, TRACKER_NOISETRACKER, 4),
    (b"6CHN", false, TRACKER_FASTTRACKER, 6),
    (b"8CHN", false, TRACKER_FASTTRACKER, 8),
    (b"CD61", true, TRACKER_OCTALYSER, 6),
    (b"CD81", true, TRACKER_OCTALYSER, 8),
    (b"TDZ1", true, TRACKER_TAKETRACKER, 1),
    (b"TDZ2", true, TRACKER_TAKETRACKER, 2),
    (b"TDZ3", true, TRACKER_TAKETRACKER, 3),
    (b"TDZ4", true, TRACKER_TAKETRACKER, 4),
    (b"FA04", true, TRACKER_DIGITALTRACKER, 4),
    (b"FA06", true, TRACKER_DIGITALTRACKER, 6),
    (b"FA08", true, TRACKER_DIGITALTRACKER, 8),
    (b".M.K", true, TRACKER_SOFTWAREVISIONS, 4),
    (b"LARD", true, TRACKER_UNKNOWN, 4),
    (b"NSMS", true, TRACKER_UNKNOWN, 4),
];

fn tracker_is_vblank(id: i32) -> bool {
    id == TRACKER_NOISETRACKER || id == TRACKER_SOUNDTRACKER
}

fn flip_word_bytes(buf: &mut [u8]) {
    for p in buf.chunks_exact_mut(2) {
        p.swap(0, 1);
    }
}

fn validate_pattern(buf: &[u8]) -> bool {
    for i in 0..64 {
        for j in 0..4 {
            if (buf[(i * 4 + j) * 4] >> 4) > 1 {
                return false;
            }
        }
    }
    true
}

fn rdb16(b: &[u8], o: usize) -> i32 {
    ((b[o] as i32) << 8) | b[o + 1] as i32
}

/// `mod_test` (the full, non-core-player version).
pub fn test(d: &[u8]) -> bool {
    test_at(&mut Hio::new(d), 0)
}

pub fn test_at(f: &mut Hio, start: usize) -> bool {
    f.seek(start + 1080);
    let mut buf = [0u8; 4];
    if f.read(&mut buf) < 4 {
        return false;
    }
    let mut found = false;
    if &buf[2..4] == b"CH" && buf[0].is_ascii_digit() && buf[1].is_ascii_digit() {
        let i = (buf[0] - b'0') as i32 * 10 + (buf[1] - b'0') as i32;
        if i > 0 && i <= 32 {
            found = true;
        }
    }
    if !found && &buf[1..4] == b"CHN" && buf[0].is_ascii_digit() && buf[0] - b'0' != 0 {
        found = true;
    }
    if found {
        return true;
    }
    let Some(&(_, detected, id, _)) = MOD_MAGIC.iter().find(|e| *e.0 == buf) else {
        return false;
    };
    let mut smp_size = 0i32;
    f.seek(start + 20);
    let mut pat_buf = [0u8; 1024];
    for _ in 0..31 {
        f.read(&mut pat_buf[..30]);
        if id == TRACKER_SOFTWAREVISIONS {
            flip_word_bytes(&mut pat_buf[22..30]);
        }
        smp_size += rdb16(&pat_buf, 22) << 1;
        let x = pat_buf[24];
        if x & 0xf0 != 0 && x != 0x20 {
            return false;
        }
        if pat_buf[25] > 0x40 {
            return false;
        }
    }
    if detected {
        return true;
    }
    let size = f.size() as i64;
    let mut num_pat = 0i32;
    f.seek(start + 952);
    for _ in 0..128 {
        let x = f.read8();
        if x > 0x7f {
            break;
        }
        if x as i32 > num_pat {
            num_pat = x as i32;
        }
    }
    num_pat += 1;
    if start as i64 + 1084 + num_pat as i64 * 0x300 + smp_size as i64 == size {
        return false;
    }
    let mut count = 0;
    for i in 0..num_pat as usize {
        f.seek(start + 1084 + 1024 * i);
        if f.read(&mut pat_buf) < 1024 {
            return false;
        }
        if !validate_pattern(&pat_buf) {
            count += 1;
        }
    }
    count <= 2
}

fn is_st_ins(s: &[u8]) -> bool {
    (s[0] == b's' || s[0] == b'S')
        && (s[1] == b't' || s[1] == b'T')
        && s[2] == b'-'
        && s[5] == b':'
        && s[3].is_ascii_digit()
        && s[4].is_ascii_digit()
}

#[derive(Clone, Default)]
struct ModIns {
    name: [u8; 22],
    size: i32,
    finetune: u8,
    volume: u8,
    loop_start: i32,
    loop_size: i32,
}

struct ModHeader {
    ins: Vec<ModIns>,
    restart: u8,
}

/// `get_tracker_id`
fn get_tracker_id(m: &mut ModuleData, mh: &ModHeader, mut id: i32) -> i32 {
    let chn = m.module.chn;
    let has_loop_0 = mh.ins.iter().any(|i| i.loop_size == 0);
    let has_vol_in_empty_ins = mh.ins.iter().any(|i| i.size == 0 && i.volume > 0);
    let restart = mh.restart as i32;

    if restart == m.module.pat as i32 {
        id = if chn == 4 { TRACKER_SOUNDTRACKER } else { TRACKER_UNKNOWN };
    } else if restart == 0x78 {
        return if chn == 4 { TRACKER_PROBABLY_NOISETRACKER } else { TRACKER_UNKNOWN };
    } else if restart < 0x7f {
        id = if chn == 4 && !has_vol_in_empty_ins { TRACKER_NOISETRACKER } else { TRACKER_UNKNOWN };
        m.module.rst = mh.restart as usize;
    } else if restart == 0x7f {
        if chn == 4 {
            if has_loop_0 {
                id = TRACKER_CLONE;
            }
        } else {
            id = TRACKER_SCREAMTRACKER3;
        }
        return id;
    } else {
        return TRACKER_UNKNOWN;
    }

    if !has_loop_0 {
        for i in 0..31 {
            if mh.ins[i].size == 1 && mh.ins[i].volume == 0 {
                return TRACKER_CONVERTED;
            }
        }
        let has_st = mh.ins.iter().any(|i| is_st_ins(&i.name));
        if !has_st {
            for i in 0..31 {
                if mh.ins[i].size != 0 || mh.ins[i].loop_size != 1 {
                    continue;
                }
                return match chn {
                    4 => {
                        if has_vol_in_empty_ins {
                            TRACKER_OPENMPT
                        } else {
                            TRACKER_NOISETRACKER
                        }
                    }
                    6 | 8 => TRACKER_OCTALYSER,
                    _ => TRACKER_UNKNOWN,
                };
            }
            id = if chn == 4 {
                TRACKER_PROTRACKER
            } else if chn == 6 || chn == 8 {
                TRACKER_FASTTRACKER
            } else {
                TRACKER_UNKNOWN
            };
        }
    } else {
        let mut i = 15;
        while i < 31 {
            if mh.ins[i].name[0] != 0 || mh.ins[i].size > 0 {
                break;
            }
            i += 1;
        }
        if i == 31 && is_st_ins(&mh.ins[14].name) {
            return TRACKER_CONVERTEDST;
        }
        if mh.ins.iter().any(|i| is_st_ins(&i.name)) {
            return TRACKER_UNKNOWN_CONV;
        }
        if chn == 4 || chn == 6 || chn == 8 {
            return TRACKER_FASTTRACKER;
        }
        id = TRACKER_UNKNOWN;
    }
    id
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    load_at(m, f, 0)
}

/// `mod_load` for a module that starts at `start` in `f`.
pub fn load_at(m: &mut ModuleData, f: &mut Hio, start: usize) -> Result<(), ()> {
    f.seek(start);
    let size = f.size() as i64;
    m.module.ins = 31;
    m.module.smp = 31;
    m.module.chn = 0;
    let mut smp_size = 0i32;
    m.period_type = PeriodType::ModRng;

    let mut patbuf = vec![0u8; 1084];
    if f.read(&mut patbuf) < 1084 {
        return Err(());
    }
    let mut magic = [0u8; 8];
    magic[..4].copy_from_slice(&patbuf[1080..1084]);

    let mut tracker_id = TRACKER_PROTRACKER;
    let mut detected = false;
    for e in MOD_MAGIC {
        if magic[..4] == e.0[..] {
            m.module.chn = e.3;
            tracker_id = e.2;
            detected = e.1;
            break;
        }
    }
    let mut needs_timing_detection = tracker_id == TRACKER_PROTRACKER;
    if tracker_id == TRACKER_DIGITALTRACKER {
        f.read32b();
    }
    if m.module.chn == 0 {
        if &magic[2..4] == b"CH" && magic[0].is_ascii_digit() && magic[1].is_ascii_digit() {
            m.module.chn = ((magic[0] - b'0') as usize) * 10 + (magic[1] - b'0') as usize;
        } else if &magic[1..4] == b"CHN" && magic[0].is_ascii_digit() {
            m.module.chn = (magic[0] - b'0') as usize;
        } else {
            return Err(());
        }
        tracker_id = if m.module.chn & 1 != 0 { TRACKER_TAKETRACKER } else { TRACKER_FASTTRACKER2 };
        detected = true;
    }
    if tracker_id == TRACKER_SOFTWAREVISIONS {
        flip_word_bytes(&mut patbuf);
    }

    let mut maybe_wow = true;
    let mut mh = ModHeader { ins: vec![ModIns::default(); 31], restart: 0 };
    for i in 0..31 {
        let pos = 20 + i * 30;
        let ins = &mut mh.ins[i];
        ins.name.copy_from_slice(&patbuf[pos..pos + 22]);
        ins.size = rdb16(&patbuf, pos + 22);
        ins.finetune = patbuf[pos + 24];
        ins.volume = patbuf[pos + 25];
        ins.loop_start = rdb16(&patbuf, pos + 26);
        ins.loop_size = rdb16(&patbuf, pos + 28);
        if ins.size != 0 && (ins.finetune != 0 || ins.volume != 64) {
            maybe_wow = false;
        }
        smp_size += 2 * ins.size;
    }
    let mh_len = patbuf[950] as usize;
    mh.restart = patbuf[951];
    let mut order = [0u8; 128];
    order.copy_from_slice(&patbuf[952..1080]);
    if mh.restart != 0 {
        maybe_wow = false;
    }

    // strncpy(mod->name, mh.name, 20)
    let nm = &patbuf[..20];
    m.module.name = String::from_utf8_lossy(&nm[..nm.iter().position(|&b| b == 0).unwrap_or(20)]).into_owned();
    m.module.len = mh_len;
    m.module.xxo = [0; XMP_MAX_MOD_LENGTH];
    m.module.xxo[..128].copy_from_slice(&order);
    if mh.restart < 0x7f && mh.restart != 0x78 && (mh.restart as usize) < m.module.len {
        m.module.rst = mh.restart as usize;
    }
    for i in 0..128 {
        if m.module.xxo[i] > 0x7f {
            break;
        }
        if m.module.xxo[i] as usize > m.module.pat {
            m.module.pat = m.module.xxo[i] as usize;
        }
    }
    m.module.pat += 1;

    m.init_instrument();
    for i in 0..m.module.ins {
        if mh.ins[i].size >= 0x8000 {
            tracker_id = TRACKER_OPENMPT;
            needs_timing_detection = false;
            detected = true;
        }
        let xxs = &mut m.module.xxs[i];
        xxs.len = 2 * mh.ins[i].size;
        xxs.lps = 2 * mh.ins[i].loop_start;
        xxs.lpe = xxs.lps + 2 * mh.ins[i].loop_size;
        if xxs.lpe > xxs.len {
            xxs.lpe = xxs.len;
        }
        xxs.flg = if mh.ins[i].loop_size > 1 && xxs.lpe >= 4 { XMP_SAMPLE_LOOP } else { 0 };
        let has = xxs.len > 0;
        let xxi = &mut m.module.xxi[i];
        xxi.sub = vec![SubInstrument {
            fin: ((mh.ins[i].finetune << 4) as i8) as i32,
            vol: mh.ins[i].volume as i32,
            pan: XMP_INST_NO_DEFAULT_PAN,
            sid: i as i32,
            ..Default::default()
        }];
        xxi.name = copy_adjust(&mh.ins[i].name, 22);
        if has {
            xxi.nsm = 1;
        }
    }

    // Experimental tracker-detection routine
    let mut ptsong = false;
    let pat = m.module.pat as i64;
    if !detected {
        let mut skip_test = false;
        // Flextrax
        let flex_pos = 0x43c + pat * 4 * m.module.chn as i64 * 0x40 + smp_size as i64;
        if flex_pos < size {
            let pos = f.tell();
            f.seek(start + flex_pos as usize);
            let mut idb = [0u8; 4];
            let num_read = f.read(&mut idb);
            // libxmp seeks back to start + pos, pos being absolute already, which
            // only works for a module at the start of the file (not in a UMX)
            f.seek(pos);
            if num_read == 4 && &idb == b"FLEX" {
                tracker_id = TRACKER_FLEXTRAX;
                needs_timing_detection = false;
                detected = true;
                skip_test = true;
            }
        }
        if !skip_test {
            if &magic[..4] == b"M.K." && maybe_wow && (0x43c + pat * 32 * 0x40 + smp_size as i64) == (size & !1) {
                m.module.chn = 8;
                tracker_id = TRACKER_MODSGRAVE;
                needs_timing_detection = false;
                detected = true;
            } else {
                ptsong = &magic[..4] == b"M.K." && (0x43c + pat * 0x400 == size);
                if ptsong {
                    tracker_id = TRACKER_PROTRACKER;
                    detected = true;
                } else {
                    tracker_id = get_tracker_id(m, &mh, tracker_id);
                }
            }
        }
    }

    if m.module.chn >= XMP_MAX_CHANNELS {
        return Err(());
    }
    m.module.trk = m.module.chn * m.module.pat;

    // Load and convert patterns
    let chn = m.module.chn;
    let mut patbuf = vec![0u8; 64 * 4 * chn];
    let mut pat_high_fxx = [0u8; 256];
    let mut out_of_range = false;
    let mut invert_loop = false;
    let mut high_fxx = false;
    let mut samerow_fxx = false;
    for i in 0..m.module.pat {
        let patlen = 64 * 4 * chn;
        m.module.alloc_pattern_tracks(i, 64);
        if f.read(&mut patbuf) < patlen {
            return Err(());
        }
        if tracker_id == TRACKER_SOFTWAREVISIONS && i == 0 {
            flip_word_bytes(&mut patbuf[..patlen]);
        }
        let mut o = 0;
        for _j in 0..64 {
            let mut speed_row = false;
            let mut bpm_row = false;
            for _k in 0..chn {
                let e = &patbuf[o..o + 4];
                let period = ((lsn(e[0]) as i32) << 8) | e[1] as i32;
                if period != 0 && (period < 108 || period > 907) {
                    out_of_range = true;
                }
                if tracker_id == TRACKER_PROBABLY_NOISETRACKER {
                    let fxt = lsn(e[2]);
                    let fxp = lsn(e[3]);
                    if (fxt > 0x06 && fxt < 0x0a) || (fxt == 0x0e && fxp > 1) {
                        tracker_id = TRACKER_UNKNOWN;
                    }
                }
                if lsn(e[2]) == 0x0f {
                    if e[3] >= 0x20 {
                        pat_high_fxx[i] = e[3];
                        m.compare_vblank = true;
                        high_fxx = true;
                        bpm_row = true;
                    } else {
                        speed_row = true;
                    }
                }
                if lsn(e[2]) == 0xe && msn(e[3]) == 0xf {
                    invert_loop = true;
                }
                o += 4;
            }
            if bpm_row && speed_row {
                samerow_fxx = true;
            }
        }
        if out_of_range {
            if tracker_id == TRACKER_UNKNOWN && mh.restart == 0x7f {
                tracker_id = TRACKER_SCREAMTRACKER3;
            }
            if tracker_id == TRACKER_PROTRACKER
                || tracker_id == TRACKER_NOISETRACKER
                || tracker_id == TRACKER_PROBABLY_NOISETRACKER
                || tracker_id == TRACKER_SOUNDTRACKER
            {
                tracker_id = TRACKER_UNKNOWN;
            }
        } else if invert_loop
            && !detected
            && (tracker_id == TRACKER_NOISETRACKER || tracker_id == TRACKER_PROBABLY_NOISETRACKER)
        {
            tracker_id = TRACKER_PROTRACKER;
        }

        let mut o = 0;
        for j in 0..64 {
            for k in 0..chn {
                let ev = m.module.event_mut(i, k, j);
                match tracker_id {
                    TRACKER_PROBABLY_NOISETRACKER | TRACKER_NOISETRACKER => decode_noisetracker_event(ev, &patbuf[o..o + 4]),
                    _ => decode_protracker_event(ev, &patbuf[o..o + 4]),
                }
                o += 4;
            }
        }
    }

    // VBlank detection routine
    if !needs_timing_detection {
        if tracker_is_vblank(tracker_id) {
            m.quirk |= QUIRK_NOBPM;
        }
        m.compare_vblank = false;
    } else if samerow_fxx {
        if tracker_id == TRACKER_NOISETRACKER
            || tracker_id == TRACKER_PROBABLY_NOISETRACKER
            || tracker_id == TRACKER_SOUNDTRACKER
        {
            tracker_id = TRACKER_UNKNOWN;
        }
        m.compare_vblank = false;
    } else if high_fxx && m.module.len >= 8 {
        let threshold = m.module.len - 2;
        let mut i = 0;
        while i < threshold {
            if pat_high_fxx[m.module.xxo[i] as usize] != 0 {
                break;
            }
            i += 1;
        }
        if i == threshold {
            let mut i = m.module.len as i64 - 1;
            while i >= threshold as i64 {
                let fxx = pat_high_fxx[m.module.xxo[i as usize] as usize];
                if fxx == 0x00 {
                    i -= 1;
                    continue;
                }
                if fxx == 0x7d {
                    break;
                }
                m.compare_vblank = false;
                m.quirk |= QUIRK_NOBPM;
                break;
            }
        }
    }

    if invert_loop && !detected && !out_of_range {
        if tracker_id != TRACKER_OPENMPT {
            tracker_id = TRACKER_PROTRACKER;
        }
        detected = true;
    }

    let mut ptkloop = false;
    let tracker = match tracker_id {
        TRACKER_PROTRACKER => {
            ptkloop = true;
            "Protracker"
        }
        TRACKER_PROBABLY_NOISETRACKER | TRACKER_NOISETRACKER => "Noisetracker",
        TRACKER_SOUNDTRACKER => "Soundtracker",
        TRACKER_FASTTRACKER | TRACKER_FASTTRACKER2 => {
            m.period_type = PeriodType::Amiga;
            "Fast Tracker"
        }
        TRACKER_TAKETRACKER => {
            m.period_type = PeriodType::Amiga;
            "Take Tracker"
        }
        TRACKER_OCTALYSER => {
            if detected {
                m.flow_mode = FLOW_MODE_OCTALYSER;
            }
            "Octalyser"
        }
        TRACKER_DIGITALTRACKER => {
            m.flow_mode = FLOW_MODE_DTM_2015;
            "Digital Tracker"
        }
        TRACKER_FLEXTRAX => "Flextrax",
        TRACKER_MODSGRAVE => "Mod's Grave",
        TRACKER_SCREAMTRACKER3 => {
            m.period_type = PeriodType::Amiga;
            "Scream Tracker"
        }
        TRACKER_SOFTWAREVISIONS => "Software Visions DMF",
        TRACKER_CONVERTEDST | TRACKER_CONVERTED => "Converted",
        TRACKER_CLONE => {
            m.period_type = PeriodType::Amiga;
            "Protracker clone"
        }
        TRACKER_OPENMPT => {
            ptkloop = true;
            "OpenMPT"
        }
        _ => {
            m.period_type = PeriodType::Amiga;
            "Unknown tracker"
        }
    };
    if out_of_range {
        m.period_type = PeriodType::Amiga;
    }
    m.module.type_ = if tracker_id == TRACKER_MODSGRAVE || tracker_id == TRACKER_SOFTWAREVISIONS {
        tracker.to_string()
    } else {
        let mg = &magic[..magic.iter().position(|&b| b == 0).unwrap_or(8)];
        format!("{} {}", tracker, String::from_utf8_lossy(mg))
    };

    // Load samples
    for i in 0..m.module.smp {
        if m.module.xxs[i].len == 0 {
            continue;
        }
        let mut flags = if ptkloop && m.module.xxs[i].lps == 0 { SAMPLE_FLAG_FULLREP } else { 0 };
        if ptsong {
            let Some(tmpname) = super::stm::name_for_fopen(&m.module.xxi[i].name, 32) else { continue };
            let Some(data) = super::stm::read_instrument_file(m, &tmpname) else { continue };
            load_sample(m, &mut Hio::new(&data), flags, i, None)?;
        } else {
            let pos = f.tell();
            let mut b5 = [0u8; 5];
            let num = f.read(&mut b5);
            if num == 5 && &b5 == b"ADPCM" {
                flags |= SAMPLE_FLAG_ADPCM;
            } else {
                f.seek(pos);
            }
            load_sample(m, f, flags, i, None)?;
        }
    }

    if tracker_id == TRACKER_PROTRACKER || tracker_id == TRACKER_OPENMPT {
        m.quirk |= QUIRK_PROTRACK;
    } else if tracker_id == TRACKER_SCREAMTRACKER3 {
        m.c4rate = C4_NTSC_RATE;
        m.quirk |= QUIRKS_ST3;
        m.read_event_type = ReadEvent::St3;
    } else if tracker_id == TRACKER_FASTTRACKER
        || tracker_id == TRACKER_FASTTRACKER2
        || tracker_id == TRACKER_TAKETRACKER
        || tracker_id == TRACKER_MODSGRAVE
        || m.module.chn > 4
    {
        m.c4rate = C4_NTSC_RATE;
        m.quirk |= QUIRKS_FT2;
        m.read_event_type = ReadEvent::Ft2;
        m.period_type = PeriodType::Amiga;
    }
    Ok(())
}
