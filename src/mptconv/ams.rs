//! Velvet Studio (`AMShdr\x1A`, AMS 2.0 to 2.02) and Extreme's Tracker (`Extreme`, AMS 1.x)
//! modules, read as libopenmpt 0.8.7's `Load_ams.cpp` reads them (`ReadAMS2`, `ReadAMS`,
//! `ReadAMSPattern`, `AMSUnpack`) and played as the Impulse Tracker module OpenMPT saves them as
//! (`GetBestSaveFormat`; OpenMPT plays them with IT's behaviours, compatible Gxx, old effects).
//!
//! Velvet Studio: instruments of up to 255 samples each (shadow samples copied from the
//! instrument they shadow), volume, panning and vibrato envelopes, fadeout, 8.8 tempo, linear or
//! Amiga slides, 32 channels; patterns of up to 256 rows with several commands per cell, which
//! OpenMPT combines into one effect and the volume column as it does; RLE + bit-plane + delta
//! packed samples. The vibrato envelope (OpenMPT's pitch envelope with Velvet's range, ±2
//! semitones) and envelopes of more than 25 nodes (up to 32, libxmp's limit) are set on the
//! libxmp instrument after the IT loader ran; a fractional tempo through the libxmp module's
//! time factor.
//!
//! Left out: reversed samples (`smpReverse`, played forwards), the song message and the channel
//! and pattern names, envelope speed (OpenMPT ignores it too).
//!
//! Ported from libopenmpt 0.8.7 (BSD-3-Clause, OpenMPT Project Developers and Contributors; see
//! LICENSE-openmpt).

use super::*;
use crate::xmp::load::Hio;
use crate::xmp::module::{XMP_ENVELOPE_ON, XMP_MAX_ENV_POINTS};

/// Whether `d` is a Velvet Studio or Extreme's Tracker module (`ProbeFileHeaderAMS2`,
/// `ProbeFileHeaderAMS`, the header and its minimum size).
pub fn test(d: &[u8]) -> bool {
    test_ams2(d) || test_ams1(d)
}

fn test_ams2(d: &[u8]) -> bool {
    if d.len() < 8 || &d[..7] != b"AMShdr\x1A" {
        return false;
    }
    let at = 8 + d[7] as usize;
    let Some(h) = d.get(at..at + 7) else { return false };
    if h[1] != 2 || h[0] > 2 {
        return false;
    }
    let (ni, np, no) = (h[2] as usize, u16::from_le_bytes([h[3], h[4]]) as usize, u16::from_le_bytes([h[5], h[6]]) as usize);
    d.len() - (at + 7) >= 36 + 11 + ni * 2 + no * 2 + np * 4
}

fn test_ams1(d: &[u8]) -> bool {
    if d.len() < 18 || &d[..7] != b"Extreme" || d[8] != 1 {
        return false;
    }
    let h = &d[7..18];
    let (ns, np, no, extra) = (h[3] as usize, u16::from_le_bytes([h[4], h[5]]) as usize, u16::from_le_bytes([h[6], h[7]]) as usize, u16::from_le_bytes([h[9], h[10]]) as usize);
    d.len() - 18 >= extra + 3 + ns * 18 + no * 2 + np * 4
}

