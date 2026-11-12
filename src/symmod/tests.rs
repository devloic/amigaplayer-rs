use super::*;

/// A chunk: its kind, then a length-prefixed body.
fn chunk(out: &mut Vec<u8>, kind: i32, body: &[u8]) {
    out.extend_from_slice(&kind.to_be_bytes());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
}

/// A small module: `channels` channels, one position of `rows` rows, the events given as
/// (command, note, param, instrument) row by row, and one 8-bit instrument.
fn build(channels: u32, rows: u16, events: &[(u8, i8, u8, u8)], sample: &[i8], looped: bool) -> Vec<u8> {
    let mut d = b"SymM".to_vec();
    d.extend_from_slice(&1u32.to_be_bytes());
    d.extend_from_slice(&(-1i32).to_be_bytes());
    d.extend_from_slice(&channels.to_be_bytes());
    d.extend_from_slice(&(-2i32).to_be_bytes());
    d.extend_from_slice(&(rows as u32).to_be_bytes());
    d.extend_from_slice(&(-6i32).to_be_bytes());
    d.extend_from_slice(&100u32.to_be_bytes());
    let mut pos = vec![0u8; 32];
    pos[4..6].copy_from_slice(&1u16.to_be_bytes()); // loop count
    pos[12..14].copy_from_slice(&rows.to_be_bytes()); // length
    pos[14..16].copy_from_slice(&6u16.to_be_bytes()); // speed
    chunk(&mut d, -10, &pos);
    let mut ev = Vec::new();
    for i in 0..channels as usize * rows as usize {
        let (c, n, p, s) = events.get(i).copied().unwrap_or((0, -1, 0, 0));
        ev.extend_from_slice(&[c, n as u8, p, s]);
    }
    chunk(&mut d, -13, &ev);
    let mut ins = vec![0u8; 256];
    ins[0] = b'a';
    if looped {
        ins[128] = 4; // TYPE_LOOP
        ins[129] = 0; // loop start 0 %
        ins[130] = 100; // loop length 100 %
    }
    ins[128 + 6] = 100; // volume
    chunk(&mut d, -14, &ins);
    let mut seq = vec![0u8; 32];
    seq[2..4].copy_from_slice(&1u16.to_be_bytes()); // one position
    seq[16 + 6..16 + 8].copy_from_slice(&(-1i16).to_be_bytes()); // end of the list
    chunk(&mut d, -15, &seq);
    let raw: Vec<u8> = sample.iter().map(|&x| x as u8).collect();
    chunk(&mut d, -11, &raw);
    d
}

fn square() -> Vec<i8> {
    (0..512).map(|i| if i % 64 < 32 { 90 } else { -90 }).collect()
}

fn song() -> Vec<u8> {
    // two channels, a note on each and a slide on the second row
    let ev = [(0, 24, 100, 0), (0, 31, 100, 0), (0, -1, 0, 0), (2, -1, 4, 0), (0, 12, 60, 0), (0, -1, 0, 0)];
    build(2, 8, &ev, &square(), true)
}

fn render_all(p: &mut Symmod, max_frames: usize) -> Vec<i16> {
    let mut out = Vec::new();
    let mut buf = vec![0i16; 4096];
    while out.len() / 2 < max_frames {
        let n = p.render(&mut buf);
        out.extend_from_slice(&buf[..n * 2]);
        if n < buf.len() / 2 {
            break;
        }
    }
    out
}

#[test]
fn plays_a_built_module() {
    let d = song();
    assert!(test(&d));
    let m = load(&d).expect("loads");
    assert_eq!(m.channels, 2);
    assert_eq!(m.tracker, "Symphonie");
    let mut p = Symmod::new(&d, 48000).unwrap();
    let a = render_all(&mut p, 48000 * 10);
    assert!(p.ended(), "the song ends at the end of its order list");
    // 8 rows of speed 6 at tempo 124: 48 ticks of 48000*5/(124*2) frames
    let spt = 48000 * 5 * 10000 / (1240000 * 2);
    assert_eq!(a.len() / 2, 48 * spt);
    assert!(p.duration_ms() > 0);
    // both sides play, the left louder at first (its note is on channel 0)
    let peak_l = a.iter().step_by(2).map(|x| x.unsigned_abs()).max().unwrap();
    let peak_r = a.iter().skip(1).step_by(2).map(|x| x.unsigned_abs()).max().unwrap();
    assert!(peak_l > 1000 && peak_r > 1000, "{peak_l} {peak_r}");
    // the same again after a restart, and from a new player
    p.start();
    assert_eq!(render_all(&mut p, 48000 * 10), a);
    let mut q = Symmod::new(&d, 48000).unwrap();
    assert_eq!(render_all(&mut q, 48000 * 10), a);
}

#[test]
fn refuses_what_is_not_a_module() {
    assert!(!test(b""));
    assert!(!test(b"SymM"));
    let mut d = song();
    d[15] = 0; // no channels
    assert!(!test(&d));
    assert!(Symmod::new(&d, 48000).is_none());
    // a header alone
    assert!(Symmod::new(&song()[..16], 48000).is_none());
}

