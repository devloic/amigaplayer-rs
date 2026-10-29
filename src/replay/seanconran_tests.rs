use super::*;

fn acts(a: &[Act]) -> String {
    let v: Vec<String> = a
        .iter()
        .map(|a| match a.t {
            Tgt::Hw(h) => format!("[\"hw\",{h},{}]", a.val),
            Tgt::Img(o) => format!("[\"store\",{o},{}]", a.val),
        })
        .collect();
    format!("[{}]", v.join(","))
}

fn opt<T: std::fmt::Display>(x: Option<T>) -> String {
    x.map_or("null".into(), |v| v.to_string())
}

/// The layout as JSON, in the field names of the specification's layout table.
fn dump(d: &[u8], l: &Layout) -> String {
    let mut o = format!("{{\"init\":{},\"play\":{},\"subs\":{},\"songs\":{},\"d2\":{},\"d3\":{},", l.init, l.play, l.subs, opt(l.songs), l.d2, l.d3);
    o += &format!("\"posstart\":{},", l.posstart.map_or("null".into(), |p| format!("{p:?}")));
    o += &format!(
        "\"irq\":{},",
        l.irq.map_or("null".into(), |p| format!("[{}]", p.iter().map(|(a, b)| format!("{{\"lc\":{a},\"len\":{b}}}")).collect::<Vec<_>>().join(",")))
    );
    let s = &l.stub;
    let wr: Vec<String> = s.writes.iter().map(|(a, v)| format!("[{a},{v}]")).collect();
    o += &format!("\"stop\":{{\"flag\":{},\"set\":{},\"test\":\"{}\",\"writes\":[{}]}},", s.flag, s.set, if s.bmi { "bmi" } else { "beq" }, wr.join(","));
    let f = &l.fin;
    o += &format!("\"final\":{{\"mv\":{},\"mvsub\":{},\"vt\":{},\"vols\":{:?}", f.mv, f.mvsub, f.vt, f.vols);
    if let Some(fd) = &f.fade {
        o += &format!(",\"fade\":{},\"fc\":{},\"fc_reload\":{}", fd.fade, fd.fc, w(d, fd.reload));
    }
    o += "},";
    let mut hw = Vec::new();
    let mut st = Vec::new();
    let (mut sw, mut mc) = ("null".to_string(), "null".to_string());
    for e in &l.initev {
        match e {
            InitEv::Hw(a, v) => hw.push(format!("[{a},{v}]")),
            InitEv::Store(a, s, v) => st.push(format!("[{a},{s},{v}]")),
            InitEv::SubWord(a) => sw = a.to_string(),
            InitEv::MasterCopy(a, b) => mc = format!("[{a},{b}]"),
            InitEv::Led => {}
        }
    }
    o += &format!("\"initinfo\":{{\"hw\":[{}],\"stores\":[{}],\"subword\":{sw},\"mastercopy\":{mc}}},", hw.join(","), st.join(","));
    let mut vs = Vec::new();
    for v in &l.voices {
        let e = &v.eff;
        let mut cmds = Vec::new();
        for (k, c) in v.cmds.iter().enumerate() {
            let s = match c {
                Cmd::SetB(a) => format!("[\"SETB\",{{\"addr\":{a}}}]"),
                Cmd::SetPtr { cell, scratch } => format!("[\"SETPTR\",{{\"addr\":{cell},\"scratch\":{scratch}}}]"),
                Cmd::Call { ret, scratch } => format!("[\"CALL\",{{\"flag\":{},\"ret\":{ret},\"scratch\":{scratch}}}]", opt(v.flag)),
                Cmd::Ret { flag, ret } => format!("[\"RET\",{{\"flag\":{flag},\"ret\":{ret}}}]"),
                Cmd::Sample { scratch, extra } => format!("[\"SAMPLE\",{{\"scratch\":{scratch},\"extra\":{}}}]", acts(extra)),
                Cmd::SampleRec { scratch } => format!("[\"SAMPLEREC\",{{\"scratch\":{scratch}}}]"),
                Cmd::MasterRel { mv } => format!("[\"MASTERREL\",{{\"mv\":{mv}}}]"),
                Cmd::Event { pre, note, cnt, pat, rest, .. } => {
                    let kind = match (note.is_some(), pre.is_empty(), rest) {
                        (true, true, _) => "NOTE",
                        (true, false, _) => "TRIG",
                        (false, _, true) => "REST",
                        (false, true, false) => "DUR",
                        _ => "RETRIG",
                    };
                    format!("[\"{kind}\",{{\"cnt\":{cnt},\"pat\":{pat},\"note\":{},\"pre\":{}}}]", opt(*note), acts(pre))
                }
                Cmd::NextPat => "[\"NEXTPAT\",{}]".into(),
                Cmd::Bogus => "[\"BOGUS\",{}]".into(),
            };
            cmds.push(format!("\"{}\":{s}", 4 * k));
        }
        let n = &v.np;
        let neg = match n.neg {
            NegKind::Loop => "loop",
            NegKind::StopFf => "stop_ff",
            NegKind::StopNeg => "stop_neg",
            NegKind::StopNow => "stop_now",
            NegKind::Hang => "hang",
            NegKind::None => "none",
        };
        vs.push(format!(
            "{{\"cnt\":{},\"cntmode\":\"{}\",\"cut\":{},\"cutacts\":{},\"pat\":{},\"np\":{{\"double\":{},\"neg\":\"{neg}\",\"negat\":{},\"pos\":{},\"stop\":{},\"stopdirect\":{},\"tab\":{}}},\"eff\":{{\"double\":{},\"eff\":{},\"mask\":{},\"note\":{},\"penvp\":{},\"penvs\":{},\"prs\":{},\"shift\":{},\"venvp\":{},\"venvs\":{},\"volb\":{},\"volidx\":{},\"vrs\":{}}},\"cmds\":{{{}}}}}",
            v.cnt,
            if v.neg_form { "neg" } else { "cmp1" },
            v.cut,
            acts(&v.cutacts),
            v.pat,
            n.double,
            opt(n.negat),
            n.pos,
            opt(n.stop),
            n.direct,
            n.tab,
            e.double,
            e.eff,
            opt(e.mask),
            e.note,
            e.penvp,
            e.penvs,
            e.prs,
            e.shift,
            e.venvp,
            e.venvs,
            e.volb,
            e.volidx,
            e.vrs,
            cmds.join(",")
        ));
    }
    o += &format!("\"voices\":[{}]}}", vs.join(","));
    o
}

