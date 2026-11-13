//! Custom modules through `Song` (`Options::allow_custom`, `host-68k` builds): a synthetic one
//! plays, and one whose 68k code hangs, faults or is damaged stops with an error; the host
//! is never hung or taken down.
#![cfg(feature = "custom-modules")]

use amigaplayer_rs::testdata::{custom_with, minimal_custom, minimal_mod};
use amigaplayer_rs::{detect, detect_custom, Error, Options, Song};
use std::time::{Duration, Instant};

const RATE: u32 = 48000;
const RTS: [u8; 2] = [0x4E, 0x75];

fn custom() -> Options {
    Options::new().allow_custom(true)
}

fn open(data: &[u8]) -> Result<Song, Error> {
    Song::open_opts("cust.test", data, &[], RATE, &custom())
}

fn render(song: &mut Song, secs: f64) -> Vec<f32> {
    let mut out = vec![0f32; 2 * (secs * RATE as f64) as usize];
    for chunk in out.chunks_mut(2 * 1024) {
        song.render_f32(chunk);
    }
    out
}

/// The dominant frequency of the left channel, from its zero crossings.
fn pitch(pcm: &[f32]) -> f64 {
    let left: Vec<f32> = pcm.chunks(2).map(|f| f[0]).collect();
    let ups = left.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    ups as f64 / (left.len() as f64 / RATE as f64)
}

#[test]
fn a_custom_module_is_told_apart() {
    let m = minimal_custom();
    assert!(detect_custom(&m));
    assert_eq!(detect(&m), None, "no open replayer claims it");
    assert!(!detect_custom(&minimal_mod()));
    assert!(!detect_custom(&[]));
    assert!(!detect_custom(&[0, 0, 3, 0xF3]));
    // an executable that is not a player
    let mut plain = m.clone();
    let at = plain.windows(8).position(|w| w == b"DELIRIUM").unwrap();
    plain[at] = b'X';
    assert!(!detect_custom(&plain));
}

#[test]
fn custom_modules_are_refused_unless_asked_for() {
    let m = minimal_custom();
    assert_eq!(Song::open("cust.test", &m, RATE).err(), Some(Error::NotRecognised));
    assert_eq!(Song::open_with("cust.test", &m, &[], RATE).err(), Some(Error::NotRecognised));
    assert_eq!(Song::open_opts("cust.test", &m, &[], RATE, &Options::default()).err(), Some(Error::NotRecognised));
    assert_eq!(Song::open_opts("cust.test", &m, &[], RATE, &Options::new().allow_custom(false)).err(), Some(Error::NotRecognised));
    assert!(open(&m).is_ok());
}

#[test]
fn the_option_changes_nothing_for_other_modules() {
    let m = minimal_mod();
    let mut a = Song::open("a.mod", &m, RATE).unwrap();
    let mut b = Song::open_opts("a.mod", &m, &[], RATE, &custom()).unwrap();
    assert_eq!(b.format(), "ProTracker");
    assert_eq!(render(&mut a, 0.5), render(&mut b, 0.5));
    assert_eq!(open(b"not a module").err(), Some(Error::NotRecognised));
}

#[test]
fn a_custom_module_plays_a_tone() {
    let mut song = open(&minimal_custom()).unwrap();
    assert_eq!(song.format(), "Custom");
    assert_eq!(song.title(), "synthetic");
    let s = song.subsongs();
    assert_eq!((s.first, s.last, s.current), (0, 1, 0));
    assert_eq!(song.duration_hint(), None);
    let pcm = render(&mut song, 1.0);
    assert!(song.error().is_none());
    assert!(pcm.iter().any(|&x| x.abs() > 0.05), "silence");
    assert!(pcm.iter().all(|&x| (-1.0..=1.0).contains(&x)));
    // period 428, 32 bytes a cycle: 3546895 / 428 / 32 = 259 Hz
    let hz = pitch(&pcm);
    assert!((250.0..270.0).contains(&hz), "{hz} Hz");
    assert!((song.elapsed_secs() - 1.0).abs() < 0.01);
}

