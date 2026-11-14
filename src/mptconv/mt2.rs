//! MadTracker 2 modules (`MT20`, versions 2.00 to 2.ff, `.mt2`), read as libopenmpt 0.8.7's
//! `Load_mt2.cpp` reads them (`ReadMT2`, `ConvertMT2Command`) and played as the Impulse Tracker
//! module OpenMPT saves them as (`GetBestSaveFormat`; OpenMPT plays them with IT's behaviours
//! and linear slides, compatible Gxx).
//!
//! What is read: up to 64 tracks of packed or plain patterns of up to 4096 rows (FastTracker 2
//! and Impulse Tracker commands, MadTracker's own portamentos, vibrato, panning, volume, tempo,
//! gapper, filter cutoff, reverse, track volume, offset + delay), the drum tracks (8 more
//! channels, a drum pattern per order merged into the order's pattern, duplicated where two
//! orders give one pattern different drum patterns), the tempo (samples per tick, or the BPM+
//! chunk, in MadTracker's classic or modern mode), track volumes and the pre-amp (TRKS),
//! instruments (sample groups, NNA / duplicate checks, volume, panning and pitch envelopes, the
//! filter envelope and the filter cutoff / resonance), the samples (8 / 16-bit, mono / stereo,
//! delta coded, the right channel of a stereo sample as a difference, loops, volume, panning,
//! transpose and finetune folded into the C-5 speed as OpenMPT does).
//!
//! Dropped, as libopenmpt drops it: automation envelopes, VST plugins and MIDI instruments (an
//! instrument that plays through one plays nothing), track effects and routing, external
//! samples, beat-synchronised sample stretching (such a sample plays at C-5 on every note, as in
//! OpenMPT), MIDI CC, unknown MadTracker commands. Dropped beyond libopenmpt, because IT or the
//! libxmp port have no place for it: per-instrument "no interpolation" (the module's resampler
//! for every sample), OpenMPT's extended filter range for cutoffs, the restart position,
//! commands IT reads with other parameter meanings where MadTracker follows FastTracker 2 (the
//! volume slide nibble priority and 1xx / 2xx above DF are written as XM plays them). The
//! modern tempo mode is played in classic ticks: the initial tempo exactly (through the libxmp
//! module's time factor), tempo commands as the nearest classic tempo at the speed of their
//! row, and a speed change as a tempo change that keeps the row length.
//!
//! Ported from libopenmpt 0.8.7 (BSD-3-Clause, Olivier Lapicque and OpenMPT Project
//! Developers and Contributors; see LICENSE-openmpt).

use super::*;
use crate::xmp::load::Hio;

/// `ValidateHeader` and `GetHeaderMinimumAdditionalSize` (256 bytes after the header).
pub fn test(d: &[u8]) -> bool {
    if d.len() < 126 + 256 || &d[..4] != b"MT20" {
        return false;
    }
    let u16at = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]);
    let version = u16at(8);
    let channels = u16at(112);
    (0x200..0x300).contains(&version) && (1..=64).contains(&channels) && u16at(106) <= 256 && u16at(122) < 256 && u16at(124) < 4000
}

