//! For the applications: what to play for a file named on the command line or added to the
//! window's list. A module is itself; an archive (feature `archives`) is extracted into a
//! cache directory of its own (see `archive::expand_file`) and its entries are played from
//! there in the archive's order, so that a player finds a module's companions (`smp.*`,
//! `*.ins`, `Instruments/`, `smpl.*`) beside it as it would on disk.

use std::path::{Path, PathBuf};

/// One thing to play.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The entry's name in the archive (`None`: the file itself, not from an archive).
    pub entry: Option<String>,
    /// Where it is on disk.
    pub path: PathBuf,
}

/// What to play for `path`: the file itself, or the entries of the archive it is (those named
/// like icons and texts left out), or only the entry `want` (its path in the archive or its
/// file name, in any case). Which entries a player takes is up to the caller: it skips those
/// that do not start.
pub fn items(path: &Path, want: Option<&str>) -> Result<Vec<Item>, String> {
    // (a file that cannot be read is left to the player, which says why as it always did)
    #[cfg(feature = "archives")]
    if let Some(x) = if path.is_file() { crate::archive::expand_file(path, None)? } else { None } {
        for s in &x.skipped {
            log::warn!("{}: {}{}{}", path.display(), s.name, if s.name.is_empty() { "" } else { ": " }, s.reason);
        }
        let all: Vec<Item> = x.entries.into_iter().map(|(n, p)| Item { entry: Some(n), path: p }).collect();
        if let Some(w) = want {
            let w = w.trim_start_matches('/');
            let found = all.iter().find(|i| i.entry.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(w))).or_else(|| {
                all.iter().find(|i| i.entry.as_deref().and_then(|n| n.rsplit('/').next()).is_some_and(|b| b.eq_ignore_ascii_case(w)))
            });
            return match found {
                Some(i) => Ok(vec![i.clone()]),
                None => Err(format!("{}: no entry named {w}", path.display())),
            };
        }
        let list: Vec<Item> = all.into_iter().filter(|i| !crate::archive::is_not_music(i.entry.as_deref().unwrap_or(""))).collect();
        if list.is_empty() {
            return Err(format!("{}: the archive has no file to play", path.display()));
        }
        return Ok(list);
    }
    if want.is_some() {
        return Err(format!("{}: --archive-entry needs an archive{}", path.display(), if cfg!(feature = "archives") { "" } else { " (and a build with the archives feature)" }));
    }
    Ok(vec![Item { entry: None, path: path.to_path_buf() }])
}
