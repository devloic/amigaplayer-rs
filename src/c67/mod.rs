//! CDFM Composer 670 modules (`.c67`): a PC tracker for the AdLib (OPL2) with four sampled
//! channels beside nine FM ones.
//!
//! The loader is a port of libopenmpt 0.8.7's `soundlib/Load_c67.cpp` (BSD-3-Clause, Copyright
//! (c) 2004-2026 OpenMPT Project Developers and Contributors): the header checks, the pattern
//! stream (note, instrument and volume; volume only; row delay; end of pattern), the reordering of
//! the OPL patch bytes and the volume tables. The playback is the part of OpenMPT's player
//! (`Snd_fx.cpp`, `Sndmix.cpp`) that an S3M-type module with no effect column uses: speed 1..15
//! ticks per row, tempo 143, a pattern break after the last row written, FM notes on the Opal
//! OPL3 emulator through `crate::opl` (with OpenMPT's "beating oscillators" detune that
//! Composer 670 needs) and sample notes on a plain mixer. See `LICENSE-openmpt`, which has to
//! travel with this file.
//!
//! The `.670` files (the compiled form, with the samples) are a different layout that OpenMPT
//! does not read either (the CDFM converter turns them back into `.c67`); they are not played.
//! The song ends where the order list does (OpenMPT then loops to the restart position).

use crate::opl::{muldivr_unsigned, Opl, OplPatch};

const HEADER: usize = 1954;
const PATTERN_TABLE: usize = 2978;
const CHANNELS: usize = 13;
const PCM_CHANNELS: usize = 4;
const TEMPO: f64 = 143.0;
/// OpenMPT's `OPL::Mix` factor for `m_nVSTiVolume` 48, applied to the 16-bit Opal output.
const OPL_FACTOR: f64 = 6169.0 / 4096.0;
/// A sample voice's level: 8-bit sample at full volume, as OpenMPT's S3M mixer scales it.
const PCM_GAIN: f64 = 0.5;

#[derive(Clone, Default)]
struct Sample {
    data: Vec<i8>,
    loop_start: usize,
    loop_end: usize,
    looped: bool,
}

#[derive(Clone, Copy)]
enum Ev {
    /// channel, note (OpenMPT numbering, 1 = C-0), instrument index 0..31, volume 0..15
    Note(u8, u8, u8, u8),
    /// channel, volume 0..15
    Vol(u8, u8),
}

#[derive(Clone, Default)]
struct Pattern {
    /// The events of each row.
    rows: Vec<Vec<Ev>>,
    /// Rows played (OpenMPT's pattern break on the row before the end marker).
    length: usize,
}

/// A loaded module.
pub struct Module {
    speed: u8,
    #[allow(dead_code)]
    restart: u8,
    pcm: Vec<Sample>,
    fm: Vec<OplPatch>,
    orders: Vec<u8>,
    patterns: Vec<Pattern>,
    pub names: Vec<String>,
}

fn le32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

const SAMPLE_NAMES: usize = 2;
const SAMPLE_HDRS: usize = SAMPLE_NAMES + 13 * 32;
const FM_NAMES: usize = SAMPLE_HDRS + 16 * 32;
const FM_INSTR: usize = FM_NAMES + 13 * 32;
const ORDERS: usize = FM_INSTR + 11 * 32;

/// `ValidateHeader`.
fn validate_header(d: &[u8]) -> bool {
    if d.len() < HEADER + 1024 || !(1..=15).contains(&d[0]) {
        return false;
    }
    if d[ORDERS..ORDERS + 256].iter().any(|&o| o >= 128 && o != 0xFF) {
        return false;
    }
    let mut any = false;
    for s in 0..32 {
        let h = SAMPLE_HDRS + 16 * s;
        let (unknown, length, loop_start, loop_end) = (le32(d, h), le32(d, h + 4), le32(d, h + 8), le32(d, h + 12));
        let fm = &d[FM_INSTR + 11 * s..FM_INSTR + 11 * s + 11];
        if d[SAMPLE_NAMES + 13 * s + 12] != 0
            || unknown != 0
            || length > 0xFFFFF
            || d[FM_NAMES + 13 * s + 12] != 0
            || fm[0] & 0xF0 != 0
            || fm[5] & 0xFC != 0
            || fm[10] & 0xFC != 0
        {
            return false;
        }
        if length != 0 && loop_end < 0xFFFFF && (loop_end > length || loop_start > loop_end) {
            return false;
        }
        if length != 0 || fm.iter().any(|&b| b != 0) {
            any = true;
        }
    }
    any
}

