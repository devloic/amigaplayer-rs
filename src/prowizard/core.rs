//! The ProWizard converter's framework, ported from NostalgicPlayer's
//! ProWizardConverterWorkerBase and its 15/31-sample subclasses (Copyright (c)
//! Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! A format reads the packed module through `Stream`, which behaves as
//! NostalgicPlayer's ModuleStream does: reads past the end return what is there
//! (a short integer read gives 0), seeks are clamped to the data, and `eof` tells
//! whether the last read or seek went past the end. The driver (`run`) writes the
//! ProTracker module the format describes, in the order the C# converter does,
//! and gives up where it does: when a format's sample, position or pattern
//! generator stops at the end of the data. The C# generators yield one item at a
//! time and the driver checks `eof` after each, so a ported generator pushes an
//! item and returns `None` if `eof` is set right then (`push!`).

/// NostalgicPlayer's SampleInfo. A `None` name is written as zeros.
#[derive(Clone, Default, Debug)]
pub struct SampleInfo {
    pub name: Option<Vec<u8>>,
    pub length: u16,
    pub loop_start: u16,
    pub loop_length: u16,
    pub volume: u8,
    pub fine_tune: u8,
}

/// The packed module, read as NostalgicPlayer's ModuleStream reads it.
pub struct Stream<'a> {
    d: &'a [u8],
    pos: usize,
    pub eof: bool,
}

impl<'a> Stream<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Stream { d, pos: 0, eof: false }
    }
    pub fn len(&self) -> i64 {
        self.d.len() as i64
    }
    pub fn position(&self) -> i64 {
        self.pos as i64
    }
    pub fn data(&self) -> &'a [u8] {
        self.d
    }
    /// Seek from the start (SeekOrigin.Begin).
    pub fn seek(&mut self, off: i64) {
        let p = off.max(0);
        self.eof = p > self.len();
        self.pos = p.min(self.len()) as usize;
    }
    /// Seek from the current position (SeekOrigin.Current).
    pub fn seek_cur(&mut self, off: i64) {
        self.seek(self.pos as i64 + off);
    }
    /// Seek from the end (SeekOrigin.End).
    pub fn seek_end(&mut self, off: i64) {
        self.seek(self.len() + off);
    }
    /// Read into `buf`, leaving what is past the end of the data untouched.
    pub fn read_into(&mut self, buf: &mut [u8]) -> usize {
        let n = buf.len().min(self.d.len() - self.pos);
        buf[..n].copy_from_slice(&self.d[self.pos..self.pos + n]);
        self.pos += n;
        self.eof = n < buf.len();
        n
    }
    fn read_n<const N: usize>(&mut self) -> Option<[u8; N]> {
        let mut b = [0u8; N];
        (self.read_into(&mut b) == N).then_some(b)
    }
    pub fn read_u8(&mut self) -> u8 {
        self.read_n::<1>().map_or(0, |b| b[0])
    }
    pub fn read_i8(&mut self) -> i8 {
        self.read_u8() as i8
    }
    pub fn read_b_u16(&mut self) -> u16 {
        self.read_n().map_or(0, u16::from_be_bytes)
    }
    pub fn read_b_i16(&mut self) -> i16 {
        self.read_b_u16() as i16
    }
    pub fn read_b_u32(&mut self) -> u32 {
        self.read_n().map_or(0, u32::from_be_bytes)
    }
    pub fn read_b_i32(&mut self) -> i32 {
        self.read_b_u32() as i32
    }
    pub fn read_b_u64(&mut self) -> u64 {
        self.read_n().map_or(0, u64::from_be_bytes)
    }
    pub fn read_array_b_u16(&mut self, buf: &mut [u16]) {
        for v in buf {
            *v = self.read_b_u16();
        }
    }
    pub fn read_array_b_u32(&mut self, buf: &mut [u32]) {
        for v in buf {
            *v = self.read_b_u32();
        }
    }
    /// ReadMark: `n` bytes with trailing zeros removed, empty if fewer were there.
    pub fn read_mark_n(&mut self, n: usize) -> Vec<u8> {
        let mut b = vec![0u8; n];
        if self.read_into(&mut b) != n {
            return Vec::new();
        }
        while b.last() == Some(&0) {
            b.pop();
        }
        b
    }
    pub fn read_mark(&mut self) -> Vec<u8> {
        self.read_mark_n(4)
    }
}

