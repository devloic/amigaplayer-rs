use crate::engine::Engine;
use std::path::Path;

fn put16(d: &mut [u8], at: usize, v: u16) {
    d[at..at + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut [u8], at: usize, v: u32) {
    d[at..at + 4].copy_from_slice(&v.to_be_bytes());
}

const SONG: usize = 0x34;
const BLOCKARR: usize = SONG + 0x314;
const BLOCK: usize = BLOCKARR + 4;
const LINES: usize = 8;
const SMPLARR: usize = BLOCK + 8 + LINES * 4 * 4;
const SAMPLE: usize = SMPLARR + 8;
const SMPLEN: usize = 1000;
const SYNTH: usize = SAMPLE + 6 + SMPLEN;

/// An MMD1 module, tempo 125, 3 ticks a row, one 8-line block: track 0 plays
/// C-2 with instrument 1 (a 1000-byte one-shot sample), track 1 note 1 with
/// instrument 2 (a synth sound on one 16-word waveform) at finetune +1
/// (command 15).
fn module() -> Vec<u8> {
    let mut d = vec![0u8; SYNTH + 0x11c + 32];
    d[0..4].copy_from_slice(b"MMD1");
    let n = d.len() as u32;
    put32(&mut d, 4, n);
    put32(&mut d, 8, SONG as u32);
    put32(&mut d, 0x10, BLOCKARR as u32);
    put32(&mut d, 0x18, SMPLARR as u32);
    for i in 0..2 {
        put16(&mut d, SONG + 8 * i + 2, 1); // no loop
        d[SONG + 8 * i + 6] = 64;
    }
    put16(&mut d, SONG + 0x1f8, 1); // blocks
    put16(&mut d, SONG + 0x1fa, 1); // song length
    put16(&mut d, SONG + 0x2fc, 125);
    d[SONG + 0x2ff] = 0x10; // volumes in hex
    d[SONG + 0x301] = 3;
    for t in 0..16 {
        d[SONG + 0x302 + t] = 64;
    }
    d[SONG + 0x312] = 64;
    d[SONG + 0x313] = 2;
    put32(&mut d, BLOCKARR, BLOCK as u32);
    put16(&mut d, BLOCK, 4);
    put16(&mut d, BLOCK + 2, LINES as u16 - 1);
    d[BLOCK + 8..BLOCK + 12].copy_from_slice(&[13, 1, 0, 0]);
    d[BLOCK + 12..BLOCK + 16].copy_from_slice(&[1, 2, 0x15, 1]);
    put32(&mut d, SMPLARR, SAMPLE as u32);
    put32(&mut d, SMPLARR + 4, SYNTH as u32);
    put32(&mut d, SAMPLE, SMPLEN as u32);
    for k in 0..SMPLEN {
        d[SAMPLE + 6 + k] = (k * 7) as u8;
    }
    put16(&mut d, SYNTH + 4, 0xffff);
    put16(&mut d, SYNTH + 0xe, 2);
    put16(&mut d, SYNTH + 0x10, 2);
    d[SYNTH + 0x12] = 1;
    d[SYNTH + 0x13] = 1;
    put16(&mut d, SYNTH + 0x14, 1);
    d[SYNTH + 0x16..SYNTH + 0x18].copy_from_slice(&[0x30, 0xff]); // volume 48, end
    d[SYNTH + 0x96..SYNTH + 0x98].copy_from_slice(&[0, 0xff]); // waveform 0, end
    put32(&mut d, SYNTH + 0x116, 0x11a);
    put16(&mut d, SYNTH + 0x11a, 16);
    d
}

#[test]
fn plays_a_sample_and_a_synth_sound() {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("mod.test"), module()).unwrap();
    let mut out = vec![0f32; 2 * 24000];
    e.render(&mut out);
    let log = e.m.bus.write_log.take().unwrap();
    let p = e.playing.as_ref().unwrap();
    assert_eq!(p.format, "MED");
    let base = e.m.deli.list.first().unwrap().0;
    let reg = |a: u32| {
        log.iter()
            .filter(|w| w.0 == a)
            .map(|w| w.1)
            .collect::<Vec<_>>()
    };
    let loc = |ch: u32| {
        let (hi, lo) = (reg(0xDFF0A0 + 16 * ch), reg(0xDFF0A2 + 16 * ch));
        hi.iter()
            .zip(lo)
            .map(|(&h, l)| (h as u32) << 16 | l as u32)
            .collect::<Vec<_>>()
    };
    // track 0 (on each turn of the song): the sample from its start, its
    // words, C-2's period; then the player's silent word with length 0, ten
    // lines after the DMA went on
    let l0 = loc(0);
    assert!(l0.len() >= 4 && l0.len() % 2 == 0);
    assert!(l0.chunks(2).all(|c| c == [base + SAMPLE as u32 + 6, l0[1]]));
    assert_ne!(l0[1], l0[0]);
    assert!(reg(0xDFF0A4).chunks(2).all(|c| c == [SMPLEN as u16 / 2, 0]));
    assert!(reg(0xDFF0A6).iter().all(|&p| p == 428));
    assert!(reg(0xDFF0A8).iter().all(|&v| v == 64));
    let on = log
        .iter()
        .position(|w| w.0 == 0xDFF096 && w.1 == 0x8003)
        .unwrap();
    let loop0 = log.iter().rposition(|w| w.0 == 0xDFF0A2).unwrap();
    assert!(loop0 > on && log[loop0].2 - log[on].2 >= 9 * crate::paula::LINE);
    // track 1: the synth sound's waveform, length then start, at finetune +1
    // two octaves down (4 * 850), at its volume program's 48
    assert!(loc(1).iter().all(|&a| a == base + (SYNTH + 0x11c) as u32));
    assert!(reg(0xDFF0B4).iter().all(|&l| l == 16));
    assert!(reg(0xDFF0B6).iter().all(|&p| p == 3400));
    assert!(reg(0xDFF0B8).iter().all(|&v| v == 48));
    // tracks 2 and 3: no note; their DMA off on the first tick, period 0
    assert!(reg(0xDFF0C6).iter().all(|&p| p == 0));
    assert!(log.iter().any(|w| w.0 == 0xDFF096 && w.1 == 8));
}

