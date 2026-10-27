//! Readable names (`recomp::names`): register tracking, naming rules, and that naming changes no
//! generated statement.
use recomp::names::{self, Given, Naming, Val};
use recomp::{discover, Func, Image, Options, Segment};
use std::collections::BTreeMap;

/// A raw image at $1000 from words.
fn image(words: &[u16]) -> Image {
    let mut img = Image::default();
    img.segs.push(Segment { name: "raw".into(), base: 0x1000, data: words.iter().flat_map(|w| w.to_be_bytes()).collect(), code: true });
    img
}

fn funcs(img: &Image, seeds: &[u32]) -> BTreeMap<u32, Func> {
    discover(img, seeds, &Options::default())
}

fn naming(img: &Image, seeds: &[u32], descriptions: &BTreeMap<u32, String>) -> (BTreeMap<u32, Func>, Naming) {
    let fs = funcs(img, seeds);
    let aliases = BTreeMap::new();
    let addr = |a: u32| format!("${:06x}", a);
    let g = Given { aliases: &aliases, descriptions, addr: &addr, crate_name: "t" };
    let n = names::derive(&fs, img, None, &g);
    (fs, n)
}

fn name_of(words: &[u16], seeds: &[u32], at: u32) -> String {
    let img = image(words);
    naming(&img, seeds, &BTreeMap::new()).1.name(at)
}

#[test]
fn register_names() {
    assert_eq!(names::custom_name(0x040), "BLTCON0");
    assert_eq!(names::custom_name(0x0A6), "AUD0PER");
    assert_eq!(names::custom_name(0x0D8), "AUD3VOL");
    assert_eq!(names::custom_name(0x0E4), "BPL2PTH");
    assert_eq!(names::custom_name(0x126), "SPR1PTL");
    assert_eq!(names::custom_name(0x144), "SPR0DATA");
    assert_eq!(names::custom_name(0x182), "COLOR01");
    assert_eq!(names::custom_name(0x1BE), "COLOR31");
    assert_eq!(names::custom_name(0x09A), "INTENA");
    // byte access to the low byte names the word register
    assert_eq!(names::hw_reg(0xDFF007).map(|r| r.1), Some("VHPOSR".to_string()));
    assert_eq!(names::hw_reg(0xBFEC01).map(|r| r.1), Some("CIAASDR".to_string()));
    assert_eq!(names::hw_reg(0xBFE001).map(|r| (r.1, r.2)), Some(("CIAAPRA".to_string(), names::Group::Input)));
    assert_eq!(names::hw_reg(0xBFD100).map(|r| (r.1, r.2)), Some(("CIABPRB".to_string(), names::Group::Disk)));
    assert_eq!(names::hw_reg(0xBFD001), None);
    assert_eq!(names::hw_reg(0x00DFEFFE), None);
    assert_eq!(names::custom_group(0x058), names::Group::Blitter);
    assert_eq!(names::custom_group(0x024), names::Group::Disk);
    assert_eq!(names::custom_group(0x084), names::Group::Copper);
}

#[test]
fn blitter_and_case_helpers() {
    assert_eq!(names::blit_kind(Some(0x09F0), Some(0)).0, "copy");
    assert_eq!(names::blit_kind(Some(0x0100), None).0, "clear");
    assert_eq!(names::blit_kind(Some(0x0FCA), None).0, "masked");
    assert_eq!(names::blit_kind(Some(0x0BCA), Some(0x0001)).0, "line");
    assert_eq!(names::snake("AllocMem"), "alloc_mem");
    assert_eq!(names::snake("LoadRGB4"), "load_rgb4");
    assert_eq!(names::snake("WBenchToFront"), "wbench_to_front");
    assert_eq!(names::snake("UnLoadSeg"), "unload_seg");
    assert_eq!(names::lvo_label(0, 198), "exec.AllocMem");
    assert_eq!(names::lvo_label(1, 30), "dos.Open");
}