/// `SCR_FILES=a:b:.. SCR_OUT=file cargo test --release seanconran::tests::layouts -- --ignored`:
/// the scanned layout of each file as one line `name<TAB>json` (`null` when the scan fails).
#[test]
#[ignore]
fn layouts() {
    let (Ok(files), Ok(out)) = (std::env::var("SCR_FILES"), std::env::var("SCR_OUT")) else { return };
    let mut o = String::new();
    for f in files.split(':') {
        let d = std::fs::read(f).unwrap();
        let name = std::path::Path::new(f).file_name().unwrap().to_string_lossy().to_string();
        let j = if header(&d) { layout(&d).map_or("null".into(), |l| dump(&d, &l)) } else { "nohdr".into() };
        o += &format!("{name}\t{j}\n");
    }
    std::fs::write(out, o).unwrap();
}

// ------------------------------------------------- the engine on a synthetic image

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

const RUN: usize = 0x3F1;
const MV: usize = 0x3F0;
const FLAG: usize = 0x3F2;
const LIST: usize = 0x700;
const TAB: usize = 0x720;

/// Voice `v`'s cells: 0x300 + 0x40 v + the offsets below.
fn cell(v: usize, k: usize) -> usize {
    0x300 + 0x40 * v + k
}

/// A synthetic image (a period table of 0x100 + n at 0, a volume table of 2x at 0x200,
/// envelopes, the pattern `pat` at 0x600 and the position list `list`) and a layout
/// whose cells are placed by hand: the scan is checked against the corpus instead.
fn synth(pat: &[u8], list: &[u8], neg0: NegKind) -> (Vec<u8>, Layout) {
    let mut d = vec![0u8; 0x800];
    for n in 0..0x80 {
        put16(&mut d, 2 * n, 0x100 + n as u16);
        put16(&mut d, 0x200 + 2 * n, 2 * n as u16);
    }
    d[0x500..0x503].copy_from_slice(&[10, 0x80, 1]);
    d[0x510..0x513].copy_from_slice(&[0, 0x80, 1]);
    d[0x600..0x600 + pat.len()].copy_from_slice(pat);
    d[LIST..LIST + list.len()].copy_from_slice(list);
    put16(&mut d, TAB, 0x600);
    let mut initev = vec![InitEv::Store(RUN, 1, 0xFF), InitEv::Store(MV, 1, 15), InitEv::Store(FLAG, 1, 0xFF), InitEv::Hw(0xDFF096, 0x000F)];
    let mut voices = Vec::new();
    for v in 0..4 {
        for (k, val) in [(10, 0xFF), (11, 0xFF), (22, 0xFF)] {
            initev.push(InitEv::Store(cell(v, k), 1, val));
        }
        initev.push(InitEv::Store(cell(v, 12), 4, VIRT + 0x500));
        initev.push(InitEv::Store(cell(v, 24), 4, VIRT + 0x510));
        // lsr.w #1,d1
        put16(&mut d, cell(v, 34), 0xE249);
        let ev = |note: bool, rest: bool| Cmd::Event { pre: Vec::new(), note: note.then_some(cell(v, 33)), cnt: cell(v, 0), pat: cell(v, 2), rest, look: 0 };
        voices.push(Voice {
            cnt: cell(v, 0),
            neg_form: true,
            cut: cell(v, 1),
            cutacts: vec![Act { t: Tgt::Hw(0xDFF096), size: 2, val: 1 << v }],
            pat: cell(v, 2),
            cmds: vec![ev(true, false), ev(false, false), Cmd::NextPat, ev(false, true)],
            np: NextPat { pos: cell(v, 6), tab: TAB, double: false, neg: if v == 0 { neg0 } else { NegKind::Loop }, start: 0, negat: None, stop: None, direct: false },
            eff: Eff {
                eff: cell(v, 10),
                vrs: cell(v, 11),
                venvs: cell(v, 12),
                venvp: cell(v, 16),
                volb: cell(v, 20),
                mask: None,
                volidx: cell(v, 21),
                prs: cell(v, 22),
                penvs: cell(v, 24),
                penvp: cell(v, 28),
                note: cell(v, 32),
                double: false,
                shift: cell(v, 34),
            },
            flag: None,
        });
    }
    let lay = Layout {
        init: 0,
        play: RUN - 1,
        run: RUN,
        led: None,
        d2: 12,
        d3: 8,
        voices,
        fin: Final { fade: None, mv: MV, mvsub: 15, vt: 0x200, vols: [cell(0, 21), cell(1, 21), cell(2, 21), cell(3, 21)], led: false },
        stub: Stub { at: FLAG - 1, flag: FLAG, set: 0, bmi: false, writes: vec![(0xDFF096, 0x000F)] },
        initev,
        subs: 1,
        songs: None,
        posstart: Some([LIST; 4]),
        irq: None,
        mega: false,
    };
    (d, lay)
}