#[test]
fn finetune_octaves_below_c1() {
    let p = super::periods();
    let t = |ft: i8| &p[(ft as u8 & 15) as usize * super::TABLE..];
    // finetune 0: ProTracker's octave times 4 and 2
    assert_eq!(t(0)[..3], [3424, 3232, 3048]);
    assert_eq!(t(0)[12], 1712);
    // the others in equal-tempered steps from 4 times C-1
    assert_eq!(t(1)[..4], [3400, 3209, 3029, 2859]);
    assert_eq!(t(-8)[3], 3051);
    assert_eq!(t(-7)[0], 3588);
    assert_eq!(t(1)[super::C1..super::C1 + 2], [850, 802]);
}

/// A MED 2.10 file, tempo2 3 and no version chunk: instrument 1 at volume 40,
/// one 4-track block of 64 lines (C-2 with instrument 1 and command 0C20 on
/// track 0, line 0), one 8-byte sample.
fn med4() -> Vec<u8> {
    let mut d = b"MED\x04".to_vec();
    d.extend([0x80, 0x80]); // one mask byte: instrument 1
    d.extend([0x4f, 1, b'a', 40]); // volume only
    d.extend(1u16.to_be_bytes()); // blocks
    d.extend(1u16.to_be_bytes()); // song length
    d.push(0);
    d.extend(125u16.to_be_bytes());
    d.extend([0, 0x08]); // transpose; flags: samples in the file
    d.extend(3u16.to_be_bytes());
    d.extend([0; 20]);
    d.extend([64; 16]);
    d.push(64);
    // the block: lines 0-31 by masks (line 0), lines 32-63 empty
    d.extend([13, 4, 63, 0, 4, 0x05]);
    d.extend(0x8000_0000u32.to_be_bytes());
    d.extend(0x8000_0000u32.to_be_bytes());
    // track 0: note 0x0d, instrument 1; command C, argument 20
    d.extend([0x80, 0xd1, 0x8c, 0x20]);
    d.extend((1u64 << 62).to_be_bytes()); // the samples in the file: instrument 1
    d.extend(8u32.to_be_bytes());
    d.extend(0u16.to_be_bytes());
    d.extend([1, 2, 3, 4, 5, 6, 7, 8]);
    d
}

