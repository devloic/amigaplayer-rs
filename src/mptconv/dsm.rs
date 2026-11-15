//! Digisound Interface Kit (DSIK) modules, the internal format of DSIK 2 (`RIFF` .. `DSMF`, or
//! `DSMF` first, `.dsm`), read as libopenmpt 0.8.7's `Load_dsm.cpp` (`ReadDSM`) reads them and
//! played as the Scream Tracker 3 module OpenMPT saves them as (its player gives a DSM module
//! S3M's behaviours, `GetBestSaveFormat`): a `SONG` chunk (title, speed, tempo, global and
//! master volume, channel panning 0..0x80, orders), `PATT` chunks (64 rows of packed cells with
//! MOD effects, panning 00..80 and A4 surround as in S3M) and `INST` chunks (sample header and
//! 8 or 16-bit signed, unsigned or delta PCM data).
//!
//! Left out: the restart position (the song ends at its end). DSIK DSM v1, a different format,
//! is not read by libopenmpt either.
//!
//! The same libopenmpt file also reads Dynamic Studio's `DSm\x1A` modules (`ReadDSm`, a
//! different format with the same extension): see [`read_dynamic`]. OpenMPT plays those as MOD
//! (Amiga periods, finetune from the PT tuned period table); they are written as S3M with the
//! MOD volume-slide and portamento rules (as for MadTracker 2).
//!
//! Ported from libopenmpt 0.8.7 (BSD-3-Clause, OpenMPT Project Developers and Contributors; see
//! LICENSE-openmpt).

use super::*;
use crate::xmp::load::Hio;

/// `ValidateHeader` + the `SONG` chunk right after it (`ProbeFileHeaderDSM`).
pub fn test(d: &[u8]) -> bool {
    song_at(d).is_some()
}

/// Where the first chunk after the file header starts, when it is the `SONG` chunk.
fn song_at(d: &[u8]) -> Option<usize> {
    if d.len() < 12 {
        return None;
    }
    let at = if &d[..4] == b"RIFF" && &d[8..12] == b"DSMF" {
        12
    } else if &d[..4] == b"DSMF" {
        16
    } else {
        return None;
    };
    (d.get(at..at + 4)? == b"SONG" && d.len() >= at + 8).then_some(at)
}

/// The module OpenMPT reads from `file`, or None when it does not load it.
pub fn read(file: &[u8]) -> Option<Module> {
    let at = song_at(file)?;
    let mut f = Reader::new(file);
    f.seek(at + 4);
    let size = f.u32() as usize;
    let h = f.struct_partial(192, size);
    let u16at = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]);
    let num_orders = u16at(36) as usize;
    let num_patterns = u16at(40) as usize;
    let num_channels = u16at(42) as usize;
    let restart = u16at(34) as usize;
    if num_orders > 128 || num_channels > 16 || num_patterns > 256 || restart > 128 {
        return None;
    }
    let channels = num_channels.max(1);
    let (global, master, speed, bpm) = (h[44], h[45], h[46], h[47]);
    let mut gv = global.min(64) as u32 * 4;
    if gv == 0 {
        gv = 256;
    }
    let preamp = if master == 0x80 { (256 / channels as u32).min(128) } else { (master & 0x7F) as u32 };
    let mut chn_pan = vec![128u16; channels];
    for (ch, p) in chn_pan.iter_mut().enumerate() {
        let v = h[48 + ch];
        if v <= 0x80 {
            *p = v as u16 * 2;
        }
    }
    // ReadOrderFromArray(.., 0xFF end, 0xFE ignore)
    let mut orders = Vec::new();
    for &o in &h[64..64 + num_orders] {
        orders.push(match o {
            0xFF => ORDER_END,
            0xFE => ORDER_SKIP,
            p => p as u16,
        });
    }

    let mut patterns: Vec<Option<Pattern>> = Vec::new();
    let mut samples: Vec<Sample> = Vec::new();
    while let Some(ch) = f.bytes(8) {
        let magic = [ch[0], ch[1], ch[2], ch[3]];
        let len = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]) as usize;
        let mut chunk = f.chunk(len);
        if &magic == b"PATT" {
            // (MAX_PATTERNS: 4000)
            if patterns.len() >= 4000 {
                continue;
            }
            let mut pat = Pattern::new(64, channels);
            chunk.skip(2);
            let mut row = 0usize;
            while chunk.can_read(1) && row < 64 {
                let flag = chunk.u8();
                if flag == 0 {
                    row += 1;
                    continue;
                }
                let c = (flag & 0x0F) as usize;
                let mut dummy = Cell::default();
                let m = if c < channels { &mut pat.cells[row * channels + c] } else { &mut dummy };
                if flag & 0x80 != 0 {
                    let mut note = chunk.u8();
                    if note != 0 {
                        if note <= 12 * 9 {
                            note += 11 + NOTE_MIN;
                        }
                        m.note = note;
                    }
                }
                if flag & 0x40 != 0 {
                    m.instr = chunk.u8();
                }
                if flag & 0x20 != 0 {
                    m.volcmd = VolCmd::Volume;
                    m.vol = chunk.u8().min(64);
                }
                if flag & 0x10 != 0 {
                    let (cmd, param) = match chunk.bytes(2) {
                        Some(b) => (b[0], b[1]),
                        None => (0, 0),
                    };
                    convert_mod_command(m, cmd, param);
                }
            }
            patterns.push(Some(pat));
        } else if &magic == b"INST" && samples.len() < 3999 {
            let (sh, _) = chunk.struct_or_zero(64);
            let u32at = |o: usize| u32::from_le_bytes([sh[o], sh[o + 1], sh[o + 2], sh[o + 3]]);
            let flags = u16::from_le_bytes([sh[13], sh[14]]);
            let mut s = Sample::new();
            s.filename = name(&sh[0..13]);
            s.c5speed = u32at(32);
            s.looped = flags & 1 != 0;
            s.length = u32at(16);
            s.loop_start = u32at(20);
            s.loop_end = u32at(24);
            s.volume = sh[15].min(64) as u16 * 4;
            s.name = name(&sh[36..64]);
            let enc = if flags & 0x40 != 0 {
                Enc::Delta
            } else if flags & 0x02 != 0 {
                Enc::Signed
            } else {
                Enc::Unsigned
            };
            s.data = read_pcm(&mut chunk, s.length, flags & 0x04 != 0, false, enc);
            s.bits16 = s.data.bits16;
            samples.push(s);
        }
    }

    Some(Module {
        title: name(&h[..28]),
        channels,
        speed,
        tempo: bpm as f64,
        global_vol: gv,
        preamp,
        linear: false,
        old_effects: false,
        compat_gxx: false,
        chn_surround: vec![false; channels],
        chn_vol: vec![64; channels],
        chn_pan,
        orders,
        patterns,
        instruments: Vec::new(),
        samples,
        rules: Rules::default(),
    })
}