/// `ConvertMT2Command`: the cell, and whether it is a legacy tempo command.
fn convert_command(m: &mut Cell, p: [u8; 7]) -> bool {
    let [note, instr, vol, pan, fxcmd, fx1, fx2] = p;
    let mut legacy = false;
    m.note = NOTE_NONE;
    if note != 0 {
        m.note = if note > 96 { NOTE_KEYOFF } else { note + NOTE_MIN + 11 };
    }
    m.instr = instr;
    match vol {
        0x10..=0x90 => {
            m.volcmd = VolCmd::Volume;
            m.vol = (vol - 0x10) / 2;
        }
        0xA0..=0xAF => {
            m.volcmd = VolCmd::VolSlideDown;
            m.vol = vol & 0x0F;
        }
        0xB0..=0xBF => {
            m.volcmd = VolCmd::VolSlideUp;
            m.vol = vol & 0x0F;
        }
        0xC0..=0xCF => {
            m.volcmd = VolCmd::FineVolDown;
            m.vol = vol & 0x0F;
        }
        0xD0..=0xDF => {
            m.volcmd = VolCmd::FineVolUp;
            m.vol = vol & 0x0F;
        }
        _ => {}
    }
    let sat = |v: u32| v.min(255) as u8;
    if fxcmd != 0 || fx1 != 0 || fx2 != 0 {
        match fxcmd {
            0x00 => {
                convert_mod_command(m, fx2, fx1);
                if fx2 == 0x0F {
                    legacy = true;
                }
            }
            0x01 => {
                m.cmd = Cmd::PortaUp;
                m.param = sat(((fx2 as u32) << 4) | (fx1 as u32 >> 4));
            }
            0x02 => {
                m.cmd = Cmd::PortaDown;
                m.param = sat(((fx2 as u32) << 4) | (fx1 as u32 >> 4));
            }
            0x03 => {
                m.cmd = Cmd::TonePorta;
                m.param = sat(((fx2 as u32) << 4) | (fx1 as u32 >> 4));
            }
            0x04 => {
                m.cmd = Cmd::Vibrato;
                m.param = (fx2 & 0xF0) | (fx1 >> 4);
            }
            0x08 => {
                if fx1 != 0 {
                    m.cmd = Cmd::Panning8;
                    m.param = fx1;
                } else if fx2 == 1 || fx2 == 2 {
                    m.cmd = Cmd::S3mCmdEx;
                    m.param = 0x91;
                }
            }
            0x0C => {
                m.cmd = Cmd::Volume;
                m.param = fx2 / 2;
            }
            0x0F => {
                if fx2 != 0 {
                    m.cmd = Cmd::Tempo;
                    m.param = fx2;
                } else {
                    m.cmd = Cmd::Speed;
                    m.param = fx1 & 0x0F;
                }
            }
            0x10 => {
                s3m_convert(m, fx2, fx1, true);
                if m.cmd == Cmd::Tempo || m.cmd == Cmd::Speed {
                    legacy = true;
                }
            }
            0x1D => {
                m.cmd = Cmd::Tremor;
                m.param = fx1;
            }
            0x20 => {
                m.cmd = Cmd::Midi;
                m.param = fx2 >> 1;
            }
            0x22 => {
                m.cmd = Cmd::Midi;
                m.param = (fx2 & 0xF0) >> 1;
            }
            0x24 => {
                m.cmd = Cmd::S3mCmdEx;
                m.param = 0x9F;
            }
            0x80 => {
                m.cmd = Cmd::ChannelVolume;
                m.param = fx2 / 4;
            }
            0x9D => {
                m.volcmd = VolCmd::Offset;
                m.vol = fx2 >> 3;
                m.cmd = Cmd::S3mCmdEx;
                m.param = 0xD0 | fx1.min(0x0F);
            }
            _ => {}
        }
    }
    if pan != 0 {
        if m.cmd == Cmd::None {
            m.cmd = Cmd::Panning8;
            m.param = pan;
        } else if m.volcmd == VolCmd::None {
            m.volcmd = VolCmd::Panning;
            m.vol = pan / 4;
        }
    }
    legacy
}

/// `ReadMT2Automation`: skipped.
fn skip_automation(version: u16, f: &mut Reader) {
    let mut flags = if version >= 0x203 {
        let fl = f.u32();
        f.u32();
        fl
    } else {
        let fl = f.u16() as u32;
        f.u16();
        fl
    };
    while flags != 0 {
        if flags & 1 != 0 {
            f.skip(4 + 4 * 64);
        }
        flags >>= 1;
    }
}

