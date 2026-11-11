// SPDX-License-Identifier: LGPL-2.1-or-later OR GPL-2.0-or-later
//
// Vendored from audacious-uade-rust (https://fg.lolo.asia/adminlolo/audacious-uade-rust.git,
// commit c3ea17194c768987c48751cd97d638ccb04b1472), file crates/au-songdb/src/blacklist.rs, whose crate
// manifest gives the licence above. It is a Rust port of src/songdb/blacklist.cc of
// audacious-uade by Matti Tiainen (https://github.com/mvtiaine/audacious-uade), which carries:
//     SPDX-License-Identifier: LGPL-2.1-or-later
//     Copyright (C) 2014-2026 Matti Tiainen <mvtiaine@cc.hut.fi>
// (audacious-uade as a whole is GPL-2.0-or-later; its src/common/ and src/songdb/ are also
// offered under LGPL-2.1-or-later). The texts are COPYING and COPYING.LGPL beside Cargo.toml.
// Changes for amigaplayer-rs: none.

//! Port of `src/songdb/blacklist.cc`.
//!
//! Three independent blacklists, all consulted before a file reaches a
//! player:
//!
//! - **extensions** — formats with no support, non-Amiga formats, sample and
//!   companion files, and archive/image/document extensions that exist only
//!   to keep playlist population from probing every file in an Aminet dump.
//! - **hashes** — specific modules that hang `uade_play()`/`uade_stop()` and
//!   leave a zombie `uadecore` behind, plus one that crashes it2play.
//! - **songdb hashes** — keys excluded from database *updates*.
//!
//! The lists themselves are data, transcribed from the C. What needs care is
//! the matching logic, which mixes case-sensitive and case-insensitive
//! comparisons in a way that is easy to get wrong; see
//! [`is_blacklisted_extension`].

use std::collections::HashSet;
use std::sync::OnceLock;

/// Extensions the C refuses outright. Stored with the leading dot, lowercase,
/// exactly as the C's `set<string>` holds them.
const EXTENSION_BLACKLIST: &[&str] = &[
    // No support.
    ".ct",    // Cybertracker
    ".fuchs", // Fuchs Tracker
    ".dux",   // GT Game Systems
    ".mxtx",  // MaxTrax
    ".spm",   // Stonetracker
    // Not Amiga.
    ".ym",
    // Sample and companion files.
    ".instr", ".x", ".set", ".ins", ".nt", ".as", ".ip", ".l", ".n", ".ssd", ".sps", ".smp",
    ".smpl",
    // Archives and containers — an Aminet-population speedup, not a
    // statement that the contents are unplayable.
    ".lha", ".lzh", ".lzx", ".zip", ".adf", ".adz", ".dms", ".bak", ".tar", ".gz", ".tgz", ".bz2",
    ".pp", ".7z", ".Z", ".xz", ".arj", ".rar",
    // Images.
    ".gif", ".png", ".jpg", ".jpeg", ".bmp", ".tga", ".ilbm", ".anim", ".pic", ".gfx", ".pbm",
    ".raw", ".pcx", ".3ds", ".lbm",
    // Amiga metadata.
    ".info", ".ico", ".font", ".fon", ".lvl", ".map", ".inf",
    // Executables and configuration.
    ".exe", ".library", ".device", ".rexx", ".prefs", ".ini", ".dll", ".prg", ".com", ".debug",
    ".cfg", ".conf", ".rev", ".cmd",
    // Source code.
    ".h", ".hpp", ".c", ".s", ".asm", ".cpp", ".cxx", ".cc", ".o", ".obj", ".po", ".in", ".am",
    ".sh", ".amos", ".bas", ".i", ".a", ".fd",
    // Documents.
    ".txt", ".readme", ".rea", ".guide", ".diz", ".nfo", ".doc", ".htm", ".html", ".hlp", ".1st",
    ".xml",
    // Catalogs.
    ".catalog", ".deutsch", ".english",
    ".findlist", // Mods Anthology — hangs uade.
];

/// Filename prefixes refused regardless of extension. Matched
/// **case-sensitively** against the original filename, which is why both
/// cases of each appear in the list.
const PREFIX_BLACKLIST: &[&str] = &["SMP.", "SMPL.", "smp.", "smpl.", "readme.", "README."];