/// `ReadAMSPattern`: the cells of `pat` (`channels` wide) from `chunk`.
fn read_pattern(pat: &mut Pattern, channels: usize, new_version: bool, chunk: &mut Reader) {
    use Cmd::*;
    const TRANS: [Cmd; 29] = [
        S3mCmdEx, PortaUp, PortaDown, Retrig, None, TonePortaVol, VibratoVol, None, //
        PanningSlide, None, VolumeSlide, None, ChannelVolume, PatternBreak, S3mCmdEx, None, //
        KeyOff, PortaUp, PortaDown, None, None, None, None, None, //
        None, None, GlobalVolSlide, None, GlobalVolume,
    ];
    let mut dummy = Cell::default();
    for row in 0..pat.rows {
        while chunk.can_read(1) {
            let flags = chunk.u8();
            if flags == 0xFF {
                break;
            }
            let chn = (flags & 0x1F) as usize;
            let m = if chn < channels { &mut pat.cells[row * channels + chn] } else { &mut dummy };
            let mut more = true;
            if flags & 0x40 == 0 {
                let mut note = chunk.u8();
                more = note & 0x80 != 0;
                note &= 0x7F;
                if note == 1 {
                    m.note = NOTE_KEYOFF;
                } else if (2..=121).contains(&note) && new_version {
                    m.note = note - 2 + NOTE_MIN;
                } else if (12..=108).contains(&note) && !new_version {
                    m.note = note + 12 + NOTE_MIN;
                }
                m.instr = chunk.u8();
            }
            while more {
                let orig = *m;
                let command = chunk.u8();
                let effect = command & 0x3F;
                more = command & 0x80 != 0;
                if command & 0x40 != 0 {
                    m.volcmd = VolCmd::Volume;
                    m.vol = effect;
                    continue;
                }
                m.param = chunk.u8();
                if effect < 0x10 {
                    let p = m.param;
                    convert_mod_command(m, effect, p);
                    match m.cmd {
                        Panning8 => m.param = (m.param & 0x0F) * 0x11,
                        Volume => {
                            m.cmd = None;
                            m.volcmd = VolCmd::Volume;
                            m.vol = ((m.param as u32 + 1) / 2).min(64) as u8;
                        }
                        ModCmdEx => {
                            if m.param == 0x80 {
                                m.cmd = None;
                            } else {
                                extended_mod_to_s3m(m);
                            }
                        }
                        _ => {}
                    }
                } else if ((effect - 0x10) as usize) < TRANS.len() {
                    m.cmd = TRANS[(effect - 0x10) as usize];
                    match effect {
                        0x10 => {
                            if m.param <= 1 {
                                m.param |= 0x9E;
                            } else {
                                m.cmd = None;
                            }
                        }
                        0x11 | 0x12 => m.param = m.param.min(0x0F) | 0xE0,
                        0x15 | 0x16 => m.param = ((m.param as u32 + 1).min(0x10) / 2) as u8 | 0xF0,
                        0x1E => match m.param >> 4 {
                            0x1 => {
                                m.cmd = PortaUp;
                                m.param |= 0xF0;
                            }
                            0x2 => {
                                m.cmd = PortaDown;
                                m.param |= 0xF0;
                            }
                            0xA => {
                                m.cmd = VolumeSlide;
                                m.param = ((((m.param & 0x0F) + 1) / 2) << 4) | 0x0F;
                            }
                            0xB => {
                                m.cmd = VolumeSlide;
                                m.param = (((m.param & 0x0F) + 1) / 2) | 0xF0;
                            }
                            _ => m.cmd = None,
                        },
                        0x1C => m.param = ((m.param as u32 + 1) / 2).min(64) as u8,
                        _ => {}
                    }
                }
                // (an effect past the table keeps the command of before with the new parameter)
                let (mut c1, mut p1, mut c2, mut p2) = (m.cmd, m.param, orig.cmd, orig.param);
                combine_effects(&mut c1, &mut p1, &mut c2, &mut p2);
                m.cmd = c1;
                m.param = p1;
                if c2.weight() > m.cmd.weight() {
                    if m.volcmd == VolCmd::None {
                        let (vc, v) = convert_to_vol_command(m.cmd, m.param, true);
                        m.volcmd = vc;
                        m.vol = v;
                    }
                    m.cmd = c2;
                    m.param = p2;
                }
            }
            if flags & 0x80 != 0 {
                break;
            }
        }
    }
}

