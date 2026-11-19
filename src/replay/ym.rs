//! Atari ST / Amstrad CPC / ZX Spectrum YM register-dump files (`.ym`, Leonard's "YM!" family:
//! YM2!, YM3!, YM3b, YM5!, YM6!): the YM2149 register values of every replay frame, played back
//! on the ST's chip model of `sndh_ym.rs`.
//!
//! Original implementation, written for this project (GPL-2.0-or-later) from the file format as
//! documented by its author (Arnaud Carre's YM format description) and StSound (MIT, by Arnaud
//! Carre) read as a behavioural reference for the YM5/YM6 per-frame effects.
//!
//! A file is a stream of frames at the player rate (usually 50 Hz); each frame holds registers
//! 0..13 (YM2/YM3) or 0..15 (YM5/YM6, whose registers 1, 3, 6, 8, 14, 15 carry the effects:
//! SID voice, digidrum, sync buzzer). The effects are what the chip alone cannot do at 50 Hz, so
//! they are run per output sample: a SID voice toggles a channel's volume register at a timer
//! rate, a digidrum plays a sample through a channel's volume register with its tone and noise
//! off (as the ST did), and the sync buzzer restarts the envelope at a timer rate. Not done: the
//! YM6 "sinus SID" (StSound itself leaves it as a TODO), the YM2 "MADMAX" digidrums (their
//! samples are built into StSound, not in the file), the MIX1 and YMT1/YMT2 formats, and loop
//! points (a file plays through once).
//!
//! Most real files are LZH-packed (`-lh5-` at offset 2): a small LH5 decoder is built in, so no
//! feature is needed (other LHA header levels use the `archives` feature's).

use super::sndh::ym::Ym;

/// The MFP's timer prescalers, as the file's effect bytes index them.
const MFP_PREDIV: [u32; 8] = [0, 4, 10, 16, 50, 64, 100, 200];
/// The MFP's clock: 2457600 Hz.
const MFP_CLOCK: f64 = 2_457_600.0;

const ATTR_INTERLEAVED: u32 = 1;
const ATTR_DRUM_4BITS: u32 = 4;

/// The sample-rate-independent part of a file.
pub struct YmFile {
    pub version: u8,
    pub name: String,
    pub author: String,
    pub comment: String,
    pub clock: u32,
    /// Frames per second.
    pub rate: u32,
    pub loop_frame: u32,
    /// Register values by frame (14 meaningful for YM2/YM3; the rest are zero).
    pub frames: Vec<[u8; 16]>,
    /// Digidrum samples as 4-bit levels.
    pub drums: Vec<Vec<u8>>,
}

fn be32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn be16(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as u32)
}

/// A canonical Huffman code, read MSB first, decoded a bit at a time (these tables are tiny).
struct Huff {
    /// codes of each length
    count: [u16; 17],
    /// symbols ordered by (length, symbol)
    symbol: Vec<u16>,
    /// the single symbol of a table that has no codes at all (its length field was 0)
    only: Option<u16>,
}

impl Huff {
    fn from_lengths(len: &[u8]) -> Option<Huff> {
        let mut count = [0u16; 17];
        for &l in len {
            *count.get_mut(l as usize)? += 1;
        }
        count[0] = 0;
        let mut symbol = Vec::new();
        for l in 1..=16u8 {
            for (i, &x) in len.iter().enumerate() {
                if x == l {
                    symbol.push(i as u16);
                }
            }
        }
        Some(Huff { count, symbol, only: None })
    }