/// Modules that wedge the player. The C keeps the original MD5 in a trailing
/// comment beside each 12-character key; those comments are the provenance
/// of the entry, not something the code reads.
const HASH_BLACKLIST: &[&str] = &[
    // uade_play()/uade_stop() get stuck; may leave a zombie uadecore.
    "e2e642be13fc",
    "c12c2d131bcc",
    "69364c10fdf0",
    "17c754bfa826",
    "58b790471c40",
    "15f2724de000",
    "82247f6a6da4",
    "75d3a18b8752",
    "4efe29ee2260",
    "c51668924406",
    "fc28103f0958",
    "beb7e0c00b50",
    "b8d4b40c16f0",
    "9d433b215e96",
    "064d9cb220c8",
    "e2edd340e000",
    "c0d7e147e000",
    "bc4130f85b83",
    "69ebb4084000",
    "383e7a1ddbc0",
    // Crashes it2play. Upstream marks this one TODO: fix.
    "bfc53ea5dd27",
];

/// Keys excluded from database updates. Upstream's note: these modules share
/// an MD5 but load different data files depending on their *filename*, so a
/// single key cannot describe them.
const SONGDB_BLACKLIST: &[&str] = &["af58e712052c"];

fn extension_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| EXTENSION_BLACKLIST.iter().copied().collect())
}

fn hash_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| HASH_BLACKLIST.iter().copied().collect())
}

fn songdb_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| SONGDB_BLACKLIST.iter().copied().collect())
}

/// Port of `songdb::blacklist::is_blacklisted_extension`.
///
/// `ext` is expected to carry its leading dot; `whitelist` holds entries
/// *without* one.
///
/// # The case handling is deliberately inconsistent
///
/// Reproduced from the C, where it is easy to misread:
///
/// - the **whitelist checks** and the **extension blacklist check** use a
///   lowercased filename and extension;
/// - the **prefix check** uses the *original* filename, so `"SMP."` and
///   `"smp."` must both be listed and `"Smp."` matches neither.
///
/// Lowercasing the filename once and using it everywhere — the obvious
/// tidy-up — would start blacklisting `Smp.foo`, which the C plays.
///
/// # Two whitelist paths
///
/// A file escapes if *either* its lowercased extension minus the dot is
/// whitelisted, **or** the part of its lowercased filename before the first
/// dot is whitelisted. The second is what lets prefix-style names such as
/// `mdat.foo` through when `mdat` is a known format tag.
pub fn is_blacklisted_extension(path: &str, ext: &str, whitelist: &HashSet<String>) -> bool {
    // `common::split(path, "/").back()` — always non-empty, and yields the
    // whole path when there is no separator.
    let filename = path.rsplit('/').next().unwrap_or(path);
    let lcfilename = filename.to_lowercase();
    let lcext = ext.to_lowercase();

    if lcext.len() > 1 && whitelist.contains(&lcext[1..]) {
        return false;
    }

    // `common::split(lcfilename, ".").front()` — everything before the first
    // dot, or the whole name when there is none.
    let lcprefix = lcfilename.split('.').next().unwrap_or(&lcfilename);
    if whitelist.contains(lcprefix) {
        return false;
    }

    if extension_set().contains(lcext.as_str()) {
        return true;
    }

    // Case-sensitive, against the original filename. See the note above.
    PREFIX_BLACKLIST
        .iter()
        .any(|prefix| filename.starts_with(prefix))
}

/// Port of `songdb::blacklist::is_blacklisted_hash`.
pub fn is_blacklisted_hash(hash: &str) -> bool {
    hash_set().contains(hash)
}