#[test]
fn constant_base_and_blit_copy() {
    // lea $dff000,a5 ; move.w #$09f0,$40(a5) ; move.w #0,$42(a5) ; move.w #$0414,$58(a5) ; rts
    let w = [0x4BF9, 0x00DF, 0xF000, 0x3B7C, 0x09F0, 0x0040, 0x3B7C, 0x0000, 0x0042, 0x3B7C, 0x0414, 0x0058, 0x4E75];
    let img = image(&w);
    let fs = funcs(&img, &[0x1000]);
    let an = names::analyze(&fs, &img, None);
    let acc = an.accesses(0x1000).unwrap();
    let con0 = acc.values().find(|a| a.name == "BLTCON0").unwrap();
    assert_eq!((con0.r, con0.w, con0.values.iter().copied().collect::<Vec<_>>()), (0, 1, vec![0x09F0]));
    assert_eq!(acc.values().find(|a| a.name == "BLTSIZE").map(|a| a.w), Some(1));
    assert_eq!(name_of(&w, &[0x1000], 0x1000), "blit_copy_001000");
}

#[test]
fn entry_value_from_callers_and_vblank_poll() {
    // movea.l #$dff000,a6 ; bsr.w $100e ; rts ; nop
    // $100e: move.l 4(a6),d0 ; andi.l #$1ff00,d0 ; cmpi.l #$12c00,d0 ; bne.s $100e ; rts
    let w = [0x2C7C, 0x00DF, 0xF000, 0x6100, 0x0006, 0x4E75, 0x4E71, 0x202E, 0x0004, 0x0280, 0x0001, 0xFF00, 0x0C80, 0x0001, 0x2C00, 0x66EE, 0x4E75];
    let img = image(&w);
    let fs = funcs(&img, &[0x1000]);
    let an = names::analyze(&fs, &img, None);
    assert_eq!(an.entry_value(0x100E, 14), Some(Val::C(0xDFF000)));
    let vposr = an.accesses(0x100E).unwrap().values().find(|a| a.name == "VPOSR").unwrap();
    assert!(vposr.polled && vposr.r == 1 && vposr.w == 0);
    let (_, n) = naming(&img, &[0x1000], &BTreeMap::new());
    assert_eq!(n.name(0x100E), "wait_vblank_00100e");
    // the caller does no hardware access itself: it keeps its address name
    assert_eq!(n.name(0x1000), "f_001000");
    let doc = n.docs[&0x100E].join("\n");
    assert!(doc.contains("VPOSR r1 (polled)") && doc.contains("a6 = $dff000") && doc.contains("Callers: f_001000"), "{}", doc);
    assert!(n.docs[&0x1000].join("\n").contains("Callees: wait_vblank_00100e"));
}

#[test]
fn saved_registers_and_calls_outside_the_crate() {
    // lea $dff000,a6 ; movem.l d0-d7/a0-a6,-(a7) ; movea.l #0,a6 ; movem.l (a7)+,d0-d7/a0-a6
    // jsr $30000 (not in the image) ; move.l #$5000,$80(a6) ; rts
    let w = [0x4DF9, 0x00DF, 0xF000, 0x48E7, 0xFFFE, 0x2C7C, 0x0000, 0x0000, 0x4CDF, 0x7FFF, 0x4EB9, 0x0003, 0x0000, 0x2D7C, 0x0000, 0x5000, 0x0080, 0x4E75];
    let img = image(&w);
    let fs = funcs(&img, &[0x1000]);
    let an = names::analyze(&fs, &img, None);
    let acc = an.accesses(0x1000).unwrap();
    // the long write to COP1LCH/COP1LCL is one register
    assert_eq!(acc.values().map(|a| (a.name.as_str(), a.w)).collect::<Vec<_>>(), vec![("COP1LC", 1)]);
    assert_eq!(name_of(&w, &[0x1000], 0x1000), "copper_set_list_001000");
}

#[test]
fn clobbered_base_is_not_used() {
    // lea $dff000,a6 ; movea.l d0,a6 ; move.w #$8020,$96(a6) ; rts: a6 unknown at the store
    let w = [0x4DF9, 0x00DF, 0xF000, 0x2C40, 0x3D7C, 0x8020, 0x0096, 0x4E75];
    let img = image(&w);
    let fs = funcs(&img, &[0x1000]);
    let an = names::analyze(&fs, &img, None);
    assert!(an.accesses(0x1000).unwrap().is_empty());
    assert_eq!(name_of(&w, &[0x1000], 0x1000), "f_001000");
}