/// The converted module being written (NostalgicPlayer's ConverterStream). The
/// sample data markers copy the data instead of pointing at it.
#[derive(Default)]
pub struct Out {
    pub buf: Vec<u8>,
}

impl Out {
    pub fn position(&self) -> i64 {
        self.buf.len() as i64
    }
    pub fn write(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }
    pub fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    pub fn write_b_u16(&mut self, v: u16) {
        self.write(&v.to_be_bytes());
    }
    pub fn write_b_u32(&mut self, v: u32) {
        self.write(&v.to_be_bytes());
    }
    /// SetSampleDataMarker: `length` bytes of sample data from the stream's
    /// position (zeros for what is missing at the end), skipping them.
    pub fn set_sample_data_marker(&mut self, s: &mut Stream, length: i64) {
        copy_data(s, self, length);
    }
}

/// StreamHelper.CopyData, padding what is missing with zeros.
pub fn copy_data(s: &mut Stream, w: &mut Out, length: i64) {
    let length = length.max(0) as usize;
    let start = s.pos;
    let n = length.min(s.d.len() - start);
    w.buf.extend_from_slice(&s.d[start..start + n]);
    w.buf.resize(w.buf.len() + length - n, 0);
    s.seek_cur(length as i64);
}

/// Companion files a format may load (Magnetic Fields Packer's samples).
pub trait Loader {
    /// GetPossibleFileNames: the module's name with a new extension, the extension
    /// appended, and the extension as a prefix in place of the module's own.
    fn possible_names(&self, ext: &str) -> Vec<String>;
    fn open_by_name(&self, name: &str) -> Option<Vec<u8>>;
    fn open_by_extension(&self, ext: &str) -> Option<Vec<u8>> {
        self.possible_names(ext).iter().find_map(|n| self.open_by_name(n))
    }
}

/// No companion files.
pub struct NoFiles;

impl Loader for NoFiles {
    fn possible_names(&self, _: &str) -> Vec<String> {
        Vec::new()
    }
    fn open_by_name(&self, _: &str) -> Option<Vec<u8>> {
        None
    }
}

/// Companion files next to the module on disk.
pub struct FsLoader {
    pub path: std::path::PathBuf,
}

impl Loader for FsLoader {
    fn possible_names(&self, ext: &str) -> Vec<String> {
        let p = &self.path;
        let full = p.to_string_lossy().to_string();
        let mut v = vec![p.with_extension(ext).to_string_lossy().to_string(), format!("{full}.{ext}")];
        let dir = p.parent().unwrap_or(std::path::Path::new(""));
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if let Some(i) = name.find('.') {
            let pre = dir.join(format!("{ext}.{}", &name[i + 1..]));
            v.push(pre.to_string_lossy().to_string());
            // Path.ChangeExtension(name, "") without its trailing dot
            v.push(pre.with_extension("").to_string_lossy().to_string());
        }
        v
    }
    fn open_by_name(&self, name: &str) -> Option<Vec<u8>> {
        std::fs::read(name).ok()
    }
}

/// The state ProWizardConverterWorkerBase keeps for its formats.
#[derive(Default)]
pub struct Base {
    /// Sample lengths in bytes, as the driver wrote them (31 or 15 of them).
    pub sample_lengths: Vec<u32>,
    pub number_of_patterns: u16,
}

pub const MAX_MISSING: i64 = 256;

