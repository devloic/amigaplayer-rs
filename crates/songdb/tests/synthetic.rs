// SPDX-License-Identifier: LGPL-2.1-or-later OR GPL-2.0-or-later
//
// Written for amigaplayer-rs (not vendored).

//! The database read from small synthetic files in the formats of audacious-uade's
//! `conf/songdb/`: lines written by hand here (with the "same as above" forms the real files
//! use), and files made by `write::write`. The real data is CC BY-NC-SA 4.0 and is not used.

use amigaplayer_songdb::write::write;
use amigaplayer_songdb::{key, Info, MetaData, ModInfo, SongDb, SongEnd, SongEndStatus, Source, SubSongInfo};
use std::path::PathBuf;

/// A fresh directory for one test.
fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("songdb-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `n` in the files' base 64: digits from `'-'`, six bits each, most significant first.
fn b64(mut n: u32) -> String {
    let mut s = Vec::new();
    loop {
        s.push((45 + (n & 63)) as u8 as char);
        n >>= 6;
        if n == 0 {
            break;
        }
    }
    s.iter().rev().collect()
}

fn sub(subsong: u8, ms: u32, status: SongEndStatus, dup: bool) -> SubSongInfo {
    SubSongInfo { subsong, songend: SongEnd::new(status, ms), is_duplicate: dup }
}

#[test]
fn hand_written_files_in_the_real_formats() {
    let d = dir("hand");
    // keys 1, 2 and 3: row 0 is the reserved hash 0, each further row a step from the last
    std::fs::write(d.join("xxh32idx.tsv"), "--------\n.\n.\n.\n").unwrap();
    // "length,status" per subsong (lengths in 20 ms), then the first subsong's number:
    //   key 1: one subsong, number 0, 94760 ms, ended by the player and a volume check
    //   key 2: 10 s; "!" the same again, a duplicate; 60 s, a loop. Numbered from 1.
    //   key 3: 1 s of silence detection, then "" the same again; numbered from 5
    let lengths = format!("{},P\t0\n{},p ! {},l\n{},s \t5\n", b64(94760 / 20), b64(500), b64(3000), b64(50));
    std::fs::write(d.join("songlengths.tsv"), lengths).unwrap();
    // index, format, channels; a row with nothing but a (relative) index repeats the last;
    // \x7f in a column is "as in the row before"
    std::fs::write(d.join("modinfos.tsv"), ".\tFormat One\t4\n.\n0\t\x7f\t8\n").unwrap();
    // index, authors (~ between several), publishers, album, date
    std::fs::write(d.join("metadata.tsv"), ".\tAnn~Bob\tPub\tAlbum One\t1997\n.\n0\t\x7f\t\tOther\t\x7f\n").unwrap();

    let db = SongDb::init(&d, &[]).unwrap();
    assert_eq!(db.len(), 4);

    let one = db.lookup("000000000001").unwrap();
    assert_eq!(one.subsongs, [sub(0, 94760, SongEndStatus::PlayerPlusVolume, false)]);
    assert_eq!(one.modinfo, Some(ModInfo { format: "Format One".into(), channels: 4 }));
    let meta = MetaData { author: "Ann & Bob".into(), album: "Album One".into(), publisher: "Pub".into(), year: 1997 };
    assert_eq!(one.metadata, Some(meta.clone()));
    assert_eq!((one.min_subsong(), one.max_subsong()), (0, 0));

    let two = db.lookup("000000000002").unwrap();
    assert_eq!(
        two.subsongs,
        [
            sub(1, 10_000, SongEndStatus::Player, false),
            sub(2, 10_000, SongEndStatus::Player, true),
            sub(3, 60_000, SongEndStatus::DetectLoop, false)
        ]
    );
    assert_eq!(two.modinfo, one.modinfo);
    assert_eq!(two.metadata, Some(meta));
    assert_eq!(db.lookup_subsong("000000000002", 3), Some(sub(3, 60_000, SongEndStatus::DetectLoop, false)));
    assert_eq!(db.lookup_subsong("000000000002", 4), None);
    assert_eq!(db.lookup_subsong("000000000002", 0), None);

    let three = db.lookup("000000000003").unwrap();
    assert_eq!(three.subsongs, [sub(5, 1000, SongEndStatus::DetectSilence, false), sub(6, 1000, SongEndStatus::DetectSilence, false)]);
    assert_eq!(three.modinfo, Some(ModInfo { format: "Format One".into(), channels: 8 }));
    assert_eq!(
        three.metadata,
        Some(MetaData { author: "Ann & Bob".into(), album: "Other".into(), publisher: String::new(), year: 1997 })
    );

    assert_eq!(db.lookup("000000000004"), None);
    assert_eq!(db.lookup("ffffffffffff"), None);
    assert_eq!(db.lookup("123"), None);
    assert_eq!(db.lookup(""), None);

    // only some of the sources
    let db = SongDb::init(&d, &[Source::Songlengths]).unwrap();
    let one = db.lookup("000000000001").unwrap();
    assert_eq!((one.subsongs.len(), one.modinfo, one.metadata), (1, None, None));
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_written_database_reads_back() {
    let d = dir("written");
    let (a, b, c) = (b"module A".to_vec(), vec![7u8; 300_000], Vec::new());
    let entries = vec![
        (
            key(&a),
            Info {
                subsongs: vec![sub(0, 94760, SongEndStatus::PlayerPlusVolume, false)],
                modinfo: Some(ModInfo { format: "Some Tracker 1.0".into(), channels: 4 }),
                metadata: Some(MetaData { author: "A. Composer".into(), album: "A Demo".into(), publisher: "A Group".into(), year: 1994 }),
            },
        ),
        (
            key(&b),
            Info {
                subsongs: vec![
                    sub(1, 0, SongEndStatus::NoSound, false),
                    sub(2, 20, SongEndStatus::Player, true),
                    sub(3, ((1 << 18) - 1) * 20, SongEndStatus::Timeout, false),
                ],
                modinfo: Some(ModInfo { format: "Other".into(), channels: 255 }),
                metadata: None,
            },
        ),
        (
            key(&c),
            Info {
                subsongs: vec![sub(127, 5000, SongEndStatus::Error, false)],
                modinfo: None,
                metadata: Some(MetaData { author: String::new(), album: String::new(), publisher: String::new(), year: 0 }),
            },
        ),
    ];
    write(&d, &entries).unwrap();
    let db = SongDb::init(&d, &[]).unwrap();
    for (k, want) in &entries {
        let got = db.lookup(k).unwrap_or_else(|| panic!("{k} not found"));
        assert_eq!(got.subsongs, want.subsongs, "{k}");
        assert_eq!(got.metadata, want.metadata, "{k}");
        // (a row without a format reads back as an empty one)
        assert_eq!(got.modinfo, want.modinfo.clone().or(Some(ModInfo::default())), "{k}");
    }
    assert_eq!(db.lookup(&key(b"module B")), None);
    // the keys are far apart: rows bridge them, and the index stays ascending
    assert!(db.len() > entries.len() + 1);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn keys_are_xxh32_of_the_first_256k_and_the_low_length_bits() {
    // xxh32("") = 02cc5d05
    assert_eq!(key(b""), "02cc5d050000");
    assert_eq!(key(b"a"), "550d74560001");
    // beyond 256 KiB only the length counts
    let mut big = vec![1u8; 262_144 + 10];
    let k1 = key(&big);
    big[262_144 + 5] = 2;
    assert_eq!(key(&big), k1);
    big[262_143] = 2;
    assert_ne!(key(&big), k1);
    assert!(k1.ends_with("000a"), "{k1}");
    assert_eq!(k1.len(), 12);
}

#[test]
fn what_cannot_be_stored_is_refused() {
    let d = dir("refused");
    let ok = Info { subsongs: vec![sub(0, 1000, SongEndStatus::Player, false)], ..Default::default() };
    let with = |f: &dyn Fn(&mut Info)| {
        let mut i = ok.clone();
        f(&mut i);
        write(&d, &[("0000000000ab".to_string(), i)])
    };
    assert!(with(&|_| {}).is_ok());
    assert!(with(&|i| i.subsongs.clear()).is_err());
    assert!(with(&|i| i.subsongs[0].songend.length = 30).is_err());
    assert!(with(&|i| i.subsongs[0].songend.length = (1 << 18) * 20).is_err());
    assert!(with(&|i| i.subsongs[0].subsong = 128).is_err());
    assert!(with(&|i| i.subsongs.push(sub(2, 1000, SongEndStatus::Player, false))).is_err());
    assert!(with(&|i| i.modinfo = Some(ModInfo { format: "a\tb".into(), channels: 1 })).is_err());
    assert!(with(&|i| i.metadata = Some(MetaData { year: 1900, ..Default::default() })).is_err());
    for k in ["", "abc", "0000000000AB", "00000000000g", "000000000000"] {
        assert!(write(&d, &[(k.to_string(), ok.clone())]).is_err(), "{k:?}");
    }
    assert!(write(&d, &[("0000000000ab".into(), ok.clone()), ("0000000000ab".into(), ok.clone())]).is_err());
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_missing_database_is_an_error() {
    let d = dir("missing");
    assert!(SongDb::init(&d, &[]).is_err());
    assert!(SongDb::init(&d.join("nowhere"), &[]).is_err());
    std::fs::remove_dir_all(&d).unwrap();
}
