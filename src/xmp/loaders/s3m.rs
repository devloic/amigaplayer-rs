//! Scream Tracker 3 modules (`loaders/s3m_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//!
//! S3M: parapointed instruments and packed 64-row patterns, played with the ST3
//! event reader and quirks. The tracker ID picks the flow mode and the ST3 bug
//! compatibility as libxmp does for files written by ModPlug, OpenMPT, Impulse
//! Tracker, Imago Orpheus, Schism Tracker and the others. OPL2 (AdLib)
//! instruments load as libxmp loads them: named, with their volume and pitch,
//! and no sample data; not in libxmp, their 12 register bytes are kept in
//! `opl_patches`, and the player plays them on the Opal OPL3 emulator
//! (`xmp::opl_glue`).

use crate::xmp::load::*;
use crate::xmp::module::*;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 48 && &d[44..48] == b"SCRM" && d[29] == 0x10
}

const NONE: u8 = 0xff;
const FX_S3M_EXTENDED: u8 = 0xfe;

/// The effect conversion table, A to Z.
const FX: [u8; 27] = [
    NONE,
    FX_S3M_SPEED,     // Axx  Set speed to xx (the default is 06)
    FX_JUMP,          // Bxx  Jump to order xx (hexadecimal)
    FX_BREAK,         // Cxx  Break pattern to row xx (decimal)
    FX_VOLSLIDE,      // Dxy  Volume slide down by y/up by x
    FX_PORTA_DN,      // Exx  Slide down by xx
    FX_PORTA_UP,      // Fxx  Slide up by xx
    FX_TONEPORTA,     // Gxx  Tone portamento with speed xx
    FX_VIBRATO,       // Hxy  Vibrato with speed x and depth y
    FX_TREMOR,        // Ixy  Tremor with ontime x and offtime y
    FX_S3M_ARPEGGIO,  // Jxy  Arpeggio with halfnote additions
    FX_VIBRA_VSLIDE,  // Kxy  Dual command: H00 and Dxy
    FX_TONE_VSLIDE,   // Lxy  Dual command: G00 and Dxy
    NONE,
    NONE,
    FX_OFFSET,        // Oxy  Set sample offset
    NONE,
    FX_MULTI_RETRIG,  // Qxy  Retrig (+volumeslide) note
    FX_TREMOLO,       // Rxy  Tremolo with speed x and depth y
    FX_S3M_EXTENDED,  // Sxx  (misc effects)
    FX_S3M_BPM,       // Txx  Tempo = xx (hex)
    FX_FINE_VIBRATO,  // Uxx  Fine vibrato
    FX_GLOBALVOL,     // Vxx  Set global volume
    NONE,
    FX_SETPAN,        // Xxx  Set pan
    NONE,
    NONE,
];

/// `xlat_fx`: the S3M effect of `e` in libxmp's terms.
fn xlat_fx(e: &mut Event) {
    let (h, l) = (msn(e.fxp), lsn(e.fxp));
    if e.fxt as usize >= FX.len() {
        e.fxt = 0;
        e.fxp = 0;
        return;
    }
    e.fxt = FX[e.fxt as usize];
    match e.fxt {
        FX_S3M_BPM => {
            if e.fxp < 0x20 {
                e.fxp = 0;
                e.fxt = 0;
            }
        }
        FX_S3M_EXTENDED => {
            e.fxt = FX_EXTENDED;
            match h {
                0x1 => e.fxp = l | (EX_GLISS << 4),
                0x2 => e.fxp = (l.wrapping_sub(8) & 0x0f) | (EX_FINETUNE << 4),
                0x3 => e.fxp = l | (EX_VIBRATO_WF << 4),
                0x4 => e.fxp = l | (EX_TREMOLO_WF << 4),
                0x5 | 0x6 | 0x7 | 0x9 | 0xa => {
                    e.fxt = 0;
                    e.fxp = 0;
                }
                0x8 => {
                    e.fxt = FX_SETPAN;
                    e.fxp = l << 4;
                }
                0xb => e.fxp = l | (EX_PATTERN_LOOP << 4),
                0xc => {
                    if l == 0 {
                        e.fxt = 0;
                        e.fxp = 0;
                    }
                }
                _ => {}
            }
        }
        FX_SETPAN => {
            // X00-X80 is left to right, XA4 surround (Saga Musix)
            if e.fxp == 0xa4 {
                e.fxt = FX_SURROUND;
                e.fxp = 1;
            } else {
                e.fxp = ((e.fxp as i32) << 1).min(0xff) as u8;
            }
        }
        NONE => {
            e.fxt = 0;
            e.fxp = 0;
        }
        _ => {}
    }
}