/// Plays `ticks` ticks; the log of each tick's writes and whether the song ended.
fn run(d: Vec<u8>, lay: Layout, ticks: usize) -> (Vec<Vec<(u32, u16)>>, bool) {
    let mut e = crate::engine::Engine::new(48000.0);
    e.m.bus.write_log = Some(Vec::new());
    let mut s = SeanConran { lay, pristine: d.clone(), img: d, base: 0x2_0000, sub: 0, started: false, songend: false, armed: [false; 4] };
    let mut out = Vec::new();
    for _ in 0..ticks {
        let mut hw = Hw { bus: &mut e.m.bus };
        s.tick(&mut hw);
        let l = e.m.bus.write_log.replace(Vec::new()).unwrap();
        out.push(l.iter().map(|w| (w.0, w.1)).filter(|w| w.0 != 0xBFE001).collect());
    }
    (out, s.songend)
}

fn regs(t: &[(u32, u16)], a: u32) -> Vec<u16> {
    t.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
}

#[test]
fn notes_envelopes_volumes_and_the_cut() {
    // note 4 for 3 ticks, then the next position (the list loops)
    let (d, lay) = synth(&[0x00, 4, 3, 0x08], &[0, 0, 0xFF, 2], NegKind::Loop);
    let (t, end) = run(d, lay, 6);
    // start-up, then the first tick: the period table's entry 4 (word 2) shifted by 1,
    // the envelope's volume 10 through the volume table (2x)
    assert_eq!(regs(&t[0], 0xDFF096), [0x000F]);
    assert_eq!(regs(&t[0], 0xDFF0A6), [0x102 >> 1]);
    assert_eq!(regs(&t[0], 0xDFF0A8), [20]);
    // ticks 2 and 3: effects only (the envelopes loop on their 0x80 marker)
    assert_eq!(regs(&t[1], 0xDFF0A6), [0x81]);
    assert_eq!(regs(&t[2], 0xDFF0B8), [20]);
    // tick 4: the counter reaches 0 with a command next: the cut, no PER
    assert_eq!(regs(&t[3], 0xDFF096), [1, 2, 4, 8]);
    assert!(regs(&t[3], 0xDFF0A6).is_empty());
    assert_eq!(regs(&t[3], 0xDFF0A8), [20]);
    // tick 5: the next position, the note again
    assert_eq!(regs(&t[4], 0xDFF0A6), [0x81]);
    assert!(!end);
}

