//! Disorder Tracker 2 modules (`PLM\x1A`, `.plm`), read as libopenmpt 0.8.7's `Load_plm.cpp`
//! (`ReadPLM`) reads them and played as the Impulse Tracker module OpenMPT saves them as
//! (`GetBestSaveFormat`; OpenMPT plays them with IT's old effects, Amiga slides and sample mode;
//! compatible Gxx is set as well, see `read`).
//!
//! A PLM file is one long song: an order list of items (a pattern, the row it starts at in the
//! song, the channel it starts at), each pattern with its own row and channel count. OpenMPT
//! flattens it into ordinary 64-row patterns, an order of the module per 64 rows, writing every
//! item's cells where it lands (a later item overwrites an earlier one cell by cell, empty cells
//! included), with one more channel than the file's, which holds the pattern breaks of the
//! "jump to order" and "break to end of order" commands. The last pattern is cut at the song's
//! last row; orders no item reaches get a blank pattern. The samples (8 or 16-bit unsigned,
//! loop, ping-pong, pan, volume, C5 speed) are written as IT samples, the amplification as the
//! module's pre-amp.
//!
//! Left out, as IT cannot hold what OpenMPT's player does for PLM: the percentage sample offset
//! (`CMD_OFFSETPERCENTAGE`, a fraction of the sample's length) is written as an ordinary sample
//! offset, as OpenMPT's IT writer does; a tone portamento that follows a normal portamento's
//! direction, the fine (0xF0 + n) tone portamento slides done on the first tick only, the
//! portamento memory shared between the slides, the sample a tone portamento does not change,
//! an instrument number without a note retriggering the channel's note, the song's maximum
//! volume for volume slides, the pattern names and colours. Offset commands without a note do
//! get the channel's last note, as OpenMPT's loader gives them.
//!
//! Ported from libopenmpt 0.8.7 (BSD-3-Clause, OpenMPT Project Developers and Contributors; see
//! LICENSE-openmpt).

use super::*;
use crate::xmp::load::Hio;

/// Size of the file header (`PLMFileHeader`).
const HEADER: usize = 96;

/// `ValidateHeader` and `GetHeaderMinimumAdditionalSize`: whether `d` is a PLM file the loader
/// takes, returning (header size, channels, samples, patterns, orders).
fn header(d: &[u8]) -> Option<(usize, usize, usize, usize, usize)> {
    if d.len() < HEADER || &d[..4] != b"PLM\x1A" || d[5] != 0x10 || d[54] == 0 || d[54] > 32 || (d[4] as usize) < HEADER {
        return None;
    }
    let (hs, ch, ns, np, no) = (d[4] as usize, d[54] as usize, d[92] as usize, d[93] as usize, u16::from_le_bytes([d[94], d[95]]) as usize);
    // `ProbeAdditionalSize` / `CanRead`: what follows the 96 bytes read
    (d.len() - HEADER >= hs - HEADER + 4 * (no + np + ns)).then_some((hs, ch, ns, np, no))
}

/// `ProbeFileHeaderPLM`.
pub fn test(d: &[u8]) -> bool {
    header(d).is_some()
}

/// `mpt::String::ReadBuf(spacePadded, ..)`: trailing spaces and NULs dropped.
fn space_padded(b: &[u8]) -> Vec<u8> {
    let n = b.iter().rposition(|&c| c != b' ' && c != 0).map_or(0, |p| p + 1);
    b[..n].to_vec()
}

/// `effTrans`: OpenMPT's command for each PLM command.
fn eff_trans(command: u8) -> Option<Cmd> {
    use Cmd::*;
    const T: [Cmd; 23] = [
        None, PortaUp, PortaDown, TonePorta, VolumeSlide, Tremolo, Vibrato, S3mCmdEx, //
        S3mCmdEx, Tempo, Speed, PositionJump, PositionJump, Offset, S3mCmdEx, Retrig, //
        S3mCmdEx, S3mCmdEx, S3mCmdEx, FineVibrato, VibratoVol, TonePortaVol,
        // CMD_OFFSETPERCENTAGE: S3MSaveConvert writes it as 'O' like the sample offset
        Offset,
    ];
    T.get(command as usize).copied()
}