/// The format name the libxmp port gives these modules.
pub const NAME: &str = "DSIK RIFF DSM";

/// The `xmp::LOADERS` entry: the module written as S3M, loaded by the libxmp port's S3M loader.
pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let file = whole_file(f);
    let md = read(&file).ok_or(())?;
    let (native, fix) = build(&md, Kind::S3m).ok_or(())?;
    load_native(m, &native, Kind::S3m, &fix, NAME)
}

// ---------------------------------------------------------------------------------------------
// Dynamic Studio (`DSm\x1A`)

/// The format name the libxmp port gives these modules.
pub const NAME_DYNAMIC: &str = "Dynamic Studio";

/// `ProTrackerTunedPeriods[finetune * 12]`: the C period of each MOD finetune 0..15.
const PT_C_PERIODS: [u32; 16] =
    [1712, 1700, 1688, 1676, 1664, 1652, 1640, 1628, 1814, 1800, 1788, 1774, 1762, 1750, 1736, 1724];

/// `DSmFileHeader::IsValid` and the minimum size of what follows it; None when invalid.
fn dynamic_header(d: &[u8]) -> Option<(usize, usize, usize)> {
    if d.len() < 64 || &d[..4] != b"DSm\x1A" {
        return None;
    }
    let (version, chn, smp, ord, gv) = (d[4], d[45] as usize, d[46] as usize, d[47] as usize, d[49]);
    if version != 0x20 || !(1..=16).contains(&chn) || smp == 0 || ord == 0 || gv > 100 {
        return None;
    }
    Some((chn, smp, ord))
}

/// `ProbeFileHeaderDSm`.
pub fn test_dynamic(d: &[u8]) -> bool {
    matches!(dynamic_header(d), Some((c, s, o)) if d.len() >= 64 + c + o + s * 32)
}