/// The module OpenMPT reads from `file` (with its modern-tempo speed changes as libxmp tempo
/// effects), or None when it does not load it.
pub fn read(file: &[u8]) -> Option<(Module, Vec<(usize, usize, usize, u8, u8)>)> {
    if !test(file) {
        return None;
    }
    let mut f = Reader::new(file);
    let h = f.bytes(126)?;
    let u16at = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]);
    let version = u16at(8);
    let num_orders = u16at(106) as usize;
    let restart = u16at(108);
    let num_patterns = u16at(110) as usize;
    let num_channels = u16at(112) as usize;
    let samples_per_tick = u16at(114);
    let ticks_per_line = h[116];
    let lines_per_beat = h[117];
    let flags = u32::from_le_bytes([h[118], h[119], h[120], h[121]]);
    let num_instruments = u16at(122) as usize;
    let num_samples = u16at(124) as usize;
    let _ = restart;

    let speed = ticks_per_line.clamp(1, 31);
    let rows_per_beat = lines_per_beat.clamp(1, 32) as f64;
    let mut preamp = 48 * 2;

    let ord_bytes = f.bytes(256)?;
    let orders_raw: Vec<u16> = ord_bytes[..num_orders].iter().map(|&o| o as u16).collect();

    let has_drums = f.u16() != 0;
    let mut drum_data = f.chunk(if has_drums { 274 } else { 0 });
    let extra_len = f.u32() as usize;
    let mut extra = f.chunk(extra_len);

    let channels_without_drums = num_channels;
    let channels = num_channels + if has_drums { 8 } else { 0 };
    let mut legacy_tempo = false;

    let mut patterns: Vec<Option<Pattern>> = vec![None; num_patterns];
    for p in patterns.iter_mut() {
        let mut rows = f.u16() as usize;
        let len = (f.u32().wrapping_add(1) & !1) as usize;
        let mut chunk = f.chunk(len);
        rows = rows.min(4096);
        if rows == 0 {
            continue;
        }
        let mut pat = Pattern::new(rows, channels);
        if flags & 1 != 0 {
            let (mut row, mut chn) = (0usize, 0usize);
            while chunk.can_read(1) {
                let mut info = chunk.u8();
                let mut repeat = 0usize;
                if info == 0xFF {
                    repeat = chunk.u8() as usize;
                    info = chunk.u8();
                }
                if info & 0x7F != 0 {
                    let mut c = [0u8; 7];
                    for (bit, v) in c.iter_mut().enumerate() {
                        if info & (1 << bit) != 0 {
                            *v = chunk.u8();
                        }
                    }
                    let at = row * channels + chn;
                    let mut cell = pat.cells[at];
                    legacy_tempo |= convert_command(&mut cell, c);
                    let fill = repeat.min(rows - (row + 1));
                    for r in 0..=fill {
                        pat.cells[at + r * channels] = cell;
                    }
                }
                row += repeat + 1;
                while row >= rows {
                    row -= rows;
                    chn += 1;
                }
                if chn >= channels_without_drums {
                    break;
                }
            }
        } else {
            for row in 0..rows {
                for chn in 0..channels_without_drums {
                    let (b, _) = chunk.struct_or_zero(7);
                    let mut cell = Cell::default();
                    legacy_tempo |= convert_command(&mut cell, [b[0], b[1], b[2], b[3], b[4], b[5], b[6]]);
                    pat.cells[row * channels + chn] = cell;
                }
            }
        }
        *p = Some(pat);
    }

    let mut tempo = 125.0f64;
    let mut modern = false;
    if samples_per_tick > 1 && samples_per_tick < 5000 {
        if legacy_tempo {
            // TEMPO::SetRaw(muldivr(110250, fractFact, samplesPerTick))
            tempo = ((110250u64 * 10000 + samples_per_tick as u64 / 2) / samples_per_tick as u64) as f64 / 10000.0;
        } else {
            tempo = 44100.0 * 60.0 / (speed as f64 * rows_per_beat * samples_per_tick as f64);
            modern = true;
        }
    }

    let mut chn_vol = vec![64u8; channels];
    let mut num_vst = 0usize;
    while extra.can_read(8) {
        let id = extra.bytes(4)?;
        let id = [id[0], id[1], id[2], id[3]];
        let len = extra.u32() as usize;
        let mut chunk = extra.chunk(len);
        match &id {
            b"BPM+" => {
                if !legacy_tempo {
                    modern = true;
                    let d = chunk.f64();
                    if d > 0.00000001 {
                        tempo = 44100.0 * 60.0 / (speed as f64 * rows_per_beat * d);
                    }
                }
            }
            b"VST2" => num_vst = chunk.u32() as usize,
            b"TRKS" => {
                preamp = (chunk.u16() / 256) as u32;
                if preamp == 0 {
                    preamp = 48;
                }
                for v in chn_vol.iter_mut() {
                    if let Some(t) = chunk.bytes(1030) {
                        *v = ((u16::from_le_bytes([t[0], t[1]]) >> 10) as u8).min(64);
                    }
                }
            }
            _ => {}
        }
    }

    // drums
    let mut drum_map = [0usize; 8];
    let mut drum_sample = [0u16; 8];
    let mut instruments: Vec<Option<Instrument>> = vec![None; num_instruments];
    let mut orders = orders_raw.clone();
    if has_drums {
        let (dh, _) = drum_data.struct_or_zero(274);
        let num_drum_patterns = u16::from_le_bytes([dh[0], dh[1]]) as usize;
        for i in 0..8 {
            drum_sample[i] = u16::from_le_bytes([dh[2 + i * 2], dh[3 + i * 2]]);
            // GetNextFreeInstrument(m_nInstruments + 1)
            if instruments.len() < 255 {
                let mut ins = Instrument::new(drum_sample[i].saturating_add(1));
                ins.name = format!("Drum #{}", i + 1).into_bytes();
                instruments.push(Some(ins));
                drum_map[i] = instruments.len();
            }
        }
        let drum_order = &dh[18..274];
        let mut chunks = Vec::with_capacity(num_drum_patterns.min(f.left() / 2 + 1));
        for _ in 0..num_drum_patterns {
            let rows = f.u16() as usize;
            chunks.push(f.chunk(rows * 32));
        }
        let mut pat_map: Vec<Option<u8>> = vec![None; num_patterns];
        for ord in 0..num_orders {
            let dp = drum_order[ord] as usize;
            if dp >= num_drum_patterns || orders[ord] as usize >= num_patterns {
                continue;
            }
            let mut write = orders[ord] as usize;
            if write < pat_map.len() {
                match pat_map[write] {
                    None => pat_map[write] = Some(dp as u8),
                    Some(d) if d as usize != dp => {
                        if let Some(Some(src)) = patterns.get(write).cloned() {
                            if patterns.len() < 4000 {
                                patterns.push(Some(src));
                                write = patterns.len() - 1;
                                orders[ord] = write as u16;
                            }
                        }
                    }
                    _ => {}
                }
            }
            let Some(Some(pat)) = patterns.get_mut(write) else { continue };
            let mut chunk = chunks[dp].clone();
            chunk.rewind();
            let drum_rows = chunk.len() / 32;
            for row in 0..pat.rows {
                for chn in 0..8 {
                    let at = row * channels + channels - 8 + chn;
                    pat.cells[at] = Cell::default();
                    if row >= drum_rows {
                        continue;
                    }
                    let (d, _) = chunk.struct_or_zero(4);
                    if d[0] & 0x80 != 0 {
                        let m = &mut pat.cells[at];
                        m.note = NOTE_MIDDLEC;
                        m.instr = drum_map[chn].min(255) as u8;
                        let delay = (d[0] & 0x1F).min(0x0F);
                        if delay != 0 {
                            m.cmd = Cmd::S3mCmdEx;
                            m.param = 0xD0 | delay;
                        }
                        m.volcmd = VolCmd::Volume;
                        m.vol = ((d[1] as u16 + 3) / 4) as u8;
                    }
                }
            }
        }
    }

    // automation envelopes: skipped
    if flags & 2 != 0 {
        let n = (if flags & 8 != 0 { channels } else { channels_without_drums }) + if version >= 0x250 { num_vst } else { 0 } + if flags & 0x10 != 0 { 1 } else { 0 };
        for _ in 0..num_patterns {
            for _ in 0..n {
                if !f.can_read(4) {
                    break;
                }
                skip_automation(version, &mut f);
            }
        }
    }

    // instruments
    struct InsInfo<'a> {
        chunk: Reader<'a>,
        plugin: bool,
    }
    let mut infos: Vec<InsInfo> = Vec::with_capacity(255);
    for i in 0..255 {
        let iname = f.bytes(32).map(|b| b.to_vec()).unwrap_or_default();
        let mut len = f.u32() as usize;
        if len == 32 {
            len += 108 + 72 * 4;
        }
        if version > 0x0201 && len != 0 {
            len += 4;
        }
        let mut chunk = f.chunk(len);
        infos.push(InsInfo { chunk: chunk.clone(), plugin: false });
        if i >= num_instruments {
            continue;
        }
        let mut ins = Instrument::new((i + 1) as u16);
        ins.name = name(&iname);
        if len == 0 {
            instruments[i] = Some(ins);
            continue;
        }
        let (ih, _) = chunk.struct_or_zero(106);
        let iflags = if version >= 0x0201 { chunk.u16() } else { 0 };
        let mut env_mask = 1u32 | 2;
        let mut vol_env_flag = false;
        if version >= 0x0202 {
            env_mask = chunk.u32();
        }
        ins.fadeout = u16::from_le_bytes([ih[102], ih[103]]) as u32;
        let nna = u16::from_le_bytes([ih[104], ih[105]]);
        // NNA: cut, continue, off, fade; DCT: none, note, sample, instrument; DNA: cut, fade
        // ("continue"), off, fade
        ins.nna = [0, 1, 2, 3][(nna & 3) as usize];
        ins.dct = [0, 1, 2, 3][((nna >> 8) & 3) as usize];
        ins.dna = [0, 2, 1, 2][((nna >> 12) & 3) as usize];
        for env in 0..4 {
            if env_mask & 1 != 0 {
                let (e, _) = chunk.struct_or_zero(72);
                let n = e[1].min(16) as usize;
                let mut out = Envelope {
                    flags: 0,
                    nodes: Vec::with_capacity(n),
                    loop_start: e[3],
                    loop_end: e[4],
                    sus_start: e[2],
                    sus_end: e[2],
                };
                for p in 0..n {
                    let x = u16::from_le_bytes([e[8 + p * 4], e[9 + p * 4]]);
                    let y = u16::from_le_bytes([e[10 + p * 4], e[11 + p * 4]]);
                    out.nodes.push((x, y.min(64) as u8));
                }
                if env == 0 {
                    vol_env_flag = e[0] & 1 != 0;
                }
                let on = e[0] & 1 != 0 && n > 0;
                out.flags = if on { 1 } else { 0 } | if e[0] & 2 != 0 { 2 } else { 0 } | if e[0] & 4 != 0 { 4 } else { 0 };
                if env == 3 && e[0] & 1 != 0 {
                    out.flags |= 0x10;
                }
                match env {
                    0 => ins.vol_env = out,
                    1 => ins.pan_env = out,
                    // the pitch and the filter envelope are one in OpenMPT (the last read wins)
                    _ => ins.pitch_env = out,
                }
            }
            env_mask >>= 1;
        }
        if !vol_env_flag && ins.nna != 3 {
            ins.fadeout = 32767;
        }
        ins.cutoff = 0x7F | 0x80;
        ins.resonance = 0x80;
        if iflags != 0 {
            let (sd, _) = chunk.struct_or_zero(32);
            if iflags & 2 != 0 {
                let cutoff = u16::from_le_bytes([sd[2], sd[3]]) as f64;
                // FrequencyToCutOff with SONG_EXFILTERRANGE
                let c = ((cutoff.max(1e-9).ln() - 4.8737671609324025) * (20.0 / std::f64::consts::LN_2)).clamp(0.0, 127.0).round() as u8;
                ins.cutoff = c.min(0x7F) | 0x80;
                ins.resonance = sd[4].min(0x7F) | 0x80;
            }
            if iflags & 4 != 0 {
                // a VSTi / MIDI instrument: plays nothing here (nMixPlug = device + 1)
                infos[i].plugin = (sd[8] as i8 as i16 + 1) as u16 != 0;
                for e in ins.keyboard.iter_mut() {
                    e.1 = 0;
                }
                let transpose = sd[12] as i8 as i32;
                if transpose != 0 {
                    for (n, e) in ins.keyboard.iter_mut().enumerate() {
                        e.0 = (n as i32 + transpose).clamp(0, 119) as u8;
                    }
                }
            }
        }
        instruments[i] = Some(ins);
    }

    // sample headers
    let mut samples: Vec<Sample> = vec![Sample::new(); num_samples];
    let mut no_interp = [false; 256];
    let mut synced = [false; 256];
    let mut external = [false; 256];
    let mut rel_tone = [0i8; 256];
    for i in 0..256 {
        let sname = f.bytes(32).map(|b| b.to_vec()).unwrap_or_default();
        let len = f.u32() as usize;
        let mut chunk = f.chunk(len);
        if i < num_samples {
            samples[i].name = name(&sname);
        }
        if len != 0 && i < num_samples {
            let s = &mut samples[i];
            let (sh, _) = chunk.struct_or_zero(26);
            let u32at = |o: usize| u32::from_le_bytes([sh[o], sh[o + 1], sh[o + 2], sh[o + 3]]);
            s.length = u32at(0);
            s.c5speed = u32at(4);
            if sh[8] > 1 {
                s.bits16 = true;
                s.length /= 2;
            }
            if sh[9] > 1 {
                s.stereo = true;
                s.length /= 2;
            }
            match sh[11] {
                1 => s.looped = true,
                2 => {
                    s.looped = true;
                    s.pingpong = true;
                }
                _ => {}
            }
            s.loop_start = u32at(12);
            s.loop_end = u32at(16);
            s.volume = u16::from_le_bytes([sh[20], sh[21]]) >> 7;
            let pan = sh[22] as i8;
            if pan == -128 {
                s.surround = true;
                s.pan = Some(128);
            } else {
                s.pan = Some((pan as i16 + 128) as u16);
            }
            rel_tone[i] = sh[23] as i8;
            if sh[10] & 2 != 0 {
                synced[i] = true;
            }
            if sh[10] & 5 != 0 {
                external[i] = true;
            }
            if sh[10] & 8 != 0 {
                no_interp[i] = true;
            }
        }
    }
    let _ = no_interp;

    // sample groups
    let mut finetune = vec![0i32; num_samples];
    for i in 0..num_instruments {
        let info = &mut infos[i];
        if info.chunk.len() == 0 {
            continue;
        }
        info.chunk.rewind();
        let (ih, _) = info.chunk.struct_or_zero(106);
        let n = u16::from_le_bytes([ih[0], ih[1]]) as usize;
        // (ReadVector: zeros, and nothing read, when the file is shorter)
        let groups: Vec<[u8; 8]> = match f.bytes(n * 8) {
            Some(b) => b.chunks_exact(8).map(|c| c.try_into().unwrap()).collect(),
            None => vec![[0u8; 8]; n.min(256)],
        };
        if info.plugin {
            continue;
        }
        let Some(ins) = instruments[i].as_mut() else { continue };
        ins.global_vol = 32;
        for e in ins.keyboard.iter_mut() {
            e.1 = 0;
        }
        let (vib_type, vib_sweep, vib_depth, vib_rate) = (ih[98], ih[99], ih[100], ih[101]);
        for note in 0..96 {
            let g = ih[2 + note] as usize;
            if g >= n {
                continue;
            }
            let Some(group) = groups.get(g) else { continue };
            let sample = group[0] as usize + 1;
            ins.keyboard[note + 11].1 = sample as u16;
            if sample <= num_samples {
                let s = &mut samples[sample - 1];
                s.vib_type = vib_type & 3;
                s.vib_sweep = vib_sweep;
                s.vib_depth = vib_depth;
                s.vib_rate = vib_rate;
                s.global_vol = (group[1] as u16 * 2).min(64) as u8;
                finetune[sample - 1] = group[2] as i8 as i32;
                if synced[sample - 1] {
                    ins.keyboard[note + 11].0 = NOTE_MIDDLEC - 1;
                }
            }
        }
    }

    // sample data
    for i in 0..num_samples {
        let s = &mut samples[i];
        let amount = -(rel_tone[i] as f64 - 49.0 - finetune[i] as f64 / 128.0) / 12.0;
        s.c5speed = (s.c5speed as f64 * 2f64.powf(amount)).round().clamp(0.0, u32::MAX as f64) as u32;
        if !external[i] {
            s.data = read_pcm(&mut f, s.length, s.bits16, s.stereo, Enc::Mt2);
        } else {
            let n = f.u32() as usize;
            f.skip(12);
            f.skip(n);
        }
    }

    // modern tempo: tempo commands as classic tempos at their row's speed, speed changes with
    // the tempo that keeps the row length
    let mut extras = Vec::new();
    if modern {
        let mut done = vec![false; patterns.len()];
        let (mut spd, mut t) = (speed as f64, tempo);
        for &o in &orders {
            let p = o as usize;
            let Some(Some(pat)) = patterns.get_mut(p) else { continue };
            if done[p] {
                continue;
            }
            done[p] = true;
            for row in 0..pat.rows {
                let cells = &mut pat.cells[row * channels..(row + 1) * channels];
                if let Some(c) = cells.iter().find(|c| c.cmd == Cmd::Speed && c.param != 0) {
                    spd = c.param as f64;
                }
                let mut tempo_set = false;
                for c in cells.iter_mut().filter(|c| c.cmd == Cmd::Tempo) {
                    t = c.param as f64;
                    c.param = (t * spd * rows_per_beat / 24.0).round().clamp(32.0, 255.0) as u8;
                    tempo_set = true;
                }
                if !tempo_set {
                    for (ch, c) in cells.iter().enumerate() {
                        if c.cmd == Cmd::Speed && c.param != 0 {
                            let b = (t * spd * rows_per_beat / 24.0).round().clamp(32.0, 255.0) as u8;
                            extras.push((p, row, ch, crate::xmp::module::FX_IT_BPM, b));
                            break;
                        }
                    }
                }
            }
        }
    }
    let classic = if modern { tempo * speed as f64 * rows_per_beat / 24.0 } else { tempo };

    let chn_pan = vec![128u16; channels];
    let _ = legacy_tempo;
    Some((
        Module {
            title: name(&h[42..106]),
            channels,
            speed,
            tempo: classic,
            global_vol: 256,
            preamp: preamp.min(2000),
            linear: true,
            old_effects: false,
            compat_gxx: true,
            chn_pan,
            chn_surround: vec![false; channels],
            chn_vol,
            orders,
            patterns,
            instruments,
            samples,
            rules: Rules { xm_volslides: true, no_fine_porta: true },
        },
        extras,
    ))
}

