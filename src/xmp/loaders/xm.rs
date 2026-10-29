//! FastTracker 2 Extended Modules (`loaders/xm_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//!
//! XM 1.02 to 1.04 (1.02/1.03 keep the instruments before the patterns and all
//! sample data after them), the ModPlug ADPCM samples, the trackers libxmp tells
//! apart by their name (FT2 and its clones get the FT2 bug emulation, old ModPlug
//! and ModPlug 1.16 the MPT flow and pre-amp), and the MPT extension chunks
//! after the samples. OggMod (vorbis-compressed) samples are decoded with the
//! port of libxmp's stb_vorbis (`xmp::vorbis`).

use crate::xmp::load::*;
use crate::xmp::module::*;

const XM_EVENT_PACKING: u8 = 0x80;
const XM_EVENT_NOTE_FOLLOWS: u8 = 0x01;
const XM_EVENT_INSTRUMENT_FOLLOWS: u8 = 0x02;
const XM_EVENT_VOLUME_FOLLOWS: u8 = 0x04;
const XM_EVENT_FXTYPE_FOLLOWS: u8 = 0x08;
const XM_EVENT_FXPARM_FOLLOWS: u8 = 0x10;
const XM_LOOP_FORWARD: u8 = 1;
const XM_LOOP_PINGPONG: u8 = 2;
const XM_SAMPLE_16BIT: u8 = 0x10;
const XM_SAMPLE_STEREO: u8 = 0x20;
const XM_LINEAR_PERIOD_MODE: u16 = 0x01;

/// Packed structure sizes.
const XM_INST_HEADER_SIZE: u32 = 29;
const XM_INST_SIZE: u32 = 212;
/// grass.near.the.house.xm has 23 samples in instrument 1; FT2 allows 16.
const XM_MAX_SAMPLES_PER_INST: usize = 32;
const MAGIC_OGGS: u32 = 0x4f67_6753;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 17 && &d[..17] == b"Extended Module: "
}

const fn magic4(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) << 24 | (b as u32) << 16 | (c as u32) << 8 | d as u32
}

fn rd16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// `strncpy`'d name: the bytes before the first NUL.
fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]).into_owned()
}