const S3M_EOR: u8 = 0;
const S3M_CH_MASK: u8 = 0x1f;
const S3M_NI_FOLLOW: u8 = 0x20;
const S3M_VOL_FOLLOWS: u8 = 0x40;
const S3M_FX_FOLLOWS: u8 = 0x80;
const S3M_MV_VOLUME: u8 = 0x7f;
const S3M_MV_STEREO: u8 = 0x80;
const S3M_CH_OFF: u8 = 0xff;
const S3M_CH_NUMBER: u8 = 0x1f;
const S3M_CH_RIGHT: u8 = 0x08;
const S3M_CH_ADLIB: u8 = 0x10;
const S3M_PAN_SET: u8 = 0x20;
const S3M_AMIGA_RANGE: u16 = 0x10;
const S3M_ST300_VOLS: u16 = 0x40;
const S3M_SAMP_LOOP: u8 = 0x01;
const S3M_SAMP_STEREO: u8 = 0x02;
const S3M_SAMP_16BIT: u8 = 0x04;

fn rd16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Whether libopenmpt 0.8.7 takes the module for one saved by Scream Tracker 3 itself (its
/// `isST3`): tracker ID 1, but not Sound Club 2 and none of the trackers that write 0x1320
/// with tell-tale header fields (ModPlug / OpenMPT 1.17, Schism, PlayerPRO, Velvet Studio,
/// Impulse Tracker before 1.03).
fn made_by_st3(h: &[u8], offsets_canonical: bool) -> bool {
    let flags = rd16(h, 38);
    let version = rd16(h, 40);
    let ordnum = rd16(h, 32);
    let (uc, dp) = (h[52], h[53]);
    let special = rd16(h, 62);
    if version >> 12 != 1 || &h[54..62] == b"SCLUB2.0" {
        return false;
    }
    let fingerprinted = version == 0x1320
        && special == 0
        && uc == 0
        && ((ordnum & 1 == 0 && flags & !0x50 == 0 && dp == 0xfc && offsets_canonical)
            || (flags == 0 && dp != 0xfc)
            || (flags == 8 && dp != 0xfc));
    !fingerprinted
}

/// The tracker libxmp names from the header's tracker ID, and the flow mode
/// and quirks that go with it.
fn tracker(m: &mut ModuleData, h: &[u8]) -> String {
    let flags = rd16(h, 38);
    let version = rd16(h, 40);
    let ordnum = rd16(h, 32);
    let (gv, mv, uc, dp) = (h[48], h[51], h[52], h[53]);
    let rsvd2 = &h[54..62];
    let special = rd16(h, 62);
    let maj = (version & 0x0f00) >> 8;
    let min = version & 0xff;
    match version >> 12 {
        1 => {
            if version == 0x1320 && special == 0 && ordnum & 0x0f == 0 && uc == 0 && flags & !0x50 == 0 && dp == 0xfc {
                m.flow_mode = FLOW_MODE_MPT_116;
                if mv & 0x80 != 0 {
                    "ModPlug Tracker / OpenMPT 1.17".into()
                } else {
                    // MPT 1.0 alpha5 doesn't set the stereo flag, but MPT 1.0 alpha6 does
                    "ModPlug Tracker 1.0 alpha".into()
                }
            } else if version == 0x1320 && special == 0 && uc == 0 && flags == 0 && dp == 0 {
                if gv == 64 && mv == 48 { "PlayerPRO".into() } else { "Velvet Studio".into() }
            } else {
                m.quirk |= QUIRK_ST3BUGS;
                if version < 0x1303 {
                    m.flow_mode = FLOW_MODE_ST3_301;
                }
                format!("Scream Tracker {maj}.{min:02x}")
            }
        }
        2 => {
            if version == 0x2013 {
                // PlayerPRO on Intel doesn't byte-swap the tracker ID bytes
                "PlayerPRO".into()
            } else {
                m.flow_mode = FLOW_MODE_ORPHEUS;
                format!("Imago Orpheus {maj}.{min:02x}")
            }
        }
        3 => {
            m.flow_mode = FLOW_MODE_IT_210;
            match version {
                0x3216 => "Impulse Tracker 2.14v3".into(),
                0x3217 => "Impulse Tracker 2.14v5".into(),
                _ => format!("Impulse Tracker {maj}.{min:02x}"),
            }
        }
        5 => {
            m.quirk |= QUIRK_ST3BUGS;
            if version == 0x5447 {
                "Graoumf Tracker".into()
            } else if rsvd2[0] != 0 || rsvd2[1] != 0 {
                format!("OpenMPT {maj}.{min:02x}.{:02x}.{:02x}", rsvd2[1], rsvd2[0])
            } else {
                format!("OpenMPT {maj}.{min:02x}")
            }
        }
        4 if version != 0x4100 => schism_tracker_string((version & 0x0fff) as i32, rsvd2[0] as i32 | (rsvd2[1] as i32) << 8),
        4 | 6 => format!("BeRoTracker {maj}.{min:02x}"),
        _ => format!("unknown ({version:04x})"),
    }
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    load_at(m, f, 0)
}

/// `s3m_load` for a module that starts at `start` in `f`.
pub fn load_at(m: &mut ModuleData, f: &mut Hio, start: usize) -> Result<(), ()> {
    f.seek(start);
    let mut h = [0u8; 96];
    if f.read(&mut h) != 96 {
        return Err(());
    }
    let ordnum = rd16(&h, 32) as usize;
    let insnum = rd16(&h, 34) as usize;
    let patnum = rd16(&h, 36) as usize;
    let flags = rd16(&h, 38);
    let ffi = rd16(&h, 42);
    if ffi != 1 && ffi != 2 {
        return Err(());
    }
    // (not libxmp 4.7.4, which refuses 256 orders: Scream Tracker 3 keeps up to 256, st3play
    // and libopenmpt 0.8.7 read them all, and they fit XMP_MAX_MOD_LENGTH; Loki's 20mc10sp)
    if ordnum > 256 || insnum > 255 || patnum > 255 {
        return Err(());
    }
    if &h[44..48] != b"SCRM" {
        return Err(());
    }
    let (is, it, mv, dp) = (h[49], h[50], h[51], h[53]);
    let chset = &h[64..96];

    m.module.name = copy_adjust(&h[..28], 28);
    if flags & S3M_AMIGA_RANGE != 0 {
        m.period_type = PeriodType::ModRng;
    }
    if flags & S3M_ST300_VOLS != 0 {
        m.quirk |= QUIRK_VSALL;
    }
    m.module.spd = is as i32;
    m.module.bpm = it as i32;
    m.module.chn = 0;

    // Mix volume and stereo flag conversion (reported by Saga Musix):
    // the old format uses mix volume 0-7 and the stereo flag 0x10; newer ST3s
    // unconditionally convert MV 0x02 and 0x12 to 0x20.
    m.mvolbase = 48;
    let stereo;
    if ffi == 1 {
        m.mvol = (((mv & 0xf) as i32 + 1) * 0x10).clamp(0x10, 0x7f);
        stereo = mv & 0x10 != 0;
    } else if mv == 0x02 || mv == 0x12 {
        m.mvol = 0x20;
        stereo = mv & 0x10 != 0;
    } else {
        m.mvol = (mv & S3M_MV_VOLUME) as i32;
        stereo = mv & S3M_MV_STEREO != 0;
        if m.mvol == 0 {
            m.mvol = 48;
        } else if m.mvol < 16 {
            m.mvol = 16;
        }
    }
    // "in stereo, the mastermul is internally multiplied by 11/8": the inverse
    // in mono affects fewer modules
    if !stereo {
        m.mvol = m.mvol * 8 / 11;
    }
    // (not libxmp) OpenMPT's level for OPL instruments, "approximately as loud
    // as in DOSBox and a real SoundBlaster 16", scaled as the mix volume in mono
    m.opl_vsti_volume = if stereo { 36 } else { (36 * 8 + 5) / 11 };

    for (i, &cs) in chset.iter().enumerate() {
        if cs == S3M_CH_OFF {
            continue;
        }
        m.module.chn = i + 1;
        let x = cs & S3M_CH_NUMBER;
        m.module.xxc[i].pan = if stereo && x < S3M_CH_ADLIB {
            if x < S3M_CH_RIGHT { 0x30 } else { 0xc0 }
        } else {
            0x80
        };
    }

    // ordnum is at most 256, XMP_MAX_MOD_LENGTH
    m.module.len = ordnum;
    if f.read(&mut m.module.xxo[..ordnum]) != ordnum {
        return Err(());
    }

    // don't trust patnum
    let mut pat: i32 = -1;
    for &o in &m.module.xxo[..ordnum] {
        if o < 0xfe && o as i32 > pat {
            pat = o as i32;
        }
    }
    if pat < 0 && patnum > 0 {
        // (not libxmp 4.7.4, which refuses a module whose order list names no pattern, "no
        // song") st3play, the model of ST3's player, plays pattern 0 then: its neworder() finds
        // no pattern and leaves the current one, 0, which plays once; the song ends there.
        // libopenmpt 0.8.7 plays nothing. SiD's DRIVIN, VOODAA and TEMP (orders FF FF) start
        // in pattern 0; Purple Motion's benefx5 has no pattern data at all and stays silent.
        m.module.len = 1;
        m.module.xxo[0] = 0;
        pat = 0;
    }
    let pat = ((pat + 1) as usize).min(patnum);
    if pat == 0 {
        return Err(());
    }
    m.module.pat = pat;
    m.module.trk = pat * m.module.chn;
    m.module.ins = insnum;
    m.module.smp = insnum;

    let pp_ins: Vec<usize> = (0..insnum).map(|_| f.read16l() as usize).collect();
    let pp_pat: Vec<usize> = (0..patnum).map(|_| f.read16l() as usize).collect();

    // default pan positions
    if dp == 0xfc {
        for i in 0..32 {
            let x = f.read8();
            if x & S3M_PAN_SET != 0 {
                m.module.xxc[i].pan = ((x as i32) << 4) & 0xff;
            }
        }
    }

    m.c4rate = C4_NTSC_RATE;
    m.flow_mode = FLOW_MODE_ST3_321;
    if rd16(&h, 40) == 0x1300 {
        m.quirk |= QUIRK_VSALL;
    }
    let t = tracker(m, &h);
    m.module.type_ = format!("{t} S3M");
    m.module.type_.truncate(63);

    let mut dummy = Event::default();
    for i in 0..pat {
        m.module.alloc_pattern_tracks(i, 64);
        if pp_pat[i] == 0 {
            continue;
        }
        if start + pp_pat[i] * 16 + 2 > f.size() {
            // (not libxmp 4.7.4, which refuses the module: its pattern length reads 0xffff
            // at the end of the file. A truncated file's last pattern: libopenmpt 0.8.7 keeps
            // it as an empty pattern, as a zero pointer; Statix's "...4LS" and "Fire Eyes",
            // whose last pattern points at the end of the file. st3play refuses them.)
            continue;
        }
        f.seek(start + pp_pat[i] * 16);
        let mut r = 0;
        let mut pat_len = f.read16l() as i32 - 2;
        while pat_len >= 0 && r < 64 {
            let b = f.read8();
            if f.take_error() {
                return Err(());
            }
            if b == S3M_EOR {
                r += 1;
                continue;
            }
            let c = (b & S3M_CH_MASK) as usize;
            let event = if c >= m.module.chn { &mut dummy } else { m.module.event_mut(i, c, r) };
            if b & S3M_NI_FOLLOW != 0 {
                event.note = match f.read8() {
                    255 => 0, // empty note
                    254 => XMP_KEY_OFF,
                    n => 13 + 12 * msn(n) + lsn(n),
                };
                event.ins = f.read8();
                pat_len -= 2;
            }
            if b & S3M_VOL_FOLLOWS != 0 {
                event.vol = f.read8().wrapping_add(1);
                pat_len -= 1;
            }
            if b & S3M_FX_FOLLOWS != 0 {
                event.fxt = f.read8();
                event.fxp = f.read8();
                xlat_fx(event);
                pat_len -= 2;
            }
        }
    }

    m.init_instrument();
    let mut buf = [0u8; 80];
    let mut gus_addresses = 0u16;
    for i in 0..insnum {
        f.seek(start + pp_ins[i] * 16);
        if f.read(&mut buf) != 80 {
            return Err(());
        }
        if buf[0] < 2 {
            gus_addresses |= rd16(&buf, 0x28);
        }
        let mut sub = SubInstrument { pan: XMP_INST_NO_DEFAULT_PAN, sid: i as i32, ..Default::default() };

        if buf[0] > 2 && &buf[76..80] != b"SCRI" {
            // (not libxmp 4.7.4, which refuses the module: no instrument type of ST3 and no
            // "SCRI", an empty slot. libopenmpt 0.8.7 keeps only its name, volume and C-5 rate,
            // with no sample or OPL patch (types 3 and up), st3play plays no sample from it
            // (type 1 only); Shrike's "DarkNess Remix", instrument 90 of type 255. Loaded as
            // libxmp loads an empty slot of type 0 with length 0.)
            sub.vol = buf[28] as i32;
            (sub.xpo, sub.fin) = c2spd_to_note(rd16(&buf, 32) as i32);
            m.module.xxi[i].name = copy_adjust(&buf[48..76], 28);
            m.module.xxi[i].sub = vec![sub];
            continue;
        }
        if buf[0] >= 2 {
            // OPL2 FM instrument: libxmp keeps no sample for it
            if &buf[76..80] != b"SCRI" {
                return Err(());
            }
            m.module.xxi[i].name = copy_adjust(&buf[48..76], 28);
            m.module.xxi[i].nsm = 1;
            sub.vol = buf[28] as i32;
            let (xpo, fin) = c2spd_to_note(rd16(&buf, 32) as i32);
            sub.xpo = xpo + 12;
            sub.fin = fin;
            m.module.xxi[i].sub = vec![sub];
            // (not libxmp: the 12 register bytes, which the OPL synthesis plays)
            if m.opl_patches.len() < insnum {
                m.opl_patches.resize(insnum, None);
            }
            let mut patch = [0u8; 12];
            patch.copy_from_slice(&buf[16..28]);
            m.opl_patches[i] = Some(patch);
            continue;
        }

        let memseg_hi = buf[13] as usize;
        let memseg = rd16(&buf, 14) as usize;
        let mut length = rd32(&buf, 16);
        if length > MAX_SAMPLE_SIZE as u32 {
            // (not libxmp 4.7.4, which refuses the module: an empty slot, type 0, holds no
            // sample, and Impulse Tracker 2.14 left a garbage length in one, 33213_alarma.s3m's
            // instrument 13; libopenmpt plays the file. Only a module libxmp refuses gets here.)
            if buf[0] != 0 {
                return Err(());
            }
            length = 0;
        }
        let loopbeg = rd32(&buf, 20);
        let loopend = rd32(&buf, 24);
        let vol = buf[28];
        let pack = buf[30];
        let sflags = buf[31];
        let c2spd = rd16(&buf, 32);
        // (not libxmp 4.7.4, which refuses any sample header without "SCRS": one of length 0
        // holds no sample, and neither st3play nor libopenmpt 0.8.7 reads the magic; Tangerine's
        // "Soul Surfer soundtrack", instruments 28 and 29: type 1, length 0, no name, no magic.)
        if buf[0] == 1 && &buf[76..80] != b"SCRS" && length != 0 {
            return Err(());
        }

        let xxs = &mut m.module.xxs[i];
        xxs.len = length as i32;
        xxs.lps = loopbeg as i32;
        xxs.lpe = loopend as i32;
        xxs.flg = if sflags & S3M_SAMP_LOOP != 0 { XMP_SAMPLE_LOOP } else { 0 };
        if sflags & S3M_SAMP_STEREO != 0 {
            xxs.flg |= XMP_SAMPLE_STEREO;
        }
        if sflags & S3M_SAMP_16BIT != 0 {
            xxs.flg |= XMP_SAMPLE_16BIT;
        }
        let mut load_flags = if ffi == 1 { 0 } else { SAMPLE_FLAG_UNS };
        if pack == 4 {
            load_flags = SAMPLE_FLAG_ADPCM;
        }

        sub.vol = vol as i32;
        let (xpo, fin) = c2spd_to_note(c2spd as i32);
        sub.xpo = xpo;
        sub.fin = fin;
        let xxi = &mut m.module.xxi[i];
        xxi.nsm = if length > 0 { 1 } else { 0 };
        xxi.name = copy_adjust(&buf[48..76], 28);
        xxi.sub = vec![sub];

        f.seek(start + 16 * (memseg + (memseg_hi << 16)));
        load_sample(m, f, load_flags, i, None)?;
    }

    // (not libxmp 4.7.4, which always applies the header's mix volume) Scream Tracker 3's
    // GUS driver doesn't use it. ST3 saved with the GUS driver loaded writes a distinct GUS
    // memory address in each used sample slot; with the SoundBlaster driver they are 0 or 1.
    // So, as libopenmpt 0.8.7 (Load_s3m.cpp) does, a module made by ST3 whose OR of
    // addresses is above 1 plays its samples at mix volume 48, whatever the header says and
    // in mono too. That keeps the balance between samples and AdLib instruments the composer
    // heard (Manwe's Rotagilla, mix volume 127, and "and this is UltrSound", 64).
    let canonical = !pp_pat.is_empty() && !pp_ins.is_empty() && pp_pat[0] > pp_ins[0];
    if gus_addresses > 1 && made_by_st3(&h, canonical) {
        m.mvol = 48;
    }

    m.quirk |= QUIRKS_ST3 | QUIRK_ARPMEM;
    m.read_event_type = ReadEvent::St3;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One pattern, one 4-byte sample, two orders, made by "Scream Tracker 3.20".
    fn synth() -> Vec<u8> {
        let mut d = vec![0u8; 0x60];
        d[..3].copy_from_slice(b"abc");
        d[28] = 0x1a;
        d[29] = 0x10;
        d[32] = 2; // orders
        d[34] = 1; // instruments
        d[36] = 1; // patterns
        d[38] = 0x08; // flags: not Velvet Studio's zero
        d[40..42].copy_from_slice(&0x1320u16.to_le_bytes());
        d[42] = 2; // unsigned samples
        d[44..48].copy_from_slice(b"SCRM");
        d[48] = 64;
        d[49] = 6;
        d[50] = 125;
        d[51] = 0xb0; // stereo, mix volume 48
        d[64..96].fill(0xff);
        d[64] = 0; // left
        d[65] = 8; // right
        d.extend([0, 0xff]); // orders at 0x60
        d.extend(7u16.to_le_bytes()); // instrument at 0x70
        d.extend(0x0cu16.to_le_bytes()); // pattern at 0xc0
        d.resize(0x70, 0);
        let mut ins = [0u8; 80];
        ins[0] = 1;
        ins[14..16].copy_from_slice(&0x0du16.to_le_bytes()); // sample at 0xd0
        ins[16..20].copy_from_slice(&4u32.to_le_bytes());
        ins[28] = 48;
        ins[32..34].copy_from_slice(&8363u16.to_le_bytes());
        ins[48..52].copy_from_slice(b"inst");
        ins[76..80].copy_from_slice(b"SCRS");
        d.extend(ins);
        d.resize(0xc0, 0);
        // row 0, channel 1: C-5, instrument 1, volume 32, speed 3, S82
        let p = [0xe1, 0x40, 1, 32, 1, 3, 0x80, 19, 0x82, 0];
        d.extend(((p.len() + 2) as u16).to_le_bytes());
        d.extend(p);
        d.resize(0xd0, 0);
        d.extend([0x80, 0x90, 0x70, 0x60]);
        d
    }

    #[test]
    fn loads_synthetic_s3m() {
        let d = synth();
        assert!(test(&d));
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&d)).unwrap();
        assert_eq!(m.module.type_, "Scream Tracker 3.20 S3M");
        assert_eq!((m.module.name.as_str(), m.module.chn, m.module.pat, m.module.len), ("abc", 2, 1, 2));
        assert_eq!((m.module.xxc[0].pan, m.module.xxc[1].pan), (0x30, 0xc0));
        assert_eq!((m.mvol, m.flow_mode, m.quirk & QUIRK_ST3BUGS), (48, FLOW_MODE_ST3_321, QUIRK_ST3BUGS));
        let e = *m.module.event(0, 1, 0);
        assert_eq!((e.note, e.ins, e.vol, e.fxt, e.fxp), (61, 1, 33, FX_S3M_SPEED, 3));
        let e = *m.module.event(0, 0, 0);
        assert_eq!((e.fxt, e.fxp), (FX_SETPAN, 0x20));
        assert_eq!(m.module.xxi[0].name, "inst");
        assert_eq!(m.module.xxs[0].data.as_ref().unwrap()[SAMPLE_GUARD..SAMPLE_GUARD + 4], [0, 0x10, 0xf0, 0xe0]);
        assert_eq!(m.read_event_type, ReadEvent::St3);
    }

    #[test]
    fn empty_slot_with_garbage_length_loads() {
        // instrument at 0x70: type 0 (empty) with a length over MAX_SAMPLE_SIZE
        let mut d = synth();
        d[0x70] = 0;
        d[0x70 + 16..0x70 + 20].copy_from_slice(&2_080_714_880u32.to_le_bytes());
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&d)).unwrap();
        assert_eq!((m.module.xxi[0].nsm, m.module.xxs[0].len), (0, 0));
        // a sample slot with that length is still refused
        d[0x70] = 1;
        assert!(load(&mut ModuleData::default(), &mut Hio::new(&d)).is_err());
    }

    /// The peak sample of the first second the libxmp port renders of `d`.
    fn peak(d: &[u8]) -> u16 {
        let mut p = crate::xmp::XmpPlayer::new(d, 48000).unwrap();
        let mut out = vec![0i16; 48000 * 2];
        let n = p.render(&mut out);
        out[..n * 2].iter().map(|s| s.unsigned_abs()).max().unwrap_or(0)
    }

    #[test]
    fn order_list_without_pattern_plays_pattern_0() {
        // orders FF FF: libxmp refuses "no song"; st3play plays pattern 0 once
        let mut d = synth();
        d[0x60] = 0xff;
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&d)).unwrap();
        assert_eq!((m.module.len, m.module.xxo[0], m.module.pat), (1, 0, 1));
        assert_eq!(m.module.event(0, 1, 0).note, 61);
        assert!(peak(&d) > 0 && peak(&d) == peak(&synth()));
        // separators only, the same
        d[0x60] = 0xfe;
        d[0x61] = 0xfe;
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&d)).unwrap();
        assert_eq!((m.module.len, m.module.xxo[0]), (1, 0));
        // a list that names a pattern is kept as it is
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&synth())).unwrap();
        assert_eq!((m.module.len, &m.module.xxo[..2]), (2, &[0u8, 0xff][..]));
        // no pattern at all: still refused
        d[36] = 0;
        assert!(load(&mut ModuleData::default(), &mut Hio::new(&d)).is_err());
    }

    #[test]
    fn pattern_at_end_of_file_is_empty() {
        // the pattern's parapointer at the end of a (truncated) file
        let mut d = synth();
        d.resize(0xe0, 0);
        d[0x64] = 0x0e;
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&d)).unwrap();
        assert_eq!(m.module.pat, 1);
        assert_eq!(m.module.event(0, 1, 0).note, 0);
        // one byte short of the pattern's length word: empty too
        d.push(0x42);
        load(&mut ModuleData::default(), &mut Hio::new(&d)).unwrap();
        // pattern data cut short after its length word: still refused, as libxmp does
        d.push(0);
        assert!(load(&mut ModuleData::default(), &mut Hio::new(&d)).is_err());
    }

    #[test]
    fn sample_header_without_magic() {
        let mut d = synth();
        d[0x70 + 76..0x70 + 80].fill(0);
        // with a length: refused, as libxmp does
        assert!(load(&mut ModuleData::default(), &mut Hio::new(&d)).is_err());
        // length 0: an empty sample
        d[0x70 + 16..0x70 + 20].fill(0);
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&d)).unwrap();
        assert_eq!((m.module.xxi[0].nsm, m.module.xxs[0].len, m.module.xxi[0].name.as_str()), (0, 0, "inst"));
    }

    #[test]
    fn unknown_instrument_type_is_an_empty_slot() {
        let mut d = synth();
        d[0x70] = 0xff;
        d[0x70 + 76..0x70 + 80].fill(0);
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&d)).unwrap();
        assert_eq!((m.module.xxi[0].nsm, m.module.xxs[0].data.is_none()), (0, true));
        assert_eq!((m.module.xxi[0].name.as_str(), m.module.xxi[0].sub[0].vol), ("inst", 48));
        assert!(m.opl_patches.is_empty());
        assert_eq!(peak(&d), 0);
        // an AdLib melody instrument (type 2) without "SCRI": still refused, as libxmp does
        d[0x70] = 2;
        assert!(load(&mut ModuleData::default(), &mut Hio::new(&d)).is_err());
    }

    #[test]
    fn up_to_256_orders() {
        // synth() with 256 orders: 254 more FFs and 2 bytes of padding move the rest 16 paragraphs on
        let s = synth();
        let mut d = s[..0x62].to_vec();
        d[32..34].copy_from_slice(&256u16.to_le_bytes());
        d.resize(0x160, 0xff);
        d.extend(23u16.to_le_bytes()); // instrument at 0x170
        d.extend(0x1cu16.to_le_bytes()); // pattern at 0x1c0
        d.resize(0x170, 0);
        d.extend(&s[0x70..]);
        d[0x170 + 14..0x170 + 16].copy_from_slice(&0x1du16.to_le_bytes()); // sample at 0x1d0
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&d)).unwrap();
        assert_eq!((m.module.len, m.module.xxo[0], m.module.xxo[255], m.module.xxi[0].name.as_str()), (256, 0, 0xff, "inst"));
        assert_eq!(m.module.event(0, 1, 0).note, 61);
        assert!(peak(&d) > 0 && peak(&d) == peak(&synth()));
        d[32..34].copy_from_slice(&257u16.to_le_bytes());
        assert!(load(&mut ModuleData::default(), &mut Hio::new(&d)).is_err());
    }

    #[test]
    fn st3_gus_ignores_mix_volume() {
        let mut d = synth();
        d[51] = 0xc0; // stereo, mix volume 64
        d[52] = 16; // ultra-clicks as ST3.20 writes them (no "other tracker" fingerprint)
        let mvol = |d: &[u8]| {
            let mut m = ModuleData::default();
            load(&mut m, &mut Hio::new(d)).unwrap();
            m.mvol
        };
        // SoundBlaster driver: GUS address 0 or 1, the header's mix volume plays
        assert_eq!(mvol(&d), 64);
        d[0x70 + 0x28] = 1;
        assert_eq!(mvol(&d), 64);
        // GUS driver: a real GUS address, mix volume 48 whatever the header says
        d[0x70 + 0x28..0x70 + 0x2a].copy_from_slice(&0x1234u16.to_le_bytes());
        assert_eq!(mvol(&d), 48);
        d[51] = 0x7f; // mono, 127
        assert_eq!(mvol(&d), 48);
        // not made by ST3 (OpenMPT's tracker ID): the header's mix volume plays
        d[51] = 0xc0;
        d[40..42].copy_from_slice(&0x5131u16.to_le_bytes());
        assert_eq!(mvol(&d), 64);
        // ST3.20's ID written by Impulse Tracker < 1.03 (flags 8, no pan table, no clicks)
        d[40..42].copy_from_slice(&0x1320u16.to_le_bytes());
        d[52] = 0;
        assert_eq!(mvol(&d), 64);
    }

    /// synth() with its instrument made an AdLib one (type 2, `SCRI`).
    fn synth_adlib() -> Vec<u8> {
        let mut d = synth();
        let ins = 0x70;
        d[ins] = 2;
        let patch = [0x21, 0x21, 0x10, 0x00, 0xf2, 0xf2, 0x05, 0x05, 0x00, 0x00, 0x01, 0];
        d[ins + 16..ins + 28].copy_from_slice(&patch);
        d[ins + 76..ins + 80].copy_from_slice(b"SCRI");
        d
    }

    #[test]
    fn adlib_instrument_plays_on_the_opl() {
        let d = synth_adlib();
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&d)).unwrap();
        assert_eq!(m.opl_patches.len(), 1);
        assert_eq!(m.opl_patches[0].unwrap()[..4], [0x21, 0x21, 0x10, 0x00]);
        assert_eq!((m.module.xxi[0].nsm, m.module.xxs[0].data.is_none()), (1, true));
        let peak = |d: &[u8]| {
            let mut p = crate::xmp::XmpPlayer::new(d, 48000).unwrap();
            let mut out = vec![0i16; 48000 * 2];
            let n = p.render(&mut out);
            out[..n * 2].iter().map(|s| s.unsigned_abs()).max().unwrap_or(0)
        };
        // the note plays (libxmp alone renders it silent)
        // (volume 32 of 64 on the right: 630 at 48 kHz)
        assert!(peak(&d) > 300);
        // a module without AdLib instruments has no OPL
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&synth())).unwrap();
        assert!(m.opl_patches.is_empty());
        let mut c = crate::xmp::load_module(&synth()).unwrap();
        c.start_player(48000);
        assert!(c.opl.is_none());
    }

    #[test]
    fn tracker_ids() {
        let mut h = synth();
        let mut m = ModuleData::default();
        h[40..42].copy_from_slice(&0x4020u16.to_le_bytes());
        assert_eq!(tracker(&mut m, &h), "Schism Tracker 0.20");
        h[40..42].copy_from_slice(&0x4fffu16.to_le_bytes());
        h[54..56].copy_from_slice(&100u16.to_le_bytes());
        assert_eq!(tracker(&mut m, &h), "Schism Tracker 2010-02-08");
        h[40..42].copy_from_slice(&0x3216u16.to_le_bytes());
        assert_eq!(tracker(&mut m, &h), "Impulse Tracker 2.14v3");
        assert_eq!(m.flow_mode, FLOW_MODE_IT_210);
    }
}