/// `AMSUnpack`: `dest` (zeroed, the sample's bytes) from the packed `source`.
fn ams_unpack(source: &[u8], dest: &mut [u8], pack: i8) {
    let mut temp = vec![0i8; dest.len()];
    let mut depack = dest.len();
    {
        let (mut i, mut j) = (source.len(), dest.len());
        let (mut inp, mut out) = (0usize, 0usize);
        while i != 0 && j != 0 {
            let mut ch = source[inp] as i8;
            inp += 1;
            i -= 1;
            if i != 0 && ch == pack {
                let mut rep = source[inp];
                inp += 1;
                rep = (rep as usize).min(j) as u8;
                i -= 1;
                if i != 0 && rep != 0 {
                    ch = source[inp] as i8;
                    inp += 1;
                    i -= 1;
                    while rep != 0 {
                        rep -= 1;
                        temp[out] = ch;
                        out += 1;
                        j -= 1;
                    }
                } else {
                    temp[out] = pack;
                    out += 1;
                    j -= 1;
                }
            } else {
                temp[out] = ch;
                out += 1;
                j -= 1;
            }
        }
        depack -= j;
    }
    {
        let mut bitcount: u32 = 0x80;
        let mut k = 0usize;
        for &t in temp.iter().take(depack) {
            let al = t as u8 as u32;
            let mut dh: u32 = 0;
            for count in 0..8u32 {
                let mut bl = al & bitcount;
                bl = ((bl | (bl << 8)) >> ((dh + 8 - count) & 7)) & 0xFFFF;
                bitcount = ((bitcount | (bitcount << 8)) >> 1) & 0xFF;
                dest[k] |= (bl & 0xFF) as u8;
                k += 1;
                if k >= dest.len() {
                    k = 0;
                    dh += 1;
                }
            }
            bitcount = ((bitcount | (bitcount << 8)) >> (dh & 31)) & 0xFF;
        }
    }
    let mut old: i8 = 0;
    for b in dest.iter_mut().take(depack) {
        let mut pos = *b as i32;
        if pos != 128 && pos & 0x80 != 0 {
            pos = -(pos & 0x7F);
        }
        old = old.wrapping_sub(pos as i8);
        *b = old as u8;
    }
}

/// `SampleIO(.., AMS).ReadSample`: a packed sample of `frames` frames.
fn read_packed(r: &mut Reader, frames: u32, bits16: bool) -> SampleData {
    let bps = if bits16 { 2 } else { 1 };
    let empty = SampleData { data: Vec::new(), frames: 0, channels: 1, bits16 };
    let file_size = r.left();
    if file_size <= 9 {
        return empty;
    }
    let start = r.pos();
    r.skip(4);
    let max_len = (r.u32() as usize).min(file_size);
    r.seek(start);
    let max_len = max_len.saturating_mul(85);
    // (beyond OpenMPT: at most 64 MB of sample)
    let frames = (frames.min(MAX_SAMPLE_LENGTH) as usize).min(max_len / bps).min((64 << 20) / bps);
    if frames == 0 {
        return empty;
    }
    r.skip(4);
    let source_size = r.u32() as usize;
    let pack = r.u8() as i8;
    let packed = r.chunk(source_size);
    let mut bytes = vec![0u8; frames * bps];
    ams_unpack(packed.rest(), &mut bytes, pack);
    let data = if bits16 {
        bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
    } else {
        bytes.iter().map(|&b| b as i8 as i16).collect()
    };
    SampleData { data, frames, channels: 1, bits16 }
}

/// A Velvet Studio envelope: (speed, sustain, loop start, loop end, nodes as (tick, value)).
struct AmsEnv {
    sus: u8,
    loop_start: u8,
    loop_end: u8,
    nodes: Vec<(u16, u8)>,
}

/// `AMS2Envelope::ConvertToMPT`.
fn read_env(f: &mut Reader) -> AmsEnv {
    let (h, _) = f.struct_or_zero(5);
    let num = h[4] as usize;
    let data = f.struct_partial(64 * 3, num * 3);
    let mut e = AmsEnv { sus: h[1], loop_start: h[2], loop_end: h[3], nodes: Vec::new() };
    if num <= 1 {
        return e;
    }
    let n = num.min(64);
    let mut tick: u16 = 0;
    for i in 0..n {
        if i != 0 {
            let d = (data[i * 3] as u16 | ((data[i * 3 + 1] as u16 & 1) << 8)).max(1);
            tick = tick.wrapping_add(d);
        }
        e.nodes.push((tick, data[i * 3 + 2]));
    }
    e
}