#[test]
fn rendering_a_custom_module_is_deterministic() {
    let m = minimal_custom();
    let a = render(&mut open(&m).unwrap(), 0.5);
    let b = render(&mut open(&m).unwrap(), 0.5);
    assert_eq!(a, b);
    let mut s = open(&m).unwrap();
    render(&mut s, 0.3);
    s.restart().unwrap();
    assert_eq!(render(&mut s, 0.5), a);
    s.seek(0.25).unwrap();
    assert!((s.elapsed_secs() - 0.25).abs() < 0.01);
}

#[test]
fn subsongs_of_a_custom_module() {
    let mut song = open(&minimal_custom()).unwrap();
    song.set_subsong(1).unwrap();
    assert_eq!(song.subsongs().current, 1);
    let hz = pitch(&render(&mut song, 1.0));
    assert!((500.0..540.0).contains(&hz), "{hz} Hz");
    song.set_subsong(0).unwrap();
    let hz = pitch(&render(&mut song, 1.0));
    assert!((250.0..270.0).contains(&hz), "{hz} Hz");
    assert_eq!(song.set_subsong(2).err(), Some(Error::SubsongOutOfRange { requested: 2, first: 0, last: 1 }));
    // a restart keeps the subsong
    song.set_subsong(1).unwrap();
    song.restart().unwrap();
    assert_eq!(song.subsongs().current, 1);
}

#[test]
fn the_song_end_of_a_custom_module_is_reported() {
    let mut song = open(&minimal_custom()).unwrap();
    render(&mut song, 2.5);
    assert!(!song.songend());
    render(&mut song, 1.0);
    assert!(song.songend(), "its interrupt calls dtg_SongEnd after 150 ticks");
    assert!(song.error().is_none());
}