/// `load_xm_pattern`: pattern `num` from the file into its tracks.
fn load_xm_pattern(m: &mut ModuleData, num: usize, version: u16, patbuf: &mut [u8], f: &mut Hio) -> Result<(), ()> {
    let headsize: u32 = if version > 0x0102 { 9 } else { 8 };
    let length = f.read32l();
    let _packing = f.read8();
    let rows = if version > 0x0102 { f.read16l() as usize } else { f.read8() as usize + 1 };
    if rows > 256 {
        return Err(());
    }
    let datasize = f.read16l() as usize;
    // uint32 arithmetic: a header shorter than its fields seeks ~4 GB ahead
    f.seek_cur(length.wrapping_sub(headsize) as i64);
    if f.take_error() {
        return Err(());
    }
    let r = if rows == 0 { 0x100 } else { rows };
    m.module.alloc_pattern_tracks(num, r);
    if datasize == 0 {
        return Ok(());
    }

    let mut size = datasize as i32;
    let n = f.read(&mut patbuf[..datasize]);
    patbuf[n..datasize].fill(0);
    let mut pat = 0usize;
    let chn = m.module.chn;
    for j in 0..r {
        for k in 0..chn {
            // cleanly truncated patterns
            if pat == datasize {
                return Ok(());
            }
            let event = m.module.event_mut(num, k, j);
            size -= 1;
            if size < 0 {
                return Err(());
            }
            let b = patbuf[pat];
            pat += 1;
            if b & XM_EVENT_PACKING != 0 {
                for (bit, field) in [
                    (XM_EVENT_NOTE_FOLLOWS, &mut event.note),
                    (XM_EVENT_INSTRUMENT_FOLLOWS, &mut event.ins),
                    (XM_EVENT_VOLUME_FOLLOWS, &mut event.vol),
                    (XM_EVENT_FXTYPE_FOLLOWS, &mut event.fxt),
                    (XM_EVENT_FXPARM_FOLLOWS, &mut event.fxp),
                ] {
                    if b & bit != 0 {
                        size -= 1;
                        if size < 0 {
                            return Err(());
                        }
                        *field = patbuf[pat];
                        pat += 1;
                    }
                }
            } else {
                size -= 4;
                if size < 0 {
                    return Err(());
                }
                event.note = b;
                event.ins = patbuf[pat];
                event.vol = patbuf[pat + 1];
                event.fxt = patbuf[pat + 2];
                event.fxp = patbuf[pat + 3];
                pat += 4;
            }

            // sanity check
            if matches!(event.fxt, 18 | 19 | 22 | 23 | 24 | 26 | 28 | 30 | 31 | 32) || event.fxt > 34 {
                event.fxt = 0;
            }

            if event.note == 0x61 {
                event.note = XMP_KEY_OFF;
            } else if event.note > 0 {
                event.note = event.note.wrapping_add(12);
            }

            if event.fxt == 0x0e {
                if msn(event.fxp) == EX_FINETUNE {
                    let val = lsn(event.fxp).wrapping_sub(8) & 0xf;
                    event.fxp = (EX_FINETUNE << 4) | val;
                }
                if matches!(event.fxp, 0x43 | 0x73) {
                    event.fxp -= 1;
                }
            }
            if event.fxt == FX_XF_PORTA && msn(event.fxp) == 0x09 {
                // MPT hacks
                match lsn(event.fxp) {
                    0x0 | 0x1 => {
                        // surround off / on
                        event.fxt = FX_SURROUND;
                        event.fxp = lsn(event.fxp);
                    }
                    0xe | 0xf => {
                        // play forward / reverse
                        event.fxt = FX_REVERSE;
                        event.fxp = lsn(event.fxp) - 0xe;
                    }
                    _ => {}
                }
            }

            if event.vol == 0 {
                continue;
            }
            // volume set
            if (0x10..=0x50).contains(&event.vol) {
                event.vol -= 0x0f;
                continue;
            }
            // volume column effects
            let v = event.vol;
            let fx = match v >> 4 {
                0x06 => Some((FX_VOLSLIDE_2, v - 0x60)),
                0x07 => Some((FX_VOLSLIDE_2, (v - 0x70) << 4)),
                0x08 => Some((FX_EXTENDED, (EX_F_VSLIDE_DN << 4) | (v - 0x80))),
                0x09 => Some((FX_EXTENDED, (EX_F_VSLIDE_UP << 4) | (v - 0x90))),
                0x0a => Some((FX_VIBRATO, (v - 0xa0) << 4)),
                0x0b => Some((FX_VIBRATO, v - 0xb0)),
                0x0c => Some((FX_SETPAN, (v - 0xc0) << 4)),
                0x0d => Some((FX_PANSL_NOMEM, (v - 0xd0) << 4)),
                0x0e => Some((FX_PANSL_NOMEM, v - 0xe0)),
                0x0f => Some((FX_TONEPORTA, (v - 0xf0) << 4)),
                _ => None,
            };
            if let Some((t, p)) = fx {
                event.f2t = t;
                event.f2p = p;
            }
            event.vol = 0;
        }
    }
    Ok(())
}

fn load_patterns(m: &mut ModuleData, version: u16, f: &mut Hio) -> Result<(), ()> {
    m.module.pat += 1;
    let mut patbuf = vec![0u8; 65536];
    let n = m.module.pat - 1;
    for i in 0..n {
        load_xm_pattern(m, i, version, &mut patbuf, f)?;
    }
    // one extra empty pattern, all its channels on a single track
    let md = &mut m.module;
    let t = n * md.chn;
    if md.xxt.len() < md.trk {
        md.xxt.resize(md.trk, Track::default());
    }
    if t >= md.trk {
        return Err(());
    }
    md.xxt[t] = Track::new(64);
    if md.xxp.len() <= n {
        md.xxp.resize(n + 1, Pattern::default());
    }
    md.xxp[n] = Pattern { rows: 64, index: vec![t; md.chn] };
    Ok(())
}

