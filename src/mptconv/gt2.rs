//! Graoumf Tracker (`GTK`, versions 1 to 4) and Graoumf Tracker 2 (`GT2`, versions 0 to 9)
//! modules, read as libopenmpt 0.8.7's `Load_gt2.cpp` reads them (`ReadGTK`, `ReadGT2`,
//! `TranslateGraoumfEffect`, `GT2LogToLinearVolume`) and played as the Impulse Tracker module
//! OpenMPT saves them as (`GetBestSaveFormat`; OpenMPT plays them with IT's behaviours).
//!
//! GT2 is a chunked format: SONG (orders), PATS / PATD (patterns of 5-byte cells: note,
//! instrument, effect, parameter, volume), SAMP / SAM2 (8 and 16-bit, mono and stereo samples,
//! big or little endian), INST (instruments with a keyboard map, volume, panning and pitch /
//! cutoff envelopes with an LFO as auto-vibrato and a filter), TCN2 / TVOL / MIXP (tempo, track
//! volumes and panning), TNAM, XCOM. GTK is the fixed-layout predecessor (samples, orders,
//! patterns of 4 or 5-byte cells).
//!
//! Left out, as the IT module cannot hold them: the instrument scripts of GT2 instruments of
//! version 0 (OpenMPT's `InstrumentSynth`: volume, tone and pan scripts; such an instrument
//! plays without envelopes, with the fade-out OpenMPT sets), the detune and auto-period
//! commands (08xy, BFxx: OpenMPT's micro-tuning `CMD_FINETUNE`, and tone portamento with a
//! duration), the high-pass filter mode, panning finer than IT's 64 steps, muted tracks (TCN1), the restart position, the track and
//! pattern names, the song message and the dates, the DSP effects; samples kept on disk
//! (`SAM2` type 1) have no data; channels above 64 and envelopes' nodes above 25 (the volume and
//! panning envelopes of up to 32 nodes are set on the libxmp instrument afterwards).
//!
//! Ported from libopenmpt 0.8.7 (BSD-3-Clause, OpenMPT Project Developers and Contributors; see
//! LICENSE-openmpt).

use super::*;
use crate::xmp::load::Hio;
use crate::xmp::module::{XMP_ENVELOPE_ON, XMP_MAX_ENV_POINTS};

fn be16(d: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([d[o], d[o + 1]])
}

fn be32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

fn rd16(r: &mut Reader) -> u16 {
    r.u16().swap_bytes()
}

fn rd32(r: &mut Reader) -> u32 {
    r.u32().swap_bytes()
}

// ---------------------------------------------------------------------------------------------
// Headers and recognition

const GTK_HEADER: usize = 206;
const GT2_HEADER: usize = 236;

/// `GTKFileHeader::Validate` and `GetHeaderMinimumAdditionalSize`, then the size check of
/// `ProbeFileHeaderGTK` / `ReadGTK`.
fn test_gtk(d: &[u8]) -> bool {
    if d.len() < GTK_HEADER || &d[..3] != b"GTK" {
        return false;
    }
    let ver = d[3] as u64;
    let (ns, nr, nc, no, restart) = (be16(d, 196) as u64, be16(d, 198) as u64, be16(d, 200) as u64, be16(d, 202) as u64, be16(d, 204) as u64);
    if !(1..=4).contains(&ver) || ns > 255 || nr == 0 || nr > 256 || nc == 0 || nc > 32 || no > 256 || restart >= no {
        return false;
    }
    let need = (if ver < 3 { 48 } else { 64 }) * ns + 512 + (if ver < 4 { 4 } else { 5 }) * nr * nc;
    (d.len() - GTK_HEADER) as u64 >= need
}

/// `GT2FileHeader::Validate` and `GetHeaderMinimumAdditionalSize`.
fn test_gt2(d: &[u8]) -> bool {
    if d.len() < GT2_HEADER || &d[..3] != b"GT2" {
        return false;
    }
    let ver = d[3];
    let year = be16(d, 202);
    if ver > 9 || !(1980..=9999).contains(&year) {
        return false;
    }
    if ver <= 5 {
        let (speed, tempo, master, panned) = (be16(d, 228), be16(d, 230), be16(d, 232), be16(d, 234));
        if speed == 0 || tempo == 0 || master > 0xFFF || panned > 99 {
            return false;
        }
    }
    let need = (be32(d, 4) as u64).max(GT2_HEADER as u64) - GT2_HEADER as u64 + 20;
    (d.len() - GT2_HEADER) as u64 >= need
}

/// Whether `d` is a Graoumf Tracker or Graoumf Tracker 2 module.
pub fn test(d: &[u8]) -> bool {
    test_gtk(d) || test_gt2(d)
}

/// The format name the libxmp port gives these modules.
pub fn name_of(file: &[u8]) -> String {
    if test_gt2(file) && file[3] > 5 {
        format!("Graoumf Tracker 2 v{}", file[3])
    } else {
        format!("Graoumf Tracker v{}", file[3])
    }
}

// ---------------------------------------------------------------------------------------------
// Values

/// `GT2LogToLinearVolume`.
fn log_to_linear(log_volume: u16) -> u16 {
    let v = log_volume.min(0xFFF);
    let x = (2f64.powf((v & 0xFF) as f64 / 256.0) * 32768.0).round().clamp(0.0, 65535.0) as u16;
    x >> (15 - (v >> 8))
}

fn sat8(x: f64) -> u8 {
    x.round().clamp(0.0, 255.0) as u8
}

/// `Util::muldivr_unsigned`.
fn muldivr(a: u64, b: u64, c: u64) -> u64 {
    (a * b + c / 2) / c
}

/// `mpt::parse<double>` of a space or NUL padded text: the number at its start, 0 if none.
fn parse_f64(b: &[u8]) -> f64 {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    let s = String::from_utf8_lossy(&b[..n]);
    let s = s.trim_start();
    for i in (1..=s.len()).rev() {
        if s.is_char_boundary(i) {
            if let Ok(v) = s[..i].parse::<f64>() {
                return if v.is_finite() { v } else { 0.0 };
            }
        }
    }
    0.0
}

/// `CSoundFile::FrequencyToCutOff` with `SONG_EXFILTERRANGE`.
fn frequency_to_cutoff(freq: f64) -> u8 {
    let c = ((freq.max(1e-9).ln() - 4.8737671609324025) * (20.0 / std::f64::consts::LN_2)).clamp(0.0, 127.0);
    c.round() as u8
}

