//! LZX: archives built here (a small crunching encoder with fixed, complete Huffman codes: it
//! has no module data in it), damaged copies of them, and the real archives of the author's
//! corpus when they are on this machine (set `LZX_CORPUS` to a directory of `.lzx` files; the
//! default is the Wanted Team sources kept in the scratch area; those tests pass trivially
//! without).

use super::*;
use crate::archive::{extract as extract_archive, format, Archive, Format, Limits};

// ---- an encoder ---------------------------------------------------------------------------

/// Bits written the way the decoder reads them: 16-bit big-endian words, low bit first.
struct BitWriter {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter { out: Vec::new(), acc: 0, n: 0 }
    }
    /// `v`'s low `k` bits, its bit 0 first.
    fn put(&mut self, v: u32, k: u32) {
        for i in 0..k {
            self.acc |= ((v >> i) & 1) << self.n;
            self.n += 1;
            if self.n == 16 {
                self.out.extend_from_slice(&(self.acc as u16).to_be_bytes());
                self.acc = 0;
                self.n = 0;
            }
        }
    }
    /// A canonical code of `len` bits, its top bit first.
    fn code(&mut self, code: u32, len: u32) {
        for i in (0..len).rev() {
            self.put((code >> i) & 1, 1);
        }
    }
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.put(0, 16 - self.n);
        }
        self.out
    }
}

/// The literal code: 256 literals of 9 bits (codes 0..256), 512 match symbols of 10 bits
/// (codes 512..1024); complete.
fn lit_code(sym: usize) -> (u32, u32) {
    if sym < 256 { (sym as u32, 9) } else { (512 + (sym - 256) as u32, 10) }
}

/// The code of the length-delta tree, whose lengths are: symbol 0 one bit, 7 two bits, 8 and
/// 19 three bits (complete).
fn tree_code(sym: usize) -> (u32, u32) {
    match sym {
        0 => (0, 1),
        7 => (2, 2),
        8 => (6, 3),
        19 => (7, 3),
        _ => unreachable!("symbol {sym}"),
    }
}

/// The encoder's options.
#[derive(Clone, Copy, PartialEq)]
struct Opt {
    aligned: bool,
    /// Bytes per block at most.
    block: usize,
    /// Send equal code lengths in runs, with symbol 19.
    runs: bool,
}

struct Encoder {
    w: BitWriter,
    lens: [u8; 768],
    last_distance: usize,
    opt: Opt,
}

fn slot_of(v: usize, slots: usize) -> usize {
    (0..slots).rev().find(|&s| slot_base(s) <= v && v - slot_base(s) < 1 << slot_bits(s)).unwrap()
}

impl Encoder {
    fn block_header(&mut self, len: usize) {
        let w = &mut self.w;
        w.put(if self.opt.aligned { 3 } else { 2 }, 3);
        if self.opt.aligned {
            for _ in 0..8 {
                w.put(3, 3);
            }
        }
        w.put((len >> 16) as u32 & 255, 8);
        w.put((len >> 8) as u32 & 255, 8);
        w.put(len as u32 & 255, 8);
        let mut pos = 0;
        for (fix, end, new) in [(1usize, 256usize, 9u8), (0, 768, 10)] {
            let mut tl = [0u8; 20];
            tl[0] = 1;
            tl[7] = 2;
            tl[8] = 3;
            tl[19] = 3;
            for l in tl {
                w.put(l as u32, 4);
            }
            while pos < end {
                let delta = (self.lens[pos] as usize + 17 - new as usize) % 17;
                let (c, l) = tree_code(delta);
                let mut run = 1;
                if self.opt.runs && end - pos >= 4 + fix {
                    // symbol 19: a run of 3 + fix + the next bit, then the delta of its first
                    let (c19, l19) = tree_code(19);
                    w.code(c19, l19);
                    w.put(1, 1);
                    run = 4 + fix;
                }
                w.code(c, l);
                self.lens[pos..pos + run].fill(new);
                pos += run;
            }
        }
    }

    fn literal(&mut self, b: u8) {
        let (c, l) = lit_code(b as usize);
        self.w.code(c, l);
    }

