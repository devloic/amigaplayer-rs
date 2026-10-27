//! Annotation layer: the JSON reader, and the loaders on the local mapping drafts (skipped when
//! the drafts are not present; they describe copyrighted games and are never committed).
use recomp::ir::annot::{self, Abi, Tag};
use recomp::ir::json::{parse, Json};
use std::path::Path;

#[test]
fn json_reader() {
    let j = parse(r#" {"a": [1, -2.5e1, true, null, "x\"é\n"], "b": {}} "#).unwrap();
    assert_eq!(j.get("a").unwrap().arr()[1], Json::Num(-25.0));
    assert_eq!(j.get("a").unwrap().arr()[4].str(), Some("x\"é\n"));
    assert!(j.get("b").unwrap().obj().unwrap().is_empty());
    assert!(parse("[1,]").is_err());
    assert!(parse("{} x").is_err());
}

const MAPS: &str = "/home/lolo/mydev";

fn present(p: &Path) -> bool {
    if !p.exists() {
        eprintln!("skipped: {} not present", p.display());
        return false;
    }
    true
}

#[test]
fn wings_map() {
    let p = Path::new(MAPS).join("wings-map/wings-map.json");
    if !present(&p) {
        return;
    }
    let s = annot::load_wings(&p).unwrap();
    eprintln!("wings: {} functions, {} named, {} overlay, {} structs, bases {:x?}", s.funcs.len(), s.named(),
        s.funcs.iter().filter(|f| f.tags.iter().any(|t| matches!(t, Tag::Overlay(_)))).count(), s.structs.len(), s.bases);
    eprintln!("wings tags: {} interrupt, {} callback, {} task, {} stub", s.tagged(&Tag::Interrupt), s.tagged(&Tag::Callback), s.tagged(&Tag::Task), s.tagged(&Tag::Stub));
    assert!(s.funcs.len() > 500 && s.named() > 20);
    assert_eq!(s.bases.first(), Some(&0x20000));
    // every key agrees with the absolute address for root functions
    for f in s.funcs.iter().filter(|f| f.key.section == 0) {
        assert_eq!(f.abs, Some(s.bases[0] + f.key.offset.unwrap()));
    }
    assert!(s.funcs.iter().all(|f| f.abi == Abi::ManxStack16));
    assert!(s.structs.iter().any(|t| t.name == "bolt_entry" && t.size == Some(12) && !t.fields.is_empty()));
}

#[test]
fn dotc_map() {
    let p = Path::new(MAPS).join("dotc-map/dotc-map.json");
    if !present(&p) {
        return;
    }
    let sets = annot::load_dotc(&p).unwrap();
    for s in &sets {
        eprintln!("dotc {}: {} functions, {} named, {} keep-interpreted, {} interrupt, {} callback, {} task, {} structs", s.program, s.funcs.len(), s.named(), s.tagged(&Tag::KeepInterpreted), s.tagged(&Tag::Interrupt), s.tagged(&Tag::Callback), s.tagged(&Tag::Task), s.structs.len());
        for f in &s.funcs {
            let (Some(a), Some(o)) = (f.abs, f.key.offset) else { panic!("{:?}", f.key) };
            assert_eq!(a, s.bases[f.key.section as usize] + o, "{} {:?}", s.program, f.key);
        }
    }
    let d = sets.iter().find(|s| s.program == "Defender").unwrap();
    assert!(d.funcs.len() > 400);
    assert!(sets.iter().filter(|s| s.program != "Defender").all(|s| s.funcs.iter().all(|f| f.abi == Abi::GreenHillsA6)));
    assert!(d.names().contains_key(&0x20000));
}

#[test]
fn sinbad_map() {
    let p = Path::new(MAPS).join("sinbad-map/sinbad-map.json");
    if !present(&p) {
        return;
    }
    let sets = annot::load_sinbad(&p, None).unwrap();
    let n: usize = sets.iter().map(|s| s.funcs.len()).sum();
    let roles = sets.iter().flat_map(|s| &s.funcs).filter(|f| f.note.is_some()).count();
    let stubs: usize = sets.iter().map(|s| s.tagged(&Tag::Stub)).sum();
    let tag = |t: Tag| sets.iter().map(|s| s.tagged(&t)).sum::<usize>();
    eprintln!("sinbad: {} programs, {} functions, {} with a role, {} stubs, {} interrupt, {} callback, {} task", sets.len(), n, roles, stubs, tag(Tag::Interrupt), tag(Tag::Callback), tag(Tag::Task));
    assert!(sets.len() >= 10 && n > 1000);
    // no bases in the map: offsets stay unknown until rebased
    assert!(sets.iter().flat_map(|s| &s.funcs).all(|f| f.key.offset.is_none()));
    let mut s = sets.into_iter().find(|s| s.program == "Sinbad").unwrap();
    s.rebase(&[0x20000]);
    assert!(s.funcs.iter().filter(|f| f.key.section == 0).all(|f| f.key.offset == Some(f.abs.unwrap() - 0x20000)));
}

#[test]
fn rr_notes_loader_and_ir_config() {
    let dir = std::env::temp_dir().join(format!("w4a-notes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("notes.txt");
    std::fs::write(&p, "# comment\n0:00006 anim_open (0:00006)\n1:0010 vbl_server interrupt server for level 3 # trailing\n0:0020 patcher self-modifying code\n").unwrap();
    let s = annot::load_rr_notes(&p, "rr", &[0x20000, 0x30000]).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(s.funcs.len(), 3);
    assert_eq!(s.funcs[0].abs, Some(0x20006));
    assert_eq!(s.funcs[0].note, None);
    assert_eq!(s.funcs[1].abs, Some(0x30010));
    assert!(s.funcs[1].tags.contains(&Tag::Interrupt));
    assert_eq!(s.entries_tagged(&Tag::KeepInterpreted), vec![0x20020]);
    assert_eq!(s.aliases().get(&0x20006).map(String::as_str), Some("anim_open"));
    let c = recomp::config::Config::parse("ir = true\nir_split = 50\nrr_notes = [\"a\"]\nkeep_interpreted = [\"0:10\"]\nguarded = [\"0:20\"]\n[[map]]\nfile = \"m.json\"\nformat = \"wings\"\n").unwrap();
    assert_eq!((c.ir, c.ir_split, c.maps.len(), c.guarded.len()), (Some(true), Some(50), 1, 1));
}
