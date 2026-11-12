//! The archive readers on small archives made with LHa for UNIX 1.14i, Info-ZIP, GNU tar,
//! gzip and xz from three synthetic files (no module data: see `files`), and on damaged
//! copies of them.

use super::*;

/// The files the fixtures hold (made by the same formulas).
fn files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("mods/tune.mod", (0..3000u32).map(|i| ((i * 7 + i / 13) % 256) as u8).collect()),
        ("smp.tune", b"smp ".repeat(200)),
        ("Instruments/bell", (0..777u32).map(|i| (i % 251) as u8).collect()),
    ]
}

fn check(a: &Archive, fmt: Format) {
    assert_eq!(a.format, fmt);
    assert!(a.skipped.is_empty(), "{:?}", a.skipped);
    assert!(!a.single);
    let mut want = files();
    want.sort();
    let mut got: Vec<(&str, Vec<u8>)> = a.entries.iter().map(|e| (e.name.as_str(), e.data.clone())).collect();
    got.sort();
    assert_eq!(got, want);
}

macro_rules! fixture {
    ($n:literal) => {
        include_bytes!(concat!("fixtures/", $n)).as_slice()
    };
}

const LHA: &[(&str, &[u8])] = &[
    ("t-o5-h0.lha", fixture!("t-o5-h0.lha")),
    ("t-o5-h1.lha", fixture!("t-o5-h1.lha")),
    ("t-o5-h2.lha", fixture!("t-o5-h2.lha")),
    ("t-o6-h1.lha", fixture!("t-o6-h1.lha")),
    ("t-o7-h1.lha", fixture!("t-o7-h1.lha")),
    ("t-o7-h2.lha", fixture!("t-o7-h2.lha")),
    ("t-z-h0.lha", fixture!("t-z-h0.lha")),
    ("t-dirs.lha", fixture!("t-dirs.lha")),
];

const OTHERS: &[(&str, &[u8], Format)] = &[
    ("t-stored.zip", fixture!("t-stored.zip"), Format::Zip),
    ("t-deflate.zip", fixture!("t-deflate.zip"), Format::Zip),
    ("t-zip64.zip", fixture!("t-zip64.zip"), Format::Zip),
    ("t.tar.gz", fixture!("t.tar.gz"), Format::Gzip),
    ("t.tar.xz", fixture!("t.tar.xz"), Format::Xz),
];

#[test]
fn lha_methods_and_header_levels() {
    for (n, d) in LHA {
        assert_eq!(format(d), Some(Format::Lha), "{n}");
        let a = extract(n, d, &Limits::default()).unwrap_or_else(|e| panic!("{n}: {e}"));
        check(&a, Format::Lha);
    }
}

#[test]
fn zip_tar_gzip_xz() {
    for (n, d, f) in OTHERS {
        assert_eq!(format(d), Some(*f), "{n}");
        let a = extract(n, d, &Limits::default()).unwrap_or_else(|e| panic!("{n}: {e}"));
        check(&a, *f);
    }
}

#[test]
fn built_in_memory() {
    let f = files();
    let refs: Vec<(&str, &[u8])> = f.iter().map(|(n, d)| (*n, d.as_slice())).collect();
    check(&extract("x.zip", &testdata::stored_zip(&refs), &Limits::default()).unwrap(), Format::Zip);
    check(&extract("x.zip", &testdata::deflated_zip(&refs), &Limits::default()).unwrap(), Format::Zip);
    check(&extract("x.tar", &testdata::tar(&refs), &Limits::default()).unwrap(), Format::Tar);
    check(&extract("x.tgz", &testdata::gzip(&testdata::tar(&refs), None), &Limits::default()).unwrap(), Format::Gzip);
}

