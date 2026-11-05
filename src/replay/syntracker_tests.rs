//! Tests on the synthetic modules of the specification (tests/data/syntracker): the
//! recognition, the parse, a short render, and the whole write stream (address, value, order)
//! against the reference player's, by hash (the reference logs hold 17 modules; five of them
//! are kept here).

use super::*;
use m68k::CpuType;

const M1: &[u8] = include_bytes!("../../tests/data/syntracker/m1_synth.synmod");
const M2: &[u8] = include_bytes!("../../tests/data/syntracker/m2_scanmorph.synmod");
const M3: &[u8] = include_bytes!("../../tests/data/syntracker/m3_sample.synmod");
const M4: &[u8] = include_bytes!("../../tests/data/syntracker/m4_effects.synmod");
const M8: &[u8] = include_bytes!("../../tests/data/syntracker/m8_eglide.synmod");

/// Starts `data` and runs `ticks` ticks; returns the writes of the start and of each tick as
/// text tokens (LC values module-relative `M+off` or buffer-relative `B+off`) and whether the
/// song end was announced.
fn run(data: &[u8], ticks: usize) -> (Vec<String>, bool) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let base = m.alloc(data.len() as u32);
    m.bus.write_bytes(base, data);
    let mut r = new(data, base, &mut m).expect("recognised");
    let mut groups = Vec::new();
    let mut zero = None;
    for n in 0..=ticks {
        if n > 0 {
            m.bus.clock += DT_TICK;
            r.tick(&mut Hw { bus: &mut m.bus });
        }
        let log = m.bus.write_log.replace(Vec::new()).unwrap();
        let mut toks = Vec::new();
        let mut i = 0;
        while i < log.len() {
            let (a, v, _) = log[i];
            if a == 0xBFE001 {
                toks.push(format!("CIA={}", (v >> 1) & 1));
            } else if (0xDFF0A0..=0xDFF0DF).contains(&a) && a & 0xF == 0 {
                let x = (v as u32) << 16 | log[i + 1].1 as u32;
                let ch = (a - 0xDFF0A0) >> 4;
                let z = *zero.get_or_insert(x);
                if x >= base && x < base + data.len() as u32 {
                    toks.push(format!("LC{ch}=M+{:x}", x - base));
                } else {
                    toks.push(format!("LC{ch}=B+{:x}", x.wrapping_sub(z)));
                }
                i += 1;
            } else {
                toks.push(format!("{:03x}={v:04x}", a & 0xFFF));
            }
            i += 1;
        }
        groups.push(format!("{}|{}\n", n as i64 - 1, toks.join(" ")));
    }
    (groups, r.songend())
}

fn fnv(groups: &[String]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in groups.iter().flat_map(|g| g.bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[test]
fn detect_by_magic() {
    assert!(detect(M1));
    assert!(!detect(&M1[..16]));
    let mut d = M1.to_vec();
    d[0] = b's';
    assert!(!detect(&d));
    assert!(!detect(b"SYNTRACKER-SONG"));
    assert!(!detect(&[]));
}

#[test]
fn parse_header() {
    let mut m = Machine::new(CpuType::M68000);
    let base = m.alloc(M1.len() as u32);
    m.bus.write_bytes(base, M1);
    let r = new(M1, base, &mut m).unwrap();
    assert_eq!(r.format(), "SynTracker");
    assert_eq!(r.subsongs(), (0, 0, 0));
    assert!(!r.songend());
    assert_eq!(be16(M1, 0x10), 2, "two positions");
    // the data area follows the last pattern
    let hdr12 = be16(M1, 0x12) as usize;
    assert!(O_PATDATA + (hdr12 + 1) * 128 < M1.len());
}

#[test]
fn start_sequence_and_first_tick() {
    let (g, _) = run(M1, 2);
    assert_eq!(
        g[0],
        "-1|0a8=0000 0b8=0000 0c8=0000 0d8=0000 0a8=0000 0b8=0000 0c8=0000 0d8=0000 09a=4000 096=000f 096=000f \
         LC0=B+0 0a4=0010 0a6=03e8 0a8=0000 LC1=B+0 0b4=0010 0b6=03e8 0b8=0000 LC2=B+0 0c4=0010 0c6=03e8 0c8=0000 \
         LC3=B+0 0d4=0010 0d6=03e8 0d8=0000 096=800f 09e=00ff CIA=1\n"
    );
    assert_eq!(g[1], "0|0a6=0000 0b6=0000 0c6=0000 0d6=0000 0a8=0000 0b8=0000 0c8=0000 0d8=0000 096=800f\n");
}

#[test]
fn render_is_deterministic() {
    let a = run(M2, 60);
    let b = run(M2, 60);
    assert_eq!(fnv(&a.0), fnv(&b.0));
    assert!(!a.1);
}

/// The reference player's streams (hash of every tick's tokens, from its write logs).
#[test]
fn write_streams_equal_the_reference() {
    for (name, d, ticks, hash) in [
        ("m1_synth", M1, 384, 0xb310f9fb70a10bd9u64),
        ("m2_scanmorph", M2, 194, 0x7a571cde574f5122),
        ("m3_sample", M3, 194, 0x23e9494eec907777),
        ("m4_effects", M4, 400, 0xc750be5767b82249),
        ("m8_eglide", M8, 384, 0xe94a3428e74dd4cc),
    ] {
        let (g, _) = run(d, ticks);
        assert_eq!(fnv(&g), hash, "{name}");
    }
}

#[test]
fn song_end_at_the_position_wrap() {
    // two positions of 32 rows at speed 6: the wrap comes in the last row tick
    let (_, end) = run(M1, 383);
    assert!(!end);
    let (_, end) = run(M1, 384);
    assert!(end);
}

#[test]
fn damaged_input_does_not_panic() {
    let mut s = 1u32;
    for len in [0x700, 0xA14 + 128, 0xA14 + 200, M3.len() - 1] {
        let mut d = M3[..len].to_vec();
        for _ in 0..40 {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let at = 0x10 + (s >> 8) as usize % (d.len() - 0x10);
            d[at] = (s >> 24) as u8;
        }
        if detect(&d) {
            let _ = run(&d, 120);
        }
    }
}