#[test]
fn a_duration_before_a_note_does_not_cut() {
    // note, then a duration command: the lookahead sees a byte below 8, no cut
    let (d, lay) = synth(&[0x00, 6, 2, 0x04, 2, 0x08], &[0, 0, 0xFF, 2], NegKind::Loop);
    let (t, _) = run(d, lay, 4);
    assert!(t.iter().all(|t| regs(t, 0xDFF096).iter().all(|&v| v == 0x000F)));
    assert_eq!(regs(&t[2], 0xDFF0A6), [0x103 >> 1]);
}

#[test]
fn voice_0_ends_the_song_at_its_list_end() {
    // a loop build: the song end on voice 0's loop back, the music goes on
    // (a note of 1 tick: the cut in tick 2, the next position in tick 3)
    let (d, lay) = synth(&[0x00, 4, 1, 0x08], &[0, 0xFF, 2], NegKind::Loop);
    let (t, end) = run(d, lay, 4);
    assert!(end);
    assert_eq!(regs(&t[1], 0xDFF096), [1, 2, 4, 8]);
    assert_eq!(regs(&t[2], 0xDFF0A6), [0x81]);
    // a stop build: the stub's writes once, then silence
    let (d, lay) = synth(&[0x00, 4, 1, 0x08], &[0, 0xFF, 0xFF], NegKind::StopFf);
    let (t, end) = run(d, lay, 5);
    assert!(end);
    assert_eq!(t[2], [(0xDFF096, 0x000F)]);
    assert!(t[3].is_empty() && t[4].is_empty());
}

#[test]
fn recognises_the_period_table_starts() {
    let mut d = vec![0u8; 0x400];
    put32(&mut d, 0, 0x0FFF_0FE2);
    put32(&mut d, 4, 0x0FC4_0FA7);
    put32(&mut d, 8, 0x0F8B_0F6E);
    assert!(header(&d));
    let mut e = d.clone();
    put32(&mut e, 0, 0x1000_0FE2);
    assert!(header(&e));
    // a sound-effect bank
    put32(&mut e, 292 + 20, 0x7F7F_7F7F);
    assert!(!header(&e));
    let mut f = vec![0u8; 0x400];
    for (k, v) in [0x0F1C_0F0E, 0x0F00_0EF2, 0x0EE4_0ED6].into_iter().enumerate() {
        put32(&mut f, 4 * k, v);
    }
    assert!(header(&f));
    put16(&mut f, 452 + 254, 0xFFFF);
    assert!(!header(&f));
    // and without the code, no layout
    assert!(!detect(&d));
}