#[test]
fn single_file_wrappers() {
    let tune = files()[0].1.clone();
    for (n, d, f) in [("Tune.MOD.gz", fixture!("tune.mod.gz"), Format::Gzip), ("Tune.MOD.xz", fixture!("tune.mod.xz"), Format::Xz)] {
        let a = extract(n, d, &Limits::default()).unwrap();
        assert_eq!(a.format, f);
        assert!(a.single);
        assert_eq!(a.entries.len(), 1);
        // gzip keeps the name ("tune.mod" here); xz does not: the archive's name without .xz
        assert_eq!(a.entries[0].name, if f == Format::Gzip { "tune.mod" } else { "Tune.MOD" });
        assert_eq!(a.entries[0].data, tune);
        // the decrunchers undo them too
        assert_eq!(crate::decrunch::decrunch(d.to_vec()), tune);
        let (p, bytes) = crate::unpack::unpack(d).unwrap().unwrap();
        assert_eq!(p.name(), f.name());
        assert_eq!(bytes, tune);
    }
    // a gzip without a name in it
    let a = extract("dir/x.mod.gz", &testdata::gzip(b"abc", None), &Limits::default()).unwrap();
    assert_eq!(a.entries[0].name, "x.mod");
}

#[test]
fn amiga_lha_names_end_at_the_comment() {
    // a level-0 header whose name field carries the file's comment after a NUL, as Amiga
    // LhA writes it: the fixture's first header, renamed (the CRC covers the data, not the name)
    let d = LHA[0].1;
    let old = d[21] as usize;
    let name = b"Instruments/bell\0a comment";
    let mut x = vec![0u8; 2];
    x.extend_from_slice(&d[2..21]);
    x.push(name.len() as u8);
    x.extend_from_slice(name);
    x.extend_from_slice(&d[22 + old..]);
    // header size (byte 0) counts the bytes after the first two up to the data
    x[0] = (d[0] as usize - old + name.len()) as u8;
    let sum: u32 = x[2..2 + x[0] as usize].iter().map(|&b| b as u32).sum();
    x[1] = sum as u8;
    let a = extract("c.lha", &x, &Limits::default()).unwrap();
    assert_eq!(a.entries[0].name, "Instruments/bell", "{a:?}");
}

#[test]
fn zip_names_in_cp437() {
    assert_eq!(CP437_HIGH.chars().count(), 128);
    assert_eq!(zip_text(b"Mot\x94rhead", 0), "Motörhead");
    assert_eq!(zip_text("Motörhead".as_bytes(), 0x800), "Motörhead");
}

#[test]
fn not_archives() {
    for d in [&b""[..], b"PK", b"plain text, nothing more", &crate::testdata::minimal_mod()] {
        assert_eq!(format(d), None);
        assert!(extract("x", d, &Limits::default()).is_err());
    }
}

#[test]
fn names_are_cleaned() {
    assert_eq!(clean_name("../../etc/passwd").as_deref(), Some("etc/passwd"));
    assert_eq!(clean_name("/abs/./x").as_deref(), Some("abs/x"));
    assert_eq!(clean_name("DH0:music\\mods\\x.mod").as_deref(), Some("music/mods/x.mod"));
    assert_eq!(clean_name("..").as_deref(), None);
    assert_eq!(clean_name("").as_deref(), None);
    let z = testdata::stored_zip(&[("../evil", b"x"), ("ok/../../y", b"y"), ("..", b"z")]);
    let a = extract("z.zip", &z, &Limits::default()).unwrap();
    let names: Vec<&str> = a.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["evil", "ok/y"]);
    assert_eq!(a.skipped.len(), 1);
}

#[test]
fn limits_skip_entries() {
    let lim = Limits { max_entry: 1000, max_total: 10_000, max_entries: 10 };
    let (n, d) = LHA[1];
    let a = extract(n, d, &lim).unwrap();
    assert_eq!(a.entries.len(), 2, "{a:?}");
    assert!(a.skipped.iter().any(|s| s.name == "mods/tune.mod" && s.reason.contains("larger")));
    let lim = Limits { max_entry: 1 << 20, max_total: 1600, max_entries: 10 };
    for (n, d, _) in OTHERS {
        let a = extract(n, d, &lim).unwrap();
        assert!(a.entries.iter().map(|e| e.data.len()).sum::<usize>() <= 1600, "{n}");
        assert!(!a.skipped.is_empty(), "{n}");
    }
    let lim = Limits { max_entries: 1, ..Limits::default() };
    let a = extract(LHA[0].0, LHA[0].1, &lim).unwrap();
    assert_eq!((a.entries.len(), a.skipped.len()), (1, 2));
    // a wrapper whose content is over the cap is refused
    let lim = Limits { max_entry: 100, ..Limits::default() };
    assert!(extract("t.gz", fixture!("tune.mod.gz"), &lim).is_err());
    assert!(extract("t.xz", fixture!("tune.mod.xz"), &lim).is_err());
}