/// `ModSample::Transpose(amount)`.
fn transpose(c5: u32, amount: f64) -> u32 {
    (c5 as f64 * 2f64.powf(amount)).round().clamp(0.0, u32::MAX as f64) as u32
}

// ---------------------------------------------------------------------------------------------
// Effects

/// What `TranslateGraoumfEffect` reads of the module so far: the samples, the instruments, the
/// tempo and speed the commands set.
struct Tr {
    samples: Vec<Sample>,
    /// `ModSample::nFineTune` (`MOD2XMFineTune` of the file's value)
    finetune: Vec<i32>,
    instruments: Vec<Option<Instrument>>,
    tempo: f64,
    speed: u32,
}

impl Tr {
    /// `CSoundFile::GetSampleIndex`.
    fn sample_index(&self, note: u8, instr: u8) -> usize {
        if !self.instruments.is_empty() {
            if (NOTE_MIN..=NOTE_MAX).contains(&note) && instr != 0 && instr as usize <= self.instruments.len() {
                if let Some(i) = &self.instruments[instr as usize - 1] {
                    return i.keyboard[(note - NOTE_MIN) as usize].1 as usize;
                }
            }
            0
        } else {
            instr as usize
        }
    }

    fn sample(&self, index: usize) -> Option<(&Sample, i32)> {
        if index == 0 {
            return None;
        }
        self.samples.get(index - 1).map(|s| (s, self.finetune[index - 1]))
    }

    /// 08xy: OpenMPT makes an instrument of every sample when there are none (for the
    /// pitch-wheel depth of its detune command).
    fn make_instruments(&mut self) {
        if !self.instruments.is_empty() {
            return;
        }
        for (k, s) in self.samples.iter().enumerate().take(255) {
            let mut i = Instrument::new(k as u16 + 1);
            i.name = s.name.clone();
            i.cutoff = 0x7F | 0x80;
            i.resonance = 0x80;
            self.instruments.push(Some(i));
        }
    }

