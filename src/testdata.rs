//! Synthetic modules for examples, doctests and tests. They are built in code, so the
//! repository holds no music files.

/// A minimal valid 4-channel ProTracker module ("M.K."): one pattern of 64 rows, a looping
/// 256-byte square-wave sample, and a note on each of the first channels every 16 rows.
/// About 2.7 seconds at the default speed. `title` is cut to 20 bytes.
pub fn minimal_mod_titled(title: &str) -> Vec<u8> {
    let mut d = vec![0u8; 1084];
    for (i, b) in title.bytes().take(20).enumerate() {
        d[i] = b;
    }
    // sample 1: 128 words, volume 64, looping over its whole length
    let s = 20;
    d[s..s + 6].copy_from_slice(b"square");
    d[s + 22..s + 24].copy_from_slice(&128u16.to_be_bytes());
    d[s + 25] = 64;
    d[s + 28..s + 30].copy_from_slice(&128u16.to_be_bytes());
    d[950] = 1; // song length
    d[951] = 127; // restart
    d[1080..1084].copy_from_slice(b"M.K.");
    // one pattern: 64 rows x 4 channels x 4 bytes
    let mut pat = vec![0u8; 1024];
    let notes: [u16; 4] = [428, 339, 285, 214]; // C-2, E-2, G-2, C-3
    for row in (0..64).step_by(16) {
        for (ch, &period) in notes.iter().enumerate() {
            if (row / 16 + ch) % 2 == 0 {
                let o = (row * 4 + ch) * 4;
                pat[o] = (period >> 8) as u8; // sample 1 has high nibble 0
                pat[o + 1] = period as u8;
                pat[o + 2] = 0x10; // sample 1 low nibble
            }
        }
    }
    d.extend_from_slice(&pat);
    // the sample: a square wave
    d.extend((0..256).map(|i| if i < 128 { 100u8 } else { 156u8 }));
    d
}

/// [`minimal_mod_titled`] with the title "minimal".
pub fn minimal_mod() -> Vec<u8> {
    minimal_mod_titled("minimal")
}