impl Base {
    /// FindHighestPatternNumber over a position list.
    pub fn find_highest_pattern(&mut self, positions: &[u8]) -> u16 {
        self.number_of_patterns = positions.iter().copied().max().unwrap_or(0) as u16 + 1;
        self.number_of_patterns
    }
    /// FindHighestPatternNumber over `len` positions read from the stream.
    pub fn find_highest_pattern_at(&mut self, s: &mut Stream, offset: i64, len: usize) -> u16 {
        s.seek(offset);
        self.find_highest_pattern_read(s, len)
    }
    pub fn find_highest_pattern_read(&mut self, s: &mut Stream, len: usize) -> u16 {
        let mut b = vec![0u8; len];
        s.read_into(&mut b);
        self.find_highest_pattern(&b)
    }
    /// BuildPositionList: numbers the distinct keys (track combinations) in order of
    /// first use; returns the position list and the keys.
    pub fn build_position_list<T: PartialEq>(&mut self, n: usize, mut key: impl FnMut(usize) -> T) -> (Vec<u8>, Vec<T>) {
        let mut list = vec![0u8; n];
        let mut used: Vec<T> = Vec::new();
        self.number_of_patterns = 0;
        for (i, l) in list.iter_mut().enumerate() {
            let k = key(i);
            match used.iter().position(|u| *u == k) {
                Some(j) => *l = j as u8,
                None => {
                    *l = self.number_of_patterns as u8;
                    self.number_of_patterns = self.number_of_patterns.wrapping_add(1);
                    used.push(k);
                }
            }
        }
        (list, used)
    }
    /// SaveMarksForAllSamples: the sample data follows at the stream's position.
    pub fn save_marks_for_all_samples(&self, s: &mut Stream, w: &mut Out) -> bool {
        for &l in &self.sample_lengths {
            let l = l as i64;
            if l != 0 {
                if s.len() - s.position() < l - MAX_MISSING {
                    return false;
                }
                w.set_sample_data_marker(s, l);
            }
        }
        true
    }
}

/// One packed format: ProWizardConverterWorkerBase's overrides.
pub trait Format {
    fn name(&self) -> &'static str;
    fn base(&mut self) -> &mut Base;
    /// A 15-sample (SoundTracker) result instead of a 31-sample one.
    fn fifteen(&self) -> bool {
        false
    }
    fn check(&mut self, s: &mut Stream) -> bool;
    fn prepare(&mut self, _s: &mut Stream) -> bool {
        true
    }
    fn module_name(&mut self, _s: &mut Stream) -> Option<Vec<u8>> {
        None
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>>;
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>>;
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        if self.fifteen() { 0 } else { 0x7f }
    }
    fn mark(&self) -> &'static [u8; 4] {
        b"M.K."
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>>;
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, files: &dyn Loader) -> bool;
}

/// Push a generator's item; stop the generator if the stream is at its end.
#[macro_export]
macro_rules! pw_push {
    ($v:expr, $x:expr, $s:expr) => {{
        $v.push($x);
        if $s.eof {
            return None;
        }
    }};
}
pub use pw_push as push;

/// Identify and convert: the module, or None when the format does not recognise it
/// or its conversion fails.
pub fn run(f: &mut dyn Format, d: &[u8], files: &dyn Loader) -> Option<Vec<u8>> {
    let mut s = Stream::new(d);
    if !f.check(&mut s) {
        return None;
    }
    convert(f, &mut s, files)
}

/// The ProWizardConverterWorker31SamplesBase/15SamplesBase Convert(), on the
/// stream the format's check recognised. NostalgicPlayer's loader seeks back to
/// the start between the two, and so does this.
pub fn convert(f: &mut dyn Format, s: &mut Stream, files: &dyn Loader) -> Option<Vec<u8>> {
    s.seek(0);
    let nsamp = if f.fifteen() { 15 } else { 31 };
    let mut w = Out::default();
    f.base().sample_lengths = vec![0; nsamp];
    if !f.prepare(s) {
        return None;
    }
    let mut name = f.module_name(s).unwrap_or_default();
    name.resize(20, 0);
    w.write(&name[..20]);
    // (the generators check the end of the data after each item, see `push!`)
    let samples = f.samples(s)?;
    if samples.is_empty() || samples.len() > nsamp {
        return None;
    }
    for (i, si) in samples.iter().enumerate() {
        let mut n = si.name.clone().unwrap_or_default();
        n.resize(22, 0);
        w.write(&n[..22]);
        w.write_b_u16(si.length);
        if nsamp == 31 {
            w.write_u8(si.fine_tune);
            w.write_u8(si.volume);
        } else {
            w.write_b_u16(si.volume as u16);
        }
        w.write_b_u16(si.loop_start);
        w.write_b_u16(si.loop_length);
        f.base().sample_lengths[i] = si.length as u32 * 2;
    }
    for _ in samples.len()..nsamp {
        w.write(&[0; 28]);
        w.write_b_u16(1);
    }
    let positions = f.position_list(s)?;
    if positions.is_empty() || s.eof || positions.len() > 128 {
        return None;
    }
    w.write_u8(positions.len() as u8);
    let r = f.restart(s);
    w.write_u8(r);
    w.write(&positions);
    w.write(&vec![0; 128 - positions.len()]);
    if nsamp == 31 {
        w.write(f.mark());
    }
    for p in f.patterns(s)? {
        w.write(&p);
    }
    if !f.write_sample_data(s, &mut w, files) {
        return None;
    }
    Some(w.buf)
}