    /// `TranslateGraoumfEffect`. `version` is the file version (negated for GTK).
    fn effect(&mut self, m: &mut Cell, effect: u8, param: u8, version: i16, last_note: u8, last_instr: u8) {
        use Cmd::*;
        if effect == 0 {
            return;
        }
        if effect >= 0xB0 && version < 0 {
            return;
        }
        m.param = param;
        let falcon = version <= 5;
        let p12 = (((effect & 0x0F) as u16) << 8) | param as u16;
        let p4 = param.min(0x0F);
        let p4slide = param.min(0x0E);
        let p4d32 = ((param as u32 + 31) / 32).min(0x0E) as u8;
        let p4d128 = ((param as u32 + 127) / 128).min(0x0E) as u8;
        match effect >> 4 {
            0x02 => {
                m.volcmd = VolCmd::Volume;
                m.vol = (p12 / 4).min(64) as u8;
                return;
            }
            0x03 => {
                m.volcmd = VolCmd::Volume;
                m.vol = if p12 == 0 { 0 } else { (log_to_linear(p12) / 4).min(64) as u8 };
                return;
            }
            0x04 => {
                m.cmd = Panning8;
                m.param = (p12 / 16) as u8;
                return;
            }
            0x05 => {
                m.cmd = GlobalVolume;
                m.param = ((p12 + 1) / 32) as u8;
                return;
            }
            0x06 => {
                m.cmd = GlobalVolume;
                m.param = if p12 == 0 { 0 } else { (log_to_linear(p12) / 32).min(255) as u8 };
                return;
            }
            0x07 => {
                m.cmd = Retrig;
                m.param = effect & 0x0F;
                return;
            }
            0x08 => {
                m.cmd = Retrig;
                if version >= 0 {
                    if m.volcmd == VolCmd::None {
                        m.volcmd = VolCmd::Panning;
                        m.vol = (((effect & 0x0F) as u32 * 64 + 7) / 15) as u8;
                    }
                } else {
                    m.param = effect & 0x0F;
                    m.volcmd = VolCmd::Volume;
                    m.vol = ((param as u32 + 3) / 4) as u8;
                }
                return;
            }
            0x09 => {
                m.cmd = Offset;
                let mut full = p12 as u32;
                if falcon {
                    if full == 0 {
                        full = 1;
                    } else if let Some((smp, _)) = self.sample(self.sample_index(last_note, last_instr)) {
                        if smp.bits16 {
                            full /= 2;
                        }
                        if smp.looped && full * 256 > smp.length {
                            full = (smp.loop_start.wrapping_add(8)) / 256;
                            full = full.min(smp.length.wrapping_sub(1) / 256);
                        }
                    }
                }
                m.param = full as u8;
                if full > 0xFF && m.volcmd == VolCmd::None {
                    m.volcmd = VolCmd::Offset;
                    m.vol = (full >> 8).min(9) as u8;
                }
                return;
            }
            _ => {}
        }
        match effect {
            0x01 | 0x02 | 0x03 | 0x04 | 0x07 | 0x0B | 0x0F => {
                convert_mod_command(m, effect, param);
                if m.cmd == Tempo {
                    self.tempo = m.param as f64;
                } else if m.cmd == Speed && m.param > 0 {
                    self.speed = m.param as u32;
                }
            }
            0x05 => {
                m.cmd = TonePorta;
                if m.volcmd == VolCmd::None {
                    m.volcmd = VolCmd::VibratoDepth;
                    m.vol = 0;
                }
            }
            0x06 => {
                m.cmd = Vibrato;
                if m.volcmd == VolCmd::None {
                    m.volcmd = VolCmd::TonePorta;
                    m.vol = 0;
                }
            }
            0x08 => {
                // 08xy detune: OpenMPT's micro-tuning command, which is left out (the
                // instruments it makes are not)
                if (NOTE_MIN..=NOTE_MAX + 8).contains(&m.note) && param != 0 {
                    self.make_instruments();
                }
            }
            0x09 => {
                m.cmd = ModCmdEx;
                m.param = 0xD0 | p4;
            }
            0x0A => m.cmd = KeyOff,
            0x0C => {
                if param <= 2 {
                    m.cmd = ModCmdEx;
                    m.param = 0x40 | param;
                }
            }
            0x0D => m.cmd = PatternBreak,
            0x0E => {
                if param <= 2 {
                    m.cmd = ModCmdEx;
                    m.param = 0x70 | param;
                }
            }
            0x10 => m.cmd = Arpeggio,
            0x11 | 0x12 => {
                m.cmd = ModCmdEx;
                m.param = (effect << 4) | p4slide;
            }
            0x13 => m.cmd = Retrig,
            0x14 => {
                m.cmd = VolumeSlide;
                m.param = p4slide << 4;
            }
            0x15 => {
                m.cmd = VolumeSlide;
                m.param = p4slide;
            }
            0x16 | 0x17 | 0x1A | 0x1B | 0x1E | 0x1F => {}
            0x18 => {
                m.cmd = TonePortaVol;
                m.param = p4slide << 4;
            }
            0x19 => {
                m.cmd = TonePortaVol;
                m.param = p4slide;
            }
            0x1C => {
                m.cmd = VibratoVol;
                m.param = p4slide << 4;
            }
            0x1D => {
                m.cmd = VibratoVol;
                m.param = p4slide;
            }
            0xA0 => {
                if param != 0 {
                    m.cmd = GlobalVolSlide;
                    m.param = p4d32 << 4;
                }
            }
            0xA1 => {
                if param != 0 {
                    m.cmd = GlobalVolSlide;
                    m.param = p4d32;
                }
            }
            0xA4 => {
                m.cmd = VolumeSlide;
                m.param = (p4slide << 4) | 0x0F;
            }
            0xA5 => {
                m.cmd = VolumeSlide;
                m.param = p4slide | 0xF0;
            }
            0xA6 => {
                if param != 0 {
                    m.cmd = GlobalVolSlide;
                    m.param = (p4d32 << 4) | 0x0F;
                }
            }
            0xA7 => {
                if param != 0 {
                    m.cmd = GlobalVolSlide;
                    m.param = p4d32 | 0xF0;
                }
            }
            0xA8 => {
                if param != 0 {
                    m.cmd = Speed;
                    self.speed = m.param as u32;
                }
            }
            0xA9 => {
                // the Falcon version's fractional tempo (49170 Hz); the Windows version adds
                // xx/256 to the tempo, which is not a command
                if falcon && self.tempo.trunc() > 0.0 {
                    let mut samples_per_tick = (49170.0 * 2.5) / self.tempo;
                    samples_per_tick += param as f64 * 2.0 - 256.0;
                    m.cmd = Tempo;
                    m.param = sat8((49170.0 * 5.0) / (samples_per_tick * 2.0));
                }
            }
            0xAA => {
                m.cmd = ModCmdEx;
                m.param = 0xE0 | p4;
            }
            0xAC => {
                m.cmd = XFinePortaUpDown;
                m.param = 0x10 | p4;
            }
            0xAD => {
                m.cmd = XFinePortaUpDown;
                m.param = 0x20 | p4;
            }
            0xAE => {
                m.cmd = PanningSlide;
                m.param = (p4slide / 2) << 4;
            }
            0xAF => {
                m.cmd = PanningSlide;
                m.param = p4slide / 2;
            }
            0xB0 => m.cmd = Tremor,
            0xB1 => {
                m.cmd = ModCmdEx;
                m.param = 0xB0 | p4;
            }
            0xB2 | 0xB3 => {
                m.cmd = S3mCmdEx;
                m.param = 0x77 + (param != 0) as u8;
            }
            0xB4 => {
                m.cmd = S3mCmdEx;
                m.param = 0x7B + if param != 0 { 2 } else { 0 };
            }
            0xB5 => {
                m.cmd = S3mCmdEx;
                m.param = 0x79 + (param != 0) as u8;
            }
            0xB6 => {
                m.cmd = S3mCmdEx;
                m.param = 0x7B + if param != 0 { 3 } else { 0 };
            }
            0xB7 => {}
            0xBA => {
                m.cmd = Offset;
                m.param = ((m.param as u32 + 15) / 16) as u8;
            }
            0xBB => {
                m.cmd = Offset;
                m.param = 1;
            }
            0xBE | 0xBF => self.auto_tempo(m, effect, param, falcon, last_note, last_instr),
            0xC0 => {
                let direction = param >> 4;
                if direction == 1 || direction == 2 {
                    m.cmd = S3mCmdEx;
                    m.param = 0x9D + direction;
                }
            }
            0xC2 => {
                m.cmd = ChannelVolume;
                m.param = m.param.wrapping_mul(4);
            }
            0xC3 => {
                m.cmd = ChannelVolume;
                m.param = (log_to_linear((param as u16) << 4) / 64).min(255) as u8;
            }
            0xC4 | 0xC6 => {
                m.cmd = ChannelVolSlide;
                m.param = p4d128 << 4;
            }
            0xC5 | 0xC7 => {
                m.cmd = ChannelVolSlide;
                m.param = p4d128;
            }
            0xC8 => {
                m.cmd = ChannelVolSlide;
                m.param = (p4slide << 4) | 0x0F;
            }
            0xC9 => {
                m.cmd = ChannelVolSlide;
                m.param = p4slide | 0xF0;
            }
            _ => {}
        }
    }