/// Whether `data` is a Composer 670 module this loads (OpenMPT's `ReadC67`, without samples).
pub fn test(data: &[u8]) -> bool {
    load(data).is_some()
}

fn text(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b[..end].iter().map(|&c| c as char).collect::<String>().trim_end().to_string()
}

/// `ReadC67`.
pub fn load(d: &[u8]) -> Option<Module> {
    if !validate_header(d) {
        return None;
    }
    let mut offs = [0usize; 128];
    let mut lens = [0usize; 128];
    for p in 0..128 {
        let o = le32(d, HEADER + 4 * p);
        let l = le32(d, HEADER + 512 + 4 * p);
        if o > 0xFF_FFFF || l < 3 || l > 0x1000 || (d.len() as u64) < 2978 + o as u64 + l as u64 {
            return None;
        }
        offs[p] = o as usize;
        lens[p] = l as usize;
    }
    let mut patterns = Vec::with_capacity(128);
    for p in 0..128 {
        let chunk = &d[PATTERN_TABLE + offs[p]..PATTERN_TABLE + offs[p] + lens[p]];
        let mut pat = Pattern { rows: vec![Vec::new(); 64], length: 64 };
        let mut row = 0usize;
        let mut i = 0usize;
        while row < 64 && i < chunk.len() {
            let cmd = chunk[i];
            i += 1;
            match cmd {
                0..=0x0C => {
                    // a short read gives zeros, as FileReader does
                    let note = chunk.get(i).copied().unwrap_or(0);
                    let iv = chunk.get(i + 1).copied().unwrap_or(0);
                    i += 2;
                    let fm = cmd >= 4;
                    let n = 1 + if fm { 12 } else { 36 } + (note & 0x0F) as u32 + ((note >> 4) & 7) as u32 * 12;
                    let ins = (iv >> 4) + ((note & 0x80) >> 3);
                    pat.rows[row].push(Ev::Note(cmd, n as u8, ins, iv & 0x0F));
                }
                0x20..=0x2C => {
                    let v = chunk.get(i).copied().unwrap_or(0);
                    i += 1;
                    pat.rows[row].push(Ev::Vol(cmd - 0x20, v & 0x0F));
                }
                0x40 => {
                    row += chunk.get(i).copied().unwrap_or(0) as usize;
                    i += 1;
                }
                0x60 => {
                    if row > 0 {
                        pat.length = row.min(64);
                    }
                    break;
                }
                _ => return None,
            }
        }
        patterns.push(pat);
    }
    // sample data follows the last pattern chunk read (pattern 127's)
    let mut at = PATTERN_TABLE + offs[127] + lens[127];
    let mut pcm = Vec::with_capacity(32);
    for s in 0..32 {
        let h = SAMPLE_HDRS + 16 * s;
        let (length, ls, le) = (le32(d, h + 4) as usize, le32(d, h + 8), le32(d, h + 12));
        let mut sm = Sample::default();
        if le <= length as u32 {
            sm.loop_start = ls as usize;
            sm.loop_end = le as usize;
            sm.looped = true;
        }
        let end = (at + length).min(d.len());
        if at < end {
            sm.data = d[at..end].iter().map(|&b| (b ^ 0x80) as i8).collect();
        }
        at = (at + length).min(d.len());
        pcm.push(sm);
    }
    let mut fm = Vec::with_capacity(32);
    for s in 0..32 {
        let f = &d[FM_INSTR + 11 * s..FM_INSTR + 11 * s + 11];
        // interleave modulator and carrier
        fm.push([f[1], f[6], f[2], f[7], f[3], f[8], f[4], f[9], f[5], f[10], f[0], 0]);
    }
    let mut orders = Vec::new();
    for &o in &d[ORDERS..ORDERS + 256] {
        if o == 0xFF {
            break;
        }
        orders.push(o);
    }
    let names = (0..32).map(|s| text(&d[FM_NAMES + 13 * s..FM_NAMES + 13 * s + 13])).collect();
    Some(Module { speed: d[0], restart: d[1], pcm, fm, orders, patterns, names })
}

