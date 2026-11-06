//! Tests on small synthetic modules built in code: the name rule, the decoder, the start and
//! interrupt write sequences and buffers (with the rules of the specification), the song end
//! and the restart, short files and malformed input.

use super::*;
use m68k::CpuType;

/// A pseudo-random byte stream.
fn bytes(n: usize, seed: u32) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            (x >> 24) as u8
        })
        .collect()
}

/// A machine with the file name set, and the player started on `data`.
fn start(name: &str, data: &[u8]) -> (Machine, Option<Box<dyn Replayer>>) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let f = m.deli.file_buf;
    m.bus.write_cstr(f, name);
    let r = new(data, 0, &mut m);
    (m, r)
}

/// The reference decoder of the specification's section 2, written out independently.
fn reference(d: &[u8], samples: usize) -> Vec<u8> {
    let (mut p, mut i) = (0i32, 0usize);
    let mut out = Vec::new();
    for k in 0..samples {
        let b = d.get(k / 2).copied().unwrap_or(0);
        let n = if k % 2 == 0 { b >> 4 } else { b & 15 };
        let m = (n & 7) as i32;
        let mut v = STEP[i] * (2 * m + 1);
        if n & 8 != 0 {
            v = -v;
        }
        p = (p + (v >> 3).clamp(-32768, 32767)).clamp(-32768, 32767);
        let adj = [-1, -1, -1, -1, 2, 4, 6, 8][m as usize];
        i = (i as i32 + adj).clamp(0, 88) as usize;
        out.push((p >> 8) as u8);
    }
    out
}

fn irq(r: &mut Box<dyn Replayer>, m: &mut Machine) {
    assert!(r.audio_irq_ch(0, &mut Hw { bus: &mut m.bus }));
}

fn ram(m: &Machine, a: u32, n: usize) -> Vec<u8> {
    m.bus.ram[a as usize..a as usize + n].to_vec()
}

#[test]
fn name_rule() {
    for ok in ["x.ADPCM", "x.adPcm", "x.ADPcm", "x.AdPCM", "a.b.ADPCM", "x.adPppcm", ".ADPCM"] {
        assert!(name_ok(ok), "{ok}");
    }
    for bad in ["x.adpcm", "x.ADPC", "xADPCM", "x.ADCM", "ADPCM", "", "x.adpcm.bak", "x.ADPPCM", "x.adppPcm", "m", "x.adpCm"] {
        assert!(!name_ok(bad), "{bad}");
    }
}

#[test]
fn detect_rules() {
    // without the file name, nothing is recognised
    set_name("");
    assert!(!detect(&bytes(600, 1)));
    set_name("x.ADPCM");
    assert!(detect(&bytes(600, 1)));
    assert!(detect(&[0]));
    assert!(!detect(&[]));
    let mut d = bytes(600, 1);
    d[..6].copy_from_slice(b"ADPCM2");
    assert!(!detect(&d));
    set_name("x.adpcm");
    assert!(!detect(&bytes(600, 1)));
    set_name("");
}

#[test]
fn decoder_rounds_towards_minus_infinity() {
    // step 7: code 0 gives +0, code 8 gives -1 (7 >> 3 = 0, -7 >> 3 = -1)
    let mut d = Dec::default();
    assert_eq!(d.code(0).0, 0);
    let mut d = Dec::default();
    assert_eq!(d.code(8).0, -1);
    // code 7 at step 7: 7 * 15 / 8 = 13, then the index goes to 8 (step 16: 16 * 15 / 8 = 30)
    let mut d = Dec::default();
    d.code(7);
    assert_eq!((d.p, d.i), (13, 8));
    d.code(7);
    assert_eq!(d.p, 43);
    // saturation at the top, the index clamped at 88
    let mut d = Dec { p: 32000, i: 88 };
    let (s, sat, clamp) = d.code(7);
    assert_eq!((d.p, s, sat, clamp), (32767, 127, true, true));
    let (_, _, clamp) = Dec { p: 0, i: 87 }.code(0);
    assert!(!clamp);
    let (_, _, clamp) = Dec { p: 0, i: 87 }.code(4);
    assert!(clamp);
    let (_, sat, _) = Dec { p: -32000, i: 88 }.code(15);
    assert!(sat);
}

#[test]
fn refuses_wrong_names_and_magic() {
    assert!(start("x.adpcm", &bytes(600, 2)).1.is_none());
    assert!(start("x.mod", &bytes(600, 2)).1.is_none());
    let mut d = bytes(600, 2);
    d[..4].copy_from_slice(b"ADPC");
    assert!(start("x.ADPCM", &d).1.is_none());
    assert!(start("x.ADPCM", &bytes(600, 2)).1.is_some());
}

