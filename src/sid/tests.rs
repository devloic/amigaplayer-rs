// Tests of the SID player on tunes assembled here (no file from an archive is needed):
// header parsing, malformed files, the driver's speeds, subsongs, determinism, and the
// bounds that keep a hostile tune from hanging or crashing the player.

use super::tune::{Clock, Compatibility, SPEED_CIA_1A, SPEED_VBI, SidModel, Tune};
use super::*;

const RATE: u32 = 48000;

/// A PSID/RSID file around `code` loaded at $1000.
struct Psid {
    magic: &'static [u8; 4],
    version: u16,
    load: u16,
    init: u16,
    play: u16,
    songs: u16,
    start: u16,
    speed: u32,
    flags: u16,
    reloc: (u8, u8),
    sid2: u8,
    sid3: u8,
    code: Vec<u8>,
}

impl Psid {
    fn new(code: Vec<u8>) -> Psid {
        Psid {
            magic: b"PSID",
            version: 2,
            load: 0x1000,
            init: 0x1000,
            play: 0x1030,
            songs: 1,
            start: 1,
            speed: 0,
            flags: 0,
            reloc: (0, 0),
            sid2: 0,
            sid3: 0,
            code,
        }
    }

    fn bytes(&self) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(self.magic);
        d.extend_from_slice(&self.version.to_be_bytes());
        d.extend_from_slice(&(if self.version >= 2 { 124u16 } else { 118 }).to_be_bytes());
        d.extend_from_slice(&self.load.to_be_bytes());
        d.extend_from_slice(&self.init.to_be_bytes());
        d.extend_from_slice(&self.play.to_be_bytes());
        d.extend_from_slice(&self.songs.to_be_bytes());
        d.extend_from_slice(&self.start.to_be_bytes());
        d.extend_from_slice(&self.speed.to_be_bytes());
        for s in ["Test tune", "Nobody", "2026 amigaplayer-rs"] {
            let mut f = [0u8; 32];
            f[..s.len()].copy_from_slice(s.as_bytes());
            d.extend_from_slice(&f);
        }
        if self.version >= 2 {
            d.extend_from_slice(&self.flags.to_be_bytes());
            d.extend_from_slice(&[self.reloc.0, self.reloc.1, self.sid2, self.sid3]);
        }
        if self.load == 0 {
            d.extend_from_slice(&0x1000u16.to_le_bytes());
        }
        d.extend_from_slice(&self.code);
        d
    }
}

/// init at $1000: volume 15, voice 1 a sawtooth at `freq` (SID register value) with
/// the gate on, the song number (A on entry) to $2002, a call counter at $2000 cleared.
/// play at $1030: counts its calls in $2000/$2001.
fn tone_code(freq: u16) -> Vec<u8> {
    let mut c = vec![
        0x8d, 0x02, 0x20, // STA $2002
        0xa9, 0x0f, 0x8d, 0x18, 0xd4, // LDA #$0f : STA $d418
        0xa9, freq as u8, 0x8d, 0x00, 0xd4, // freq lo
        0xa9, (freq >> 8) as u8, 0x8d, 0x01, 0xd4, // freq hi
        0xa9, 0xf0, 0x8d, 0x06, 0xd4, // sustain 15
        0xa9, 0x21, 0x8d, 0x04, 0xd4, // sawtooth + gate
        0xa9, 0x00, 0x8d, 0x00, 0x20, 0x8d, 0x01, 0x20, // counter = 0
        0x60, // RTS
    ];
    c.resize(0x30, 0xea);
    c.extend_from_slice(&[0xee, 0x00, 0x20, 0xd0, 0x03, 0xee, 0x01, 0x20, 0x60]); // INC $2000 : BNE + : INC $2001 : RTS
    c
}

/// The SID frequency register value for `hz` on a PAL machine.
fn pal_freq(hz: f64) -> u16 {
    (hz * 16777216.0 / 985248.6) as u16
}

fn render(p: &mut SidPlayer, secs: f64) -> Vec<i16> {
    let mut out = vec![0i16; (secs * RATE as f64) as usize * 2];
    for chunk in out.chunks_mut(4096) {
        assert_eq!(p.render_i16(chunk), chunk.len() / 2);
    }
    out
}