/// CDFM's volume (0..15) as the OPL's linear scale 8..64 (`TranslateVolume`).
fn fm_volume(v: u8) -> u32 {
    const T: [u32; 16] = [0x08, 0x10, 0x18, 0x20, 0x28, 0x2C, 0x30, 0x34, 0x36, 0x38, 0x3A, 0x3C, 0x3D, 0x3E, 0x3F, 0x40];
    T[(v & 15) as usize]
}

/// ST3 periods of the octave 0 C..B (`FreqS3MTable`).
const FREQ_S3M: [u64; 12] = [1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960, 907];

/// The playback frequency (`GetPeriodFromNote`, then `GetFreqFromPeriod` for non-linear slides)
/// of OpenMPT note `note` (1 = C-0) of a sample with `c5speed`, in 1/16 Hz.
fn freq16(note: u32, c5speed: u64) -> u64 {
    let n = (note - 1) as u64;
    let period = (8363 * (FREQ_S3M[(n % 12) as usize] << 5) / (c5speed << (n / 12))).max(1);
    (8363 * (1712u64 << 8) << 4) / (period << 8)
}

fn pcm_volume(v: u8) -> u32 {
    4 + (v & 15) as u32 * 4
}

#[derive(Clone, Default)]
struct Voice {
    sample: Option<usize>,
    pos: f64,
    step: f64,
    vol: u32,
}

/// A module playing at a sample rate, rendering 16-bit stereo.
pub struct C67 {
    m: Module,
    rate: u32,
    opl: Opl,
    voices: [Voice; PCM_CHANNELS],
    order: usize,
    row: usize,
    tick: u8,
    /// samples left in the current tick
    left: f64,
    done: bool,
    ending: bool,
    nearest: bool,
    format: String,
}

impl C67 {
    /// Loads `data` and starts it at `rate` Hz; None for what is not a module this plays.
    pub fn new(data: &[u8], rate: u32) -> Option<C67> {
        let m = load(data)?;
        if m.orders.is_empty() {
            return None;
        }
        let mut c = C67 {
            m,
            rate,
            opl: Opl::new(rate, CHANNELS),
            voices: Default::default(),
            order: 0,
            row: 0,
            tick: 0,
            left: 0.0,
            done: false,
            ending: false,
            nearest: false,
            format: "CDFM (Composer 670)".to_string(),
        };
        c.restart();
        Some(c)
    }

    pub fn format(&self) -> &str {
        &self.format
    }

    pub fn title(&self) -> &str {
        ""
    }

    pub fn set_nearest(&mut self, nearest: bool) {
        self.nearest = nearest;
    }

    pub fn restart(&mut self) {
        self.opl = Opl::new(self.rate, CHANNELS);
        self.voices = Default::default();
        self.order = 0;
        self.row = 0;
        self.tick = 0;
        self.left = 0.0;
        self.done = false;
        self.ending = false;
    }

    pub fn ended(&self) -> bool {
        self.done
    }

    pub fn position(&self) -> (i32, i32, i32) {
        let pat = self.m.orders.get(self.order).copied().unwrap_or(0) as i32;
        (self.order as i32, pat, self.row as i32)
    }

    /// OpenMPT's classic tempo mode: a whole number of samples per tick, no carry.
    fn tick_len(&self) -> f64 {
        ((self.rate as u64 * 5) / (TEMPO as u64 * 2)).max(1) as f64
    }

    pub fn duration_ms(&self) -> i32 {
        let rows: usize = self.m.orders.iter().map(|&o| self.m.patterns[o as usize].length).sum();
        (rows as f64 * self.m.speed as f64 * self.tick_len() / self.rate as f64 * 1000.0).round() as i32
    }

