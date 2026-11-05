//! Writes the synthetic ProTracker module of `amigaplayer_rs::testdata` to a file, for scripts
//! that need a module without shipping one: `make_demo_mod OUT.mod`.

fn main() -> std::io::Result<()> {
    let Some(out) = std::env::args().nth(1) else {
        eprintln!("usage: make_demo_mod OUT.mod");
        std::process::exit(2);
    };
    std::fs::write(out, amigaplayer_rs::testdata::minimal_mod())
}
