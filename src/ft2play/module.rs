//! The MOD loader of ft2play's `pmplay.c` (`loadMusicMOD`, `upDateInstrs`, `fixSample`,
//! `checkSampleRepeat`), including its memory file's end-of-file rule.

use super::tables::AMIGA_PERIOD;

/// The signatures of `MODSig` (the channel count is twice the one's index plus one).
const MOD_SIG: [&[u8; 4]; 16] = [
    b"2CHN", b"M.K.", b"6CHN", b"8CHN", b"10CH", b"12CH", b"14CH", b"16CH", b"18CH", b"20CH", b"22CH", b"24CH", b"26CH", b"28CH",
    b"30CH", b"32CH",
];

/// One cell of a pattern (`tonTyp`; the volume column of an XM is never set by a MOD).
#[derive(Clone, Copy, Default)]
pub(super) struct Cell {
    pub ton: u8,
    pub instr: u8,
    pub eff_typ: u8,
    pub eff: u8,
}

/// A sample (`sampleTyp`). `data` holds `len` bytes and the two taps ft2play's `malloc(len+2)`
/// leaves behind; it is empty for a sample with no data.
#[derive(Clone, Default)]
pub(super) struct Smp {
    pub len: i32,
    pub rep_s: i32,
    pub rep_l: i32,
    pub vol: u8,
    pub fine: i8,
    pub typ: u8,
    pub pan: u8,
    pub data: Vec<i8>,
}

/// A loaded module: ft2play's `song`, `patt`, `pattLens` and `instr` after `loadMusicMOD`.
pub(super) struct Module {
    pub name: String,
    pub chn: usize,
    pub len: u16,
    pub rep_s: u16,
    pub song_tab: [u8; 256],
    /// patterns 0..=highest used; `None` is an empty pattern (ft2play frees those)
    pub patt: Vec<Option<Vec<Cell>>>,
    /// instruments 0..=31; 0 is ft2play's placeholder, `None` an instrument with no sample
    pub instr: Vec<Option<Smp>>,
}

/// ft2play's `MEMFILE`: a read that takes the last byte sets the end-of-file flag, which a
/// later read then honours by reading nothing.
struct MemFile<'a> {
    d: &'a [u8],
    ptr: usize,
    eof: bool,
}

impl MemFile<'_> {
    fn read(&mut self, n: usize) -> &[u8] {
        if self.eof {
            return &[];
        }
        let take = n.min(self.d.len() - self.ptr);
        let r = &self.d[self.ptr..self.ptr + take];
        self.ptr += take;
        if self.ptr >= self.d.len() {
            self.eof = true;
        }
        r
    }

    fn seek(&mut self, at: usize) {
        self.ptr = at.min(self.d.len());
        self.eof = at >= self.d.len();
    }
}

/// The channel count `loadMusicMOD` takes from the signature (0: a 15-instrument module, which
/// ft2play loads as four channels but which the engine never gives it).
pub(super) fn channels_of(sig: &[u8]) -> usize {
    let mut j = 0;
    for (i, s) in MOD_SIG.iter().enumerate() {
        if sig == &s[..] {
            j = 2 * (i + 1);
        }
    }
    if sig == b"M!K!" || sig == b"FLT4" || sig == b"4CHN" {
        j = 4;
    }
    if sig == b"OCTA" {
        j = 8;
    }
    j
}