    /// BExx (auto tempo) and BFxx (auto period).
    fn auto_tempo(&mut self, m: &mut Cell, effect: u8, param: u8, falcon: bool, last_note: u8, last_instr: u8) {
        if param == 0 || last_note == NOTE_NONE || last_instr == 0 || self.tempo.trunc() <= 0.0 || self.speed == 0 {
            return;
        }
        let index = self.sample_index(last_note, last_instr);
        let Some((smp, fine)) = self.sample(index) else { return };
        let mut length = smp.length as f64;
        if smp.looped {
            length = smp.loop_end as f64;
            if falcon {
                length -= smp.loop_start as f64;
            }
            if smp.pingpong && !falcon {
                length *= 2.0;
            }
        }
        if falcon && length < 1024.0 {
            return;
        }
        if length == 0.0 || smp.c5speed == 0 {
            return;
        }
        let mut trans = 0i32;
        if (last_instr as usize) <= self.instruments.len() {
            if let Some(Some(i)) = self.instruments.get(last_instr as usize - 1) {
                if (NOTE_MIN..=NOTE_MAX).contains(&last_note) {
                    trans = i.keyboard[(last_note - NOTE_MIN) as usize].0 as i32 + 1 - last_note as i32;
                }
            }
        }
        let finetune = if falcon && effect == 0xBE { fine } else { 0 };
        let rate = smp.c5speed as f64 * 2f64.powf(finetune as f64 / (-128.0 * 12.0) + trans as f64 / 12.0);
        if effect == 0xBE {
            // (the middle-C frequency was doubled when the samples were converted)
            let tempo = 2.5 * (self.speed as f64 * param as f64) * (rate * 0.5) / length;
            m.cmd = Cmd::Tempo;
            m.param = sat8(tempo);
            self.tempo = m.param as f64;
        } else {
            let duration = length / rate;
            let wanted = (param as f64 * self.speed as f64 * 2.5) / self.tempo;
            let ratio = duration / wanted;
            let note = (NOTE_MIDDLEC as f64 + ratio.log2() * 12.0).clamp(1.0, 128.0);
            let had_note = (NOTE_MIN..=NOTE_MAX + 8).contains(&m.note);
            m.note = note as u8;
            // (a fraction of a note is OpenMPT's micro-tuning command, left out)
            if !had_note {
                if m.volcmd == VolCmd::None {
                    m.volcmd = VolCmd::TonePorta;
                    m.vol = 9;
                }
                // (else tone portamento with a duration, left out)
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Samples

/// `SampleIO::ReadSample` for signed PCM, mono or interleaved stereo, big or little endian.
/// A sample is cut to the data the file holds.
fn read_pcm_gt(r: &mut Reader, frames: u32, bits16: bool, stereo: bool, big: bool) -> SampleData {
    let channels = if stereo { 2 } else { 1 };
    let bps = if bits16 { 2 } else { 1 };
    let fb = bps * channels;
    let left = r.left();
    let frames = (frames.min(MAX_SAMPLE_LENGTH) as usize).min(left / fb + (left % fb != 0) as usize);
    let src = r.rest();
    let mut data = vec![0i16; frames * channels];
    for (i, v) in data.iter_mut().enumerate() {
        let o = i * bps;
        if o + bps > src.len() {
            break;
        }
        *v = if bits16 {
            let b = [src[o], src[o + 1]];
            if big {
                i16::from_be_bytes(b)
            } else {
                i16::from_le_bytes(b)
            }
        } else {
            src[o] as i8 as i16
        };
    }
    r.skip(frames * fb);
    SampleData { data, frames, channels, bits16 }
}

/// `MOD2XMFineTune` of a file value.
fn xm_finetune(v: i16) -> i32 {
    mod2xm_finetune(v as u8)
}

// ---------------------------------------------------------------------------------------------
// Chunks

struct Chunk<'a> {
    id: [u8; 4],
    data: Reader<'a>,
}

/// `ReadChunksUntil<GT2Chunk>(1, ENDC)`.
fn read_chunks<'a>(f: &mut Reader<'a>) -> Vec<Chunk<'a>> {
    let mut out = Vec::new();
    while f.can_read(8) {
        let id: [u8; 4] = f.bytes(4).unwrap().try_into().unwrap();
        let len = (rd32(f) as usize).max(8) - 8;
        out.push(Chunk { id, data: f.chunk(len) });
        if &id == b"ENDC" {
            break;
        }
    }
    out
}

fn first<'a, 'b>(chunks: &'b [Chunk<'a>], id: &[u8; 4]) -> Option<Reader<'a>> {
    chunks.iter().find(|c| &c.id == id).map(|c| c.data.clone())
}

fn all<'a, 'b>(chunks: &'b [Chunk<'a>], id: &'static [u8; 4]) -> impl Iterator<Item = Reader<'a>> + 'b {
    chunks.iter().filter(move |c| &c.id == id).map(|c| c.data.clone())
}

// ---------------------------------------------------------------------------------------------
// GTK

fn read_gtk(file: &[u8]) -> Option<Module> {
    if !test_gtk(file) {
        return None;
    }
    let version = file[3];
    let (ns, rows, channels, no, _restart) = (be16(file, 196) as usize, be16(file, 198) as usize, be16(file, 200) as usize, be16(file, 202) as usize, be16(file, 204));
    let mut f = Reader::new(file);
    f.skip(GTK_HEADER);
    let mut samples: Vec<Sample> = Vec::new();
    let mut fine: Vec<i32> = Vec::new();
    for _ in 0..ns {
        let mut s = Sample::new();
        let nm = f.bytes(if version == 1 { 32 } else { 28 }).unwrap_or(&[]);
        s.name = name(nm);
        if version >= 3 {
            f.skip(14);
            let pan = rd16(&mut f) as i16;
            if pan >= 0 {
                s.pan = Some(muldivr(pan as u64, 256, 4095) as u16);
            }
        }
        let mut bytes_per_sample = 1;
        if version >= 2 {
            bytes_per_sample = rd16(&mut f);
            s.c5speed = rd16(&mut f) as u32;
        }
        let (length, loop_start, loop_len) = (rd32(&mut f), rd32(&mut f), rd32(&mut f));
        s.length = length;
        s.loop_start = loop_start;
        s.loop_end = loop_start.wrapping_add(loop_len);
        s.looped = loop_start > 0 || loop_len > 2;
        if bytes_per_sample == 2 {
            s.bits16 = true;
            s.length /= 2;
            s.loop_start /= 2;
            s.loop_end /= 2;
        }
        s.volume = rd16(&mut f);
        let ft = rd16(&mut f) as i16;
        if ft != 0 {
            s.c5speed = transpose(s.c5speed, ft as f64 / (12.0 * 8.0));
        }
        fine.push(xm_finetune(ft));
        samples.push(s);
    }
    let mut chunk = f.chunk(512);
    let mut orders = Vec::new();
    for _ in 0..no.min(256) {
        if !chunk.can_read(2) {
            break;
        }
        orders.push(rd16(&mut chunk));
    }
    let num_patterns = ((orders.iter().copied().max().unwrap_or(0) as u32 + 1) & 0xFFFF) as usize;
    let event_size = if version < 4 { 4 } else { 5 };
    let mut tr = Tr { samples, finetune: fine, instruments: Vec::new(), tempo: 125.0, speed: 6 };
    let mut patterns: Vec<Option<Pattern>> = vec![None; num_patterns];
    for p in patterns.iter_mut() {
        if !f.can_read(rows * channels * event_size) {
            continue;
        }
        let mut pat = Pattern::new(rows, channels);
        let mut last = vec![(NOTE_NONE, 0u8); channels];
        for row in 0..rows {
            for ch in 0..channels {
                let m = &mut pat.cells[row * channels + ch];
                let mut data = [0u8; 5];
                data[..event_size].copy_from_slice(f.bytes(event_size).unwrap_or(&[0; 5][..event_size]));
                if (24..84).contains(&data[0]) {
                    m.note = data[0] + NOTE_MIN + 12;
                    last[ch].0 = m.note;
                }
                if data[1] != 0 {
                    m.instr = data[1];
                    last[ch].1 = m.instr;
                }
                tr.effect(m, data[2], data[3], -(version as i16), last[ch].0, last[ch].1);
                if data[4] != 0 {
                    m.volcmd = VolCmd::Volume;
                    m.vol = ((data[4] as u32 + 1) / 4) as u8;
                }
            }
        }
        *p = Some(pat);
    }
    for (k, s) in tr.samples.iter_mut().enumerate() {
        s.data = read_pcm_gt(&mut f, s.length, s.bits16, false, true);
        let _ = k;
    }
    let instruments = tr.instruments;
    Some(Module {
        title: name(&file[4..36]),
        channels,
        speed: 6,
        tempo: 125.0,
        global_vol: 256,
        preamp: 48,
        linear: false,
        old_effects: false,
        compat_gxx: false,
        chn_pan: (0..channels).map(|c| if c & 3 == 1 || c & 3 == 2 { 0xC0 } else { 0x40 }).collect(),
        chn_surround: vec![false; channels],
        chn_vol: vec![64; channels],
        orders,
        patterns,
        instruments,
        samples: tr.samples,
        rules: Rules::default(),
    })
}