#[test]
fn start_writes_and_first_buffer() {
    let d = bytes(1200, 3);
    let (m, r) = start("t.ADPCM", &d);
    let r = r.unwrap();
    assert_eq!(r.format(), "Adaptative Delta Pulse Code Modulation");
    assert!(!r.songend());
    let log = m.bus.write_log.clone().unwrap();
    let regs: Vec<(u32, u16)> = log.iter().map(|&(a, v, _)| (a, v)).collect();
    // volumes, locations (hi, lo), lengths, periods, interrupt enable, DMA
    let at = |i: usize| regs[i].0;
    assert_eq!([at(0), at(1), at(2), at(3)], [0xDFF0A8, 0xDFF0B8, 0xDFF0C8, 0xDFF0D8]);
    assert_eq!([regs[0].1, regs[1].1, regs[2].1, regs[3].1], [64, 64, 1, 1]);
    assert_eq!(regs.len(), 4 + 8 + 4 + 4 + 2);
    let buf = ((regs[4].1 as u32) << 16) | regs[5].1 as u32;
    for ch in 0..4 {
        let (hi, lo) = (regs[4 + 2 * ch], regs[5 + 2 * ch]);
        assert_eq!((hi.0, lo.0), (0xDFF0A0 + 16 * ch as u32, 0xDFF0A2 + 16 * ch as u32));
        assert_eq!(((hi.1 as u32) << 16) | lo.1 as u32, buf);
        assert_eq!(regs[12 + ch], (0xDFF0A4 + 16 * ch as u32, 0x200));
        assert_eq!(regs[16 + ch], (0xDFF0A6 + 16 * ch as u32, 125));
    }
    assert_eq!(regs[20], (0xDFF09A, 0x8080));
    assert_eq!(regs[21], (0xDFF096, 0x800F));
    assert_eq!(ram(&m, buf, 1024), reference(&d, 1024));
    // the gaps of the specification's start table
    let t: Vec<u64> = log.iter().map(|&(_, _, t)| t).collect();
    assert_eq!([t[1] - t[0], t[2] - t[1], t[3] - t[2]], [6, 8, 6]);
    assert_eq!([t[5] - t[4], t[6] - t[4], t[8] - t[6]], [0, 8, 8]);
    assert_eq!([t[12] - t[10], t[13] - t[12], t[16] - t[15], t[17] - t[16]], [18, 6, 8, 6]);
    assert_eq!([t[20] - t[19], t[21] - t[20]], [85, 10]);
}

#[test]
fn interrupts_alternate_buffers_end_pad_and_restart() {
    // 600 bytes: 1200 samples; the first buffer 1024, the second ends after 176
    let d = bytes(600, 4);
    let (mut m, r) = start("t.ADPCM", &d);
    let mut r = r.unwrap();
    let lc = |m: &Machine| {
        let log = m.bus.write_log.as_ref().unwrap();
        let v: Vec<u32> = log.iter().filter(|w| w.0 == 0xDFF0A0).map(|w| w.1 as u32).collect();
        let w: Vec<u32> = log.iter().filter(|w| w.0 == 0xDFF0A2).map(|w| w.1 as u32).collect();
        v.iter().zip(&w).map(|(a, b)| a << 16 | b).collect::<Vec<u32>>()
    };
    let want = reference(&d, 1200);
    irq(&mut r, &mut m);
    assert!(r.songend(), "the stream ended inside this buffer");
    let locs = lc(&m);
    assert_eq!(locs.len(), 2);
    assert_eq!(locs[1], locs[0] + 1024, "the other buffer");
    let b = ram(&m, locs[1], 1024);
    assert_eq!(b[..176], want[1024..]);
    assert!(b[176..].iter().all(|&x| x == 0), "zero padding");
    // the next one restarts the stream: the first buffer again, in the first buffer's place
    irq(&mut r, &mut m);
    let locs = lc(&m);
    assert_eq!(locs[2], locs[0]);
    // the stream restarted after the partial fill, so it decodes from its start
    assert_eq!(ram(&m, locs[2], 1024), want[..1024]);
    irq(&mut r, &mut m);
    let locs = lc(&m);
    assert_eq!(locs[3], locs[1]);
    assert_eq!(ram(&m, locs[3], 1024)[..176], want[1024..]);
}