/// Upward zero crossings per second of the left channel (with a little hysteresis).
fn pitch(pcm: &[i16]) -> f64 {
    let left: Vec<i32> = pcm.chunks_exact(2).map(|f| f[0] as i32).collect();
    let mean = left.iter().map(|&x| x as i64).sum::<i64>() / left.len().max(1) as i64;
    let peak = left.iter().map(|&x| (x as i64 - mean).abs()).max().unwrap_or(0);
    let th = (peak / 8).max(1);
    let (mut below, mut n) = (false, 0);
    for &x in &left {
        let v = x as i64 - mean;
        if v < -th {
            below = true;
        } else if v > th && below {
            below = false;
            n += 1;
        }
    }
    n as f64 * RATE as f64 / left.len() as f64
}

fn calls(p: &SidPlayer) -> u32 {
    p.c64.peek_ram(0x2000) as u32 | (p.c64.peek_ram(0x2001) as u32) << 8
}

#[test]
fn header_fields() {
    let mut f = Psid::new(tone_code(pal_freq(440.0)));
    f.songs = 5;
    f.start = 3;
    f.speed = 0b10110;
    f.flags = (1 << 2) | (2 << 4); // PAL, 8580
    let t = Tune::load(&f.bytes()).unwrap();
    assert_eq!((t.format, t.version), ("PSID", 2));
    assert_eq!((t.load_addr, t.init_addr, t.play_addr), (0x1000, 0x1000, 0x1030));
    assert_eq!((t.songs, t.start_song, t.current_song), (5, 3, 3));
    assert_eq!((t.name.as_str(), t.author.as_str(), t.released.as_str()), ("Test tune", "Nobody", "2026 amigaplayer-rs"));
    assert_eq!(t.compatibility, Compatibility::C64);
    assert_eq!(t.clock_speed, Clock::Pal);
    assert_eq!(t.sid_model(0), SidModel::Mos8580);
    assert_eq!(t.sid_chip_base(1), 0);
    assert_eq!(t.c64_data_len as usize, f.code.len());
    // the speed word, one bit per song: VBI, CIA, CIA, VBI, CIA
    let mut t = t;
    let speeds: Vec<u8> = (1..=5).map(|s| {
        t.select_song(s);
        t.song_speed
    }).collect();
    assert_eq!(speeds, [SPEED_VBI, SPEED_CIA_1A, SPEED_CIA_1A, SPEED_VBI, SPEED_CIA_1A]);
    // an invalid song number selects the start song
    assert_eq!(t.select_song(77), 3);
}

#[test]
fn header_variants() {
    // load address 0: taken from the first two bytes of the data
    let mut f = Psid::new(tone_code(1000));
    f.load = 0;
    f.init = 0;
    let t = Tune::load(&f.bytes()).unwrap();
    assert_eq!((t.load_addr, t.init_addr), (0x1000, 0x1000));
    assert_eq!(t.c64_data_len as usize, f.code.len());

    // version 1: PSID specific, no flags
    let mut f = Psid::new(tone_code(1000));
    f.version = 1;
    let t = Tune::load(&f.bytes()).unwrap();
    assert_eq!(t.compatibility, Compatibility::Psid);
    assert_eq!(t.clock_speed, Clock::Unknown);
    assert_eq!(t.sid_model(0), SidModel::Unknown);

    // NTSC, any model; PSID-specific flag
    let mut f = Psid::new(tone_code(1000));
    f.flags = (2 << 2) | (3 << 4) | 2;
    let t = Tune::load(&f.bytes()).unwrap();
    assert_eq!((t.clock_speed, t.sid_model(0), t.compatibility), (Clock::Ntsc, SidModel::Any, Compatibility::Psid));

    // version 3 and 4: extra SID addresses, their models, invalid addresses ignored
    let mut f = Psid::new(tone_code(1000));
    f.version = 4;
    f.sid2 = 0x42; // $d420
    f.sid3 = 0xe0; // $de00
    f.flags = (1 << 4) | (2 << 6);
    let t = Tune::load(&f.bytes()).unwrap();
    assert_eq!(t.sid_chip_addresses, [0xd400, 0xd420, 0xde00]);
    assert_eq!(t.sid_models, [SidModel::Mos6581, SidModel::Mos8580, SidModel::Unknown]);
    for bad in [0x41u8, 0x43, 0x80, 0xdf, 0x00] {
        f.sid2 = bad;
        f.sid3 = bad;
        assert_eq!(Tune::load(&f.bytes()).unwrap().sid_chip_addresses, [0xd400], "{bad:#x}");
    }

    // songs 0 and a start song out of range are repaired
    let mut f = Psid::new(tone_code(1000));
    f.songs = 0;
    f.start = 9;
    let t = Tune::load(&f.bytes()).unwrap();
    assert_eq!((t.songs, t.start_song), (1, 1));
}

