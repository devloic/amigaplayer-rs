//! Prints the YM2149 register writes an SNDH tune makes on the built-in Atari ST machine, one per
//! line as `SAMPLE REG VALUE` (the output sample, at 48 kHz, the write falls on, counted from the
//! start of the subsong, decimal; the init's writes at 0), for a check against another player's
//! register log (AtariAudio patched to log its writes, for one).
//!
//!   cargo run --release --no-default-features --features archives --example sndh_regs -- FILE [SUBSONG [TICKS]]
//!
//! FILE may be ICE!-packed; SUBSONG is 1-based (default: the file's own); TICKS player ticks
//! (default 500). The first line, `# tick-rate N [vbl]`, gives the player's tick rate (and whether
//! the tune asks for the vertical blank).

use amigaplayer_rs::replay::sndh::{self, SndhPlayer};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(file) = args.first() else {
        eprintln!("usage: sndh_regs FILE [SUBSONG [TICKS]]");
        std::process::exit(2);
    };
    let data = std::fs::read(file).unwrap_or_else(|e| {
        eprintln!("{file}: {e}");
        std::process::exit(1);
    });
    let data = amigaplayer_rs::decrunch::decrunch(data);
    if !sndh::detect(&data) {
        eprintln!("{file}: not an SNDH file");
        std::process::exit(1);
    }
    let mut p = SndhPlayer::new(&data, 48000).unwrap();
    if let Some(s) = args.get(1).and_then(|s| s.parse::<u16>().ok()) {
        p.start(s.saturating_sub(1));
    }
    let ticks = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(500);
    println!("# tick-rate {}{}", p.info().rate, if p.info().vbl { " vbl" } else { "" });
    for (t, r, v) in p.ym_writes(ticks) {
        println!("{t} {r} {v}");
    }
    if p.dead() {
        println!("# the tune's code ended (crash or no return)");
    }
}