#[test]
fn interrupt_vector_wins_over_hardware() {
    // move.l #$100c,$6c.w ; rts ; nop ; $100c: move.w #$20,$dff09c ; rte
    let w = [0x21FC, 0x0000, 0x100C, 0x006C, 0x4E75, 0x4E71, 0x33FC, 0x0020, 0x00DF, 0xF09C, 0x4E73];
    assert_eq!(name_of(&w, &[0x1000, 0x100C], 0x100C), "irq_level3_00100c");
    // without the installer, the RTE makes it an interrupt handler
    assert_eq!(name_of(&w, &[0x100C], 0x100C), "irq_handler_00100c");
    // and the hardware rule alone says what it does
    let img = image(&w);
    let fs = funcs(&img, &[0x100C]);
    let an = names::analyze(&fs, &img, None);
    assert_eq!(names::hw_name(an.accesses(0x100C).unwrap(), 0, false).unwrap().stem, "irq_ack");
}

#[test]
fn exec_and_library_bases() {
    // movea.l 4.w,a6 ; move.l #$100,d0 ; moveq #0,d1 ; jsr -198(a6) ; rts
    let w = [0x2C78, 0x0004, 0x203C, 0x0000, 0x0100, 0x7200, 0x4EAE, 0xFF3A, 0x4E75];
    assert_eq!(name_of(&w, &[0x1000], 0x1000), "alloc_mem_001000");
    // movea.l 4.w,a6 ; lea name(pc),a1 ; moveq #0,d0 ; jsr -552(a6) (OpenLibrary) ; move.l d0,$2000
    // bsr.w $1020 ; rts ; ... $1020: movea.l $2000,a6 ; jsr -30(a6) (dos Open) ; rts
    // $1030: "dos.library"
    let mut w = vec![0x2C78, 0x0004, 0x43FA, 0x002A, 0x7000, 0x4EAE, 0xFDD8, 0x23C0, 0x0000, 0x2000, 0x6100, 0x000A, 0x4E75];
    w.resize(0x10, 0x4E71);
    w.extend([0x2C79, 0x0000, 0x2000, 0x4EAE, 0xFFE2, 0x4E75, 0x4E71, 0x4E71]);
    w.extend([0x646F, 0x732E, 0x6C69, 0x6272, 0x6172, 0x7900]);
    let img = image(&w);
    let fs = funcs(&img, &[0x1000]);
    let an = names::analyze(&fs, &img, None);
    assert_eq!(an.os_calls(0x1020).into_iter().collect::<Vec<_>>(), vec![("dos.Open".to_string(), 1)]);
    let (_, n) = naming(&img, &[0x1000], &BTreeMap::new());
    assert_eq!(n.name(0x1020), "file_open_001020");
    assert_eq!(n.name(0x1000), "open_library_001000");
}

#[test]
fn random_and_math() {
    // move.w $dff006,d0 ; mulu.w #$41c6,d0 ; eor.w d1,d0 ; rts
    assert_eq!(name_of(&[0x3039, 0x00DF, 0xF006, 0xC0FC, 0x41C6, 0xB340, 0x4E75], &[0x1000], 0x1000), "random_001000");
    // move.l d0,d2 ; mulu.w d1,d0 ; add.l d2,d0 ; rts
    assert_eq!(name_of(&[0x2400, 0xC0C1, 0xD082, 0x4E75], &[0x1000], 0x1000), "math_mul_001000");
    // a leaf that reads memory is not arithmetic only
    assert_eq!(name_of(&[0x2010, 0xC0C1, 0x4E75], &[0x1000], 0x1000), "f_001000");
}