#[test]
fn rsid_rules() {
    let mut f = Psid::new(tone_code(1000));
    f.magic = b"RSID";
    f.load = 0;
    f.play = 0;
    let t = Tune::load(&f.bytes()).unwrap();
    assert_eq!((t.format, t.compatibility, t.song_speed), ("RSID", Compatibility::R64, SPEED_CIA_1A));
    // reserved fields must be 0
    for change in 0..3 {
        let mut g = Psid { magic: b"RSID", load: 0, play: 0, ..Psid::new(tone_code(1000)) };
        match change {
            0 => g.play = 0x1030,
            1 => g.speed = 1,
            _ => g.load = 0x1000,
        }
        assert!(Tune::load(&g.bytes()).is_err(), "{change}");
    }
    // version 1 is not RSID; init under a ROM is not allowed; nor a load below $07e8
    let mut g = Psid { magic: b"RSID", load: 0, play: 0, ..Psid::new(tone_code(1000)) };
    g.version = 1;
    assert!(Tune::load(&g.bytes()).is_err());
    let mut g = Psid { magic: b"RSID", load: 0, play: 0, ..Psid::new(tone_code(1000)) };
    g.init = 0xa000;
    assert!(Tune::load(&g.bytes()).is_err());
    let mut d = Psid { magic: b"RSID", load: 0, play: 0, init: 0x0400, ..Psid::new(tone_code(1000)) }.bytes();
    d[124] = 0x00;
    d[125] = 0x04;
    assert!(Tune::load(&d).is_err());
    // the BASIC flag: needs the BASIC ROM, refused by the player
    let mut g = Psid { magic: b"RSID", load: 0, play: 0, init: 0, ..Psid::new(tone_code(1000)) };
    g.flags = 2;
    assert_eq!(Tune::load(&g.bytes()).unwrap().compatibility, Compatibility::Basic);
    assert!(SidPlayer::open(&g.bytes(), RATE).is_err());
}

#[test]
fn malformed_files() {
    let good = Psid::new(tone_code(1000)).bytes();
    assert!(detect(&good));
    assert!(!detect(b"PSI"));
    assert!(!detect(b"FORM\0\0\0\0"));
    // every truncation: an error or a tune, never a panic
    for n in 0..good.len() {
        let _ = SidPlayer::open(&good[..n], RATE);
    }
    assert!(SidPlayer::open(&good[..4], RATE).is_err());
    assert!(SidPlayer::open(&good[..119], RATE).is_err());
    assert!(SidPlayer::open(&good[..124], RATE).is_err()); // no data
    // unknown versions
    for v in [0u16, 5, 0xffff] {
        let mut d = good.clone();
        d[4..6].copy_from_slice(&v.to_be_bytes());
        assert!(SidPlayer::open(&d, RATE).is_err());
    }
    // a data offset past the end of the file
    let mut d = good.clone();
    d[6..8].copy_from_slice(&0xfff0u16.to_be_bytes());
    assert!(SidPlayer::open(&d, RATE).is_err());
    // data that does not fit below $10000
    let mut f = Psid::new(vec![0x60; 0x2000]);
    f.load = 0xf000;
    f.init = 0xf000;
    assert!(SidPlayer::open(&f.bytes(), RATE).is_err());
    // the MUS flag
    let mut f = Psid::new(tone_code(1000));
    f.flags = 1;
    assert!(SidPlayer::open(&f.bytes(), RATE).is_err());
    // bad relocation information (inside the tune, under the ROMs)
    for reloc in [(0x10u8, 1u8), (0xa0, 1), (0xd0, 1), (0x03, 1), (0x9f, 4)] {
        let mut f = Psid::new(tone_code(1000));
        f.reloc = reloc;
        assert!(SidPlayer::open(&f.bytes(), RATE).is_err(), "{reloc:?}");
    }
    // no room for the driver
    let mut f = Psid::new(tone_code(1000));
    f.reloc = (0xff, 0);
    assert!(SidPlayer::open(&f.bytes(), RATE).is_err());
    // sample rates
    assert!(SidPlayer::open(&good, 7999).is_err());
    assert!(SidPlayer::open(&good, 192001).is_err());
    // bytes flipped all over the header: no panic, and what opens renders
    let mut seed = 0x2545f491u32;
    for _ in 0..300 {
        let mut d = good.clone();
        for _ in 0..3 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let at = 4 + (seed as usize >> 8) % 120;
            d[at] = seed as u8;
        }
        if let Ok(mut p) = SidPlayer::open(&d, 8000) {
            let mut out = [0i16; 512];
            p.render_i16(&mut out);
        }
    }
}

