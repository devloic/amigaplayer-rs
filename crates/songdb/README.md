# amigaplayer-songdb

A reader of the song database of [audacious-uade](https://github.com/mvtiaine/audacious-uade):
for some 476,000 Amiga and tracker modules, the length of each subsong and how it ends, the
format and channel count, and author, album, publisher and year. A module is found by a key
made of its own bytes (`key()`: XXH32 of the first 256 KiB, then the low 16 bits of the
length), so no file names are involved.

```rust
use amigaplayer_songdb::{key, SongDb};

let db = SongDb::init(std::path::Path::new("/usr/share/audacious-uade/songdb"), &[])?;
let bytes = std::fs::read("AHX.Cruisin")?;
if let Some(info) = db.lookup(&key(&bytes)) {
    for s in &info.subsongs {
        println!("subsong {}: {} ms ({})", s.subsong, s.songend.length, s.songend.status);
    }
}
```

amigaplayer-rs uses it through its `songdb` feature (`amigaplayer_rs::songdb`), which adds a
panic-free wrapper; see `docs/LIBRARY.md` there.

## Where the code comes from

This crate is a vendored copy of the `au-songdb` crate, and of the few `au-common` items it
uses (`SongEnd`, `SongEndStatus`, and `from_chars`, `mk_string`, `split_view_n`,
`split_view_x_n` of `strings`), from

* **audacious-uade-rust**, <https://fg.lolo.asia/adminlolo/audacious-uade-rust.git>, commit
  `c3ea17194c768987c48751cd97d638ccb04b1472` (`crates/au-songdb`, `crates/au-common`),

which is a Rust port of `src/songdb/` and `src/common/` of

* **audacious-uade** by Matti Tiainen, <https://github.com/mvtiaine/audacious-uade>
  (`Copyright (C) 2014-2026 Matti Tiainen <mvtiaine@cc.hut.fi>`).

Each vendored file names its origin in its first lines. `src/xxh32.rs` (the XXH32 hash, after
the algorithm's public specification), `key()` in `src/lib.rs` and `tests/synthetic.rs` were
written for amigaplayer-rs. The port's tests that compare with upstream's C build are not
vendored (they need that build).

## Licence

**Code: `LGPL-2.1-or-later OR GPL-2.0-or-later`**, at your choice. audacious-uade as a whole
is GPL-2.0-or-later; its `src/common/` and `src/songdb/` carry
`SPDX-License-Identifier: LGPL-2.1-or-later` and are, as its README says, "also licensed under
LGPL-2.1-or-later". The Rust port keeps both in the manifests of the two crates
(`license = "LGPL-2.1-or-later OR GPL-2.0-or-later"`), and so does this copy. The texts are
`COPYING` (GPL 2) and `COPYING.LGPL` (LGPL 2.1) in this directory. amigaplayer-rs links it
under GPL-2.0-or-later.

**Data: not included, and not under a software licence.** The database itself
(`xxh32idx.tsv`, `songlengths.tsv`, `modinfos.tsv`, `metadata.tsv`; 9.5 MB) is
audacious-uade's `conf/songdb/`, licensed
[CC BY-NC-SA 4.0](https://creativecommons.org/licenses/by-nc-sa/4.0/) (attribution,
non-commercial, share-alike; its sources are listed at
<https://github.com/mvtiaine/audacious-uade-tools#sources>). It is not in this repository,
not compiled into anything and not downloaded by any build script. Get it from an
audacious-uade checkout or package (`conf/songdb/`, installed as
`share/audacious-uade/songdb/`) and give its directory at run time. Whether you may use it is
between you and that licence: a commercial service, for one, may not.

The tests build a small synthetic database in the same file formats.