#[test]
fn companions_and_find() {
    let a = extract(LHA[0].0, LHA[0].1, &Limits::default()).unwrap();
    let i = a.find("TUNE.MOD").unwrap();
    assert_eq!(a.entries[i].name, "mods/tune.mod");
    assert_eq!(a.find("mods/tune.mod"), Some(i));
    assert_eq!(a.find("nothing"), None);
    let c = a.companions(i);
    assert_eq!(c.len(), 2);
    assert!(c.iter().all(|(n, _)| *n != "mods/tune.mod"));
}

/// A tiny xorshift for the damage below (deterministic).
fn rng(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

/// `extract`'s result, failing on a panic it caught (under `panic = "abort"`, as in the
/// WebAssembly build, that would have been an abort).
fn no_panic(r: Result<Archive, ArchiveError>) -> Result<Archive, ArchiveError> {
    if let Err(e) = &r {
        assert!(!e.0.contains("decoder failed"), "{e}");
    }
    r
}

#[test]
fn damaged_archives_do_not_panic() {
    let mut seed = 0x1234_5678_9abc_def1u64;
    let all: Vec<(&str, &[u8])> = LHA.iter().copied().chain(OTHERS.iter().map(|(n, d, _)| (*n, *d))).chain([
        ("tune.mod.gz", fixture!("tune.mod.gz")),
        ("tune.mod.xz", fixture!("tune.mod.xz")),
    ]).collect();
    let lim = Limits::default();
    for (n, d) in all {
        // every truncation
        for cut in 0..d.len() {
            let r = no_panic(extract(n, &d[..cut], &lim));
            if let Ok(a) = &r {
                // what is extracted from a truncated archive is right or reported
                for e in &a.entries {
                    if let Some((_, want)) = files().into_iter().find(|(f, _)| *f == e.name) {
                        assert_eq!(e.data, want, "{n} cut at {cut}: {}", e.name);
                    }
                }
            }
        }
        // random byte changes, then random bytes after a good header
        for _ in 0..400 {
            let mut x = d.to_vec();
            for _ in 0..1 + rng(&mut seed) % 4 {
                let at = (rng(&mut seed) % x.len() as u64) as usize;
                x[at] = rng(&mut seed) as u8;
            }
            let _ = no_panic(extract(n, &x, &lim));
            let _ = crate::decrunch::decrunch(x);
        }
        for len in [24usize, 100, 600, 3000] {
            let mut x = d[..d.len().min(24)].to_vec();
            while x.len() < len {
                x.push(rng(&mut seed) as u8);
            }
            let _ = no_panic(extract(n, &x, &lim));
        }
    }
}

#[test]
fn expand_into_cache_dir() {
    let root = std::env::temp_dir().join(format!("amigaplayer-archive-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let src = root.join("t.lha");
    std::fs::write(&src, LHA[0].1).unwrap();
    let cache = root.join("cache");
    let x = expand_file(&src, Some(&cache)).unwrap().unwrap();
    assert_eq!(x.entries.len(), 3);
    for (name, path) in &x.entries {
        assert!(path.starts_with(&x.dir));
        let want = files().into_iter().find(|(f, _)| f == name).unwrap().1;
        assert_eq!(std::fs::read(path).unwrap(), want);
    }
    // a second visit finds it extracted
    let y = expand_file(&src, Some(&cache)).unwrap().unwrap();
    assert_eq!(y.dir, x.dir);
    // a plain file, and a gzip of one file, are not archives of files
    let plain = root.join("x.mod");
    std::fs::write(&plain, crate::testdata::minimal_mod()).unwrap();
    assert!(expand_file(&plain, Some(&cache)).unwrap().is_none());
    let gz = root.join("t.mod.gz");
    std::fs::write(&gz, fixture!("tune.mod.gz")).unwrap();
    assert!(expand_file(&gz, Some(&cache)).unwrap().is_none());
    std::fs::remove_dir_all(&root).unwrap();
}