#[test]
fn plays_a_tone_at_pitch() {
    for (flags, model) in [(1 << 4, "6581"), (2 << 4, "8580"), (0, "6581")] {
        let mut f = Psid::new(tone_code(pal_freq(440.0)));
        f.flags = flags;
        let mut p = SidPlayer::open(&f.bytes(), RATE).unwrap();
        assert_eq!(p.sid_models(), [model]);
        assert_eq!(p.clock(), ("PAL", "VBI"));
        let pcm = render(&mut p, 1.5);
        let tail = &pcm[RATE as usize..];
        let hz = pitch(tail);
        assert!((hz - 440.0).abs() < 440.0 * 0.01, "{model}: {hz} Hz");
        let peak = tail.iter().map(|&s| (s as i32).abs()).max().unwrap();
        assert!(peak > 1000, "{model}: peak {peak}");
        // both channels carry the one chip
        assert!(pcm.chunks_exact(2).all(|x| x[0] == x[1]));
        assert!(!p.halted());
    }
}

#[test]
fn the_output_rate_is_honoured() {
    for rate in [8000u32, 22050, 44100, 48000, 96000] {
        let f = Psid::new(tone_code(pal_freq(440.0)));
        let mut p = SidPlayer::open(&f.bytes(), rate).unwrap();
        let mut out = vec![0i16; rate as usize * 2];
        assert_eq!(p.render_i16(&mut out), rate as usize);
        // one second of output is one second of the machine: 50 calls of play (PAL)
        let n = calls(&p);
        assert!((49..=51).contains(&n), "{rate}: {n} calls");
        let left: Vec<i16> = out.chunks_exact(2).flat_map(|x| [x[0], x[0]]).collect();
        let (mut below, mut cross) = (false, 0);
        for x in left.chunks_exact(2).skip(rate as usize / 2) {
            if x[0] < -500 {
                below = true;
            } else if x[0] > 500 && below {
                below = false;
                cross += 1;
            }
        }
        assert!((215..=225).contains(&cross), "{rate}: {cross} periods in half a second");
    }
}

#[test]
fn speeds_vbi_and_cia() {
    // PAL vertical blank: 50 Hz
    let f = Psid::new(tone_code(1000));
    let mut p = SidPlayer::open(&f.bytes(), RATE).unwrap();
    render(&mut p, 4.0);
    let n = calls(&p);
    assert!((199..=201).contains(&n), "PAL VBI: {n} calls in 4 s");

    // CIA 1 timer A as the driver sets it: 60 Hz
    let mut f = Psid::new(tone_code(1000));
    f.speed = 1;
    let mut p = SidPlayer::open(&f.bytes(), RATE).unwrap();
    assert_eq!(p.clock(), ("PAL", "CIA"));
    render(&mut p, 4.0);
    let n = calls(&p);
    assert!((239..=241).contains(&n), "PAL CIA: {n} calls in 4 s");

    // NTSC vertical blank: 60 Hz, and the machine's clock is the NTSC one
    let mut f = Psid::new(tone_code(1000));
    f.flags = 2 << 2;
    let mut p = SidPlayer::open(&f.bytes(), RATE).unwrap();
    assert_eq!(p.clock(), ("NTSC", "VBI"));
    render(&mut p, 4.0);
    let n = calls(&p);
    assert!((238..=241).contains(&n), "NTSC VBI: {n} calls in 4 s");

    // a tune that sets the CIA timer itself (100 Hz: 9852 cycles)
    let mut code = tone_code(1000);
    let at = code.iter().position(|&b| b == 0x60).unwrap();
    code.splice(at..at + 1, [0xa9, 0x7c, 0x8d, 0x04, 0xdc, 0xa9, 0x26, 0x8d, 0x05, 0xdc, 0x60]);
    code.drain(at + 11..at + 21); // ten of the padding NOPs
    assert_eq!(code[0x30], 0xee);
    let mut f = Psid::new(code);
    f.speed = 1;
    let mut p = SidPlayer::open(&f.bytes(), RATE).unwrap();
    render(&mut p, 4.0);
    let n = calls(&p);
    assert!((398..=402).contains(&n), "CIA at 100 Hz: {n} calls in 4 s");
}