#[test]
fn interrupt_write_sequence_and_timing() {
    let d = bytes(1024, 5);
    let (mut m, r) = start("t.ADPCM", &d);
    let mut r = r.unwrap();
    m.bus.write_log.as_mut().unwrap().clear();
    m.bus.clock += 1000;
    irq(&mut r, &mut m);
    let log = m.bus.write_log.clone().unwrap();
    let regs: Vec<(u32, u16)> = log.iter().map(|&(a, v, _)| (a, v)).collect();
    assert_eq!(regs[0], (0xDFF09A, 0x0080));
    assert_eq!(regs[1], (0xDFF09C, 0x0080));
    assert_eq!(regs[2].0, 0xDFF0A0);
    assert_eq!(*regs.last().unwrap(), (0xDFF09A, 0x8080));
    assert_eq!(regs.len(), 2 + 8 + 1);
    let t: Vec<u64> = log.iter().map(|&(_, _, t)| t).collect();
    // the interrupt starts 68 colour clocks after its request (here the clock when called)
    assert!(t[0] >= 1000 + IRQ_LATENCY);
    assert_eq!(t[1] - t[0], 8);
    // the four locations 8 apart, the enable 16 after the last
    assert_eq!([t[4] - t[2], t[6] - t[4], t[8] - t[6]], [8, 8, 8]);
    assert_eq!(t[10] - t[8], 16);
    // a full decode costs at least 63507 colour clocks
    assert!(t[2] - t[1] >= 63_507);
}

#[test]
fn exact_multiple_ends_in_a_silent_buffer() {
    // 1024 samples: init takes them all; the next fill is silence (cheap) and ends the song
    let d = bytes(512, 6);
    let (mut m, r) = start("t.ADPCM", &d);
    let mut r = r.unwrap();
    m.bus.write_log.as_mut().unwrap().clear();
    irq(&mut r, &mut m);
    assert!(r.songend());
    let log = m.bus.write_log.clone().unwrap();
    assert_eq!(log[2].2 - log[1].2, SILENT_COST);
    let a = ((log[2].1 as u32) << 16) | log[3].1 as u32;
    assert!(ram(&m, a, 1024).iter().all(|&x| x == 0));
}

#[test]
fn short_files_do_not_panic() {
    for n in [1usize, 2, 3, 100, 300, 511] {
        let d = bytes(n, 7);
        let (mut m, r) = start("t.ADPCM", &d);
        let mut r = r.unwrap();
        // the first buffer is the stream followed by the decoded zero bytes past the file
        let log = m.bus.write_log.clone().unwrap();
        let a = ((log[4].1 as u32) << 16) | log[5].1 as u32;
        let mut padded = d.clone();
        padded.resize(512, 0);
        let want = reference(&padded, 1024);
        assert_eq!(ram(&m, a, 1024), want, "{n}");
        // and the song never ends
        for _ in 0..4 {
            irq(&mut r, &mut m);
        }
        assert!(!r.songend(), "{n}");
    }
}

#[test]
fn odd_byte_is_not_played() {
    let d = bytes(601, 8);
    let (mut m, r) = start("t.ADPCM", &d);
    let mut r = r.unwrap();
    irq(&mut r, &mut m);
    let log = m.bus.write_log.clone().unwrap();
    let a = ((log[24].1 as u32) << 16) | log[25].1 as u32;
    let b = ram(&m, a, 1024);
    assert!(b[176..].iter().all(|&x| x == 0));
    assert_eq!(b[..176], reference(&d[..600], 1200)[1024..]);
}

#[test]
fn deterministic() {
    let d = bytes(5000, 9);
    let run = || {
        let (mut m, r) = start("t.ADPCM", &d);
        let mut r = r.unwrap();
        for _ in 0..6 {
            irq(&mut r, &mut m);
        }
        m.bus.write_log.clone().unwrap()
    };
    assert_eq!(run(), run());
}

#[test]
fn library_takes_it_by_its_name_only() {
    let d = bytes(3000, 10);
    let mut s = crate::Song::open("dir/t.ADPCM", &d, 48000).expect("played");
    assert_eq!(s.format(), "Adaptative Delta Pulse Code Modulation");
    let mut out = vec![0f32; 2 * 4800];
    s.render_f32(&mut out);
    assert!(out.iter().any(|&x| x != 0.0));
    assert!(crate::Song::open("t.adpcm", &d, 48000).is_err());
    assert!(crate::Song::open("t.bin", &d, 48000).is_err());
    // and the name does not stay behind for the next question
    assert_eq!(crate::detect(&d), None);
}