/// The OggMod check: a sample whose data starts with a length and "OggS".
fn is_ogg_sample(f: &mut Hio, len: i32) -> bool {
    // Bonnie's Bookstore music.oxm has zero-length samples followed by Ogg ones
    if len < 4 {
        return false;
    }
    let _size = f.read32l();
    let id = f.read32b();
    if f.take_error() || !f.seek_cur(-8) {
        return false;
    }
    id == MAGIC_OGGS
}

/// `oggdec`: an OggMod sample (a 32-bit size, then an Ogg Vorbis stream) of
/// `len` bytes in the file, decoded into sample `sid`.
fn oggdec(m: &mut ModuleData, f: &mut Hio, sid: usize, len: i32) -> Result<(), ()> {
    let len = len as usize;
    // calloc(1, len) and a read of len - 4 bytes after the size: the stream's
    // last 4 bytes stay zero
    f.read32b();
    if f.take_error() || len - 4 > f.size().saturating_sub(f.tell()) {
        return Err(());
    }
    let mut data = vec![0u8; len];
    if f.read(&mut data[..len - 4]) != len - 4 {
        return Err(());
    }
    let pcm16 = super::super::vorbis::decode_memory_mono(&data).ok_or(())?;
    let mut n = pcm16.len();
    let xxs = &mut m.module.xxs[sid];
    let buf: Vec<u8> = if xxs.flg & XMP_SAMPLE_16BIT == 0 && n > 0 {
        pcm16.iter().map(|&v| (v >> 8) as u8).collect()
    } else {
        pcm16.iter().flat_map(|v| v.to_le_bytes()).collect()
    };
    if xxs.flg & XMP_SAMPLE_STEREO != 0 {
        // OXM stereo is a single channel non-interleaved stream
        n >>= 1;
    }
    xxs.len = n as i32;
    load_sample(m, f, SAMPLE_FLAG_NOLOAD, sid, Some(&buf))
}

#[derive(Clone, Copy, Default)]
struct SampleHeader {
    length: u32,
    reserved: u8,
}