    fn matched(&mut self, distance: usize, len: usize) {
        let lslot = slot_of(len - 3, 16);
        let (dslot, extra_val) = if distance == self.last_distance {
            (0, 0)
        } else {
            let s = slot_of(distance, 32);
            (s, distance - slot_base(s))
        };
        let (c, l) = lit_code(256 + (lslot << 5 | dslot));
        self.w.code(c, l);
        let bits = slot_bits(dslot);
        if self.opt.aligned && bits >= 3 {
            self.w.put((extra_val >> 3) as u32, bits - 3);
            self.w.code((extra_val & 7) as u32, 3);
        } else {
            self.w.put(extra_val as u32, bits);
        }
        self.w.put((len - 3 - slot_base(lslot)) as u32, slot_bits(lslot));
        self.last_distance = distance;
    }
}

/// `data` as an LZX stream: greedy matches found through a table of the last place of each
/// three-byte string, blocks of at most `opt.block` bytes (a match may reach back into an
/// earlier block).
fn crunch(data: &[u8], opt: Opt) -> Vec<u8> {
    let mut e = Encoder { w: BitWriter::new(), lens: [0; 768], last_distance: 1, opt };
    let mut table = vec![usize::MAX; 1 << 16];
    let key = |i: usize| (data[i] as usize * 31 + data[i + 1] as usize * 7 + data[i + 2] as usize) & 0xFFFF;
    let mut pos = 0;
    while pos < data.len() {
        let end = (pos + opt.block).min(data.len());
        e.block_header(end - pos);
        while pos < end {
            let mut best = (0usize, 0usize);
            if pos + 3 <= data.len() {
                let cand = table[key(pos)];
                if cand != usize::MAX && pos - cand < 65536 {
                    let mut l = 0;
                    while l < 258 && pos + l < end && data[cand + l] == data[pos + l] {
                        l += 1;
                    }
                    if l >= 3 {
                        best = (pos - cand, l);
                    }
                }
            }
            let step = if best.1 >= 3 {
                e.matched(best.0, best.1);
                best.1
            } else {
                e.literal(data[pos]);
                1
            };
            for i in pos..pos + step {
                if i + 3 <= data.len() {
                    table[key(i)] = i;
                }
            }
            pos += step;
        }
    }
    e.w.finish()
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Stored,
    Crunched(Opt),
}

/// One file header (31 bytes, name, comment).
fn header(name: &str, comment: &str, size: usize, packed: usize, mode: u8, merged: bool, data_crc: u32) -> Vec<u8> {
    let mut h = vec![0u8; 31];
    h[0] = 0x0f;
    h[2..6].copy_from_slice(&(size as u32).to_le_bytes());
    h[6..10].copy_from_slice(&(packed as u32).to_le_bytes());
    h[10] = 10;
    h[11] = mode;
    h[12] = merged as u8;
    h[14] = comment.len() as u8;
    h[15] = 10;
    // 2026-11-12 13:14:15: day 12, month 10 (0 = January), year 56, hour, minute, second
    let date: u32 = 12 << 27 | 10 << 23 | 56 << 17 | 13 << 12 | 14 << 6 | 15;
    h[18..22].copy_from_slice(&date.to_be_bytes());
    h[22..26].copy_from_slice(&data_crc.to_le_bytes());
    h[30] = name.len() as u8;
    let mut all = h;
    all.extend_from_slice(name.as_bytes());
    all.extend_from_slice(comment.as_bytes());
    let sum = crc32(0, &all);
    all[26..30].copy_from_slice(&sum.to_le_bytes());
    all
}

/// An archive of groups: each group is a list of files that are one stream (merged when it
/// has several); `mode` is how the stream is kept.
fn archive(groups: &[(&[(&str, &[u8])], Mode)]) -> Vec<u8> {
    let mut a = b"LZX\0\x0c\0\x0a\x04\0\0".to_vec();
    for (files, mode) in groups {
        let all: Vec<u8> = files.iter().flat_map(|(_, d)| d.iter().copied()).collect();
        let packed = match mode {
            Mode::Stored => all,
            Mode::Crunched(opt) => crunch(&all, *opt),
        };
        for (i, (name, d)) in files.iter().enumerate() {
            let last = i + 1 == files.len();
            a.extend(header(
                name,
                if i == 0 { "a comment" } else { "" },
                d.len(),
                if last { packed.len() } else { 0 },
                if *mode == Mode::Stored { 0 } else { 2 },
                files.len() > 1,
                crc32(0, d),
            ));
        }
        a.extend_from_slice(&packed);
    }
    a
}

