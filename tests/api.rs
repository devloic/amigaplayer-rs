//! Integration tests of the public API, on the synthetic modules of `amigaplayer_rs::testdata`.

use amigaplayer_rs::testdata::{minimal_mod, minimal_mod_titled};
use amigaplayer_rs::{detect, formats, Error, Song};

const RATE: u32 = 44100;

fn render_all(song: &mut Song, frames: usize) -> Vec<f32> {
    let mut out = vec![0f32; frames * 2];
    for chunk in out.chunks_mut(2 * 1000) {
        song.render_f32(chunk);
    }
    out
}

#[test]
fn detects_a_protracker_module_and_not_garbage() {
    assert_eq!(detect(&minimal_mod()).as_deref(), Some("ProTracker"));
    assert_eq!(detect(&[]), None);
    assert_eq!(detect(b"hello, this is not a module"), None);
    assert_eq!(detect(&vec![0u8; 5000]), None);
}

#[test]
fn format_list_has_the_big_names() {
    let f = formats();
    assert!(f.contains(&"ProTracker"));
    assert!(f.contains(&"AIFF"));
    assert!(f.len() > 100, "{} formats", f.len());
}

#[test]
fn opens_and_describes_the_module() {
    let song = Song::open("dir/tune.mod", &minimal_mod_titled("my tune"), RATE).unwrap();
    assert_eq!(song.title(), "my tune");
    assert_eq!(song.format(), "ProTracker");
    assert_eq!(song.sample_rate(), RATE);
    let s = song.subsongs();
    assert_eq!((s.first, s.last, s.current, s.count()), (0, 0, 0, 1));
    assert_eq!(song.duration_hint(), None);
    assert!(!song.songend());
}

#[test]
fn rendering_makes_sound_and_is_deterministic() {
    let m = minimal_mod();
    let a = render_all(&mut Song::open("a.mod", &m, RATE).unwrap(), 20000);
    let b = render_all(&mut Song::open("b.mod", &m, RATE).unwrap(), 20000);
    assert_eq!(a, b);
    assert!(a.iter().any(|&x| x != 0.0), "silence");
    assert!(a.iter().all(|&x| (-1.0..=1.0).contains(&x)));
}

#[test]
fn i16_output_is_the_rounded_f32_output() {
    let m = minimal_mod();
    let f = render_all(&mut Song::open("a.mod", &m, RATE).unwrap(), 4000);
    let mut s = Song::open("a.mod", &m, RATE).unwrap();
    let mut i = vec![0i16; 8000];
    assert_eq!(s.render_i16(&mut i), 4000);
    for (x, &y) in f.iter().zip(&i) {
        assert_eq!(y, (x * 32768.0).round().clamp(-32768.0, 32767.0) as i16);
    }
}

#[test]
fn render_counts_frames_and_ignores_a_trailing_odd_sample() {
    let mut s = Song::open("a.mod", &minimal_mod(), RATE).unwrap();
    let mut out = [7f32; 11];
    assert_eq!(s.render_f32(&mut out), 5);
    assert_eq!(out[10], 7.0);
    assert_eq!(s.render_f32(&mut []), 0);
}

#[test]
fn restart_and_seek_replay_the_same_samples() {
    let mut s = Song::open("a.mod", &minimal_mod(), RATE).unwrap();
    let first = render_all(&mut s, 30000);
    assert!(s.elapsed_secs() > 0.6);
    s.restart().unwrap();
    assert_eq!(s.elapsed_secs(), 0.0);
    assert_eq!(render_all(&mut s, 30000), first);
    // seeking to 0.25 s, then rendering, continues the first run from there
    let skip = (0.25 * RATE as f64) as usize;
    s.seek(0.25).unwrap();
    let tail = render_all(&mut s, 1000);
    assert_eq!(&tail[..], &first[skip * 2..skip * 2 + 2000]);
}

#[test]
fn subsong_range_is_enforced() {
    let mut s = Song::open("a.mod", &minimal_mod(), RATE).unwrap();
    s.set_subsong(0).unwrap();
    assert_eq!(s.set_subsong(3), Err(Error::SubsongOutOfRange { requested: 3, first: 0, last: 0 }));
}

#[test]
fn errors_are_typed() {
    assert_eq!(Song::open("x.mod", b"garbage", RATE).err(), Some(Error::NotRecognised));
    assert_eq!(Song::open("x.mod", &[], RATE).err(), Some(Error::NotRecognised));
    assert_eq!(Song::open("x.mod", &minimal_mod(), 0).err(), Some(Error::InvalidRate(0)));
    assert_eq!(Song::open("x.mod", &minimal_mod(), 5_000_000).err(), Some(Error::InvalidRate(5_000_000)));
    let e = Error::NotRecognised;
    assert!(!e.to_string().is_empty());
    let _: &dyn std::error::Error = &e;
}

#[test]
fn companion_files_are_accepted() {
    let m = minimal_mod();
    let s = Song::open_with("tune.mod", &m, &[("smpl.tune", b"abc"), ("../Instruments/x", b"y")], RATE).unwrap();
    assert_eq!(s.format(), "ProTracker");
}

#[test]
fn the_name_is_only_a_hint() {
    let m = minimal_mod();
    for name in ["", "/", "a/b/c.mod", "..\\x.mod", "no extension"] {
        assert!(Song::open(name, &m, RATE).is_ok(), "{name:?}");
    }
}

