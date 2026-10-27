//! The DeliTracker host: runs the original 68k DeliPlayers on an emulated
//! Amiga, and the Eagleplayers too. `engine::Engine` is the entry point; the
//! binary adds the SDL3 UI.

pub mod bus;
pub mod cia;
pub mod deli;
pub mod decrunch;
pub mod eagle;
pub mod engine;
pub mod heap;
pub mod hunk;
pub mod machine;
pub mod notes;
pub mod os;
pub mod paula;
pub mod paulaout;
pub mod protracker;
pub mod replay;

use std::path::{Path, PathBuf};

pub const SAMPLE_RATE: u32 = 48000;

/// The DeliPlayers directory: `$DELIPLAYERS`, `~/.config/deliplayer/DeliPlayers`,
/// then `./DeliPlayers`, whichever exists first.
pub fn default_players_dir() -> Option<PathBuf> {
    let mut c = Vec::new();
    if let Ok(p) = std::env::var("DELIPLAYERS") {
        c.push(PathBuf::from(p));
    }
    if let Ok(h) = std::env::var("HOME") {
        c.push(Path::new(&h).join(".config/deliplayer/DeliPlayers"));
    }
    c.push(PathBuf::from("DeliPlayers"));
    c.into_iter().find(|p| p.is_dir())
}

/// The DeliGenies directory: `$DELIGENIES`, `~/.config/deliplayer/DeliGenies`, then
/// `./DeliGenies`, whichever exists first.
pub fn default_genies_dir() -> Option<PathBuf> {
    let mut c = Vec::new();
    if let Ok(p) = std::env::var("DELIGENIES") {
        c.push(PathBuf::from(p));
    }
    if let Ok(h) = std::env::var("HOME") {
        c.push(Path::new(&h).join(".config/deliplayer/DeliGenies"));
    }
    c.push(PathBuf::from("DeliGenies"));
    c.into_iter().find(|p| p.is_dir())
}

/// The Eagleplayers directory: `$EAGLEPLAYERS`, `~/.config/deliplayer/Eagleplayers`,
/// then `./Eagleplayers`, whichever exists first.
pub fn default_eagleplayers_dir() -> Option<PathBuf> {
    let mut c = Vec::new();
    if let Ok(p) = std::env::var("EAGLEPLAYERS") {
        c.push(PathBuf::from(p));
    }
    if let Ok(h) = std::env::var("HOME") {
        c.push(Path::new(&h).join(".config/deliplayer/Eagleplayers"));
    }
    c.push(PathBuf::from("Eagleplayers"));
    c.into_iter().find(|p| p.is_dir())
}