    fn constant(c: u16) -> Huff {
        Huff { count: [0; 17], symbol: Vec::new(), only: Some(c) }
    }
}

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> u32 {
        let b = self.d.get(self.pos >> 3).map_or(0, |&v| v >> (7 - (self.pos & 7)) & 1);
        self.pos += 1;
        b as u32
    }
    fn get(&mut self, n: u32) -> u32 {
        (0..n).fold(0, |v, _| v << 1 | self.bit())
    }
    fn decode(&mut self, h: &Huff) -> Option<u16> {
        if let Some(c) = h.only {
            return Some(c);
        }
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for l in 1..=16 {
            code |= self.bit() as i32;
            let count = h.count[l] as i32;
            if code - count < first {
                return h.symbol.get((index + (code - first)) as usize).copied();
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        None
    }
}

/// The code-length code (`nn` symbols, `nbit` bits for the count) of an LH5 block, with the LHA
/// "skip" after symbol `special` (-1: none).
fn read_pt(b: &mut Bits, nn: usize, nbit: u32, special: i32) -> Option<Huff> {
    let n = b.get(nbit) as usize;
    if n == 0 {
        return Some(Huff::constant(b.get(nbit) as u16));
    }
    let mut len = vec![0u8; nn];
    let mut i = 0;
    while i < n.min(nn) {
        let mut c = b.get(3);
        if c == 7 {
            while b.bit() == 1 {
                c += 1;
                if c > 16 {
                    return None;
                }
            }
        }
        len[i] = c as u8;
        i += 1;
        if i as i32 == special {
            for _ in 0..b.get(2) {
                if i < nn {
                    len[i] = 0;
                    i += 1;
                }
            }
        }
    }
    Huff::from_lengths(&len)
}

/// An LH5 stream (8 KiB window) of `size` bytes of output.
fn lh5(packed: &[u8], size: usize) -> Option<Vec<u8>> {
    const NC: usize = 510; // 256 literals + 254 match lengths (3 ..= 256), as LHA's UCHAR_MAX + MAXMATCH + 2 - THRESHOLD
    const NT: usize = 19;
    const NP: usize = 14;
    let mut b = Bits { d: packed, pos: 0 };
    let mut out: Vec<u8> = Vec::with_capacity(size);
    let mut block = 0u32;
    let (mut ct, mut pt): (Option<Huff>, Option<Huff>) = (None, None);
    while out.len() < size {
        if b.pos > packed.len() * 8 + 64 {
            return None;
        }
        if block == 0 {
            block = b.get(16);
            if block == 0 {
                return None;
            }
            let tt = read_pt(&mut b, NT, 5, 3)?;
            // the literal/length code lengths, themselves coded with `tt`
            let n = b.get(9) as usize;
            ct = Some(if n == 0 {
                Huff::constant(b.get(9) as u16)
            } else {
                let mut len = vec![0u8; NC];
                let mut i = 0;
                while i < n.min(NC) {
                    let c = b.decode(&tt)? as usize;
                    if c <= 2 {
                        let run = match c {
                            0 => 1,
                            1 => b.get(4) as usize + 3,
                            _ => b.get(9) as usize + 20,
                        };
                        i += run; // zero lengths
                    } else {
                        len[i] = (c - 2) as u8;
                        i += 1;
                    }
                }
                Huff::from_lengths(&len)?
            });
            pt = Some(read_pt(&mut b, NP, 4, -1)?);
        }
        block -= 1;
        let c = b.decode(ct.as_ref()?)? as usize;
        if c < 256 {
            out.push(c as u8);
        } else {
            let len = c - 256 + 3;
            let j = b.decode(pt.as_ref()?)? as u32;
            let dist = if j == 0 { 0 } else { (1u32 << (j - 1)) + b.get(j - 1) } as usize + 1;
            for _ in 0..len {
                // (before the start of the output the window holds spaces)
                let v = if dist > out.len() { b' ' } else { out[out.len() - dist] };
                out.push(v);
            }
        }
    }
    out.truncate(size);
    Some(out)
}

/// The packed file's content when `d` is a single-file LHA archive (`-lh5-`, header level 0, the
/// form these files come in), else `d` itself. Built in, so it needs no `archives` feature.
fn unpack(d: &[u8]) -> Option<Vec<u8>> {
    if d.len() > 22 && &d[2..7] == b"-lh5-" {
        if d[20] == 0 {
            let head = d[0] as usize + 2;
            let packed = u32::from_le_bytes([d[7], d[8], d[9], d[10]]) as usize;
            let size = u32::from_le_bytes([d[11], d[12], d[13], d[14]]) as usize;
            if size == 0 || size > 64 << 20 || head > d.len() {
                return None;
            }
            let end = (head + packed).min(d.len());
            return lh5(&d[head..end], size);
        }
        #[cfg(feature = "archives")]
        {
            use delharc::stub_io::Read as _;
            let mut rd = delharc::LhaDecodeReader::new(d).ok()?;
            let size = rd.header().original_size as usize;
            if size == 0 || size > 64 << 20 || !rd.is_decoder_supported() {
                return None;
            }
            let mut buf = vec![0u8; size];
            let mut got = 0;
            while got < size {
                match rd.read_all(&mut buf[got..]) {
                    Ok(0) | Err(_) => return None,
                    Ok(n) => got += n,
                }
            }
            return Some(buf);
        }
        #[cfg(not(feature = "archives"))]
        return None;
    }
    Some(d.to_vec())
}

fn magic_ok(d: &[u8]) -> bool {
    matches!(d.get(0..4), Some(b"YM2!") | Some(b"YM3!") | Some(b"YM3b") | Some(b"YM5!") | Some(b"YM6!"))
}

/// Whether `d` (plain or `-lh5-` packed) is a YM file this player takes.
pub fn detect(d: &[u8]) -> bool {
    if magic_ok(d) {
        return true;
    }
    // (the cheap check first: the packed header's method id is at offset 2)
    d.len() > 22 && &d[2..7] == b"-lh5-" && unpack(d).is_some_and(|u| magic_ok(&u))
}

fn cstr(d: &[u8], o: &mut usize) -> String {
    let s = d.get(*o..).unwrap_or(&[]);
    let n = s.iter().position(|&b| b == 0).unwrap_or(s.len());
    *o += (n + 1).min(s.len());
    // (the files are Atari ST text: Latin-1 stands in for it)
    s[..n].iter().map(|&b| b as char).collect()
}

pub fn parse(d: &[u8]) -> Result<YmFile, String> {
    let d = unpack(d).ok_or("YM: cannot unpack")?;
    let id = d.get(0..4).ok_or("YM: too short")?;
    let simple = matches!(id, b"YM2!" | b"YM3!" | b"YM3b");
    if simple {
        let version = if id == b"YM2!" { 2 } else { 3 };
        let n = (d.len() - 4) / 14;
        if n == 0 {
            return Err("YM: no frames".into());
        }
        let loop_frame = if id == b"YM3b" { be32(&d, d.len() - 4).map(|v| v.swap_bytes()).unwrap_or(0) } else { 0 };
        let mut frames = vec![[0u8; 16]; n];
        for (r, plane) in d[4..4 + 14 * n].chunks_exact(n).enumerate() {
            for (f, &v) in plane.iter().enumerate() {
                frames[f][r] = v;
            }
        }
        return Ok(YmFile {
            version,
            name: String::new(),
            author: String::new(),
            comment: String::new(),
            clock: 2_000_000,
            rate: 50,
            loop_frame,
            frames,
            drums: Vec::new(),
        });
    }
    // YM5! / YM6!
    if d.get(4..12) != Some(b"LeOnArD!") {
        return Err("YM: not a valid YM5/YM6 file".into());
    }
    let n = be32(&d, 12).ok_or("YM: short header")? as usize;
    let attr = be32(&d, 16).ok_or("YM: short header")?;
    let nd = be16(&d, 20).ok_or("YM: short header")? as usize;
    let clock = be32(&d, 22).ok_or("YM: short header")?;
    let rate = be16(&d, 26).ok_or("YM: short header")?;
    let loop_frame = be32(&d, 28).ok_or("YM: short header")?;
    let skip = be16(&d, 32).ok_or("YM: short header")? as usize;
    if n == 0 || n > 4_000_000 || rate == 0 || nd > 64 || clock == 0 {
        return Err("YM: implausible header".into());
    }
    let mut o = 34 + skip;
    let mut drums = Vec::new();
    for _ in 0..nd {
        let sz = be32(&d, o).ok_or("YM: short digidrum")? as usize;
        o += 4;
        let raw = d.get(o..o + sz).ok_or("YM: short digidrum")?;
        o += sz;
        // 4-bit drums are stored one level a byte; 8-bit ones are reduced to the 4-bit level
        // the chip's volume register takes
        drums.push(raw.iter().map(|&b| if attr & ATTR_DRUM_4BITS != 0 { b & 15 } else { b >> 4 }).collect());
    }
    let name = cstr(&d, &mut o);
    let author = cstr(&d, &mut o);
    let comment = cstr(&d, &mut o);
    // (some files are a few bytes short of what their header promises: the missing tail reads as
    // zeros, as it does in StSound, but a file missing much of it is refused)
    let stream = d.get(o..).unwrap_or(&[]);
    if stream.len() < 16 * n * 9 / 10 {
        return Err("YM: frame data cut short".into());
    }
    let at = |i: usize| stream.get(i).copied().unwrap_or(0);
    let mut frames = vec![[0u8; 16]; n];
    for f in 0..n {
        for r in 0..16 {
            frames[f][r] = if attr & ATTR_INTERLEAVED != 0 { at(r * n + f) } else { at(f * 16 + r) };
        }
    }
    Ok(YmFile {
        version: if id == b"YM6!" { 6 } else { 5 },
        name,
        author,
        comment,
        clock,
        rate,
        loop_frame,
        frames,
        drums,
    })
}

#[derive(Clone, Copy, Default)]
struct Sid {
    on: bool,
    step: u32,
    phase: u32,
    vol: u8,
}

#[derive(Clone, Copy, Default)]
struct Drum {
    on: bool,
    pos: f64,
    step: f64,
    idx: usize,
}

pub struct YmPlayer {
    f: YmFile,
    chip: Ym,
    rate: u32,
    frame: usize,
    /// Output samples left in the frame being played, as a fraction carried forward.
    left: f64,
    per_frame: f64,
    sid: [Sid; 3],
    drum: [Drum; 3],
    buzz_step: u32,
    buzz_phase: u32,
    buzz_shape: u8,
    /// The frame's mixer and volume registers, which the effects override per sample.
    r7: u8,
    vols: [u8; 3],
    done: bool,
}

impl YmPlayer {
    pub fn new(d: &[u8], rate: u32) -> Result<YmPlayer, String> {
        let f = parse(d)?;
        // the chip model runs at the ST's 2 MHz; another clock (CPC 1 MHz, ZX 1.77 MHz) is that
        // model with its host rate scaled
        let host = ((rate as u64 * 2_000_000) / f.clock.max(1) as u64).clamp(1, u32::MAX as u64) as u32;
        let per_frame = rate as f64 / f.rate as f64;
        Ok(YmPlayer {
            chip: Ym::new(host),
            rate,
            frame: 0,
            left: 0.0,
            per_frame,
            sid: [Sid::default(); 3],
            drum: [Drum::default(); 3],
            buzz_step: 0,
            buzz_phase: 0,
            buzz_shape: 0,
            r7: 0x3F,
            vols: [0; 3],
            done: false,
            f,
        })
    }

    fn effect(&mut self, regs: &[u8; 16], code: usize, prediv: usize, count: usize) {
        let c = regs[code] & 0xF0;
        let voice = ((c & 0x30) >> 4) as usize;
        if voice == 0 {
            return;
        }
        let voice = voice - 1;
        let div = MFP_PREDIV[((regs[prediv] >> 5) & 7) as usize] * regs[count] as u32;
        if div == 0 {
            return;
        }
        let freq = 2_457_600.0 / div as f64;
        match c & 0xC0 {
            0x00 => self.sid_start(voice, freq, regs[8 + voice] & 15),
            0x40 => {
                let n = (regs[8 + voice] & 31) as usize;
                if n < self.f.drums.len() && !self.f.drums[n].is_empty() {
                    let step = freq / self.rate as f64;
                    self.drum[voice] = Drum { on: true, pos: 0.0, step, idx: n };
                }
            }
            0xC0 => {
                self.buzz_step = (freq * (1u64 << 31) as f64 / self.rate as f64) as u32;
                self.buzz_phase = 0;
                self.buzz_shape = regs[8 + voice] & 15;
            }
            _ => {} // sinus-SID: not played (nor by StSound)
        }
    }

    fn sid_start(&mut self, voice: usize, freq: f64, vol: u8) {
        let step = (freq * (1u64 << 31) as f64 / self.rate as f64) as u32;
        self.sid[voice] = Sid { on: true, step, phase: self.sid[voice].phase, vol };
    }

    /// Starts frame `self.frame`: its registers into the chip, its effects started.
    fn start_frame(&mut self) {
        let regs = self.f.frames[self.frame];
        for r in 0..=10u8 {
            self.chip.write(r, regs[r as usize]);
        }
        self.r7 = regs[7];
        self.vols = [regs[8], regs[9], regs[10]];
        self.sid = [Sid { on: false, ..self.sid[0] }, Sid { on: false, ..self.sid[1] }, Sid { on: false, ..self.sid[2] }];
        self.buzz_step = 0;
        self.chip.write(11, regs[11]);
        self.chip.write(12, regs[12]);
        if regs[13] != 0xFF {
            self.chip.write(13, regs[13]);
        }
        match self.f.version {
            2 => {
                if regs[13] != 0xFF {
                    self.chip.write(12, 0);
                    self.chip.write(13, 10);
                }
            }
            6 => {
                self.effect(&regs, 1, 6, 14);
                self.effect(&regs, 3, 8, 15);
            }
            5 => {
                let c = ((regs[1] >> 4) & 3) as usize;
                if c != 0 {
                    let div = MFP_PREDIV[((regs[6] >> 5) & 7) as usize] * regs[14] as u32;
                    if div != 0 {
                        self.sid_start(c - 1, 2_457_600.0 / div as f64, regs[7 + c] & 15);
                    }
                }
                let c = ((regs[3] >> 4) & 3) as usize;
                if c != 0 {
                    let v = c - 1;
                    let n = (regs[8 + v] & 31) as usize;
                    let div = MFP_PREDIV[((regs[8] >> 5) & 7) as usize] * regs[15] as u32;
                    if div != 0 && n < self.f.drums.len() && !self.f.drums[n].is_empty() {
                        self.drum[v] = Drum { on: true, pos: 0.0, step: MFP_CLOCK / div as f64 / self.rate as f64, idx: n };
                    }
                }
            }
            _ => {}
        }
    }

    /// One output sample: the per-sample effects applied to the chip, then the chip's.
    fn sample(&mut self) -> f32 {
        let mut r7 = self.r7;
        for v in 0..3 {
            if self.sid[v].on {
                let s = &mut self.sid[v];
                self.chip.write(8 + v as u8, if s.phase & (1 << 31) != 0 { s.vol } else { 0 });
                s.phase = s.phase.wrapping_add(s.step);
            }
            if self.drum[v].on {
                let d = &mut self.drum[v];
                let data = &self.f.drums[d.idx];
                let i = d.pos as usize;
                if i >= data.len() {
                    d.on = false;
                    self.chip.write(8 + v as u8, self.vols[v]);
                } else {
                    // tone and noise off for the channel: its volume register alone sounds
                    r7 |= 1 << v | 1 << (3 + v);
                    self.chip.write(8 + v as u8, data[i]);
                    d.pos += d.step;
                }
            }
        }
        self.chip.write(7, r7);
        if self.buzz_step != 0 {
            self.buzz_phase = self.buzz_phase.wrapping_add(self.buzz_step);
            if self.buzz_phase & (1 << 31) != 0 {
                self.buzz_phase &= 0x7FFF_FFFF;
                self.chip.write(13, self.buzz_shape);
            }
        }
        self.chip.sample() as f32 / 32768.0
    }
}

impl crate::aiff::Stream for YmPlayer {
    fn render(&mut self, out: &mut [f32]) -> usize {
        for fr in out.chunks_exact_mut(2) {
            if self.done {
                fr[0] = 0.0;
                fr[1] = 0.0;
                continue;
            }
            if self.left < 1.0 {
                if self.frame >= self.f.frames.len() {
                    self.done = true;
                    fr[0] = 0.0;
                    fr[1] = 0.0;
                    continue;
                }
                self.start_frame();
                self.frame += 1;
                self.left += self.per_frame;
            }
            self.left -= 1.0;
            let s = self.sample();
            fr[0] = s;
            fr[1] = s;
        }
        out.len() / 2
    }

    fn ended(&self) -> bool {
        self.done
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, 0, 0)
    }

    fn set_subsong(&mut self, _n: u16) {}

    fn duration_secs(&self) -> Option<f64> {
        Some(self.f.frames.len() as f64 / self.f.rate as f64)
    }

    fn author(&self) -> String {
        self.f.author.clone()
    }
}