/// `AMS2Instrument::ApplyFlags`: the envelope with its flags (`shift` 0 volume, 1 panning, 2
/// vibrato).
fn apply_flags(e: &AmsEnv, env_flags: u16, shift: u32) -> Envelope {
    let flags = (env_flags >> (shift * 3)) as u32;
    let mut out = Envelope {
        flags: 0,
        nodes: e.nodes.clone(),
        loop_start: e.loop_start,
        loop_end: e.loop_end,
        sus_start: e.sus,
        sus_end: e.sus,
    };
    let (mut on, mut lp, mut sus) = (flags & 4 != 0, flags & 1 != 0, flags & 2 != 0);
    if !sus && lp && flags & (1 << (9 - shift * 2)) != 0 {
        out.sus_start = out.loop_start;
        out.sus_end = out.loop_end;
        sus = true;
        lp = false;
    }
    if out.nodes.is_empty() {
        on = false;
    }
    out.flags = if on { 1 } else { 0 } | if sus { 2 } else { 0 } | if lp { 4 } else { 0 };
    out
}

/// `Util::muldivrfloor(a, b, c)`.
fn muldivrfloor(a: i64, b: i64, c: i64) -> i64 {
    (a * b + c / 2).div_euclid(c)
}

/// The vibrato envelopes, per instrument (value in OpenMPT's 0..255 range with Velvet's ±32
/// fine steps), to set on the libxmp instruments; and envelopes of more than 25 nodes.
#[derive(Default)]
struct EnvFix {
    /// instrument -> (kind 0 volume / 1 panning / 2 pitch, nodes in libxmp's values)
    envs: Vec<(usize, usize, Vec<(i16, i16)>)>,
}