/// The format name the libxmp port gives these modules.
pub fn name_of(file: &[u8]) -> String {
    let v = u16::from_le_bytes([file[8], file[9]]);
    format!("MadTracker {}.{:02X}", v >> 8, v & 0xFF)
}

/// The module written as IT, with its fixups.
pub fn to_native(file: &[u8]) -> Option<(Vec<u8>, Fixups)> {
    let (md, extras) = read(file)?;
    let (native, mut fix) = build(&md, Kind::It)?;
    fix.extras.extend(remap_extras(&md, extras));
    Some((native, fix))
}

/// The extras of the source module's patterns at the written module's pattern numbers.
fn remap_extras(md: &Module, extras: Vec<(usize, usize, usize, u8, u8)>) -> Vec<(usize, usize, usize, u8, u8)> {
    let map = written_patterns(md);
    extras.into_iter().filter_map(|(p, r, c, t, v)| map.get(p).copied().flatten().map(|np| (np, r, c, t, v))).collect()
}

/// The written pattern number of each source pattern (as `build` numbers them).
fn written_patterns(md: &Module) -> Vec<Option<usize>> {
    let mut map = vec![None; md.patterns.len()];
    let mut n = 0;
    for &o in &md.orders {
        let p = o as usize;
        if o >= ORDER_SKIP || !matches!(md.patterns.get(p), Some(Some(_))) {
            continue;
        }
        if map[p].is_none() && n < 0xFE {
            map[p] = Some(n);
            n += 1;
        }
    }
    map
}

/// The `xmp::LOADERS` entry: the module written as IT, loaded by the libxmp port's IT loader.
pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let file = whole_file(f);
    let (native, fix) = to_native(&file).ok_or(())?;
    load_native(m, &native, Kind::It, &fix, &name_of(&file))
}