#[test]
fn subsongs() {
    let mut f = Psid::new(tone_code(pal_freq(440.0)));
    f.songs = 3;
    f.start = 2;
    f.speed = 0b100;
    let mut p = SidPlayer::open(&f.bytes(), RATE).unwrap();
    assert_eq!((p.subsongs(), p.subsong()), (3, 1));
    render(&mut p, 0.5);
    // init got the song number in A
    assert_eq!(p.c64.peek_ram(0x2002), 1);
    assert_eq!(p.clock().1, "VBI");
    p.set_subsong(2).unwrap();
    assert_eq!((p.subsong(), p.clock().1), (2, "CIA"));
    render(&mut p, 1.0);
    assert_eq!(p.c64.peek_ram(0x2002), 2);
    let n = calls(&p);
    assert!((59..=61).contains(&n), "{n}");
    assert!(p.set_subsong(3).is_err());
    // a song started again is the same samples
    p.set_subsong(0).unwrap();
    let a = render(&mut p, 0.5);
    p.set_subsong(0).unwrap();
    let b = render(&mut p, 0.5);
    assert_eq!(a, b);
}

#[test]
fn deterministic() {
    let f = Psid::new(tone_code(pal_freq(261.63))).bytes();
    let a = render(&mut SidPlayer::open(&f, RATE).unwrap(), 2.0);
    let b = render(&mut SidPlayer::open(&f, RATE).unwrap(), 2.0);
    assert_eq!(a, b);
    // the size of the caller's blocks does not matter
    let mut p = SidPlayer::open(&f, RATE).unwrap();
    let mut c = vec![0i16; a.len()];
    let mut at = 0;
    let mut n = 2;
    while at < c.len() {
        let end = (at + n).min(c.len());
        p.render_i16(&mut c[at..end]);
        at = end;
        n = (n * 3 + 2) % 2000 & !1;
        n = n.max(2);
    }
    assert_eq!(a, c);
}

#[test]
fn multi_sid() {
    // init: a tone on the second SID at $d420 only
    let mut code = vec![
        0xa9, 0x0f, 0x8d, 0x38, 0xd4, // volume
        0xa9, 0x45, 0x8d, 0x20, 0xd4, 0xa9, 0x1d, 0x8d, 0x21, 0xd4, // freq
        0xa9, 0xf0, 0x8d, 0x26, 0xd4, 0xa9, 0x21, 0x8d, 0x24, 0xd4, 0x60,
    ];
    code.resize(0x30, 0xea);
    code.push(0x60);
    let mut f = Psid::new(code);
    f.version = 3;
    f.sid2 = 0x42;
    let mut p = SidPlayer::open(&f.bytes(), RATE).unwrap();
    assert_eq!(p.sid_models().len(), 2);
    let pcm = render(&mut p, 1.0);
    let tail = &pcm[RATE as usize..];
    // the second chip is on the right at full level, on the left at half
    let (l, r) = tail.chunks_exact(2).fold((0i64, 0i64), |(l, r), x| (l + (x[0] as i64).abs(), r + (x[1] as i64).abs()));
    assert!(r > 100_000 && l > 0, "{l} {r}");
    assert!(l < r, "{l} {r}");
}

#[test]
fn hostile_code_is_bounded() {
    // init never returns
    let mut f = Psid::new(vec![0x4c, 0x00, 0x10]);
    f.play = 0;
    let mut p = SidPlayer::open(&f.bytes(), RATE).unwrap();
    render(&mut p, 1.0);
    assert!(!p.halted());

    // a halt instruction in init: the tune is over
    let mut f = Psid::new(vec![0x02, 0xea]);
    f.play = 0;
    let mut p = SidPlayer::open(&f.bytes(), RATE).unwrap();
    render(&mut p, 0.5);
    assert!(p.halted());

    // random bytes as code, at every speed, with interrupts enabled by the driver
    let mut seed = 0x9e3779b9u32;
    for i in 0..40 {
        let code: Vec<u8> = (0..2000)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed >> 11) as u8
            })
            .collect();
        let mut f = Psid::new(code);
        f.speed = i & 1;
        f.play = if i & 2 != 0 { 0x1400 } else { 0 };
        if i & 4 != 0 {
            f.magic = b"RSID";
            f.load = 0;
            f.play = 0;
            f.speed = 0;
        }
        if let Ok(mut p) = SidPlayer::open(&f.bytes(), 8000) {
            let mut out = vec![0i16; 8000];
            p.render_i16(&mut out);
        }
    }
}