/// The module and what the IT file cannot hold of its envelopes.
fn read2(file: &[u8]) -> Option<(Module, EnvFix)> {
    if !test_ams2(file) {
        return None;
    }
    let mut f = Reader::new(file);
    f.skip(7);
    let title = f.sized_string()?;
    let (h, ok) = f.struct_or_zero(7);
    if !ok {
        return None;
    }
    let (ver_low, num_ins) = (h[0], h[2] as usize);
    let num_pats = u16::from_le_bytes([h[3], h[4]]) as usize;
    let num_ords = u16::from_le_bytes([h[5], h[6]]) as usize;
    let channels = 32;

    let (tempo, speed, header_flags);
    if ver_low >= 2 {
        let t = f.u16().max(32 << 8);
        tempo = ((t as u64 * 10000) >> 8) as f64 / 10000.0;
        speed = f.u8().max(1);
        f.skip(3);
        header_flags = f.u16();
    } else {
        tempo = f.u8().max(32) as f64;
        speed = f.u8().max(1);
        header_flags = f.u8() as u16;
    }

    let mut instruments: Vec<Option<Instrument>> = Vec::new();
    let mut samples: Vec<Sample> = Vec::new();
    let mut first_sample: Vec<usize> = Vec::new();
    let mut settings: Vec<u16> = Vec::new();
    let mut fix = EnvFix::default();
    for ins in 0..num_ins {
        let Some(nm) = f.sized_string() else { break };
        let mut instr = Instrument::new(0);
        instr.name = name(&nm);
        let num_samples = f.u8() as usize;
        let mut assign = [0u8; 120];
        if num_samples == 0 {
            instruments.push(Some(instr));
            continue;
        }
        if ver_low > 0 {
            match f.bytes(120) {
                Some(b) => assign.copy_from_slice(b),
                None => {
                    instruments.push(Some(instr));
                    continue;
                }
            }
        } else {
            let k = 96.min(f.left());
            if k == 0 {
                instruments.push(Some(instr));
                continue;
            }
            assign[12..12 + k].copy_from_slice(&f.rest()[..k]);
            f.skip(k);
        }
        let base = samples.len();
        for (i, e) in instr.keyboard.iter_mut().enumerate() {
            *e = (i as u8, (assign[i] as usize + base + 1).min(0xFFFF) as u16);
        }
        let vol = read_env(&mut f);
        let pan = read_env(&mut f);
        let vib = read_env(&mut f);
        let (ih, _) = f.struct_or_zero(5);
        let shadow = ih[0];
        let vf = u16::from_le_bytes([ih[1], ih[2]]);
        let env_flags = u16::from_le_bytes([ih[3], ih[4]]);
        instr.fadeout = (vf & 0xFFF) as u32;
        let vib_amp = 1i64 << ((vf & 0x3000) >> 12);
        let mut ve = apply_flags(&vol, env_flags, 0);
        let mut pe = apply_flags(&pan, env_flags, 1);
        let mut fe = apply_flags(&vib, env_flags, 2);
        for p in ve.nodes.iter_mut() {
            p.1 = ((p.1 as u32 * 64 + 64) / 127).min(64) as u8;
        }
        for p in pe.nodes.iter_mut() {
            p.1 = ((p.1 as u32 * 64 + 128) / 255).min(64) as u8;
        }
        // the vibrato envelope: OpenMPT's 0..255 (128 = none), read with ±32 fine steps
        let mut fine = Vec::new();
        for p in fe.nodes.iter_mut() {
            let v = (128 + muldivrfloor(p.1.wrapping_sub(128) as i8 as i64, vib_amp, 8)) as u8;
            // (`InstrumentEnvelope::GetValueFromPosition(.., 64, 255) - 32`, in libxmp's
            // pitch envelope units: 100 per semitone, 16 fine steps per semitone)
            let steps = v as f64 * 64.0 / 255.0 - 32.0;
            fine.push((p.0 as i16, (steps * 6.25).round() as i16));
            p.1 = (32.0 + steps / 8.0).round().clamp(0.0, 64.0) as u8;
        }
        if fe.flags & 1 != 0 {
            fix.envs.push((ins, 2, fine));
        }
        for (kind, e) in [(0usize, &ve), (1, &pe)] {
            if e.flags & 1 != 0 && e.nodes.len() > 25 {
                let off = if kind == 1 { -32 } else { 0 };
                fix.envs.push((ins, kind, e.nodes.iter().map(|&(t, v)| (t as i16, v as i16 + off)).collect()));
            }
        }
        instr.vol_env = ve;
        instr.pan_env = pe;
        instr.pitch_env = fe;

        let first = samples.len();
        for smp in 0..num_samples {
            let sname = f.sized_string().unwrap_or_default();
            let (sh, _) = f.struct_or_zero(20);
            if first + smp >= 3999 {
                continue;
            }
            let u32at = |o: usize| u32::from_le_bytes([sh[o], sh[o + 1], sh[o + 2], sh[o + 3]]);
            let length = u32at(0);
            let pan_fine = sh[14];
            let c4speed = u16::from_le_bytes([sh[15], sh[16]]) as u32;
            let rel = sh[17] as i8;
            let volume = sh[18];
            let sflags = sh[19];
            let mut s = Sample::new();
            s.name = name(&sname);
            s.length = length;
            s.loop_start = u32at(4).min(length);
            s.loop_end = u32at(8).min(length);
            let c5 = if c4speed == 0 { 8363 * 2 } else { c4speed * 2 };
            let new_c4 = transpose_to_frequency(rel as i32, mod2xm_finetune(pan_fine & 0x0F));
            s.c5speed = c5.wrapping_mul(new_c4) / 8363;
            s.volume = ((volume.min(127) as u32 * 256 + 64) / 127) as u16;
            if pan_fine & 0xF0 != 0 {
                s.pan = Some((pan_fine & 0xF0) as u16);
            }
            s.bits16 = sflags & 0x04 != 0;
            if sflags & 0x08 != 0 && s.loop_start < s.loop_end {
                s.looped = true;
                s.pingpong = sflags & 0x10 != 0;
            }
            samples.push(s);
            settings.push(shadow as u16 | (((smp as u16) << 8) & 0x7F00) | if sflags & 0x03 != 0 { 0x8000 } else { 0 });
        }
        first_sample.push(first);
        instruments.push(Some(instr));
    }

    // composer, channel names
    f.sized_string();
    for _ in 0..32 {
        f.sized_string();
    }
    let mut orders = Vec::new();
    let mut patterns: Vec<Option<Pattern>> = Vec::new();
    // the packed description: without it, no orders, patterns or sample data
    if let Some(d) = f.bytes(11) {
        let packed_len = u32::from_le_bytes([d[0], d[1], d[2], d[3]]) as usize;
        if packed_len > 11 && f.can_read(packed_len - 11) {
            f.skip(packed_len - 11);
        }
        if f.can_read(num_ords * 2) {
            for _ in 0..num_ords {
                orders.push(f.u16());
            }
        }
        patterns.resize(num_pats, None);
        for p in patterns.iter_mut() {
            if !f.can_read(4) {
                break;
            }
            let len = f.u32() as usize;
            let mut chunk = f.chunk(len);
            let rows = chunk.u8() as usize + 1;
            chunk.skip(1);
            let mut pat = Pattern::new(rows, channels);
            chunk.sized_string();
            read_pattern(&mut pat, channels, true, &mut chunk);
            *p = Some(pat);
        }
        for (k, s) in samples.iter_mut().enumerate() {
            let st = settings.get(k).copied().unwrap_or(1);
            if st & 0xFF == 0 {
                s.data = if st & 0x8000 != 0 {
                    read_packed(&mut f, s.length, s.bits16)
                } else {
                    read_pcm(&mut f, s.length, s.bits16, false, Enc::Signed)
                };
            }
        }
        for k in 0..samples.len() {
            let st = settings.get(k).copied().unwrap_or(0);
            let src_ins = (st & 0xFF) as usize;
            if src_ins == 0 || src_ins - 1 >= first_sample.len() {
                continue;
            }
            let src = ((st & 0x7F00) >> 8) as usize + first_sample[src_ins - 1];
            if src >= samples.len() || samples[src].data.frames == 0 {
                continue;
            }
            let d = samples[src].data.clone();
            samples[k].bits16 = d.bits16;
            samples[k].data = d;
        }
    }

    let md = Module {
        title: name(&title),
        channels,
        speed,
        tempo,
        global_vol: 256,
        preamp: 48,
        linear: header_flags & 0x40 != 0,
        // OpenMPT's: IT's old effects and compatible Gxx. The conversion check (openmpt123 on the
        // AMS file against openmpt123 on the IT, 120 s) agrees: `fall of the golden rose` is
        // 54 dB under the signal with both, 27 dB with the old effects alone, 20 dB with
        // compatible Gxx alone, 19 dB with neither; `Algorythm` 58 dB with the old effects, 35
        // dB without (compatible Gxx makes no difference there)
        old_effects: true,
        compat_gxx: true,
        chn_pan: (0..channels).map(|c| if c & 3 == 1 || c & 3 == 2 { 0xC0 } else { 0x40 }).collect(),
        chn_surround: vec![false; channels],
        chn_vol: vec![64; channels],
        orders,
        patterns,
        instruments,
        samples,
        rules: Rules::default(),
    };
    Some((md, fix))
}

