//! Lists the formats the library plays; with file arguments, says what plays each file.
//!
//! ```text
//! cargo run --release --no-default-features --example list_formats
//! cargo run --release --no-default-features --example list_formats -- a.mod b.ahx
//! ```

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        let formats = amigaplayer_rs::formats();
        for f in &formats {
            println!("{f}");
        }
        eprintln!("{} formats", formats.len());
        return;
    }
    for f in files {
        match std::fs::read(&f) {
            Ok(d) => println!("{f}: {}", amigaplayer_rs::detect(&d).as_deref().unwrap_or("not recognised")),
            Err(e) => println!("{f}: {e}"),
        }
    }
}