#[test]
fn converts_med4() {
    let c = super::med4::to_mmd0(&med4()).unwrap();
    assert_eq!(&c[0..4], b"MMD0");
    let be32 = |o: usize| u32::from_be_bytes(c[o..o + 4].try_into().unwrap()) as usize;
    let song = be32(8);
    assert_eq!(c[song + 6], 40);
    assert_eq!(c[song + 0x301], 6);
    let blk = be32(be32(0x10));
    assert_eq!(c[blk..blk + 5], [4, 63, 0x0d, 0x1c, 0x20]);
    let smp = be32(be32(0x18));
    assert_eq!(smp % 8, 0);
    assert_eq!(c[smp..smp + 14], [0, 0, 0, 8, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]);
    assert!(super::detect(&med4()));
}

// ---- OctaMED SoundStudio (MMD3) on the amplifier

const O_SONG: usize = 0x34;
const O_PSEQTAB: usize = O_SONG + 0x314;
const O_PSEQ: usize = O_PSEQTAB + 4;
const O_SECT: usize = O_PSEQ + 0x2a + 2;
const O_TRKVOL: usize = O_SECT + 2;
const O_TRKPAN: usize = O_TRKVOL + 8;
const O_BLOCKARR: usize = O_TRKPAN + 8;
const O_BLOCK: usize = O_BLOCKARR + 4;
const O_LINES: usize = 4;
const O_SMPLARR: usize = O_BLOCK + 8 + O_LINES * 5 * 4;
const O_SMP1: usize = O_SMPLARR + 8;
const O_LEN1: usize = 200;
const O_SMP2: usize = O_SMP1 + 6 + O_LEN1;
const O_FRAMES2: usize = 100;

/// An MMD3 module in mix mode, 5 tracks and channels, 125 BPM at 4 lines a beat
/// and 6 ticks a line, one 4-line block: track 0 plays note 25 of instrument 1
/// (a 200-byte 8-bit sample looping over its second half), track 4 note 25 of
/// instrument 2 (a 100-frame 16-bit stereo sample), both at volume 0C20 (32 of
/// 64, in the new volume command's hexadecimal).
fn oss_module() -> Vec<u8> {
    let mut d = vec![0u8; O_SMP2 + 6 + 4 * O_FRAMES2 + 16];
    d[0..4].copy_from_slice(b"MMD3");
    let n = d.len() as u32;
    put32(&mut d, 4, n);
    put32(&mut d, 8, O_SONG as u32);
    put32(&mut d, 0x10, O_BLOCKARR as u32);
    put32(&mut d, 0x18, O_SMPLARR as u32);
    // the 8-bit instrument loops over its second half (in words), both at volume 64
    put16(&mut d, O_SONG, 50);
    put16(&mut d, O_SONG + 2, 50);
    d[O_SONG + 6] = 64;
    d[O_SONG + 8 + 6] = 64;
    put16(&mut d, O_SONG + 0x1f8, 1); // blocks
    put16(&mut d, O_SONG + 0x1fa, 1); // sections
    put32(&mut d, O_SONG + 0x1fc, O_PSEQTAB as u32);
    put32(&mut d, O_SONG + 0x200, O_SECT as u32);
    put32(&mut d, O_SONG + 0x204, O_TRKVOL as u32);
    put16(&mut d, O_SONG + 0x208, 5); // tracks
    put16(&mut d, O_SONG + 0x20a, 1); // play sequences
    put32(&mut d, O_SONG + 0x20c, O_TRKPAN as u32);
    put16(&mut d, O_SONG + 0x214, 100);
    put16(&mut d, O_SONG + 0x216, 5); // channels
    put16(&mut d, O_SONG + 0x2fc, 125);
    d[O_SONG + 0x2ff] = 0x10; // volumes in hex
    d[O_SONG + 0x300] = 0xa3; // mix, BPM, 4 lines a beat
    d[O_SONG + 0x301] = 6;
    d[O_SONG + 0x312] = 64;
    d[O_SONG + 0x313] = 2;
    put32(&mut d, O_PSEQTAB, O_PSEQ as u32);
    put16(&mut d, O_PSEQ + 0x28, 1);
    put16(&mut d, O_PSEQ + 0x2a, 0);
    for t in 0..5 {
        d[O_TRKVOL + t] = 64;
    }
    put32(&mut d, O_BLOCKARR, O_BLOCK as u32);
    put16(&mut d, O_BLOCK, 5);
    put16(&mut d, O_BLOCK + 2, O_LINES as u16 - 1);
    d[O_BLOCK + 8..O_BLOCK + 12].copy_from_slice(&[25, 1, 0x0c, 0x20]);
    d[O_BLOCK + 8 + 16..O_BLOCK + 8 + 20].copy_from_slice(&[25, 2, 0x0c, 0x20]);
    put32(&mut d, O_SMPLARR, O_SMP1 as u32);
    put32(&mut d, O_SMPLARR + 4, O_SMP2 as u32);
    put32(&mut d, O_SMP1, O_LEN1 as u32);
    for k in 0..O_LEN1 {
        d[O_SMP1 + 6 + k] = k as u8;
    }
    put32(&mut d, O_SMP2, 2 * O_FRAMES2 as u32); // bytes a channel
    put16(&mut d, O_SMP2 + 4, 0x30); // 16-bit stereo
    for k in 0..O_FRAMES2 {
        put16(&mut d, O_SMP2 + 6 + 2 * k, (k as i16 * 256 + 512) as u16);
        put16(&mut d, O_SMP2 + 6 + 2 * O_FRAMES2 + 2 * k, (k as i16 * 256 - 512) as u16);
    }
    d
}