/// The recorded-audio player of the engine on a YM file.
pub fn player(d: &[u8], rate: u32) -> Result<crate::aiff::AiffPlayer, String> {
    let p = YmPlayer::new(d, rate)?;
    let title = p.f.name.clone();
    let format = match p.f.version {
        2 => "YM2",
        3 => "YM3",
        5 => "YM5",
        _ => "YM6",
    };
    Ok(crate::aiff::AiffPlayer::from_stream(Box::new(p), format, title, rate))
}

#[cfg(test)]
mod tests {
    use crate::aiff::Stream;

    /// A YM3 file of `n` frames holding one steady tone on channel A.
    fn tone_file(n: usize) -> Vec<u8> {
        let mut d = b"YM3!".to_vec();
        for r in 0..14 {
            let v = match r {
                0 => 100u8, // channel A period, low byte
                7 => 0x3E,  // tone A on, everything else off
                8 => 15,    // channel A volume
                _ => 0,
            };
            d.extend(std::iter::repeat(v).take(n));
        }
        d
    }

    #[test]
    fn plays_a_tone_and_ends() {
        let d = tone_file(50);
        assert!(super::detect(&d));
        let mut p = super::YmPlayer::new(&d, 44100).unwrap();
        assert_eq!(p.duration_secs(), Some(1.0));
        let mut buf = vec![0f32; 2 * 44100];
        p.render(&mut buf);
        let peak = buf.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.05, "peak {peak}");
        assert!(!p.ended() || buf.len() == 88200);
        p.render(&mut buf);
        assert!(p.ended());
    }

    #[test]
    fn refuses_what_is_not_a_ym_file() {
        assert!(!super::detect(b"YM9!junkjunkjunkjunk"));
        assert!(super::parse(b"YM5!LeOnArD!").is_err());
        assert!(super::YmPlayer::new(&[], 44100).is_err());
    }

    /// Every `NAME.lzh` in `$YM_TEST_DIR` must unpack to exactly the `NAME.raw` beside it
    /// (made with the `lha` tool). A file `lha` itself rejects has an empty `.raw` and must be
    /// refused or differ -- it is counted, not compared.
    #[test]
    #[ignore]
    fn lh5_matches_lha() {
        let dir = std::path::PathBuf::from(std::env::var("YM_TEST_DIR").unwrap());
        let (mut same, mut lha_failed, mut bad) = (0, 0, Vec::new());
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "lzh") {
                let packed = std::fs::read(&p).unwrap();
                let raw = std::fs::read(p.with_extension("raw")).unwrap();
                if raw.is_empty() {
                    lha_failed += 1;
                } else if super::unpack(&packed).as_deref() == Some(&raw[..]) {
                    same += 1;
                } else {
                    if bad.len() < 4 {
                        let got = super::unpack(&packed);
                        let at = got.as_ref().map(|g| g.iter().zip(&raw).position(|(a, b)| a != b));
                        eprintln!("{}: raw {} got {:?} first diff {:?}", p.display(), raw.len(), got.as_ref().map(|g| g.len()), at);
                    }
                    bad.push(p);
                }
            }
        }
        eprintln!("identical {same}, lha rejected {lha_failed}, DIFFERENT {}", bad.len());
        assert!(bad.is_empty(), "{} differ", bad.len());
    }
}