// ---- data ---------------------------------------------------------------------------------

fn rng(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

/// Bytes that compress in many ways: text-like repeats, runs, noise, and copies from far back.
fn sample(len: usize, seed: u64) -> Vec<u8> {
    let mut s = seed | 1;
    let mut v: Vec<u8> = Vec::new();
    while v.len() < len {
        match rng(&mut s) % 5 {
            0 => v.extend(b"the quick brown fox jumps over the lazy dog ".iter().cycle().take(20 + (rng(&mut s) % 300) as usize)),
            1 => v.extend(std::iter::repeat((rng(&mut s) % 256) as u8).take(1 + (rng(&mut s) % 500) as usize)),
            2 => v.extend((0..1 + rng(&mut s) % 400).map(|_| (rng(&mut s) >> 20) as u8)),
            3 if v.len() > 10 => {
                let from = (rng(&mut s) % v.len() as u64) as usize;
                let n = (3 + rng(&mut s) % 330) as usize;
                for i in 0..n {
                    v.push(v[from + i % (v.len() - from)]);
                }
            }
            _ => v.extend((0..50).map(|i| (i * 3 + (rng(&mut s) & 3) as usize) as u8)),
        }
    }
    v.truncate(len);
    v
}

const OPTS: [Opt; 6] = [
    Opt { aligned: false, block: 1 << 24, runs: false },
    Opt { aligned: true, block: 1 << 24, runs: false },
    Opt { aligned: false, block: 3000, runs: true },
    Opt { aligned: true, block: 777, runs: true },
    Opt { aligned: true, block: 40_000, runs: false },
    Opt { aligned: false, block: 65_536, runs: true },
];

fn names(a: &Archive) -> Vec<&str> {
    a.entries.iter().map(|e| e.name.as_str()).collect()
}

fn unlimited() -> Limits {
    Limits { max_entry: 1 << 28, max_total: 1 << 29, max_entries: 1000 }
}

// ---- tests --------------------------------------------------------------------------------

#[test]
fn slots_follow_the_rule() {
    let bases: Vec<usize> = (0..32).map(slot_base).collect();
    assert_eq!(&bases[..12], &[0, 1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48]);
    assert_eq!(&bases[28..], &[16384, 24576, 32768, 49152]);
    assert_eq!((0..32).map(slot_bits).collect::<Vec<_>>(), [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 14]);
    // the slots tile the numbers with no gap
    for s in 1..31 {
        assert_eq!(slot_base(s) + (1 << slot_bits(s)), slot_base(s + 1), "slot {s}");
    }
}

#[test]
fn crc32_is_the_usual_one() {
    assert_eq!(crc32(0, b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(crc32(0, b"1234"), b"56789"), 0xCBF4_3926);
}

#[test]
fn round_trip_crunched_and_stored() {
    let data = [sample(200, 1), sample(5000, 2), sample(70_000, 3), sample(150_000, 4), Vec::new(), vec![7], b"ab".to_vec()];
    for (k, d) in data.iter().enumerate() {
        for mode in OPTS.iter().map(|&o| Mode::Crunched(o)).chain([Mode::Stored]) {
            let a = archive(&[(&[("dir/file.bin", d.as_slice())], mode)]);
            assert_eq!(format(&a), Some(Format::Lzx));
            let x = extract_archive("t.lzx", &a, &unlimited()).unwrap_or_else(|e| panic!("sample {k}: {e}"));
            assert!(x.skipped.is_empty(), "sample {k}: {:?}", x.skipped);
            assert_eq!(x.format, Format::Lzx);
            assert_eq!(names(&x), ["dir/file.bin"]);
            assert!(x.entries[0].data == *d, "sample {k} differs");
        }
    }
}

#[test]
fn merged_groups_share_one_stream() {
    let (a, b, c, d) = (sample(3000, 10), sample(100, 11), Vec::new(), sample(90_000, 12));
    // a stored file, a merged group of four (one empty), a single crunched file, a merged stored pair
    for opt in OPTS {
        let z = archive(&[
            (&[("one", &a[..])], Mode::Stored),
            (&[("m/a", &a[..]), ("m/b", &b[..]), ("m/empty", &c[..]), ("m/d", &d[..])], Mode::Crunched(opt)),
            (&[("single", &d[..])], Mode::Crunched(opt)),
            (&[("s1", &b[..]), ("s2", &a[..])], Mode::Stored),
        ]);
        let x = extract_archive("t.lzx", &z, &unlimited()).unwrap();
        assert!(x.skipped.is_empty(), "{:?}", x.skipped);
        assert_eq!(names(&x), ["one", "m/a", "m/b", "m/empty", "m/d", "single", "s1", "s2"]);
        let want: [&[u8]; 8] = [&a, &a, &b, &c, &d, &d, &b, &a];
        for (e, w) in x.entries.iter().zip(want) {
            assert!(e.data == w, "{}", e.name);
        }
    }
}

#[test]
fn a_stream_is_cut_across_files_blocks_and_windows() {
    // two files in one stream, small blocks, and 200 KB so the 64 KiB window wraps three times
    let d: Vec<u8> = sample(200_000, 99);
    let (x, y) = d.split_at(77_777);
    let z = archive(&[(&[("x", x), ("y", y)], Mode::Crunched(Opt { aligned: true, block: 5000, runs: true }))]);
    let r = extract_archive("t.lzx", &z, &unlimited()).unwrap();
    assert!(r.entries[0].data == x && r.entries[1].data == y);
}

#[test]
fn header_fields() {
    let z = archive(&[(&[("a b/c.txt", b"hello")], Mode::Stored)]);
    let h = parse_header(&z, INFO_LEN).unwrap();
    assert_eq!((h.attributes, h.unpacked, h.packed, h.machine, h.mode), (0x0f, 5, 5, 10, 0));
    assert_eq!(h.date, (2026, 11, 12, 13, 14, 15));
    assert_eq!((h.name.as_slice(), h.comment.as_slice(), h.crc), (&b"a b/c.txt"[..], &b"a comment"[..], crc32(0, b"hello")));
    assert_eq!(h.len, 31 + 9 + 9);
}

#[test]
fn what_is_an_lzx_archive() {
    let ok = archive(&[(&[("a", b"hello")], Mode::Stored)]);
    assert!(is_lzx(&ok));
    // only the info header: an archive with no file; extract says so
    assert!(is_lzx(&ok[..10]));
    assert!(extract_archive("t.lzx", &ok[..10], &Limits::default()).is_err());
    // cut inside the first header: still LZX, and an error
    assert!(is_lzx(&ok[..30]));
    assert!(extract_archive("t.lzx", &ok[..30], &Limits::default()).is_err());
    // "LZX" is not enough
    let mut d = b"LZX and then some text that is not an archive at all, long enough".to_vec();
    assert!(!is_lzx(&d));
    assert_eq!(format(&d), None);
    d.truncate(9);
    assert!(!is_lzx(&d));
    // an unknown flag, a damaged first header
    let mut bad = ok.clone();
    bad[3] = 4;
    assert!(!is_lzx(&bad));
    bad = ok.clone();
    bad[10 + 20] ^= 1;
    assert!(!is_lzx(&bad));
    assert_eq!(format(&bad), None);
}

#[test]
fn damaged_data_is_reported_not_returned() {
    let d = sample(4000, 5);
    let z = archive(&[(&[("good", &d[..])], Mode::Crunched(OPTS[0])), (&[("bad", &d[..])], Mode::Stored), (&[("also good", &d[..])], Mode::Stored)]);
    // flip a byte in the second file's data (stored): its CRC no longer matches
    let first = 10 + 31 + 4 + 9 + parse_header(&z, 10).unwrap().packed as usize;
    let at = first + 31 + 3 + 100;
    let mut x = z.clone();
    x[at] ^= 0x55;
    let r = extract_archive("t.lzx", &x, &unlimited()).unwrap();
    assert_eq!(names(&r), ["good", "also good"]);
    assert_eq!(r.skipped.len(), 1);
    assert_eq!((r.skipped[0].name.as_str(), r.skipped[0].reason.as_str()), ("bad", "LZX CRC error"));
    // a damaged header in the middle: what came before stays, the rest is reported
    let mut x = z.clone();
    x[first + 5] ^= 1;
    let r = extract_archive("t.lzx", &x, &unlimited()).unwrap();
    assert_eq!(names(&r), ["good"]);
    assert!(r.skipped[0].reason.contains("damaged after this point"));
    // a damaged first header is an error
    let mut x = z.clone();
    x[10 + 6] ^= 1;
    assert!(extract_archive("t.lzx", &x, &unlimited()).is_err());
    // an unknown pack mode
    let mut m = archive(&[(&[("m", b"data")], Mode::Stored)]);
    let h = parse_header(&m, 10).unwrap();
    let fixed = header("m", "", 4, 4, 9, false, h.crc);
    m.splice(10..10 + h.len, fixed);
    let r = extract_archive("t.lzx", &m, &unlimited()).unwrap();
    assert!(r.entries.is_empty() && r.skipped[0].reason.contains("pack mode 9"), "{r:?}");
    // a crunched stream cut short: the files before the cut are had, and it is reported
    let z = archive(&[(&[("p", &d[..2000]), ("q", &d[2000..])], Mode::Crunched(OPTS[2]))]);
    let r = extract_archive("t.lzx", &z[..z.len() - 40], &unlimited()).unwrap();
    assert!(r.entries.iter().all(|e| e.name == "p" && e.data == d[..2000]));
    assert!(r.skipped.iter().any(|s| s.name == "q" && s.reason.contains("truncated")), "{:?}", r.skipped);
}

#[test]
fn limits_apply() {
    let d = sample(5000, 6);
    let z = archive(&[(&[("big", &d[..]), ("small", &d[..100])], Mode::Crunched(OPTS[2])), (&[("later", &d[..200])], Mode::Stored)]);
    // a file over the per-entry cap ends its group (it is too big to run through), not the archive
    let r = extract_archive("t.lzx", &z, &Limits { max_entry: 1000, ..Limits::default() }).unwrap();
    assert_eq!(names(&r), ["later"]);
    assert!(r.skipped[0].reason.contains("larger"));
    assert_eq!(r.skipped.len(), 2);
    // over the total: the stream is run through, so "small" is still had
    let r = extract_archive("t.lzx", &z, &Limits { max_total: 250, ..Limits::default() }).unwrap();
    assert_eq!(names(&r), ["small"]);
    assert_eq!(r.skipped.len(), 2);
    let r = extract_archive("t.lzx", &z, &Limits { max_entries: 2, ..Limits::default() }).unwrap();
    assert_eq!((r.entries.len(), r.skipped.len()), (2, 1));
    let r = extract_archive("t.lzx", &z, &Limits { max_entry: 10, ..Limits::default() }).unwrap();
    assert!(r.entries.is_empty());
    assert_eq!(r.skipped.len(), 3);
}

#[test]
fn names_are_cleaned_like_the_others() {
    let z = archive(&[(&[("../../etc/x", b"1"), ("DH0:music/y.mod", b"2"), ("..", b"3")], Mode::Stored)]);
    let r = extract_archive("t.lzx", &z, &unlimited()).unwrap();
    assert_eq!(names(&r), ["etc/x", "music/y.mod"]);
    assert_eq!(r.skipped.len(), 1);
}

/// `extract` called directly: a panic in it is a failure here, not the caught error the public
/// function turns it into.
fn direct(d: &[u8]) -> Result<(), ArchiveError> {
    let lim = Limits { max_entry: 1 << 24, max_total: 1 << 25, max_entries: 100 };
    let mut out = Out::new(&lim);
    extract(d, &mut out)
}

#[test]
fn truncated_and_corrupted_archives_do_not_panic() {
    let a = sample(6000, 21);
    let b = sample(3000, 22);
    for opt in [OPTS[1], OPTS[2]] {
        let z = archive(&[(&[("a", &a[..]), ("b", &b[..])], Mode::Crunched(opt)), (&[("c", &b[..])], Mode::Stored)]);
        for cut in 0..z.len() {
            let _ = direct(&z[..cut]);
        }
        let mut seed = 0xDEAD_BEEF_1234_5678u64;
        for _ in 0..3000 {
            let mut x = z.clone();
            for _ in 0..1 + rng(&mut seed) % 5 {
                let at = (rng(&mut seed) % x.len() as u64) as usize;
                x[at] = rng(&mut seed) as u8;
            }
            let _ = direct(&x);
        }
    }
    // random bytes after a good header
    let mut seed = 99u64;
    let z = archive(&[(&[("a", b"hello")], Mode::Crunched(OPTS[0]))]);
    for _ in 0..500 {
        let mut x = z[..10 + 31 + 1 + 9].to_vec();
        x.extend((0..rng(&mut seed) % 200).map(|_| rng(&mut seed) as u8));
        let _ = direct(&x);
    }
    // a header that claims four gigabytes
    let mut big = archive(&[(&[("a", &a[..100])], Mode::Crunched(OPTS[0]))]);
    let h = parse_header(&big, 10).unwrap();
    big.splice(10..10 + h.len, header("a", "a comment", 0xFFFF_FFF0, h.packed as usize, 2, false, 0));
    assert!(direct(&big).is_ok());
}

#[test]
fn expands_into_the_cache_like_the_others() {
    let root = std::env::temp_dir().join(format!("amigaplayer-lzx-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let (a, b) = (sample(2000, 31), sample(700, 32));
    let z = archive(&[(&[("mods/tune.mod", &a[..]), ("smp.tune", &b[..])], Mode::Crunched(OPTS[1]))]);
    let src = root.join("t.lzx");
    std::fs::write(&src, &z).unwrap();
    let x = crate::archive::expand_file(&src, Some(&root.join("cache"))).unwrap().unwrap();
    assert_eq!(x.format, Format::Lzx);
    assert_eq!(x.entries.len(), 2);
    assert_eq!(x.entries[0].0, "mods/tune.mod");
    assert_eq!(std::fs::read(&x.entries[0].1).unwrap(), a);
    assert_eq!(std::fs::read(&x.entries[1].1).unwrap(), b);
    std::fs::remove_dir_all(&root).unwrap();
}

// ---- real archives ------------------------------------------------------------------------

fn corpus() -> Vec<std::path::PathBuf> {
    let dir = std::env::var_os("LZX_CORPUS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/media/lolo/kingston2tb/claude-scratch/deliplayerrust/wt/dl".into());
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lzx"))).collect())
        .unwrap_or_default();
    v.sort();
    v
}

#[test]
fn real_archives_extract_with_good_crcs() {
    let files = corpus();
    if files.is_empty() {
        eprintln!("no .lzx corpus here: skipped");
        return;
    }
    let mut entries = 0;
    for p in &files {
        let d = std::fs::read(p).unwrap();
        assert_eq!(format(&d), Some(Format::Lzx), "{}", p.display());
        let a = extract_archive(&p.to_string_lossy(), &d, &Limits::default()).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        // (every file's CRC was checked: one that failed it would be in `skipped`)
        assert!(a.skipped.is_empty(), "{}: {:?}", p.display(), a.skipped);
        assert!(!a.entries.is_empty());
        entries += a.entries.len();
    }
    eprintln!("{} real archives, {entries} files, all CRCs good", files.len());
}

#[test]
fn real_archives_damaged_do_not_panic() {
    let files = corpus();
    let mut seed = 0x0123_4567_89ab_cdefu64;
    for p in files.iter().step_by(7) {
        let d = std::fs::read(p).unwrap();
        let stride = (d.len() / 400).max(1);
        for cut in (0..d.len()).step_by(stride) {
            let _ = direct(&d[..cut]);
        }
        for _ in 0..150 {
            let mut x = d.clone();
            for _ in 0..1 + rng(&mut seed) % 4 {
                let at = (rng(&mut seed) % x.len() as u64) as usize;
                x[at] = rng(&mut seed) as u8;
            }
            let _ = direct(&x);
        }
    }
}
