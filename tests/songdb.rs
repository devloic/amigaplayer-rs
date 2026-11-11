//! `amigaplayer_rs::songdb` on a synthetic database (feature `songdb`). The real database is
//! not free to redistribute and is nowhere in this repository.
#![cfg(feature = "songdb")]

use amigaplayer_rs::songdb::{key, SongDb, SongEndStatus};
use amigaplayer_rs::testdata::{minimal_mod, minimal_mod_titled, write_songdb};
use std::path::PathBuf;

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("songdb-api-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_key_of_a_file() {
    assert_eq!(key(b""), "02cc5d050000");
    let m = minimal_mod();
    let k = key(&m);
    assert_eq!(k.len(), 12);
    assert!(k.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
    assert_eq!(&k[8..], format!("{:04x}", m.len() & 0xffff));
    assert_ne!(key(&minimal_mod_titled("other")), k);
}

#[test]
fn lookups_by_bytes_and_by_key() {
    let d = dir("lookup");
    let (a, b) = (minimal_mod(), minimal_mod_titled("second"));
    write_songdb(&d, &[&a, &b]).unwrap();
    let db = SongDb::open(&d).unwrap();
    assert!(db.len() >= 2 && !db.is_empty());

    let i = db.lookup(&a).unwrap();
    assert_eq!(i.key, key(&a));
    assert_eq!((i.format.as_str(), i.channels), ("Synthetic 0", 4));
    assert_eq!((i.author.as_str(), i.album.as_str(), i.publisher.as_str(), i.year), ("Author 0", "Album 0", "Publisher 0", 1990));
    assert_eq!(i.subsongs.len(), 2);
    let s = i.subsong(0).unwrap();
    assert_eq!((s.subsong, s.length_ms, s.status, s.is_duplicate), (0, 2000, SongEndStatus::Player, false));
    assert_eq!(s.status.as_str(), "player");
    let s = i.subsong(1).unwrap();
    assert_eq!((s.length_ms, s.status.to_string().as_str()), (500, "silence"));
    assert!(i.subsong(2).is_none());

    let j = db.lookup_key(&key(&b)).unwrap();
    assert_eq!((j.author.as_str(), j.year, j.subsong(0).unwrap().length_ms), ("Author 1", 1991, 3000));
    // upper case is the same key
    assert_eq!(db.lookup_key(&key(&b).to_ascii_uppercase()), Some(j));

    assert_eq!(db.lookup(b"not in the database"), None);
    assert_eq!(db.lookup(&[]), None);
    for k in ["", "abc", "000000000000", "zzzzzzzzzzzz", "0123456789abc", "::::::::::::", "ééééé\u{e9}"] {
        assert_eq!(db.lookup_key(k), None, "{k:?}");
    }
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn the_database_is_shared_between_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SongDb>();
    let d = dir("threads");
    let m = minimal_mod();
    write_songdb(&d, &[&m]).unwrap();
    let db = SongDb::open(&d).unwrap();
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| assert_eq!(db.lookup(&m).unwrap().year, 1990));
        }
    });
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_missing_or_damaged_database_is_an_error_not_a_panic() {
    let d = dir("damaged");
    assert_eq!(SongDb::open(d.join("nowhere")).unwrap_err().kind(), std::io::ErrorKind::NotFound);
    assert_eq!(SongDb::open(&d).unwrap_err().kind(), std::io::ErrorKind::NotFound);
    let m = minimal_mod();
    write_songdb(&d, &[&m]).unwrap();
    let files = ["xxh32idx.tsv", "songlengths.tsv", "modinfos.tsv", "metadata.tsv"];
    let good: Vec<Vec<u8>> = files.iter().map(|f| std::fs::read(d.join(f)).unwrap()).collect();
    let mut seed = 0x9E37_79B9u32;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let mut errors = 0;
    for (f, bytes) in files.iter().zip(&good) {
        let mut variants: Vec<Vec<u8>> = vec![Vec::new(), b"\n".to_vec(), b"\n\n\n".to_vec(), b"\t\t\t\t\n".to_vec(), vec![0xFF; 40]];
        // cut short, lines out of order, bytes damaged
        variants.push(bytes[..bytes.len() / 2].to_vec());
        variants.push(bytes[..bytes.len() - 1].to_vec());
        variants.push(bytes.iter().rev().copied().collect());
        variants.push([bytes.as_slice(), bytes.as_slice()].concat());
        for _ in 0..40 {
            let mut v = bytes.clone();
            for _ in 0..1 + rnd() % 4 {
                let at = rnd() as usize % v.len();
                v[at] = rnd() as u8;
            }
            variants.push(v);
        }
        for v in variants {
            std::fs::write(d.join(f), &v).unwrap();
            // an error or a database: either way the lookups answer
            match SongDb::open(&d) {
                Ok(db) => {
                    let _ = db.lookup(&m);
                    let _ = db.lookup_key("0000000000ff");
                }
                Err(_) => errors += 1,
            }
        }
        std::fs::write(d.join(f), bytes).unwrap();
        assert!(SongDb::open(&d).is_ok());
    }
    assert!(errors > 4, "{errors} damaged databases were refused");
    std::fs::remove_dir_all(&d).unwrap();
}