// ---------------------------------------------------------------------------------------------
// GT2

/// An envelope as read: the nodes, the flags and the LFO.
struct Env2 {
    flags: u16,
    fade_out: u16,
    lfo_depth: u16,
    lfo_speed: u16,
    lfo_sweep: u16,
    lfo_wave: u16,
    env: Envelope,
}

/// `GT2Envelope::ConvertToMPT` (`kind` 0 volume, 1 panning, 2 pitch).
fn read_envelope(r: &mut Reader, kind: u8) -> Env2 {
    let (d, _) = r.struct_or_zero(282);
    let num = (be16(&d, 0) as usize).min(64);
    let flags = be16(&d, 12);
    let mut e = Envelope {
        flags: 0,
        nodes: Vec::with_capacity(num),
        loop_start: be16(&d, 14) as u8,
        loop_end: be16(&d, 16) as u8,
        sus_start: be16(&d, 20) as u8,
        sus_end: be16(&d, 22) as u8,
    };
    let (min, max): (i32, i32) = match kind {
        1 => (-128, 127),
        2 => (-9999 * 16 / 99, 9999 * 16 / 99),
        _ => (0, 4096),
    };
    let mut tick: u16 = 0;
    for i in 0..num {
        let dur = be16(&d, 26 + i * 4);
        let value = be16(&d, 28 + i * 4) as i16 as i32 + min;
        let v = ((value as i64 * 64 + (max - min) as i64 / 2) / (max - min) as i64).clamp(0, 255) as u8;
        e.nodes.push((tick, v));
        tick = tick.wrapping_add(dur.max(1));
    }
    e.flags = if !e.nodes.is_empty() { 1 } else { 0 } | if flags & 0x20 != 0 { 2 } else { 0 } | if flags & 0x10 != 0 { 4 } else { 0 };
    Env2 { flags, fade_out: be16(&d, 10), lfo_depth: be16(&d, 2), lfo_speed: be16(&d, 4), lfo_sweep: be16(&d, 6), lfo_wave: be16(&d, 8), env: e }
}

/// What the IT file cannot hold of the volume and panning envelopes: the nodes beyond 25.
type EnvFix = Vec<(usize, usize, Vec<(i16, i16)>)>;