#[test]
fn plays_an_octamed_soundstudio_module_on_the_amplifier() {
    use crate::amplifier::LOG_BASE;
    let m = oss_module();
    assert!(!super::detect(&m), "the 4-channel replayer leaves MMD3 alone");
    assert!((super::OPEN_OSS.detect)(&m));
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("mod.test"), m).unwrap();
    let mut out = vec![0f32; 2 * 24000];
    e.render(&mut out);
    let log = e.m.bus.write_log.take().unwrap();
    assert_eq!(e.playing.as_ref().unwrap().format, "OctaMED SoundStudio");
    let base = e.m.deli.list.first().unwrap().0;
    let reg = |v: u32, r: u32| log.iter().filter(|w| w.0 == LOG_BASE | v << 4 | r).map(|w| w.1).collect::<Vec<_>>();
    let loc = |v: u32| {
        let (hi, lo) = (reg(v, 0), reg(v, 2));
        hi.iter().zip(lo).map(|(&h, l)| (h as u32) << 16 | l as u32).collect::<Vec<_>>()
    };
    // note 25 is index 24 of the frequency table (C-3, 4186 Hz): the period is
    // counted in NTSC clocks, 3579545 / 4186 = 855, on both tracks
    assert_eq!(super::mix::freq_table()[8][24], 4186);
    assert!(reg(0, 6).contains(&855) && reg(4, 6).contains(&855));
    // volume 32 of 64 on both, through the track and master volumes
    assert!(reg(0, 8).contains(&32) && reg(4, 8).contains(&32));
    // the 8-bit sample in place: from its start, then the loop of its second half
    let l0 = loc(0);
    // (the voices start on a word of silence)
    assert_eq!(l0[1], base + O_SMP1 as u32 + 6);
    assert_eq!(l0[2], base + O_SMP1 as u32 + 6 + 100);
    assert_eq!(reg(0, 4)[1..3], [100, 50]);
    // the 16-bit stereo sample became 8-bit mono where it lay: the mean of the
    // channels' high bytes (frame k: k plus 2 and minus 2)
    let at = base + O_SMP2 as u32 + 6;
    for k in [0u32, 1, 7, 99] {
        assert_eq!(e.m.bus.r8(at + k) as i8, k as i8, "frame {k}");
    }
    assert_eq!(loc(4)[1], at);
    assert_eq!(reg(4, 4)[1], O_FRAMES2 as u16 / 2);
    // five voices on the amplifier
    assert!(log.iter().all(|w| w.0 & !0xF != LOG_BASE | 5 << 4));
}
