//! Lists or extracts an archive with `amigaplayer_rs::archive` (LhA, LZX, zip, tar, gzip, xz), and
//! says which entries a replayer of the library recognises (with the other entries as their
//! companions).
//!
//!   cargo run --release --no-default-features --features archives --example unarchive -- ARCHIVE [OUTDIR]
//!
//! With OUTDIR the entries are written below it (their cleaned names: relative, no `..`).

use amigaplayer_rs::archive::{self, Limits};
use std::path::PathBuf;

fn main() {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: unarchive ARCHIVE [OUTDIR]");
        std::process::exit(2);
    };
    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("{}: {e}", path.display());
        std::process::exit(1);
    });
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let a = match archive::extract(&name, &data, &Limits::default()) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            std::process::exit(1);
        }
    };
    println!("{} archive, {} entries{}", a.format, a.entries.len(), if a.single { " (a single file)" } else { "" });
    for (i, e) in a.entries.iter().enumerate() {
        let fmt = amigaplayer_rs::detect_with(&e.name, &e.data, &a.companions(i));
        println!("{:>9}  {}{}", e.data.len(), e.name, fmt.map(|f| format!("  [{f}]")).unwrap_or_default());
        if let Some(out) = args.get(1) {
            let p = e.name.split('/').fold(out.clone(), |p: PathBuf, c| p.join(c));
            if let Err(err) = p.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&p, &e.data)) {
                eprintln!("{}: {err}", p.display());
            }
        }
    }
    for s in &a.skipped {
        println!("skipped  {}: {}", s.name, s.reason);
    }
}