fn read2(file: &[u8]) -> Option<(Module, EnvFix)> {
    if !test_gt2(file) {
        return None;
    }
    let version = file[3];
    let mut f = Reader::new(file);
    f.skip(GT2_HEADER);
    let panned_n = be16(file, 234) as usize;
    let mut panned = Vec::new();
    if f.can_read(panned_n * 2) {
        for _ in 0..panned_n {
            panned.push(rd16(&mut f));
        }
    }
    let chunks = read_chunks(&mut f);

    let channels = {
        let mut c = first(&chunks, b"PATS").filter(|c| c.can_read(2))?;
        let n = rd16(&mut c) as usize;
        if !(1..=127).contains(&n) {
            return None;
        }
        n
    };
    let mut md_speed: u32 = 6;
    let mut md_tempo: f64 = 125.0;
    let mut global_vol: u32 = 384 / (3 + channels as u32);
    let mut linear = false;
    let mut chn_pan = vec![128u16; channels];
    let mut chn_vol = vec![64u8; channels];

    if version <= 5 {
        md_speed = (be16(file, 228) as u32).max(1);
        md_tempo = (be16(file, 230) as u32).max(1) as f64;
        global_vol = (muldivr(be16(file, 232) as u64, 256, 4095) as u32).min(256);
        for (c, &p) in panned.iter().take(channels).enumerate() {
            chn_pan[c] = (muldivr(p as u64, 256, 4095) as u16).min(256);
        }
    }

    let mut orders = Vec::new();
    {
        let mut c = first(&chunks, b"SONG").filter(|c| c.can_read(2))?;
        let n = rd16(&mut c) as usize;
        let _restart = rd16(&mut c);
        let n = n.min(c.left() / 2);
        for _ in 0..n {
            orders.push(rd16(&mut c));
        }
    }

    if let Some(mut c) = first(&chunks, b"TCN2").filter(|c| c.can_read(12)) {
        let (chunk_version, bpm_int, bpm_fract, speed) = (rd16(&mut c), rd16(&mut c), rd16(&mut c), rd16(&mut c));
        md_tempo = bpm_int.clamp(32, 999) as f64 + (muldivr(bpm_fract as u64, 10000, 65536) as f64) / 10000.0;
        md_speed = speed.clamp(1, 255) as u32;
        let _time_signature = (rd16(&mut c), rd16(&mut c));
        if chunk_version >= 1 {
            linear = rd16(&mut c) == 0;
        }
    }

    if let Some(mut c) = first(&chunks, b"TVOL").filter(|c| c.can_read(2)) {
        let n = (rd16(&mut c) as usize).min(channels);
        for v in chn_vol.iter_mut().take(n) {
            *v = (rd16(&mut c).min(4096) / 64) as u8;
        }
    }

    // only the first mix preset
    if let Some(mut c) = first(&chunks, b"MIXP").filter(|c| c.can_read(56)) {
        let (h, _) = c.struct_or_zero(56);
        if be16(&h, 34) == 4 && be16(&h, 50) == 0x101 {
            global_vol = muldivr(be16(&h, 42).min(0x4000) as u64, 256, 0x1000) as u32;
            for _ in 0..be16(&h, 38) {
                let Some(t) = c.bytes(8) else { break };
                let (index, volume, balance) = (be16(t, 2) as usize, be16(t, 4), be16(t, 6));
                if t[0] == 0 && index < channels {
                    chn_pan[index] = muldivr(balance as u64, 256, 0xFFF) as u16;
                    chn_vol[index] = muldivr(volume as u64, 64, 0x1000) as u8;
                }
            }
        }
    }

    let mut samples: Vec<Sample> = Vec::new();
    let mut fine: Vec<i32> = Vec::new();
    let put_sample = |samples: &mut Vec<Sample>, fine: &mut Vec<i32>, num: usize, s: Sample, ft: i32| {
        if samples.len() < num {
            samples.resize(num, Sample::new());
            fine.resize(num, 0);
        }
        samples[num - 1] = s;
        fine[num - 1] = ft;
    };

    for mut c in all(&chunks, b"SAM2") {
        let Some(h) = c.bytes(78) else { continue };
        let num = be16(h, 0) as usize;
        let (ty, coding) = (be16(h, 30), be16(h, 48));
        if num == 0 || num >= 4000 || coding > 1 || ty > 1 {
            continue;
        }
        let mut s = Sample::new();
        s.global_vol = (be16(h, 40) / 4).min(255) as u8;
        let pan = be16(h, 38) as i16;
        if pan > 0 {
            s.pan = Some(muldivr(pan as u64, 256, 4095) as u16);
        }
        s.length = be32(h, 58);
        s.loop_start = be32(h, 62);
        s.loop_end = s.loop_start.wrapping_add(be32(h, 66));
        let loop_type = be16(h, 44);
        s.looped = loop_type != 0;
        s.pingpong = loop_type & 2 != 0;
        s.c5speed = be32(h, 54).wrapping_mul(2);
        let ft = be16(h, 42) as i16;
        if ft != 0 {
            s.c5speed = transpose(s.c5speed, ft as f64 / (12.0 * 8.0));
        }
        s.name = name(&h[2..30]);
        let (bits, endian, nch, data_offset, fname_len) = (be16(h, 32), be16(h, 34), be16(h, 36), be32(h, 74), be16(h, 50) as usize);
        let off = data_offset.wrapping_sub(8) as usize;
        if data_offset >= 8 && off <= c.len() {
            c.seek(off);
            if ty == 0 {
                s.bits16 = bits != 8;
                s.data = read_pcm_gt(&mut c, s.length, bits != 8, nch != 1, endian == 0);
            }
            let take = fname_len.min(c.left());
            let fname = c.rest()[..take].to_vec();
            s.filename = fname.iter().copied().take_while(|&b| b != 0).collect();
        }
        put_sample(&mut samples, &mut fine, num, s, xm_finetune(ft));
    }

    for mut c in all(&chunks, b"SAMP") {
        let Some(h) = c.bytes(56) else { continue };
        let num = be16(h, 0) as usize;
        if num == 0 || num >= 4000 || be16(h, 54) != 0 {
            continue;
        }
        let mut s = Sample::new();
        let flags = be16(h, 30);
        let volume = be16(h, 50) as i16;
        s.global_vol = (((volume as i32 as u32) / 4) as u16).min(255) as u8;
        let pan = be16(h, 32) as i16;
        if pan > 0 {
            s.pan = Some(muldivr(pan as u64, 256, 4095) as u16);
        }
        let bits = be16(h, 34);
        s.length = be32(h, 38);
        s.loop_start = be32(h, 42);
        s.loop_end = s.loop_start.wrapping_add(be32(h, 46));
        if bits == 16 {
            s.length /= 2;
            s.loop_start /= 2;
            s.loop_end /= 2;
        }
        s.looped = s.loop_start > 0 || be32(h, 46) > 2;
        s.pingpong = flags & 2 != 0;
        s.c5speed = (be16(h, 36) as u32) * 2;
        let ft = be16(h, 52) as i16;
        if ft != 0 {
            s.c5speed = transpose(s.c5speed, ft as f64 / (12.0 * 8.0));
        }
        s.name = name(&h[2..30]);
        s.bits16 = bits != 8;
        s.data = read_pcm_gt(&mut c, s.length, bits != 8, flags & 1 != 0, true);
        put_sample(&mut samples, &mut fine, num, s, xm_finetune(ft));
    }

    // instruments
    let mut instruments: Vec<Option<Instrument>> = Vec::new();
    let mut fix: EnvFix = Vec::new();
    for mut c in all(&chunks, b"INST") {
        let Some(h) = c.bytes(308) else { continue };
        let (num, ty) = (be16(h, 0) as usize, be16(h, 30));
        if ty != 0 || num == 0 || num >= 256 {
            continue;
        }
        if instruments.len() < num {
            instruments.resize(num, None);
        }
        let mut ins = Instrument::new(0);
        ins.name = name(&h[2..30]);
        let pan = be16(h, 34) as i16;
        if pan > 0 {
            ins.pan = Some(muldivr(pan as u64, 256, 4095) as u16);
        }
        let mut used: Vec<u16> = Vec::new();
        for i in 0..120 {
            let (smp, tr) = (h[52 + i * 2], h[53 + i * 2] as i8);
            let note = (ins.keyboard[i].0 as i32 + 1 + tr as i32).clamp(1, 128);
            ins.keyboard[i] = ((note - 1).min(255) as u8, smp as u16);
            if smp != 0 && !used.contains(&(smp as u16)) {
                used.push(smp as u16);
            }
        }
        // (the samples of the keyboard beyond 120 notes are not played)
        for i in 120..128 {
            let smp = h[52 + i * 2] as u16;
            if smp != 0 && !used.contains(&smp) {
                used.push(smp);
            }
        }
        for &smp in &used {
            if smp as usize <= samples.len() {
                samples[smp as usize - 1].volume = be16(h, 32);
            }
        }
        ins.cutoff = 0x7F | 0x80;
        ins.resonance = 0x80;
        let (vol_id, tone_id, pan_id, cutoff_id) = (be16(h, 36), be16(h, 38), be16(h, 40), be16(h, 42));
        let inst_version = be16(h, 50);
        if inst_version == 0 {
            // the old "envelopes" are scripts, which are left out
            ins.fadeout = if vol_id != 0 { 0 } else { 32768 };
        } else {
            let ext = c.struct_partial(102, if inst_version == 1 { 98 } else { 102 });
            let flags = be16(&ext, 0);
            let filter_on = flags & 1 != 0;
            if filter_on {
                let ftype = flags & 0x18;
                let lowpass_like = ftype != 0x08;
                if flags & 2 != 0 && lowpass_like {
                    let max_cut = parse_f64(&ext[2..26]);
                    if max_cut > 20.0 {
                        ins.cutoff = frequency_to_cutoff(max_cut).min(0x7F) | 0x80;
                    }
                }
                if flags & 4 != 0 && lowpass_like {
                    let max_reso = parse_f64(&ext[50..74]);
                    ins.resonance = sat8(max_reso * 127.0 / 24.0).min(0x7F) | 0x80;
                }
            }
            let num_env = ext[101];
            let enabled = [vol_id != 0, tone_id != 0, pan_id != 0, cutoff_id != 0 && filter_on];
            for env in 0..num_env as usize {
                let kind = [0u8, 2, 1, 2];
                let e = read_envelope(&mut c, kind.get(env).copied().unwrap_or(0));
                if env >= 4 || !enabled[env] {
                    continue;
                }
                if env == 3 && ins.pitch_env.flags & 1 != 0 {
                    continue;
                }
                let mut out = e.env.clone();
                if env == 3 {
                    out.flags |= 0x10;
                }
                match env {
                    0 => {
                        ins.fadeout = if e.flags & 1 != 0 { (e.fade_out & 0x7FFF) as u32 } else { 0 };
                        if out.flags & 1 != 0 && out.nodes.len() > 25 {
                            fix.push((num - 1, 0, out.nodes.iter().map(|&(t, v)| (t as i16, v as i16)).collect()));
                        }
                        ins.vol_env = out;
                    }
                    2 => {
                        if out.flags & 1 != 0 && out.nodes.len() > 25 {
                            fix.push((num - 1, 1, out.nodes.iter().map(|&(t, v)| (t as i16, v as i16 - 32)).collect()));
                        }
                        ins.pan_env = out;
                    }
                    _ => {
                        ins.pitch_env = out;
                        if e.flags & 2 != 0 {
                            let wave = [0u8, 1, 0, 2, 3, 4];
                            let vt = wave[if (e.lfo_wave as usize) < wave.len() { e.lfo_wave as usize } else { 0 }];
                            let sweep = (255 / e.lfo_sweep.max(1)) as u8;
                            let depth = (muldivr(e.lfo_depth as u64, 64, 100)).min(255) as u8;
                            let speed = (256 / e.lfo_speed.max(1)).min(255) as u8;
                            for &smp in &used {
                                if let Some(s) = samples.get_mut(smp as usize - 1) {
                                    s.vib_type = vt;
                                    s.vib_sweep = sweep;
                                    s.vib_depth = depth;
                                    s.vib_rate = speed;
                                }
                            }
                        }
                    }
                }
            }
        }
        instruments[num - 1] = Some(ins);
    }

    // patterns
    let mut tr = Tr { samples, finetune: fine, instruments, tempo: md_tempo, speed: md_speed };
    let mut patterns: Vec<Option<Pattern>> = Vec::new();
    for mut c in all(&chunks, b"PATD") {
        if !c.can_read(24) {
            continue;
        }
        let pat_num = rd16(&mut c) as usize;
        c.skip(16);
        let coding = rd16(&mut c);
        let rows = rd16(&mut c) as usize;
        let tracks = rd16(&mut c) as usize;
        if (c.left() as u64) < 5 * rows as u64 * tracks as u64 {
            continue;
        }
        let exists = matches!(patterns.get(pat_num), Some(Some(_)));
        if coding > 1 || rows == 0 || rows > 1024 || pat_num >= 0xFFFF || exists {
            continue;
        }
        if patterns.len() <= pat_num {
            patterns.resize(pat_num + 1, None);
        }
        let mut pat = Pattern::new(rows, channels);
        let mut last = vec![(NOTE_NONE, 0u8); tracks];
        let mut dummy = Cell::default();
        for row in 0..rows {
            for ch in 0..tracks {
                let m = if ch < channels { &mut pat.cells[row * channels + ch] } else { &mut dummy };
                let d = c.bytes(5).unwrap_or(&[0; 5]);
                if d[0] > 0 && d[0] <= 128 {
                    m.note = d[0] + NOTE_MIN;
                    last[ch].0 = m.note;
                }
                if d[1] != 0 {
                    m.instr = d[1];
                    last[ch].1 = m.instr;
                }
                if d[4] != 0 {
                    m.volcmd = VolCmd::Volume;
                    m.vol = if coding == 0 { d[4] / 4 } else { d[4].wrapping_sub(0x10) };
                }
                tr.effect(m, d[2], d[3], version as i16, last[ch].0, last[ch].1);
                if ch >= channels {
                    dummy = Cell::default();
                }
            }
        }
        patterns[pat_num] = Some(pat);
    }

    let md = Module {
        title: name(&file[8..40]),
        channels,
        speed: md_speed.min(255) as u8,
        tempo: md_tempo,
        global_vol,
        preamp: 256,
        linear,
        old_effects: false,
        compat_gxx: false,
        chn_pan,
        chn_surround: vec![false; channels],
        chn_vol,
        orders,
        patterns,
        instruments: tr.instruments,
        samples: tr.samples,
        rules: Rules::default(),
    };
    Some((md, fix))
}