#[test]
fn truncated_modules_do_not_panic() {
    let d = song();
    for n in 0..d.len() {
        if let Some(mut p) = Symmod::new(&d[..n], 48000) {
            render_all(&mut p, 48000 * 5);
        }
    }
}

#[test]
fn mangled_modules_do_not_panic() {
    let base = song();
    let mut x = 0x2545_F491_4F6C_DD1Du64;
    let mut rnd = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for _ in 0..3000 {
        let mut d = base.clone();
        for _ in 0..1 + rnd() % 8 {
            let i = 16 + (rnd() as usize % (d.len() - 16));
            d[i] = rnd() as u8;
        }
        if let Some(mut p) = Symmod::new(&d, 48000) {
            render_all(&mut p, 48000 * 3);
        }
    }
    // events of every command with random parameters
    for _ in 0..400 {
        let ev: Vec<(u8, i8, u8, u8)> = (0..32).map(|_| ((rnd() % 30) as u8, (rnd() % 100) as i8 - 10, rnd() as u8, (rnd() % 3) as u8)).collect();
        let d = build(2, 16, &ev, &square(), rnd() & 1 == 0);
        if let Some(mut p) = Symmod::new(&d, 44100) {
            render_all(&mut p, 44100 * 20);
        }
    }
}

#[test]
fn oversized_claims_are_bounded() {
    // a packed chunk that says it unpacks to 4 GiB
    let mut d = song();
    let mut packed = b"PACK\xFF\xFF".to_vec();
    packed.extend_from_slice(&u32::MAX.to_be_bytes());
    packed.extend_from_slice(&[1, 255, 0, 0, 0, 0, 0xFF]);
    chunk(&mut d, -16, &packed);
    assert!(Symmod::new(&d, 48000).is_some());
    // 256 channels in a tiny file, and a track length past the limit
    let mut d = song();
    d[12..16].copy_from_slice(&256u32.to_be_bytes());
    let _ = Symmod::new(&d, 48000);
    let mut d = song();
    d[20..24].copy_from_slice(&5000u32.to_be_bytes());
    assert!(Symmod::new(&d, 48000).is_none());
    // a chunk length past the end of the file
    let mut d = song();
    d.extend_from_slice(&(-13i32).to_be_bytes());
    d.extend_from_slice(&u32::MAX.to_be_bytes());
    let _ = Symmod::new(&d, 48000);
}

/// The modules of the archive, when they are there (they are not part of the repository).
#[test]
fn archive_modules() {
    let dir = std::path::Path::new("/media/lolo/kingston2tb/claude-scratch/deliplayerrust/dl-modules/Symphonie");
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(d) = std::fs::read(e.path()) else { continue };
        if !test(&d) {
            continue;
        }
        let mut p = Symmod::new(&d, 48000).unwrap_or_else(|| panic!("{:?} does not load", e.path()));
        let a = render_all(&mut p, 48000 * 4);
        assert!(a.iter().any(|&x| x.unsigned_abs() > 500), "{:?} is silent", e.path());
        let mut q = Symmod::new(&d, 48000).unwrap();
        assert_eq!(render_all(&mut q, 48000 * 4), a, "{:?} is not deterministic", e.path());
    }
}

#[test]
#[ignore]
fn dump() {
    let path = std::env::var("SYM").unwrap();
    let d = std::fs::read(path).unwrap();
    let m = load(&d).unwrap();
    println!("ch {} tempo {} pats {} order {:?}", m.channels, m.tempo, m.patterns.len(), &m.order[..m.order.len().min(20)]);
    for (i, s) in m.samples.iter().enumerate() {
        if s.data.len() == 0 {
            continue;
        }
        let n = s.data.len();
        let rms = ((0..n).map(|k| (s.data.at16(k) as f64).powi(2)).sum::<f64>() / n as f64).sqrt();
        println!("smp {i} len {} loop {} {}-{} c5 {} gv {} cue {:x} rms {rms:.0}", s.data.len(), s.loop_on, s.loop_start, s.loop_end, s.c5, s.gv, s.cue);
    }
    for (i, z) in m.zxx.iter().enumerate() {
        if !matches!(z, load::Macro::Unused) {
            println!("Z{:02X} {z:?}", i + 0x80);
        }
    }
    for (i, s) in m.instruments.iter().enumerate() {
        if let Some(s) = s {
            println!("ins {i} smp {} gv {} nodsp {} {}", s.sample, s.gv, s.no_dsp, s.name);
        }
    }
    let p = &m.patterns[m.order[0].unwrap()];
    for r in 0..p.rows.min(16) {
        let cells: Vec<String> = p.cells[r * m.channels..(r + 1) * m.channels].iter().map(|c| format!("{:3} {:2} {:?}{:02x} {:?}{:02x}", c.note, c.instr, c.volcmd, c.vol, c.cmd, c.param)).collect();
        println!("{r:2} {}", cells.join(" | "));
    }
}