fn load_instruments(m: &mut ModuleData, version: u16, mpt_ins_headers: &mut bool, f: &mut Hio) -> Result<(), ()> {
    // an estimate, the number is known at the end
    m.module.smp = MAX_SAMPLES;
    m.init_instrument();

    let mut sample_num = 0usize;
    let mut buf = [0u8; 208];
    let mut xsh = [SampleHeader::default(); XM_MAX_SAMPLES_PER_INST];
    for i in 0..m.module.ins {
        let instr_pos = f.tell();
        // MOD2XM 1.0 always says 31 instruments, XMLiTE strips them, files are
        // truncated: a short instrument header ends the list
        if f.read(&mut buf[..XM_INST_HEADER_SIZE as usize + 4]) != XM_INST_HEADER_SIZE as usize + 4 {
            break;
        }
        let size = rd32(&buf, 0);
        let name = buf[4..26].to_vec();
        let samples = rd16(&buf, 27) as usize;
        let sh_size = rd32(&buf, 29);

        if (size as i32) < XM_INST_HEADER_SIZE as i32 {
            return Err(());
        }
        if samples > XM_MAX_SAMPLES_PER_INST || (samples > 0 && sh_size > 0x100) {
            return Err(());
        }
        // ModPlug Tracker saves huge zero-filled headers for unused instruments
        if size == 0x107 && samples == 0 && sh_size == 0 {
            *mpt_ins_headers = true;
        }

        m.module.xxi[i].name = copy_adjust(&name, 22);
        m.module.xxi[i].nsm = samples as i32;
        if samples == 0 {
            // FT2 puts the sample header size in the instrument header, the
            // specs in the instrument; some files reserve the instrument data
            // anyway (size 263): skip to the end of the header either way
            if !f.seek_cur(size as i32 as i64 - (XM_INST_HEADER_SIZE as i64 + 4)) {
                return Err(());
            }
            continue;
        }
        m.module.xxi[i].sub = vec![SubInstrument::default(); samples];

        // `struct xm_instrument`
        let xi;
        if size < XM_INST_HEADER_SIZE + XM_INST_SIZE {
            // BoobieSqueezer strips the instrument data (uint32 arithmetic)
            xi = [0u8; 208];
            f.seek_cur(size.wrapping_sub(XM_INST_HEADER_SIZE + 4) as i64);
        } else {
            if f.read(&mut buf) != 208 {
                return Err(());
            }
            xi = buf;
            if !f.seek_cur(size as i32 as i64 - (XM_INST_HEADER_SIZE + XM_INST_SIZE) as i64) {
                return Err(());
            }
            let xxi = &mut m.module.xxi[i];
            let b = &xi[96..];
            xxi.rls = (rd16(b, 110) as i32) << 1;
            let set_env = |env: &mut Envelope, pts: usize, npt: u8, sus: u8, lps: u8, lpe: u8, flg: u8| {
                env.npt = npt as i32;
                env.sus = sus as i32;
                env.lps = lps as i32;
                env.lpe = lpe as i32;
                env.flg = flg as i32;
                if npt == 0 || npt > 12 {
                    env.flg &= !XMP_ENVELOPE_ON;
                } else {
                    for k in 0..npt as usize * 2 {
                        env.data[k] = rd16(b, pts + 2 * k) as i16;
                    }
                }
            };
            set_env(&mut xxi.aei, 0, b[96], b[98], b[99], b[100], b[104]);
            set_env(&mut xxi.pei, 48, b[97], b[101], b[102], b[103], b[105]);
            for j in 12..108 {
                xxi.map[j].ins = xi[j - 12];
                if xxi.map[j].ins as usize >= samples {
                    xxi.map[j].ins = 0xff;
                }
            }
        }
        let (y_wave, y_sweep, y_depth, y_rate) = (xi[96 + 106], xi[96 + 107], xi[96 + 108], xi[96 + 109]);

        // subinstrument and sample parameters
        for j in 0..samples {
            if sample_num >= m.module.smp {
                let n = m.module.smp * 3 / 2;
                m.realloc_samples(n);
            }
            let mut sb = [0u8; 40];
            if f.read(&mut sb) != 40 {
                return Err(());
            }
            let length = rd32(&sb, 0);
            if length > MAX_SAMPLE_SIZE as u32 {
                return Err(());
            }
            let loop_start = rd32(&sb, 4);
            let loop_length = rd32(&sb, 8);
            let (volume, finetune, typ, pan, relnote) = (sb[12], sb[13] as i8, sb[14], sb[15], sb[16] as i8);
            xsh[j] = SampleHeader { length, reserved: sb[17] };

            m.module.xxi[i].sub[j] = SubInstrument {
                vol: volume as i32,
                pan: pan as i32,
                xpo: relnote as i32,
                fin: finetune as i32,
                vwf: y_wave as i32,
                vde: (y_depth as i32) << 2,
                vra: y_rate as i32,
                vsw: y_sweep as i32,
                sid: sample_num as i32,
                ..Default::default()
            };

            let xxs = &mut m.module.xxs[sample_num];
            xxs.name = copy_adjust(&sb[18..40], 22);
            xxs.len = length as i32;
            xxs.lps = loop_start as i32;
            xxs.lpe = loop_start.wrapping_add(loop_length) as i32;
            xxs.flg = 0;
            if typ & XM_SAMPLE_16BIT != 0 {
                xxs.flg |= XMP_SAMPLE_16BIT;
                xxs.len >>= 1;
                xxs.lps >>= 1;
                xxs.lpe >>= 1;
            }
            if typ & XM_SAMPLE_STEREO != 0 {
                xxs.flg |= XMP_SAMPLE_STEREO;
                xxs.len >>= 1;
                xxs.lps >>= 1;
                xxs.lpe >>= 1;
            }
            if typ & XM_LOOP_FORWARD != 0 {
                xxs.flg |= XMP_SAMPLE_LOOP;
            }
            if typ & XM_LOOP_PINGPONG != 0 {
                xxs.flg |= XMP_SAMPLE_LOOP | XMP_SAMPLE_LOOP_BIDIR;
            }
            sample_num += 1;
        }

        // sample data
        let mut total_sample_size = 0i64;
        for j in 0..samples {
            let sid = m.module.xxi[i].sub[j].sid as usize;
            let flags = if xsh[j].reserved == 0xad { SAMPLE_FLAG_ADPCM } else { SAMPLE_FLAG_DIFF };
            if version > 0x0103 {
                if is_ogg_sample(f, m.module.xxs[sid].len) {
                    oggdec(m, f, sid, xsh[j].length as i32)?;
                    total_sample_size += xsh[j].length as i64;
                    continue;
                }
                load_sample(m, f, flags, sid, None)?;
                if flags & SAMPLE_FLAG_ADPCM != 0 {
                    total_sample_size += 16 + ((xsh[j].length as i64 + 1) >> 1);
                } else {
                    total_sample_size += xsh[j].length as i64;
                }
            }
        }

        // reposition, for 16-bit samples of odd length in the file ("Lead Lined
        // for '99")
        let pos = instr_pos as i64 + size as i64 + 40 * samples as i64 + total_sample_size;
        f.seek(pos as usize);
    }

    m.realloc_samples(sample_num);
    Ok(())
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    load_at(m, f, 0)
}