/// `ReadAMS`: an Extreme's Tracker module.
fn read1(file: &[u8]) -> Option<Module> {
    if !test_ams1(file) {
        return None;
    }
    let mut f = Reader::new(file);
    f.skip(7);
    let (h, _) = f.struct_or_zero(11);
    let channels = (h[2] & 0x1F) as usize + 1;
    let num_samps = h[3] as usize;
    let num_pats = u16::from_le_bytes([h[4], h[5]]) as usize;
    let num_ords = u16::from_le_bytes([h[6], h[7]]) as usize;
    if !f.skip(u16::from_le_bytes([h[9], h[10]]) as usize) {
        return None;
    }
    let mut samples = Vec::new();
    let mut packed = Vec::new();
    for _ in 0..num_samps {
        let (sh, _) = f.struct_or_zero(17);
        let u32at = |o: usize| u32::from_le_bytes([sh[o], sh[o + 1], sh[o + 2], sh[o + 3]]);
        let length = u32at(0);
        let pan_fine = sh[12];
        let rate = u16::from_le_bytes([sh[13], sh[14]]) as u32;
        let mut s = Sample::new();
        s.length = length;
        s.loop_start = u32at(4).min(length);
        s.loop_end = u32at(8).min(length);
        s.volume = ((sh[15].min(127) as u32 * 256 + 64) / 127) as u16;
        if pan_fine & 0xF0 != 0 {
            s.pan = Some((pan_fine & 0xF0) as u16);
        }
        let c5 = if rate == 0 { 2 * 8363 } else { 2 * rate };
        s.c5speed = c5.wrapping_mul(transpose_to_frequency(0, mod2xm_finetune(pan_fine & 0x0F))) / 8363;
        s.looped = s.loop_start < s.loop_end;
        s.bits16 = sh[16] & (0x80 | 0x04) != 0;
        packed.push(sh[16] & 0x03 != 0);
        samples.push(s);
    }
    let title = f.sized_string().unwrap_or_default();
    for s in samples.iter_mut() {
        s.name = name(&f.sized_string().unwrap_or_default());
    }
    for _ in 0..channels {
        f.sized_string();
    }
    let mut patterns: Vec<Option<Pattern>> = vec![None; num_pats];
    for p in patterns.iter_mut() {
        f.sized_string();
        *p = Some(Pattern::new(64, channels));
    }
    let packed_len = f.u16() as usize;
    if packed_len != 0 && f.can_read(packed_len) {
        f.skip(packed_len);
    }
    let mut orders = Vec::new();
    if f.can_read(num_ords * 2) {
        for _ in 0..num_ords {
            orders.push(f.u16());
        }
    }
    for p in patterns.iter_mut() {
        if !f.can_read(4) {
            break;
        }
        let len = f.u32() as usize;
        let mut chunk = f.chunk(len);
        if let Some(pat) = p.as_mut() {
            read_pattern(pat, channels, false, &mut chunk);
        }
    }
    for (k, s) in samples.iter_mut().enumerate() {
        s.data = if packed[k] { read_packed(&mut f, s.length, s.bits16) } else { read_pcm(&mut f, s.length, s.bits16, false, Enc::Signed) };
    }
    Some(Module {
        title: name(&title),
        channels,
        speed: 6,
        tempo: 125.0,
        global_vol: 256,
        preamp: 48,
        linear: false,
        old_effects: true,
        compat_gxx: true,
        chn_pan: (0..channels).map(|c| if c & 3 == 1 || c & 3 == 2 { 0xC0 } else { 0x40 }).collect(),
        chn_surround: vec![false; channels],
        chn_vol: vec![64; channels],
        orders,
        patterns,
        instruments: Vec::new(),
        samples,
        rules: Rules::default(),
    })
}

