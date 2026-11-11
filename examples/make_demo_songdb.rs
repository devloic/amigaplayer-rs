//! Writes a small synthetic song database that knows the given files, for trying
//! `amigaplayer_rs::songdb` and `amigaplayer-render --songdb` without the real database
//! (audacious-uade's `conf/songdb`, which is CC BY-NC-SA 4.0 and not in this repository).
//!
//! ```text
//! cargo run --release --no-default-features --features songdb --example make_demo_songdb -- DIR FILE...
//! ```
//!
//! File number `i` gets what `amigaplayer_rs::testdata::write_songdb` gives it: subsong 0 of
//! 2000 + 1000 * i ms, author "Author i", and so on.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: make_demo_songdb DIR FILE...");
        std::process::exit(2);
    }
    let files: Vec<Vec<u8>> = args[1..].iter().map(std::fs::read).collect::<Result<_, _>>()?;
    let refs: Vec<&[u8]> = files.iter().map(|f| f.as_slice()).collect();
    std::fs::create_dir_all(&args[0])?;
    amigaplayer_rs::testdata::write_songdb(std::path::Path::new(&args[0]), &refs)?;
    for (name, bytes) in args[1..].iter().zip(&files) {
        println!("{} {name}", amigaplayer_rs::songdb::key(bytes));
    }
    Ok(())
}