/// What a failed song looks like to the host from then on.
fn assert_stopped(song: &mut Song, what: &str) {
    match song.error() {
        Some(Error::Guest(why)) => assert!(why.contains(what), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(!song.songend());
    // it still says what it is, and renders silence without running any more code
    assert_eq!(song.format(), "Custom");
    assert_eq!(song.title(), "synthetic");
    assert_eq!(song.subsongs().last, 1);
    let t = Instant::now();
    let pcm = render(song, 5.0);
    assert!(pcm.iter().all(|&x| x == 0.0));
    assert!(t.elapsed() < Duration::from_secs(2));
    assert!(matches!(song.set_subsong(1), Err(Error::Guest(_))));
    // and can be started again
    song.restart().unwrap();
    assert!(song.error().is_none());
}

#[test]
fn an_interrupt_that_never_returns_stops_the_song() {
    // bra.s *
    let mut song = open(&custom_with(&[], &[0x60, 0xFE])).unwrap();
    assert!(song.error().is_none());
    let t = Instant::now();
    let mut pcm = vec![0f32; 2 * RATE as usize];
    assert_eq!(song.render_f32(&mut pcm), RATE as usize, "render returns, with all its frames");
    let took = t.elapsed();
    assert!(took < Duration::from_secs(30), "{took:?}");
    // the tone that InitSound started plays until the first interrupt, then nothing
    assert!(pcm[..400].iter().any(|&x| x != 0.0));
    assert!(pcm[4000..].iter().all(|&x| x == 0.0));
    assert_stopped(&mut song, "did not return");
}

#[test]
fn an_init_that_never_returns_is_an_error() {
    let t = Instant::now();
    match open(&custom_with(&[0x60, 0xFE], &[])) {
        Err(Error::Invalid(why)) => assert!(why.contains("did not return"), "{why}"),
        other => panic!("{:?}", other.err()),
    }
    assert!(t.elapsed() < Duration::from_secs(30));
}

#[test]
fn an_illegal_instruction_stops_the_song() {
    // illegal
    let mut song = open(&custom_with(&[], &[0x4A, 0xFC])).unwrap();
    render(&mut song, 0.5);
    assert_stopped(&mut song, "IllegalInstruction");
    match open(&custom_with(&[0x4A, 0xFC], &[])) {
        Err(Error::Invalid(why)) => assert!(why.contains("IllegalInstruction"), "{why}"),
        other => panic!("{:?}", other.err()),
    }
}

#[test]
fn other_faults_stop_the_song() {
    let faults: [(&str, &[u8]); 6] = [
        // jmp $1.l: an odd address
        ("address error", &[0x4E, 0xF9, 0x00, 0x00, 0x00, 0x01]),
        // moveq #0,d1 ; divu d1,d0 ; rts
        ("division by zero", &[0x72, 0x00, 0x80, 0xC1, 0x4E, 0x75]),
        // an F-line opcode
        ("line F", &[0xF2, 0x00, 0x4E, 0x75]),
        // jmp $f00000.l: no memory there, it reads as zeroes (ori.b #0,d0 for ever)
        ("a jump out of memory", &[0x4E, 0xF9, 0x00, 0xF0, 0x00, 0x00]),
        // addq.l #4,sp ; rts: returns to the wrong place
        ("a broken stack", &[0x58, 0x8F, 0x4E, 0x75]),
        // stop #$2700
        ("stop", &[0x4E, 0x72, 0x27, 0x00]),
    ];
    for (what, code) in faults {
        let mut song = open(&custom_with(&[], code)).unwrap();
        let t = Instant::now();
        render(&mut song, 0.5);
        assert!(matches!(song.error(), Some(Error::Guest(_))), "{what}: {:?}", song.error());
        assert!(t.elapsed() < Duration::from_secs(30), "{what}");
        let pcm = render(&mut song, 0.5);
        assert!(pcm.iter().all(|&x| x == 0.0), "{what}");
    }
}

#[test]
fn memory_outside_the_machine_is_not_a_fault() {
    // tst.b $f00000.l ; move.w d0,$f00000.l ; rts: reads give 0, writes go nowhere
    let code = [0x4A, 0x39, 0x00, 0xF0, 0x00, 0x00, 0x33, 0xC0, 0x00, 0xF0, 0x00, 0x00, 0x4E, 0x75];
    let mut song = open(&custom_with(&[], &code)).unwrap();
    let pcm = render(&mut song, 0.5);
    assert!(song.error().is_none());
    assert!(pcm.iter().any(|&x| x != 0.0));
}

#[test]
fn guest_code_cannot_reach_the_hosts_files() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("amigaplayer-custom-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (new, old) = (dir.join("created"), dir.join("existing"));
    std::fs::write(&old, b"untouched").unwrap();
    // InitSound calls dos.library's Open(name, mode): a file that appears, or is cut to
    // nothing, is the failure.
    //   movea.l dtg_DOSBase(a5),a6 ; lea name(pc),a0 ; move.l a0,d1 ; move.l #mode,d2
    //   jsr _LVOOpen(a6) ; bra.w over the name
    let call = |name: &std::path::Path, mode: u32| {
        let mut name = name.to_str().unwrap().as_bytes().to_vec();
        name.push(0);
        if name.len() % 2 != 0 {
            name.push(0);
        }
        let mut c = vec![0x2C, 0x6D, 0x00, 0x04, 0x41, 0xFA, 0x00, 0x12, 0x22, 0x08, 0x24, 0x3C];
        c.extend_from_slice(&mode.to_be_bytes());
        c.extend_from_slice(&[0x4E, 0xAE, 0xFF, 0xE2, 0x60, 0x00]);
        c.extend_from_slice(&(name.len() as u16 + 2).to_be_bytes());
        c.extend_from_slice(&name);
        c
    };
    let mut prefix = call(&new, 1006); // MODE_NEWFILE
    prefix.extend(call(&old, 1006));
    prefix.extend(call(&old, 1005)); // MODE_OLDFILE
    let mut song = open(&custom_with(&prefix, &[])).unwrap();
    render(&mut song, 0.2);
    assert!(song.error().is_none());
    assert!(!new.exists(), "guest code created a file of the host");
    assert_eq!(std::fs::read(&old).unwrap(), b"untouched");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn damaged_custom_modules_do_not_take_the_host_down() {
    let m = minimal_custom();
    let deadline = Instant::now() + Duration::from_secs(120);
    let try_one = |d: &[u8]| {
        let _ = detect_custom(d);
        if let Ok(mut song) = open(d) {
            let mut pcm = vec![0f32; 2 * 2000];
            song.render_f32(&mut pcm);
            let _ = (song.title().len(), song.format().len(), song.subsongs(), song.songend(), song.error());
            let _ = song.set_subsong(song.subsongs().last);
            song.render_f32(&mut pcm);
        }
    };
    // every truncation
    for n in 0..m.len() {
        try_one(&m[..n]);
    }
    // every byte of the header, tags and code damaged in turn, in a few ways
    let mut seed = 0x2545_F491u32;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let mut done = 0;
    'all: for at in 0..m.len() {
        for v in [0x00, 0xFF, m[at] ^ 0x80, rnd() as u8] {
            let mut d = m.clone();
            d[at] = v;
            try_one(&d);
            done += 1;
            if Instant::now() > deadline {
                break 'all;
            }
        }
    }
    assert!(done > 400, "only {done} variants in two minutes");
    // random words in place of the interrupt routine and the start of InitSound
    for _ in 0..300 {
        let code: Vec<u8> = (0..24).map(|_| rnd() as u8).collect();
        try_one(&custom_with(&[], &code));
        try_one(&custom_with(&code[..8], &RTS));
        if Instant::now() > deadline {
            break;
        }
    }
}

/// Game modules that carry their own 68000 replay code, found in the project's scratch corpus
/// (the test is skipped where it is not): the library plays them on its own host, from the
/// module's code alone, with no DeliPlayer or Eagleplayer file anywhere (`Song` loads none).
/// Each is (path under the corpus, first subsong, last subsong). The application's Paula write
/// log of each, with the 68k players loaded, was compared with `--open-replayers` and every
/// player directory empty: identical for 30 s.
#[test]
fn real_custom_modules_play_on_the_hosts_own_code() {
    const ROOT: &str = "/media/lolo/kingston2tb/claude-scratch/deliplayerrust/corpus-seagate/amiga-archive/exotica/UnExoticA/Game/";
    let files = [
        ("Pieket_Weeserik_Ronald/Rodland/Rodland/cust.Rodland", 1, 9),
        ("Cuomo_Jim/Defender_of_the_Crown/Defender_of_the_Crown/cust.Defender_of_the_Crown", 0, 8),
        ("van_der_Valk_Paul/Hybris/Hybris/Custom_Version/cust.ingame", 0, 5),
    ];
    for (rel, first, last) in files {
        let Ok(data) = std::fs::read(format!("{ROOT}{rel}")) else {
            eprintln!("{rel}: not here, skipped");
            continue;
        };
        let name = rel.rsplit('/').next().unwrap();
        assert!(detect_custom(&data), "{name}");
        assert_eq!(Song::open(name, &data, RATE).err(), Some(Error::NotRecognised), "{name}: only when asked for");
        let mut song = Song::open_opts(name, &data, &[], RATE, &custom()).unwrap();
        assert_eq!(song.format(), "Custom", "{name}");
        let s = song.subsongs();
        assert_eq!((s.first, s.last), (first, last), "{name}");
        let pcm = render(&mut song, 5.0);
        assert!(song.error().is_none(), "{name}: {:?}", song.error());
        assert!(pcm.iter().any(|&x| x.abs() > 0.05), "{name}: silence");
    }
}