/// ProWizardConverterWorkerBase's period table (as hi/lo bytes): C-1 to B-3, then
/// the last period repeated.
pub const PERIODS: [[u8; 2]; 45] = [
    [0x3, 0x58], [0x3, 0x28], [0x2, 0xfa], [0x2, 0xd0], [0x2, 0xa6], [0x2, 0x80], [0x2, 0x5c], [0x2, 0x3a], [0x2, 0x1a],
    [0x1, 0xfc], [0x1, 0xe0], [0x1, 0xc5], [0x1, 0xac], [0x1, 0x94], [0x1, 0x7d], [0x1, 0x68], [0x1, 0x53], [0x1, 0x40],
    [0x1, 0x2e], [0x1, 0x1d], [0x1, 0x0d], [0x0, 0xfe], [0x0, 0xf0], [0x0, 0xe2], [0x0, 0xd6], [0x0, 0xca], [0x0, 0xbe],
    [0x0, 0xb4], [0x0, 0xaa], [0x0, 0xa0], [0x0, 0x97], [0x0, 0x8f], [0x0, 0x87], [0x0, 0x7f], [0x0, 0x78], [0x0, 0x71],
    [0x0, 0x71], [0x0, 0x71], [0x0, 0x71], [0x0, 0x71], [0x0, 0x71], [0x0, 0x71], [0x0, 0x71], [0x0, 0x71], [0x0, 0x71],
];