/// Port of `songdb::blacklist::is_blacklisted_songdb_hash`.
pub fn is_blacklisted_songdb_hash(hash: &str) -> bool {
    songdb_set().contains(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whitelist(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn blacklists_listed_extensions() {
        let wl = whitelist(&[]);
        assert!(is_blacklisted_extension("/music/foo.ct", ".ct", &wl));
        assert!(is_blacklisted_extension("/music/foo.zip", ".zip", &wl));
        assert!(is_blacklisted_extension("/music/foo.txt", ".txt", &wl));
        assert!(is_blacklisted_extension("foo.findlist", ".findlist", &wl));
    }

    #[test]
    fn allows_unlisted_extensions() {
        let wl = whitelist(&[]);
        assert!(!is_blacklisted_extension("/music/foo.mod", ".mod", &wl));
        assert!(!is_blacklisted_extension("/music/foo.ahx", ".ahx", &wl));
    }

    /// The extension comparison is case-insensitive, so an upper-case
    /// spelling is refused just the same.
    #[test]
    fn extension_matching_ignores_case() {
        let wl = whitelist(&[]);
        assert!(is_blacklisted_extension("/music/FOO.ZIP", ".ZIP", &wl));
        assert!(is_blacklisted_extension("/music/Foo.Txt", ".Txt", &wl));
    }

    /// ...but the prefix comparison does not. This is the asymmetry the doc
    /// comment warns about; a "tidied" implementation fails here.
    #[test]
    fn prefix_matching_is_case_sensitive() {
        let wl = whitelist(&[]);
        assert!(is_blacklisted_extension("/x/SMP.foo", ".foo", &wl));
        assert!(is_blacklisted_extension("/x/smp.foo", ".foo", &wl));
        assert!(is_blacklisted_extension("/x/README.foo", ".foo", &wl));
        // Neither spelling in the list: not blacklisted.
        assert!(!is_blacklisted_extension("/x/Smp.foo", ".foo", &wl));
        assert!(!is_blacklisted_extension("/x/ReadMe.foo", ".foo", &wl));
    }

    #[test]
    fn whitelisted_extension_overrides_the_blacklist() {
        let wl = whitelist(&["zip"]);
        assert!(!is_blacklisted_extension("/music/foo.zip", ".zip", &wl));
        // Whitelist entries carry no dot; the lookup strips it.
        let wl_dotted = whitelist(&[".zip"]);
        assert!(is_blacklisted_extension("/music/foo.zip", ".zip", &wl_dotted));
    }

    #[test]
    fn whitelisted_filename_prefix_overrides_the_blacklist() {
        // The second escape hatch: `mdat.foo` passes when `mdat` is known,
        // even though `.foo` would otherwise be judged on its own.
        let wl = whitelist(&["mdat"]);
        assert!(!is_blacklisted_extension("/x/mdat.txt", ".txt", &wl));
        assert!(!is_blacklisted_extension("/x/MDAT.txt", ".txt", &wl));
        // And it beats the prefix blacklist too, since it returns early.
        let wl_smp = whitelist(&["smp"]);
        assert!(!is_blacklisted_extension("/x/SMP.foo", ".foo", &wl_smp));
    }

    #[test]
    fn uses_only_the_final_path_component() {
        let wl = whitelist(&[]);
        // A directory named like a blacklisted prefix must not blacklist the
        // file inside it.
        assert!(!is_blacklisted_extension("/SMP.dir/song.mod", ".mod", &wl));
        // No separator at all: the whole string is the filename.
        assert!(is_blacklisted_extension("smp.thing", ".thing", &wl));
    }

    #[test]
    fn hash_blacklists_are_exact_matches() {
        assert!(is_blacklisted_hash("e2e642be13fc"));
        assert!(is_blacklisted_hash("bfc53ea5dd27"));
        assert!(!is_blacklisted_hash("e2e642be13fd"));
        assert!(!is_blacklisted_hash(""));
        // Case-sensitive, consistent with keys always being lowercase hex.
        assert!(!is_blacklisted_hash("E2E642BE13FC"));
    }

    #[test]
    fn songdb_blacklist_is_separate_from_the_hash_blacklist() {
        assert!(is_blacklisted_songdb_hash("af58e712052c"));
        // The two lists are disjoint in the C; a key in one is not in the
        // other, and conflating them would silently widen both.
        assert!(!is_blacklisted_hash("af58e712052c"));
        assert!(!is_blacklisted_songdb_hash("e2e642be13fc"));
    }

    /// Guards the transcription itself: a dropped or duplicated entry is the
    /// most likely error in a hand-copied data table.
    #[test]
    fn transcribed_lists_have_the_expected_shape() {
        assert_eq!(HASH_BLACKLIST.len(), 21);
        assert_eq!(SONGDB_BLACKLIST.len(), 1);
        assert_eq!(PREFIX_BLACKLIST.len(), 6);
        assert_eq!(extension_set().len(), EXTENSION_BLACKLIST.len(), "duplicate extension");
        assert_eq!(hash_set().len(), HASH_BLACKLIST.len(), "duplicate hash");
        for h in HASH_BLACKLIST.iter().chain(SONGDB_BLACKLIST) {
            assert_eq!(h.len(), 12, "key {h} is not 12 hex characters");
            assert!(h.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        }
        for e in EXTENSION_BLACKLIST {
            assert!(e.starts_with('.'), "extension {e} lacks its dot");
        }
        for p in PREFIX_BLACKLIST {
            assert!(p.ends_with('.'), "prefix {p} lacks its dot");
        }
    }
}