// --- no panics on damaged input -------------------------------------------------------------

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Opens `data` and renders a little; the only failure is a panic (`Error::Internal`) or an
/// `Ok` that renders non-finite samples.
fn survive(data: &[u8], what: &str) {
    match Song::open("fuzz", data, 22050) {
        Ok(mut s) => {
            let mut out = vec![0f32; 2 * 1500];
            for _ in 0..4 {
                s.render_f32(&mut out);
            }
            assert!(out.iter().all(|x| x.is_finite()), "{what}: non-finite samples");
            let _ = s.title();
            let _ = s.format();
            let sub = s.subsongs();
            let _ = s.set_subsong(sub.last);
            s.render_f32(&mut out);
            assert!(!matches!(s.set_subsong(sub.first), Err(Error::Internal(_))), "{what}: panic in set_subsong");
        }
        Err(Error::Internal(why)) => panic!("{what}: replayer panicked: {why}"),
        Err(_) => {}
    }
}

#[test]
fn truncated_modules_do_not_panic() {
    let m = minimal_mod();
    for n in [0, 1, 20, 100, 950, 1000, 1083, 1084, 1085, 1200, 2108, 2300] {
        survive(&m[..n.min(m.len())], &format!("minimal mod cut at {n}"));
    }
}

#[test]
fn mutated_modules_do_not_panic() {
    let m = minimal_mod();
    let mut rng = Rng(0x1234_5678_9abc_def1);
    for round in 0..120 {
        let mut d = m.clone();
        for _ in 0..1 + rng.below(12) {
            let i = rng.below(d.len());
            d[i] = rng.next() as u8;
        }
        if round % 5 == 0 {
            d.truncate(rng.below(d.len()));
        }
        survive(&d, &format!("mutation round {round}"));
    }
}

/// Format identification strings of many replayers; each followed by random bytes, so that a
/// detector that accepts the magic alone starts a replayer on garbage.
const MAGICS: &[&[u8]] = &[
    b"TFMX-SONG", b"TFMX", b"SIDMON II - THE MIDI VERSION", b"COSO", b"FC14", b"SMOD", b"MMD0", b"MMD1", b"MMD2",
    b"AHX\x00", b"AHX\x01", b"THX\x01", b"HVL\x01", b"OKTASONG", b"RJP", b"DIGI Booster module", b"DBM0",
    b"IMPM", b"Extended Module: ", b"FORM", b"MTM", b"FAR\xfe", b"MThd", b"SYMMUSIC", b"BeEp", b"MUGICIAN",
    b" MUGICIAN2", b"ADPCM", b"SOARV1.0", b"SONIC ARRANGER", b"Fuchs Tracker", b"CUST", b"DSM\x00", b"JamCracker",
    b"BP30", b"MaxTrax", b"SPS", b"PSID", b"PTM ", b"STM", b"UN05", b"S3M", b"SYNTRACKER-SONG:",
];

#[test]
fn magic_followed_by_garbage_does_not_panic() {
    let mut rng = Rng(0xfeed_f00d_dead_beef);
    let mut accepted = 0;
    for (k, magic) in MAGICS.iter().enumerate() {
        for variant in 0..24 {
            let len = [64, 400, 1500, 3000, 20000, 70000][variant % 6];
            let mut d: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            // sparse bodies reach more code than white noise (zero runs are valid lengths and counts)
            if variant % 4 >= 2 {
                for b in d.iter_mut() {
                    if rng.below(4) != 0 {
                        *b = 0;
                    }
                }
            }
            let at = if variant % 3 == 2 { 1080.min(len - magic.len()) } else { 0 };
            d[at..at + magic.len()].copy_from_slice(magic);
            if detect(&d).is_some() {
                accepted += 1;
            }
            survive(&d, &format!("magic {k} ({:?}) variant {variant}", String::from_utf8_lossy(magic)));
        }
    }
    eprintln!("{accepted} of {} garbage inputs were claimed by a replayer", MAGICS.len() * 24);
}

/// Every open replayer's detector, and the libxmp loaders' tests, on short and damaged input:
/// they must answer, not index out of bounds.
#[test]
fn detectors_survive_short_and_random_input() {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    let mut rng = Rng(0x0bad_cafe_0bad_cafe);
    let mut inputs: Vec<Vec<u8>> = vec![Vec::new()];
    for n in [1, 2, 3, 4, 7, 8, 11, 12, 16, 17, 31, 32, 63, 64, 100, 255, 256, 600, 1083, 1084, 1085, 1600, 4096] {
        inputs.push(vec![0; n]);
        inputs.push(vec![0xff; n]);
        inputs.push((0..n).map(|_| rng.next() as u8).collect());
    }
    inputs.push(minimal_mod());
    let mut bad = Vec::new();
    for o in amigaplayer_rs::replay::REPLAYERS {
        for d in &inputs {
            if catch_unwind(AssertUnwindSafe(|| (o.detect)(d))).is_err() {
                bad.push(format!("{} on {} bytes", o.name, d.len()));
                break;
            }
        }
    }
    for l in amigaplayer_rs::xmp::LOADERS {
        for d in &inputs {
            if catch_unwind(AssertUnwindSafe(|| (l.test)(d))).is_err() {
                bad.push(format!("xmp {} on {} bytes", l.name, d.len()));
                break;
            }
        }
    }
    assert!(bad.is_empty(), "detectors that panic: {bad:#?}");
}