/// The module OpenMPT reads from `file`, or None when it does not load it.
pub fn read(file: &[u8]) -> Option<Module> {
    let (header_size, num_ch, num_smp, num_pat, num_ord) = header(file)?;
    let channels = num_ch + 1; // one more for the pattern breaks
    let (amplify, tempo, speed) = (file[57], file[58], file[59]);
    let chn_pan: Vec<u16> = (0..channels).map(|c| if c < num_ch { file[60 + c] as u16 * 0x11 } else { 128 }).collect();

    let mut f = Reader::new(file);
    f.seek(header_size);
    // (`header` checked that the lists are there)
    let order: Vec<(u16, u8, u8)> = (0..num_ord)
        .map(|_| {
            let b = f.struct_or_zero(4).0;
            (u16::from_le_bytes([b[0], b[1]]), b[2], b[3])
        })
        .collect();
    let pattern_pos: Vec<usize> = (0..num_pat).map(|_| f.u32() as usize).collect();
    let sample_pos: Vec<usize> = (0..num_smp).map(|_| f.u32() as usize).collect();

    let mut samples: Vec<Sample> = Vec::with_capacity(num_smp);
    for &pos in &sample_pos {
        let mut s = Sample::new();
        let mut f = Reader::new(file);
        if pos == 0 || pos > file.len() {
            samples.push(s);
            continue;
        }
        f.seek(pos);
        let Some(h) = f.bytes(71) else {
            samples.push(s);
            continue;
        };
        let u32at = |o: usize| u32::from_le_bytes([h[o], h[o + 1], h[o + 2], h[o + 3]]);
        // layout: 0 magic, 4 headerSize, 5 version, 6 name[32], 38 filename[12], 50 panning,
        // 51 volume, 52 flags, 53 sampleRate (u16), 55 unused[4], 59 loopStart, 63 loopEnd,
        // 67 length
        let (panning, volume, flags) = (h[50], h[51], h[52]);
        let bits16 = flags & 1 != 0;
        s.name = name(&h[6..38]);
        s.filename = name(&h[38..50]);
        if panning <= 15 {
            s.pan = Some(panning as u16 * 0x11);
        }
        s.global_vol = volume.min(64);
        s.c5speed = u16::from_le_bytes([h[53], h[54]]) as u32;
        let (mut ls, mut le, mut len) = (u32at(59), u32at(63), u32at(67));
        if bits16 {
            ls /= 2;
            le /= 2;
            len /= 2;
        }
        s.length = len;
        s.loop_start = ls;
        s.loop_end = le;
        if le > ls {
            s.looped = true;
            s.pingpong = flags & 2 != 0;
        }
        s.bits16 = bits16;
        f.seek(pos + h[4] as usize);
        s.data = read_pcm(&mut f, len, bits16, false, Enc::Unsigned);
        samples.push(s);
    }

    // PLM is one huge continuous pattern; it is split into patterns of 64 rows
    const ROWS: usize = 64;
    let mut max_pos: u32 = 0;
    let mut orders: Vec<Option<usize>> = Vec::new();
    let mut patterns: Vec<Option<Pattern>> = Vec::new();
    for &(x, y, pattern) in &order {
        let (y, pattern) = (y as usize, pattern as usize);
        if pattern >= num_pat || y > num_ch || pattern_pos[pattern] > file.len() {
            continue;
        }
        f.seek(pattern_pos[pattern]);
        let ph = f.struct_or_zero(32).0;
        let (pat_rows, pat_ch) = (ph[4] as usize, ph[5] as usize);
        if pat_rows == 0 {
            continue;
        }
        let mut cur_ord = x as usize / ROWS;
        let mut cur_row = x as usize % ROWS;
        let n_ch = pat_ch.min(num_ch - y);
        let pattern_end = x as u32 + pat_rows as u32;
        max_pos = max_pos.max(pattern_end);

        let mut last_note = [NOTE_NONE; 32];
        for _ in 0..pat_rows {
            if cur_row >= ROWS {
                cur_row = 0;
                cur_ord += 1;
            }
            if cur_ord >= orders.len() {
                orders.resize(cur_ord + 1, None);
                patterns.push(Some(Pattern::new(ROWS, channels)));
                orders[cur_ord] = Some(patterns.len() - 1);
            }
            let pat = orders[cur_ord]?;
            for c in 0..n_ch {
                let b = match f.bytes(5) {
                    Some(b) => [b[0], b[1], b[2], b[3], b[4]],
                    None => {
                        f.seek(file.len());
                        [0; 5]
                    }
                };
                let (note, instr, volume, command, param) = (b[0], b[1], b[2], b[3], b[4]);
                let at = cur_row * channels + y + c;
                let p = patterns[pat].as_mut()?;
                let m = &mut p.cells[at];
                if note > 0 && note < 0x90 {
                    let n = (note >> 4) as u32 * 12 + (note & 0x0F) as u32 + 12 + NOTE_MIN as u32;
                    // (a note above 119 is no note OpenMPT writes, nor any other loader here)
                    m.note = n as u8;
                    last_note[c] = m.note;
                } else {
                    m.note = NOTE_NONE;
                }
                m.instr = instr;
                m.volcmd = VolCmd::Volume;
                if volume != 0xFF {
                    m.vol = volume;
                } else {
                    m.volcmd = VolCmd::None;
                }
                let Some(cmd) = eff_trans(command) else { continue };
                m.cmd = cmd;
                m.param = param;
                match command {
                    0x07 => m.param = 0x40 | (m.param & 0x03),
                    0x08 => m.param = 0x30 | (m.param & 0x03),
                    0x0B => {
                        if (m.param as usize) < order.len() {
                            let target = order[m.param as usize].0;
                            m.param = (target as usize / ROWS) as u8;
                            let brk = &mut p.cells[cur_row * channels + channels - 1];
                            brk.cmd = Cmd::PatternBreak;
                            brk.param = (target as usize % ROWS) as u8;
                        }
                    }
                    0x0C => {
                        m.param = (pattern_end as usize / ROWS) as u8;
                        let brk = &mut p.cells[cur_row * channels + channels - 1];
                        brk.cmd = Cmd::PatternBreak;
                        brk.param = (pattern_end as usize % ROWS) as u8;
                    }
                    0x0E => m.param = 0x80 | (m.param & 0x0F),
                    0x10 => m.param = 0xD0 | m.param.min(0x0F),
                    0x11 => m.param = 0xC0 | m.param.min(0x0F),
                    0x12 => m.param = 0xE0 | m.param.min(0x0F),
                    0x04 | 0x14 | 0x15 => {
                        // both nibbles set: a fine volume slide up
                        if m.param & 0xF0 != 0 && m.param & 0x0F != 0 && m.param & 0xF0 != 0xF0 {
                            m.param |= 0x0F;
                        }
                    }
                    0x0D | 0x16 => {
                        // offset without a note
                        if m.note == NOTE_NONE {
                            m.note = last_note[c];
                        }
                    }
                    _ => {}
                }
            }
            if pat_ch > n_ch {
                f.skip(5 * (pat_ch - n_ch));
            }
            cur_row += 1;
        }
    }
    // the module ends with the last row of the last order item
    let end_rows = max_pos as usize % ROWS;
    let end_order = max_pos as usize / ROWS;
    if end_rows > 0 {
        if let Some(Some(p)) = orders.get(end_order).copied().flatten().map(|i| patterns.get_mut(i)) {
            if let Some(p) = p.as_mut() {
                p.rows = end_rows;
                p.cells.truncate(end_rows * channels);
            }
        }
    }
    // orders no item reached: one blank pattern
    let mut blank: Option<usize> = None;
    let orders: Vec<u16> = orders
        .iter()
        .map(|o| match o {
            Some(i) => *i as u16,
            None => *blank.get_or_insert_with(|| {
                patterns.push(Some(Pattern::new(ROWS, channels)));
                patterns.len() - 1
            }) as u16,
        })
        .collect();

    Some(Module {
        title: space_padded(&file[6..54]),
        channels,
        speed,
        tempo: tempo as f64,
        global_vol: 256,
        preamp: (amplify as u32).min(128),
        linear: false,
        // IT's old effects and compatible Gxx both on. OpenMPT sets only the old effects for PLM
        // (and plays it with its PLM-only rules), but written so (IT flags 0x31) openmpt123
        // renders the module as it renders the PLM file over the whole 375 s of the archive's
        // file: 38.5 dB under the signal (the pans IT's scale rounds; sample for sample with
        // them moved onto it), against 30.3 dB with the old effects alone (it differs from 262
        // s on, tone portamentos), 34.2 dB with compatible Gxx alone and 29.4 dB with neither
        old_effects: true,
        compat_gxx: true,
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
pub const NAME: &str = "Disorder Tracker 2 PLM";

/// The `xmp::LOADERS` entry: the module written as IT, loaded by the libxmp port's IT loader.
pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let file = whole_file(f);
    let md = read(&file).ok_or(())?;
    let (native, fix) = build(&md, Kind::It).ok_or(())?;
    load_native(m, &native, Kind::It, &fix, NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PLM file: 2 channels, one 2-row pattern (row 0: C-4 [byte 0x31], instrument 1, volume 32
    /// on channel 0; row 1: "break to end of order" on channel 0) placed twice, at row 0 on
    /// channel 0 and at row 70 on channel 1, one 16-byte unsigned 8-bit sample.
    fn synth_plm() -> Vec<u8> {
        let mut d = vec![0u8; 96];
        d[..4].copy_from_slice(b"PLM\x1A");
        d[4] = 96;
        d[5] = 0x10;
        d[6..10].copy_from_slice(b"tiny");
        (d[54], d[57], d[58], d[59]) = (2, 0x40, 125, 6);
        (d[92], d[93]) = (1, 1);
        d[94] = 2;
        for (x, y) in [(0u16, 0u8), (70, 1)] {
            d.extend_from_slice(&x.to_le_bytes());
            d.extend_from_slice(&[y, 0]);
        }
        d.extend_from_slice(&112u32.to_le_bytes()); // pattern
        d.extend_from_slice(&(112u32 + 32 + 20).to_le_bytes()); // sample
        assert_eq!(d.len(), 112);
        let mut p = vec![0u8; 32];
        p[0] = 52;
        (p[4], p[5]) = (2, 2);
        d.extend_from_slice(&p);
        d.extend_from_slice(&[0x31, 1, 32, 0, 0, 0, 0, 0xFF, 0, 0]);
        d.extend_from_slice(&[0, 0, 0xFF, 0x0C, 0, 0, 0, 0xFF, 0, 0]);
        let mut s = vec![0u8; 71];
        s[..4].copy_from_slice(b"PLS\x1A");
        s[4] = 71;
        (s[50], s[51], s[52]) = (255, 64, 0);
        s[53..55].copy_from_slice(&8363u16.to_le_bytes());
        s[67..71].copy_from_slice(&16u32.to_le_bytes());
        d.extend_from_slice(&s);
        d.extend((0..16).map(|k| (k * 16) as u8));
        d
    }

    #[test]
    fn flattens_the_order_list() {
        let f = synth_plm();
        assert!(test(&f));
        let md = read(&f).unwrap();
        assert_eq!((md.channels, md.orders.len(), md.patterns.len()), (3, 2, 2));
        let (p0, p1) = (md.patterns[md.orders[0] as usize].as_ref().unwrap(), md.patterns[md.orders[1] as usize].as_ref().unwrap());
        // the last pattern ends at the song's last row (72)
        assert_eq!((p0.rows, p1.rows), (64, 8));
        assert_eq!((p0.cells[0].note, p0.cells[0].instr, p0.cells[0].vol), (50, 1, 32));
        assert_eq!(p0.cells[3].cmd, Cmd::PositionJump); // row 1, channel 0
        assert_eq!(p0.cells[3 + 2].cmd, Cmd::PatternBreak); // the extra channel
        // the second item: row 6, channel 1 (only one of its two channels fits)
        assert_eq!(p1.cells[6 * 3 + 1].note, 50);
        assert_eq!((p1.cells[7 * 3 + 1].cmd, p1.cells[7 * 3 + 1].param, p1.cells[7 * 3 + 2].param), (Cmd::PositionJump, 1, 8));
        assert_eq!(md.samples[0].data.data[1], 16 - 128);
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&f)).unwrap();
        assert_eq!((m.module.chn, m.module.spd, m.module.bpm), (3, 6, 125));
        assert_eq!(m.module.type_, NAME);
    }

    /// `old_effects` / `compat_gxx` are IT's header flags 0x10 / 0x20 as written (`build`
    /// turns them into the MO3 header's inverted bits, which the writer inverts back).
    #[test]
    fn it_header_flags() {
        let mut md = read(&synth_plm()).unwrap();
        assert_eq!(build(&md, Kind::It).unwrap().0[0x2C] & 0x30, 0x30);
        for (oe, gxx, want) in [(false, false, 0x00), (true, false, 0x10), (false, true, 0x20), (true, true, 0x30)] {
            (md.old_effects, md.compat_gxx) = (oe, gxx);
            let it = build(&md, Kind::It).unwrap().0;
            assert_eq!(&it[..4], b"IMPM");
            assert_eq!(it[0x2C] & 0x30, want, "{oe} {gxx}");
        }
    }

    #[test]
    fn refuses_what_libopenmpt_refuses() {
        let f = synth_plm();
        for (at, v) in [(5, 0x11), (54, 0), (54, 33), (4, 95)] {
            let mut g = f.clone();
            g[at] = v;
            assert!(!test(&g), "{at}");
        }
        assert!(!test(&f[..100]));
    }

    /// Damaged files (cut short, bytes changed) load or are refused, never panic.
    #[test]
    fn damaged_files() {
        let f = synth_plm();
        for n in 0..f.len() {
            let _ = load(&mut ModuleData::default(), &mut Hio::new(&f[..n]));
        }
        for at in 0..f.len() {
            for v in [0x00, 0xFF, f[at].wrapping_add(1), 0x80] {
                let mut g = f.clone();
                g[at] = v;
                let _ = load(&mut ModuleData::default(), &mut Hio::new(&g));
            }
        }
    }

    /// Writes the module OpenMPT would save the PLM file `$PLM_IN` as to `$PLM_OUT` (for
    /// openmpt123 to compare with the original).
    #[test]
    #[ignore]
    fn write_converted() {
        let (Ok(i), Ok(o)) = (std::env::var("PLM_IN"), std::env::var("PLM_OUT")) else { return };
        let md = read(&std::fs::read(i).unwrap()).unwrap();
        std::fs::write(o, build(&md, Kind::It).unwrap().0).unwrap();
    }
}