impl Module {
    /// `loadMusicFromData` for a MOD with a 31-instrument header; `None` where ft2play's loader
    /// fails (a file that ends within the header or exactly at the end of a pattern).
    pub fn load(d: &[u8]) -> Option<Module> {
        const HEADER: usize = 20 + 31 * 30 + 2 + 128 + 4;
        if d.len() <= 336 {
            // (the XM header read of loadMusicFromData runs into the end of the file)
            return None;
        }
        let mut f = MemFile { d, ptr: 0, eof: false };
        let ha = f.read(HEADER).to_vec();
        if f.eof {
            return None;
        }
        let name: String = ha[..20].iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
        let sig = &ha[1080..1084];
        let chn = channels_of(sig);
        if chn == 0 {
            return None;
        }
        f.seek(HEADER);
        if f.eof {
            return None;
        }
        let len = ha[950] as u16;
        let rep_s = ha[951] as u16;
        let mut song_tab = [0u8; 256];
        song_tab[..128].copy_from_slice(&ha[952..1080]);
        let b = song_tab[..128].iter().copied().max().unwrap_or(0) as usize;

        let mut patt: Vec<Option<Vec<Cell>>> = Vec::with_capacity(b + 1);
        for _ in 0..=b {
            let buf = f.read(chn * 4 * 64).to_vec();
            if f.eof {
                return None;
            }
            let mut cells = vec![Cell::default(); 64 * chn];
            for (cell, bytes) in cells.iter_mut().zip(buf.chunks_exact(4)) {
                let period = (((bytes[0] & 0x0F) as u16) << 8) | bytes[1] as u16;
                if let Some(k) = AMIGA_PERIOD.iter().position(|&p| period >= p) {
                    cell.ton = k as u8 + 1;
                }
                cell.instr = (bytes[0] & 0xF0) | (bytes[2] >> 4);
                cell.eff_typ = bytes[2] & 0x0F;
                cell.eff = bytes[3];
                match cell.eff_typ {
                    0xC => cell.eff = cell.eff.min(64),
                    0x1 | 0x2 | 0xA if cell.eff == 0 => cell.eff_typ = 0,
                    0x5 if cell.eff == 0 => cell.eff_typ = 3,
                    0x6 if cell.eff == 0 => cell.eff_typ = 4,
                    // (ft2play's E case compares the low nibble of effTyp, which is always 14: it never fires)
                    _ => {}
                }
            }
            // (`patternEmpty`: judged on the converted cells)
            let empty = cells.iter().all(|c| c.ton == 0 && c.instr == 0 && c.eff_typ == 0 && c.eff == 0);
            patt.push(if empty { None } else { Some(cells) });
        }

        let mut instr: Vec<Option<Smp>> = vec![None; 32];
        instr[0] = Some(Smp { vol: 0, pan: 128, ..Smp::default() });
        for a in 1..=31usize {
            let o = 20 + (a - 1) * 30;
            let w = |i: usize| ((ha[o + i] as u32) << 8) | ha[o + i + 1] as u32;
            let len = 2 * w(22) as i32;
            if len == 0 {
                continue;
            }
            let mut rep_s = 2 * w(26) as i32;
            let mut rep_l = 2 * w(28) as i32;
            if rep_l <= 2 {
                rep_s = 0;
                rep_l = 0;
            }
            if rep_s + rep_l > len {
                if rep_s >= len {
                    rep_s = 0;
                    rep_l = 0;
                } else {
                    rep_l = len - rep_s;
                }
            }
            let fine = ha[o + 24];
            let mut data = vec![0i8; len as usize + 2];
            // (a sample the file ends within keeps what is there; later ones read nothing)
            for (dst, &src) in data.iter_mut().zip(f.read(len as usize)) {
                *dst = src as i8;
            }
            instr[a] = Some(Smp {
                len,
                rep_s,
                rep_l,
                vol: ha[o + 25].min(64),
                fine: (8 * ((2 * ((fine & 15) ^ 8) as i32) - 16)) as i8,
                typ: (rep_l > 2) as u8,
                pan: 128,
                data,
            });
        }
        let rep_s = if rep_s > len { 0 } else { rep_s };

        let mut m = Module { name, chn, len, rep_s, song_tab, patt, instr };
        m.update_instrs();
        Some(m)
    }

    /// `upDateInstrs`: `checkSampleRepeat` and `fixSample` (the tap sample after the loop or the
    /// end, for the mixer's branchless interpolation) over every instrument.
    fn update_instrs(&mut self) {
        for s in self.instr.iter_mut().flatten() {
            if s.data.is_empty() {
                s.len = 0;
                s.rep_s = 0;
                s.rep_l = 0;
                continue;
            }
            s.rep_s = s.rep_s.clamp(0, s.len);
            if s.rep_s + s.rep_l > s.len {
                s.rep_l = s.len - s.rep_s;
            }
            if s.rep_l < 1 {
                s.typ &= !3;
            }
            let (len, loop_start, loop_end) = (s.len as usize, s.rep_s as usize, (s.rep_s + s.rep_l) as usize);
            if s.typ & 1 != 0 {
                s.data[loop_end] = s.data[loop_start];
            } else {
                s.data[len] = 0;
            }
        }
    }
}
