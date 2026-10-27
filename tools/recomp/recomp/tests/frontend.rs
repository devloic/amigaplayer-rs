//! Front-end tests on small hand-encoded programs: generic jump-table idioms, hints from a config,
//! coverage validation.
use recomp::config::Config;
use recomp::*;

fn image(words: &[u16]) -> Image {
    let data = words.iter().flat_map(|w| w.to_be_bytes()).collect();
    let mut img = Image::default();
    img.segs.push(Segment { name: "raw".into(), base: 0x1000, data, code: true });
    img
}

/// cmp.w #3,d0 ; b<cond>.s def ; add.w d0,d0 ; move.w tab(pc,d0.w),d0 ; jmp tab(pc,d0.w)
/// tab: dc.w 6,8,10 ; rts ; rts ; rts ; def: rts
fn switch(bcc: u16) -> Image {
    image(&[0x0C40, 0x0003, bcc | 0x16, 0xD040, 0x303B, 0x0006, 0x4EFB, 0x0002, 6, 8, 10, 0x4E75, 0x4E75, 0x4E75, 0x4E75])
}

fn opts(p: Profile) -> Options {
    Options { profile: p, ..Options::default() }
}

#[test]
fn generic_rel16_table_with_bhs_bound() {
    let f = discover(&switch(0x6400), &[0x1000], &opts(Profile::Generic));
    let f = &f[&0x1000];
    assert_eq!(f.jump_tables, vec![(0x100c, vec![0x1016, 0x1018, 0x101a])]);
    assert!(f.unresolved_jumps.is_empty());
}

#[test]
fn generic_bhi_bound_reads_one_more_entry() {
    // With `bhi` the bound admits 4 entries; the 4th word is an opcode (odd target): rejected.
    let f = discover(&switch(0x6200), &[0x1000], &opts(Profile::Generic));
    let f = &f[&0x1000];
    assert!(f.jump_tables.is_empty());
    assert_eq!(f.unresolved_jumps, vec![0x100c]);
}

#[test]
fn no_bound_no_table() {
    // Same code with the cmp replaced by two nops: nothing proves the index range.
    let mut img = switch(0x6400);
    img.segs[0].data[0..4].copy_from_slice(&[0x4E, 0x71, 0x4E, 0x71]);
    let f = discover(&img, &[0x1000], &opts(Profile::Generic));
    assert_eq!(f[&0x1000].unresolved_jumps, vec![0x100c]);
}

#[test]
fn config_hint_resolves_table() {
    let img = switch(0x6400);
    let c = Config::parse("profile = \"asm\"\n[[hint]]\nat = 0x100c\ntable = \"$1010\"\ncount = 2\nformat = \"rel16\"\n").unwrap();
    let (at, ts) = c.hints[0].resolve(&img, &[0x1000]).unwrap();
    assert_eq!((at, ts.clone()), (0x100c, vec![0x1016, 0x1018]));
    let mut o = opts(Profile::Manx); // Manx's own recogniser needs 4 instructions: the hint wins anyway
    o.hints.insert(at, ts);
    let f = discover(&img, &[0x1000], &o);
    assert_eq!(f[&0x1000].hinted, vec![(0x100c, 2)]);
    assert!(f[&0x1000].leaders.contains(&0x1018));
}

#[test]
fn config_rejects_unknown_keys() {
    assert!(Config::parse("profil = \"manx\"\n").is_err());
}

#[test]
fn coverage_plan() {
    // lea $1010,a0 ; jmp (a0) ; nop ; nop ; rts  (the jmp target is unknown statically)
    let img = image(&[0x41F9, 0x0000, 0x1010, 0x4ED0, 0x4E71, 0x4E71, 0x4E71, 0x4E71, 0x4E75]);
    let funcs = discover(&img, &[0x1000], &Options::default());
    assert_eq!(funcs[&0x1000].unresolved_jumps, vec![0x1006]);
    let hits = coverage::parse("001010 001006\n001011\n002000\n001000\n").unwrap();
    let p = coverage::plan(&img, &hits, &funcs);
    assert_eq!(p.accepted, 1);
    assert_eq!(p.known, 1);
    assert_eq!(p.rejected.len(), 2);
    assert_eq!(p.targets.get(&0x1006), Some(&vec![0x1010]));
    let mut o = Options::default();
    o.hints = p.targets.clone();
    let after = discover(&img, &[0x1000], &o);
    assert!(after[&0x1000].leaders.contains(&0x1010));
    assert!(after[&0x1000].unresolved_jumps.is_empty());
}