#[test]
fn given_names_win_and_keep_the_address() {
    assert_eq!(names::given_name("anim_open_views (0:00006)").as_deref(), Some("anim_open_views"));
    assert_eq!(names::given_name("main: the game loop").as_deref(), Some("main"));
    assert_eq!(names::given_name("blit_copy_04a2c6").as_deref(), Some("blit_copy_04a2c6"));
    assert_eq!(names::given_name("Main loop of the game"), None);
    assert_eq!(names::given_name("unreferenced f_001000"), None);
    assert_eq!(names::with_addr("my_blit", 0x4a2c6), "my_blit_04a2c6");
    assert_eq!(names::with_addr("my_blit_04a2c6", 0x4a2c6), "my_blit_04a2c6");
    assert_eq!(names::with_addr("3d-draw", 0x1000), "f_3d_draw_001000");
    // A names file (an edited names.txt fed back) overrides the derived name.
    let w = [0x4BF9, 0x00DF, 0xF000, 0x3B7C, 0x09F0, 0x0040, 0x3B7C, 0x0414, 0x0058, 0x4E75];
    let img = image(&w);
    let mut given = BTreeMap::new();
    given.insert(0x1000, "draw_tile".to_string());
    let (_, n) = naming(&img, &[0x1000], &given);
    assert_eq!(n.name(0x1000), "draw_tile_001000");
    assert!(n.reasons[&0x1000].1.contains("derived: blit_copy"), "{:?}", n.reasons[&0x1000]);
    // names.txt round trip: the file's lines are `<address> <name>  # reason`
    let line = n.file.lines().find(|l| l.starts_with("$001000")).unwrap();
    assert_eq!(line.split_whitespace().nth(1), Some("draw_tile_001000"));
    let mut back = BTreeMap::new();
    let dir = std::env::temp_dir().join(format!("recomp-names-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("names.txt");
    std::fs::write(&p, n.file.replace("draw_tile_001000", "edited_001000")).unwrap();
    recomp::config::read_names(p.to_str().unwrap(), &[0x1000], &mut back).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let (_, n2) = naming(&img, &[0x1000], &back);
    assert_eq!(n2.name(0x1000), "edited_001000");
}

/// Strips what naming adds (doc lines, the alias) and maps names back to `f_xxxxxx`.
fn unname(code: &str, n: &Naming) -> String {
    let mut out = String::new();
    let mut skip = false;
    for l in code.lines() {
        if skip {
            skip = false;
            continue;
        }
        if l == "#[doc(hidden)]" {
            skip = true;
            continue;
        }
        if l.starts_with("/// Name:") || l.starts_with("/// Hardware:") || l.starts_with("/// OS calls:") || l.starts_with("/// At entry") || l.starts_with("/// Callers:") || l.starts_with("/// Callees:") {
            continue;
        }
        out.push_str(l);
        out.push('\n');
    }
    for (&e, name) in &n.names {
        out = out.replace(name.as_str(), &recomp::fn_name(e));
    }
    out.trim_end().to_string()
}

#[test]
fn naming_changes_no_statement() {
    // the vblank caller/callee pair: a direct Rust call between named functions
    let w = [0x2C7C, 0x00DF, 0xF000, 0x6100, 0x0006, 0x4E75, 0x4E71, 0x202E, 0x0004, 0x0280, 0x0001, 0xFF00, 0x0C80, 0x0001, 0x2C00, 0x66EE, 0x4E75];
    let img = image(&w);
    let (fs, n) = naming(&img, &[0x1000], &BTreeMap::new());
    let stubs = BTreeMap::new();
    let opts = Options::default();
    let mode = recomp::emit::Mode::Func { direct: true };
    let plain: BTreeMap<u32, String> = fs.keys().map(|&e| (e, format!("crate::{}", recomp::fn_name(e)))).collect();
    let named: BTreeMap<u32, String> = fs.keys().map(|&e| (e, format!("crate::{}", n.name(e)))).collect();
    for f in fs.values() {
        let a = recomp::emit_func(f, mode, &plain, &stubs, &opts, None);
        let b = recomp::emit_func_named(f, mode, &named, &stubs, &opts, None, Some(&n));
        assert_eq!(unname(&b, &n), a.trim_end());
        if n.name(f.entry) != recomp::fn_name(f.entry) {
            assert!(b.contains(&format!("pub use self::{} as {};", n.name(f.entry), recomp::fn_name(f.entry))));
        }
    }
    assert!(recomp::emit_func_named(&fs[&0x1000], mode, &named, &stubs, &opts, None, Some(&n)).contains("crate::wait_vblank_00100e"));
    // the same from the IR
    let prog = recomp::ir::cfg::build_program(&fs, &img);
    let ex = recomp::ir::emit::Extras { naming: Some(n.clone()), ..Default::default() };
    for f in prog.funcs.values() {
        let a = recomp::ir::emit::emit_func(f, mode, &plain, &stubs, &opts, None, 120);
        let b = recomp::ir::emit::emit_func_with(f, mode, &named, &stubs, &opts, None, 120, &ex);
        assert_eq!(unname(&b, &n), a.trim_end());
    }
}

#[test]
fn config_names_key() {
    use recomp::config::Config;
    let c = Config::parse("names = true\nir = true\n").unwrap();
    assert!(c.names.enabled() && c.names.table().is_empty());
    let c = Config::parse("[names]\n\"0:00006\" = \"anim_open_views\"\n").unwrap();
    assert!(!c.names.enabled());
    assert_eq!(c.names.table().get("0:00006").map(|s| s.as_str()), Some("anim_open_views"));
    assert!(!Config::parse("").unwrap().names.enabled());
}