/// `xm_load` for a module that starts at `start` in `f`.
pub fn load_at(m: &mut ModuleData, f: &mut Hio, start: usize) -> Result<(), ()> {
    f.seek(start);
    let mut buf = [0u8; 80];
    if f.read(&mut buf) != 80 {
        return Err(());
    }
    let name = &buf[17..37];
    let tracker = &buf[38..58];
    let version = rd16(&buf, 58);
    let headersz = rd32(&buf, 60);
    let songlen = rd16(&buf, 64) as usize;
    let restart = rd16(&buf, 66) as usize;
    let channels = rd16(&buf, 68) as usize;
    let patterns = rd16(&buf, 70) as usize;
    let instruments = rd16(&buf, 72) as usize;
    let flags = rd16(&buf, 74);
    let tempo = rd16(&buf, 76);
    let bpm = rd16(&buf, 78);

    if songlen > 256 || patterns > 256 || instruments > 255 || channels > XMP_MAX_CHANNELS {
        return Err(());
    }
    // FT2 and MPT allow up to 255 BPM, OpenMPT 1000
    if (tempo >= 32 || bpm < 32 || bpm > 1000) && &tracker[..6] != b"MED2XM" {
        return Err(());
    }
    // the header size, honoured for BoobieSqueezer
    let len = headersz.wrapping_sub(0x14) as i32;
    if !(0..=256).contains(&len) {
        return Err(());
    }
    let mut order = [0u8; 256];
    // hio_read(order, len, 1): a zero length reads nothing and fails too
    if len == 0 || f.read(&mut order[..len as usize]) != len as usize {
        return Err(());
    }

    let md = &mut m.module;
    md.name = cstr(name);
    md.len = songlen;
    md.chn = channels;
    md.pat = patterns;
    md.ins = instruments;
    md.rst = if restart >= songlen { 0 } else { restart };
    md.spd = tempo as i32;
    md.bpm = bpm as i32;
    md.trk = md.chn * md.pat + 1;
    m.c4rate = C4_NTSC_RATE;
    m.period_type = if flags & XM_LINEAR_PERIOD_MODE != 0 { PeriodType::Linear } else { PeriodType::Amiga };
    m.module.xxo[..songlen].copy_from_slice(&order[..songlen]);

    // snprintf("%-20.20s") then the trailing spaces dropped: the name up to a NUL
    let mut tn: Vec<u8> = tracker[..tracker.iter().position(|&c| c == 0).unwrap_or(20)].to_vec();
    while tn.last() == Some(&b' ') {
        tn.pop();
    }

    let mut claims_ft2 = false;
    let mut is_mpt_old = false;
    let mut is_mpt_116 = false;
    if tn.starts_with(b"FastTracker v2.00") {
        m.quirk |= QUIRK_FT2BUGS;
        claims_ft2 = true;
    } else if tn.starts_with(b"Fasttracker II clone") || tn.starts_with(b"OpenMPT ") {
        // OpenMPT emulates FT2's bugs
        m.quirk |= QUIRK_FT2BUGS;
    }
    if headersz == 0x0113 {
        tn = b"unknown tracker".to_vec();
        m.quirk &= !QUIRK_FT2BUGS;
    } else if tn.is_empty() {
        tn = b"Digitrakker".to_vec(); // best guess
        m.quirk &= !QUIRK_FT2BUGS;
    }
    // see the MMD1 loader
    if tn.starts_with(b"MED2XM by J.Pynnone") {
        if m.module.bpm <= 10 {
            m.module.bpm = 125 * (0x35 - m.module.bpm * 2) / 33;
        }
        m.quirk &= !QUIRK_FT2BUGS;
    }
    if tn.starts_with(b"FastTracker v 2.00") {
        tn = b"old ModPlug Tracker".to_vec();
        m.quirk &= !QUIRK_FT2BUGS;
        is_mpt_old = true;
    }
    if tn == b"Skale Tracker" || tn == b"Sk@le Tracker" {
        // Skale Tracker lets Dxx Byy jump to row x
        m.flow_mode |= FLOW_JUMP_NO_ROW_SET;
    }
    let vstr = format!("XM {}.{:02}", version >> 8, version & 0xff);
    m.module.type_ = format!("{} {vstr}", String::from_utf8_lossy(&tn));

    // the header size, honoured
    f.seek(start + headersz as usize + 60);

    let mut mpt_ins_headers = false;
    // XM 1.02/1.03: instruments before patterns
    if version <= 0x0103 {
        load_instruments(m, version, &mut mpt_ins_headers, f)?;
        load_patterns(m, version, f)?;
    } else {
        load_patterns(m, version, f)?;
        load_instruments(m, version, &mut mpt_ins_headers, f)?;
    }

    // XM 1.02 stores all samples after the patterns
    if version <= 0x0103 {
        for i in 0..m.module.ins {
            for j in 0..m.module.xxi[i].nsm as usize {
                let sid = m.module.xxi[i].sub[j].sid as usize;
                load_sample(m, f, SAMPLE_FLAG_DIFF, sid, None)?;
            }
        }
    }

    // MPT properties at the end of the file
    let mut have_comment = false;
    loop {
        let ext = f.read32b();
        let mut sz = f.read32l();
        if f.take_error() || sz > 0x7fff_ffff {
            break;
        }
        let mut known = false;
        if ext == magic4(b't', b'e', b'x', b't') {
            // song comment
            known = true;
            if !have_comment && sz as usize <= f.size() {
                let mut c = vec![0u8; sz as usize];
                let n = f.read(&mut c);
                c.truncate(n);
                m.comment = c
                    .iter()
                    .map(|&b| match b {
                        b'\r' => '\n',
                        b'\n' | b'\t' => b as char,
                        32..=127 => b as char,
                        _ => '.',
                    })
                    .collect();
                have_comment = true;
                sz = 0;
            }
        } else {
            // MIDI config, pattern and channel names, channel plugins, instrument
            // extensions; plugin definitions ("FX..")
            if [b"MIDI", b"PNAM", b"CNAM", b"CHFX", b"XTPM"].iter().any(|t| ext == magic4(t[0], t[1], t[2], t[3])) {
                known = true;
            }
            let fx = magic4(b'F', b'X', 0, 0);
            if ext & fx == fx {
                known = true;
            }
        }
        if known && claims_ft2 {
            is_mpt_116 = true;
        }
        if sz != 0 && !f.seek_cur(sz as i64) {
            break;
        }
        if ext == magic4(b'X', b'T', b'P', b'M') {
            break;
        }
    }
    if claims_ft2 && mpt_ins_headers {
        is_mpt_116 = true;
    }
    if is_mpt_116 {
        m.module.type_ = format!("ModPlug Tracker 1.16 {vstr}");
    }
    if is_mpt_116 || is_mpt_old {
        m.quirk &= !QUIRK_FT2BUGS;
        m.flow_mode = FLOW_MODE_MPT_116;
        m.mvolbase = 48;
        m.mvol = 48;
        apply_mpt_preamp(m);
    }

    for i in 0..m.module.chn {
        m.module.xxc[i].pan = 0x80;
    }
    m.quirk |= QUIRKS_FT2 | QUIRK_FT2ENV;
    m.read_event_type = ReadEvent::Ft2;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_xm() {
        assert!(test(b"Extended Module: song"));
        assert!(!test(b"Extended module: song"));
        assert!(!test(b"Extended Module:"));
    }
}