/// The format name the libxmp port gives these modules.
pub fn name_of(file: &[u8]) -> String {
    if test_ams2(file) {
        let at = 8 + file[7] as usize;
        format!("Velvet Studio AMS {}.{:02}", file[at + 1], file[at])
    } else {
        format!("Extreme's Tracker AMS {}.{}", file[8], file[7 + 0])
    }
}

/// The module written as IT, and its fixups.
pub fn to_native(file: &[u8]) -> Option<(Vec<u8>, Fixups, Vec<(usize, usize, Vec<(i16, i16)>)>)> {
    let (md, envs) = if test_ams2(file) { read2(file)? } else { (read1(file)?, EnvFix::default()) };
    let (native, fix) = build(&md, Kind::It)?;
    Some((native, fix, envs.envs))
}

/// The `xmp::LOADERS` entry: the module written as IT, loaded by the libxmp port's IT loader,
/// with the envelopes IT cannot hold set on its instruments.
pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let file = whole_file(f);
    let (native, fix, envs) = to_native(&file).ok_or(())?;
    load_native(m, &native, Kind::It, &fix, &name_of(&file))?;
    for (ins, kind, nodes) in envs {
        let Some(xi) = m.module.xxi.get_mut(ins) else { continue };
        let env = match kind {
            0 => &mut xi.aei,
            1 => &mut xi.pei,
            _ => &mut xi.fei,
        };
        if env.flg & XMP_ENVELOPE_ON == 0 {
            continue;
        }
        let n = nodes.len().min(XMP_MAX_ENV_POINTS);
        for (k, &(t, v)) in nodes.iter().take(n).enumerate() {
            env.data[k * 2] = t;
            // (the IT loader adds 32 to a panning envelope)
            env.data[k * 2 + 1] = if kind == 1 { v + 32 } else { v };
        }
        env.npt = n as i32;
    }
    Ok(())
}