    fn row_events(&mut self) {
        let pat = self.m.orders[self.order] as usize;
        let evs = self.m.patterns[pat].rows[self.row].clone();
        for e in evs {
            match e {
                Ev::Note(ch, note, ins, vol) => {
                    let ch = ch as usize;
                    if ch < PCM_CHANNELS {
                        let sm = &self.m.pcm[ins as usize];
                                                let hz = freq16(note as u32, 8287) as f64 / 16.0;
                        let v = &mut self.voices[ch];
                        v.vol = pcm_volume(vol);
                        if sm.data.is_empty() {
                            v.sample = None;
                        } else {
                            v.sample = Some(ins as usize);
                            v.pos = 0.0;
                            v.step = hz / self.rate as f64;
                        }
                    } else {
                        let patch = self.m.fm[ins as usize];
                        self.opl.patch(ch, &patch);
                        self.opl.note_off(ch);
                        // 8363 Hz is middle C, 261.625 Hz
                        let mhz = ((freq16(note as u32, 8363) * 261625 + (8363 << 3)) / (8363 << 4)) as u32;
                        self.opl.frequency_beating(ch, mhz, false, true);
                        self.set_fm_volume(ch, vol);
                        self.opl.pan(ch, 128);
                    }
                }
                Ev::Vol(ch, vol) => {
                    let ch = ch as usize;
                    if ch < PCM_CHANNELS {
                        self.voices[ch].vol = pcm_volume(vol);
                    } else {
                        self.set_fm_volume(ch, vol);
                    }
                }
            }
        }
    }

    fn set_fm_volume(&mut self, ch: usize, vol: u8) {
        // nCalcVolume * global * instrument volume scaled to 0..63
        let v = muldivr_unsigned(fm_volume(vol), 63, 64).min(63);
        self.opl.volume(ch, v as u8, false);
    }

    /// Moves to the next row; false at the end of the order list.
    fn advance(&mut self) -> bool {
        self.row += 1;
        let pat = self.m.orders[self.order] as usize;
        if self.row >= self.m.patterns[pat].length {
            self.row = 0;
            self.order += 1;
            if self.order >= self.m.orders.len() {
                return false;
            }
        }
        true
    }

    fn mix_pcm(&mut self, n: usize, out: &mut [f32]) {
        for (ch, v) in self.voices.iter_mut().enumerate() {
            let Some(si) = v.sample else { continue };
            let sm = &self.m.pcm[si];
            // channels alternate between 25% and 75% pan
            let (gl, gr) = if ch & 1 == 0 { (0.75, 0.25) } else { (0.25, 0.75) };
            let g = PCM_GAIN * v.vol as f64 / 64.0;
            for f in 0..n {
                let i = v.pos as usize;
                if i >= sm.data.len() {
                    v.sample = None;
                    break;
                }
                let a = sm.data[i] as f64;
                let s = if self.nearest {
                    a
                } else {
                    let b = sm.data.get(i + 1).map_or(a, |&b| b as f64);
                    a + (b - a) * (v.pos - i as f64)
                };
                let s = s / 128.0 * g;
                out[f * 2] += (s * gl) as f32;
                out[f * 2 + 1] += (s * gr) as f32;
                v.pos += v.step;
                if sm.looped && v.pos >= sm.loop_end as f64 && sm.loop_end > sm.loop_start {
                    v.pos -= (sm.loop_end - sm.loop_start) as f64;
                }
            }
        }
    }

    /// Fills `out` (interleaved stereo, normalised floats) and returns the frames written.
    pub fn render_f32(&mut self, out: &mut [f32]) -> usize {
        let want = out.len() / 2;
        let mut done = 0;
        while done < want && !self.done {
            if self.left < 1.0 {
                if self.ending {
                    self.done = true;
                    break;
                }
                if self.tick == 0 {
                    self.row_events();
                }
                self.left += self.tick_len();
                self.tick += 1;
                if self.tick >= self.m.speed {
                    self.tick = 0;
                    if !self.advance() {
                        // finish this tick's audio, then stop
                        self.ending = true;
                    }
                }
            }
            let n = (self.left as usize).clamp(1, want - done);
            let seg = &mut out[done * 2..(done + n) * 2];
            seg.fill(0.0);
            self.mix_pcm(n, seg);
            if self.opl.is_active() {
                for f in seg.chunks_exact_mut(2) {
                    let (l, r) = self.opl.sample();
                    f[0] += (l as f64 * OPL_FACTOR / 32768.0) as f32;
                    f[1] += (r as f64 * OPL_FACTOR / 32768.0) as f32;
                }
            }
            self.left -= n as f64;
            done += n;
        }
        done
    }

    /// Fills `out` (interleaved stereo) and returns the frames written: fewer at the end.
    pub fn render(&mut self, out: &mut [i16]) -> usize {
        let mut tmp = vec![0f32; out.len() & !1];
        let n = self.render_f32(&mut tmp);
        for (o, x) in out.iter_mut().zip(&tmp[..n * 2]) {
            *o = (x * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
        }
        n
    }
}

#[cfg(test)]
mod tests;