/// The module OpenMPT's `ReadDSm` reads from `file`, or None when it does not load it.
pub fn read_dynamic(file: &[u8]) -> Option<Module> {
    let (channels, num_samples, num_orders) = dynamic_header(file)?;
    if file.len() < 64 + channels + num_orders + num_samples * 32 {
        return None;
    }
    let mut f = Reader::new(file);
    f.seek(64);
    let global_vol = ((file[49] as u32 * 256) * 2 + 100) / 200;
    let chn_pan: Vec<u16> = (0..channels).map(|_| (f.u8() & 0x0F) as u16 * 0x11).collect();
    let mut orders = Vec::with_capacity(num_orders);
    for _ in 0..num_orders {
        orders.push(f.u8() as u16);
    }
    let num_patterns = orders.iter().copied().max().unwrap_or(0) as usize + 1;
    if !f.can_read(num_patterns * channels * 8 + num_samples * 32 + num_patterns * channels * 64 * 4) {
        return None;
    }
    // track names: nothing plays them
    f.skip(num_patterns * channels * 8);

    let mut samples: Vec<Sample> = Vec::with_capacity(num_samples);
    for _ in 0..num_samples {
        let (h, _) = f.struct_or_zero(32);
        let mut s = Sample::new();
        s.name = name(&h[0..22]);
        let ft = h[25] & 0x0F;
        s.c5speed = (8363.0 * 1712.0 / PT_C_PERIODS[ft as usize] as f64).round() as u32;
        s.bits16 = h[22] == 16;
        s.length = u16::from_le_bytes([h[23], h[24]]) as u32;
        s.volume = h[26].min(64) as u16 * 4;
        let loop_start = u16::from_le_bytes([h[27], h[28]]) as u32;
        let loop_len = u16::from_le_bytes([h[29], h[30]]) as u32;
        s.loop_start = loop_start;
        s.loop_end = loop_start + loop_len;
        s.looped = loop_len > 2;
        samples.push(s);
    }

    let mut patterns: Vec<Option<Pattern>> = Vec::with_capacity(num_patterns);
    for _ in 0..num_patterns {
        let mut pat = Pattern::new(64, channels);
        for m in pat.cells.iter_mut() {
            let d = f.bytes(4)?;
            if d[1] > 0 && d[1] <= 84 * 2 {
                m.note = (d[1] >> 1) + NOTE_MIN + 35;
            }
            m.instr = d[0];
            m.param = d[3];
            if d[2] == 0x08 {
                match m.param & 0xF0 {
                    0x00 => {
                        m.cmd = Cmd::ModCmdEx;
                        m.param |= 0x80;
                    }
                    0x10 => {
                        m.cmd = Cmd::VolumeSlide;
                        m.param <<= 4;
                    }
                    0x20 => {
                        m.cmd = Cmd::ModCmdEx;
                        m.param |= 0xA0;
                    }
                    0x30 | 0x40 => {
                        m.cmd = Cmd::ModCmdEx;
                        m.param -= 0x20;
                    }
                    _ => {}
                }
            } else if d[2] == 0x13 {
                // 3D Simulate
                m.cmd = Cmd::Panning8;
                let p = m.param as u32;
                let mut v = (p & 0x7F) * 2;
                if p <= 0x40 {
                    v += 0x80;
                } else if p < 0x80 {
                    v = 0x180 - v;
                } else if p < 0xC0 {
                    v = 0x80 - v;
                } else {
                    v -= 0x80;
                }
                m.param = v.min(255) as u8;
            } else if d[2] & 0xF0 == 0x20 {
                // offset + volume
                m.cmd = Cmd::Offset;
                m.volcmd = VolCmd::Volume;
                m.vol = (d[2] & 0x0F) * 4 + 4;
            } else if d[2] <= 0x0F || d[2] == 0x11 || d[2] == 0x12 {
                convert_mod_command(m, d[2] & 0x0F, d[3]);
                // Cxx sets the volume as the S3M volume column does (a free column here)
                if m.cmd == Cmd::Volume {
                    m.cmd = Cmd::None;
                    m.volcmd = VolCmd::Volume;
                    m.vol = m.param.min(64);
                }
            }
        }
        patterns.push(Some(pat));
    }

    for s in samples.iter_mut() {
        let bits16 = s.bits16;
        s.data = read_pcm(&mut f, s.length, bits16, false, Enc::Signed);
    }

    Some(Module {
        title: name(&file[5..25]),
        channels,
        speed: 6,
        tempo: 125.0,
        global_vol,
        preamp: 48,
        linear: false,
        old_effects: false,
        compat_gxx: false,
        chn_surround: vec![false; channels],
        chn_vol: vec![64; channels],
        chn_pan,
        orders,
        patterns,
        instruments: Vec::new(),
        samples,
        rules: Rules { xm_volslides: true, no_fine_porta: true },
    })
}

/// The `xmp::LOADERS` entry: the module written as S3M, loaded by the libxmp port's S3M loader.
pub fn load_dynamic(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let file = whole_file(f);
    let md = read_dynamic(&file).ok_or(())?;
    let (native, fix) = build(&md, Kind::S3m).ok_or(())?;
    load_native(m, &native, Kind::S3m, &fix, NAME_DYNAMIC)
}