/// ProWizardConverterWorker31SamplesBase's periods for each fine tune.
pub const TUNING_PERIODS: [[i16; 36]; 16] = [
    [856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113],
    [850, 802, 757, 715, 674, 637, 601, 567, 535, 505, 477, 450, 425, 401, 379, 357, 337, 318, 300, 284, 268, 253, 239, 225, 213, 201, 189, 179, 169, 159, 150, 142, 134, 126, 119, 113],
    [844, 796, 752, 709, 670, 632, 597, 563, 532, 502, 474, 447, 422, 398, 376, 355, 335, 316, 298, 282, 266, 251, 237, 224, 211, 199, 188, 177, 167, 158, 149, 141, 133, 125, 118, 112],
    [838, 791, 746, 704, 665, 628, 592, 559, 528, 498, 470, 444, 419, 395, 373, 352, 332, 314, 296, 280, 264, 249, 235, 222, 209, 198, 187, 176, 166, 157, 148, 140, 132, 125, 118, 111],
    [832, 785, 741, 699, 660, 623, 588, 555, 524, 495, 467, 441, 416, 392, 370, 350, 330, 312, 294, 278, 262, 247, 233, 220, 208, 196, 185, 175, 165, 156, 147, 139, 131, 124, 117, 110],
    [826, 779, 736, 694, 655, 619, 584, 551, 520, 491, 463, 437, 413, 390, 368, 347, 328, 309, 292, 276, 260, 245, 232, 219, 206, 195, 184, 174, 164, 155, 146, 138, 130, 123, 116, 109],
    [820, 774, 730, 689, 651, 614, 580, 547, 516, 487, 460, 434, 410, 387, 365, 345, 325, 307, 290, 274, 258, 244, 230, 217, 205, 193, 183, 172, 163, 154, 145, 137, 129, 122, 115, 109],
    [814, 768, 725, 684, 646, 610, 575, 543, 513, 484, 457, 431, 407, 384, 363, 342, 323, 305, 288, 272, 256, 242, 228, 216, 204, 192, 181, 171, 161, 152, 144, 136, 128, 121, 114, 108],
    [907, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120],
    [900, 850, 802, 757, 715, 675, 636, 601, 567, 535, 505, 477, 450, 425, 401, 379, 357, 337, 318, 300, 284, 268, 253, 238, 225, 212, 200, 189, 179, 169, 159, 150, 142, 134, 126, 119],
    [894, 844, 796, 752, 709, 670, 632, 597, 563, 532, 502, 474, 447, 422, 398, 376, 355, 335, 316, 298, 282, 266, 251, 237, 223, 211, 199, 188, 177, 167, 158, 149, 141, 133, 125, 118],
    [887, 838, 791, 746, 704, 665, 628, 592, 559, 528, 498, 470, 444, 419, 395, 373, 352, 332, 314, 296, 280, 264, 249, 235, 222, 209, 198, 187, 176, 166, 157, 148, 140, 132, 125, 118],
    [881, 832, 785, 741, 699, 660, 623, 588, 555, 524, 494, 467, 441, 416, 392, 370, 350, 330, 312, 294, 278, 262, 247, 233, 220, 208, 196, 185, 175, 165, 156, 147, 139, 131, 123, 117],
    [875, 826, 779, 736, 694, 655, 619, 584, 551, 520, 491, 463, 437, 413, 390, 368, 347, 328, 309, 292, 276, 260, 245, 232, 219, 206, 195, 184, 174, 164, 155, 146, 138, 130, 123, 116],
    [868, 820, 774, 730, 689, 651, 614, 580, 547, 516, 487, 460, 434, 410, 387, 365, 345, 325, 307, 290, 274, 258, 244, 230, 217, 205, 193, 183, 172, 163, 154, 145, 137, 129, 122, 115],
    [862, 814, 768, 725, 684, 646, 610, 575, 543, 513, 484, 457, 431, 407, 384, 363, 342, 323, 305, 288, 272, 256, 242, 228, 216, 203, 192, 181, 171, 161, 152, 144, 136, 128, 121, 114],
];

/// ProWizardForPcBase.TestSample.
pub fn test_sample(size: u16, loop_start: u16, loop_size: u16, volume: u8, fine_tune: u8) -> bool {
    let (size, ls, lz) = (size as u32, loop_start as u32, loop_size as u32);
    !(ls > size
        || lz > size + 2
        || ls + lz > size + 2
        || (ls != 0 && lz < 2)
        || ((ls != 0 || lz > 2) && size == 0)
        || volume > 0x40
        || fine_tune > 0x0f)
}

/// CryptoburnersFormatBase.Check: the common checks of the Cryptoburners packers
/// (ProPacker 2.1/3.0, ...). Returns the total sample size.
pub fn cryptoburners_check(s: &mut Stream, module_start: i64, check_start: i64) -> Option<u32> {
    s.seek(check_start);
    let t = s.read_u8();
    if t == 0 || t > 127 {
        return None;
    }
    if s.read_u8() > 0x7f {
        return None;
    }
    if s.read_u8() > 63 || s.read_u8() > 63 {
        return None;
    }
    s.seek(check_start + 0x17a);
    let m = s.read_mark();
    if [&b"M.K."[..], b"M&K!", b"FLT4", b"EXO4", b"UNIC", b"SNT."].contains(&&m[..]) {
        return None;
    }
    s.seek(module_start);
    let mut size = 0u32;
    for _ in 0..31 {
        let len = s.read_b_u16();
        size = size.wrapping_add(len as u32 * 2);
        if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
            return None;
        }
        if len != 0 {
            let a = s.read_b_u16() as u32;
            let b = s.read_b_u16() as u32;
            if a + b > len as u32 {
                return None;
            }
        } else {
            s.seek_cur(4);
        }
    }
    Some(size)
}

/// Big-endian reads from a byte slice; zero past its end.
pub fn be16(d: &[u8], o: usize) -> u16 {
    match d.get(o..o + 2) {
        Some(b) => u16::from_be_bytes([b[0], b[1]]),
        None => 0,
    }
}