/// The module written as IT, with its fixups and the envelopes IT cannot hold.
pub fn to_native(file: &[u8]) -> Option<(Vec<u8>, Fixups, Vec<(usize, usize, Vec<(i16, i16)>)>)> {
    let (mut md, envs) = if test_gt2(file) { read2(file)? } else { (read_gtk(file)?, Vec::new()) };
    // (`ModInstrument::Sanitize`: a keyboard sample the module does not have is none)
    let n = md.samples.len() as u16;
    for i in md.instruments.iter_mut().flatten() {
        for e in i.keyboard.iter_mut() {
            if e.1 > n {
                e.1 = 0;
            }
        }
    }
    let (native, mut fix) = build(&md, Kind::It)?;
    fix.ext_filter = test_gt2(file);
    Some((native, fix, envs))
}

/// The `xmp::LOADERS` entry: the module written as IT, loaded by the libxmp port's IT loader,
/// with the envelopes IT cannot hold set on its instruments.
pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let file = whole_file(f);
    let (native, fix, envs) = to_native(&file).ok_or(())?;
    load_native(m, &native, Kind::It, &fix, &name_of(&file))?;
    if test_gt2(&file) {
        // GT2 sets OpenMPT's sample pre-amp to 256, which an IT file's mix volume (at most 128)
        // cannot hold: the libxmp module's mix volume is raised to it
        m.mvol = m.mvol * 256 / 128;
    }
    for (ins, kind, nodes) in envs {
        let Some(xi) = m.module.xxi.get_mut(ins) else { continue };
        let env = if kind == 0 { &mut xi.aei } else { &mut xi.pei };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn put16(v: &mut Vec<u8>, x: u16) {
        v.extend_from_slice(&x.to_be_bytes());
    }
    fn put32(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_be_bytes());
    }
    fn chunk(v: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
        v.extend_from_slice(id);
        put32(v, body.len() as u32 + 8);
        v.extend_from_slice(body);
    }

    /// A GT2 version 4 file: one track, one 4-row pattern (C-4 instrument 1, volume column 32,
    /// a set-speed command on row 2), one 8-bit sample of 16 bytes (SAMP).
    fn synth_gt2() -> Vec<u8> {
        let mut v = b"GT2\x04".to_vec();
        put32(&mut v, 236);
        let mut name = b"tiny".to_vec();
        name.resize(32, b' ');
        v.extend_from_slice(&name);
        v.extend_from_slice(&[b' '; 160]);
        v.extend_from_slice(&[1, 2]);
        put16(&mut v, 1999);
        v.extend_from_slice(&[b' '; 24]);
        for x in [6u16, 125, 0xFFF, 0] {
            put16(&mut v, x);
        }
        let mut pats = Vec::new();
        put16(&mut pats, 1);
        chunk(&mut v, b"PATS", &pats);
        let mut song = Vec::new();
        for x in [1u16, 0, 0] {
            put16(&mut song, x);
        }
        chunk(&mut v, b"SONG", &song);
        let mut patd = Vec::new();
        put16(&mut patd, 0);
        patd.extend_from_slice(&[b' '; 16]);
        for x in [0u16, 4, 1] {
            put16(&mut patd, x);
        }
        patd.extend_from_slice(&[49, 1, 0, 0, 128]);
        patd.extend_from_slice(&[0; 5]);
        patd.extend_from_slice(&[0, 0, 0x0F, 3, 0]);
        patd.extend_from_slice(&[0; 5]);
        chunk(&mut v, b"PATD", &patd);
        let mut samp = Vec::new();
        put16(&mut samp, 1);
        samp.extend_from_slice(&[b' '; 28]);
        for x in [0u16, 0xFFFF, 8, 8363] {
            put16(&mut samp, x);
        }
        for x in [16u32, 0, 2] {
            put32(&mut samp, x);
        }
        for x in [255u16, 0, 0] {
            put16(&mut samp, x);
        }
        samp.extend((0..16).map(|i| (i * 8) as u8));
        chunk(&mut v, b"SAMP", &samp);
        chunk(&mut v, b"ENDC", &[]);
        v
    }

    /// A GTK version 4 file: one channel, 4 rows, one order, one 8-bit sample.
    fn synth_gtk() -> Vec<u8> {
        let mut v = b"GTK\x04".to_vec();
        v.extend_from_slice(&[b' '; 32 + 160]);
        for x in [1u16, 4, 1, 1, 0] {
            put16(&mut v, x);
        }
        v.extend_from_slice(&[b' '; 28]);
        v.extend_from_slice(&[0; 14]);
        put16(&mut v, 0xFFFF);
        put16(&mut v, 1);
        put16(&mut v, 8363);
        for x in [16u32, 0, 2] {
            put32(&mut v, x);
        }
        put16(&mut v, 255);
        put16(&mut v, 0);
        let mut orders = vec![0u8; 512];
        orders[1] = 0;
        v.extend_from_slice(&orders);
        v.extend_from_slice(&[49, 1, 0, 0, 128]);
        v.extend_from_slice(&[0; 15]);
        v.extend((0..16).map(|i| (i * 8) as u8));
        v
    }

    #[test]
    fn log_volume() {
        assert_eq!(log_to_linear(0), 1);
        assert_eq!(log_to_linear(0xC00), 4096);
        assert_eq!(log_to_linear(0xFFFF), log_to_linear(0xFFF));
    }

    #[test]
    fn loads_synthetic() {
        for (f, name, note) in [(synth_gt2(), "Graoumf Tracker v4", 50), (synth_gtk(), "Graoumf Tracker v4", 62)] {
            assert!(test(&f));
            let mut m = ModuleData::default();
            load(&mut m, &mut Hio::new(&f)).unwrap();
            assert!(m.module.type_.starts_with(name), "{}", m.module.type_);
            assert_eq!(m.module.chn, 1);
            assert_eq!(m.module.xxp[0].rows, 4);
            let e = m.module.event_mut(0, 0, 0);
            assert_eq!(e.note, note);
            assert_eq!(e.ins, 1);
        }
    }

    /// Damaged files (cut short, bytes changed) load or are refused, never panic.
    #[test]
    fn damaged_files() {
        for f in [synth_gt2(), synth_gtk()] {
            for n in 0..f.len() {
                let _ = load(&mut ModuleData::default(), &mut Hio::new(&f[..n]));
            }
            let mut seed = 12345u32;
            for _ in 0..600 {
                let mut g = f.clone();
                for _ in 0..3 {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    let at = (seed >> 8) as usize % g.len();
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    g[at] = (seed >> 16) as u8;
                }
                let _ = load(&mut ModuleData::default(), &mut Hio::new(&g));
            }
        }
    }

    /// Writes the modules of the files in `$GT2_DUMP_IN` put into IT to `$GT2_DUMP_OUT` (for
    /// checks against a reference player):
    /// `GT2_DUMP_IN=dir GT2_DUMP_OUT=dir cargo test --release gt2::tests::dump -- --ignored`
    #[test]
    #[ignore]
    fn dump() {
        let (Ok(inp), Ok(out)) = (std::env::var("GT2_DUMP_IN"), std::env::var("GT2_DUMP_OUT")) else { return };
        let mut names: Vec<_> = std::fs::read_dir(&inp).unwrap().flatten().map(|e| e.path()).collect();
        names.sort();
        for p in names {
            let d = std::fs::read(&p).unwrap();
            match to_native(&d) {
                Some((m, _, _)) => {
                    let stem = p.file_name().unwrap().to_string_lossy().into_owned();
                    std::fs::write(std::path::Path::new(&out).join(format!("{stem}.it")), m).unwrap();
                }
                None => println!("refused: {}", p.display()),
            }
        }
    }
}
