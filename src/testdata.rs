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

/// A minimal custom module: an AmigaOS executable with a DeliTracker player header
/// (`DTP_CustomPlayer`) and a few dozen 68000 instructions that play a looping square wave on
/// Paula's first channel. Two subsongs (0 and 1, an octave apart), the title "synthetic", and
/// the song's end reported after three seconds (150 timer interrupts). For
/// `Song::open_opts` with `Options::allow_custom`, in builds with the `host-68k` feature.
pub fn minimal_custom() -> Vec<u8> {
    custom_with(&[], &[])
}

/// [`minimal_custom`] with 68000 code of the caller's: `init_prefix` runs at the start of
/// `DTP_InitSound`, and `interrupt`, unless empty, is the whole `DTP_Interrupt` routine
/// (called 50 times a second; it ends with `rts`, `4E 75`). Both must be position
/// independent. For tests of what happens when a module's code misbehaves.
#[doc(hidden)]
pub fn custom_with(init_prefix: &[u8], interrupt: &[u8]) -> Vec<u8> {
    const DTP: u32 = 0x8000_4454;
    const TAGS: [u32; 6] = [DTP + 1, DTP + 47, DTP + 14, DTP + 17, DTP + 10, DTP + 18];
    fn long(d: &mut Vec<u8>, v: u32) {
        d.extend_from_slice(&v.to_be_bytes());
    }
    // an address inside the hunk: its offset, relocated by the loader
    fn abs(d: &mut Vec<u8>, relocs: &mut Vec<u32>, off: u32) {
        relocs.push(d.len() as u32);
        long(d, off);
    }
    let mut relocs = Vec::new();
    let mut d = vec![0x70, 0xFF, 0x4E, 0x75]; // moveq #-1,d0 ; rts
    d.extend_from_slice(b"DELIRIUM");
    abs(&mut d, &mut relocs, 16);
    let tags = d.len();
    d.resize(tags + TAGS.len() * 8 + 4, 0);
    // data: the title's pointer and text, the tick counter, the sample
    let name_ptr = d.len() as u32;
    abs(&mut d, &mut relocs, name_ptr + 4);
    d.extend_from_slice(b"synthetic\0\0\0");
    let counter = d.len() as u32;
    d.extend_from_slice(&[0; 4]);
    let sample = d.len() as u32;
    d.extend((0..32).map(|i| if i < 16 { 100u8 } else { 156u8 }));
    // DTP_SubSongRange: moveq #0,d0 ; moveq #1,d1 ; rts
    let range = d.len() as u32;
    d.extend_from_slice(&[0x70, 0x00, 0x72, 0x01, 0x4E, 0x75]);
    // DTP_InitSound
    let init = d.len() as u32;
    d.extend_from_slice(init_prefix);
    d.extend_from_slice(&[0x41, 0xF9]); // lea counter,a0
    abs(&mut d, &mut relocs, counter);
    d.extend_from_slice(&[0x42, 0x50]); // clr.w (a0)
    d.extend_from_slice(&[0x41, 0xF9]); // lea sample,a0
    abs(&mut d, &mut relocs, sample);
    d.extend_from_slice(&[
        0x23, 0xC8, 0x00, 0xDF, 0xF0, 0xA0, // move.l a0,$dff0a0 (AUD0LC)
        0x33, 0xFC, 0x00, 0x10, 0x00, 0xDF, 0xF0, 0xA4, // move.w #16,$dff0a4 (AUD0LEN)
        0x30, 0x3C, 0x01, 0xAC, // move.w #428,d0
        0x4A, 0x6D, 0x00, 0x2C, // tst.w dtg_SndNum(a5)
        0x67, 0x04, // beq.s +4
        0x30, 0x3C, 0x00, 0xD6, // move.w #214,d0
        0x33, 0xC0, 0x00, 0xDF, 0xF0, 0xA6, // move.w d0,$dff0a6 (AUD0PER)
        0x33, 0xFC, 0x00, 0x40, 0x00, 0xDF, 0xF0, 0xA8, // move.w #64,$dff0a8 (AUD0VOL)
        0x33, 0xFC, 0x82, 0x01, 0x00, 0xDF, 0xF0, 0x96, // move.w #$8201,$dff096 (DMACON)
        0x4E, 0x75, // rts
    ]);
    // DTP_Interrupt
    let int = d.len() as u32;
    if interrupt.is_empty() {
        d.extend_from_slice(&[0x41, 0xF9]); // lea counter,a0
        abs(&mut d, &mut relocs, counter);
        d.extend_from_slice(&[
            0x52, 0x50, // addq.w #1,(a0)
            0x0C, 0x50, 0x00, 0x96, // cmpi.w #150,(a0)
            0x66, 0x06, // bne.s +6
            0x20, 0x6D, 0x00, 0x5C, // movea.l dtg_SongEnd(a5),a0
            0x4E, 0x90, // jsr (a0)
            0x4E, 0x75, // rts
        ]);
    } else {
        d.extend_from_slice(interrupt);
    }
    // DTP_EndSound: move.w #$000f,$dff096 ; rts
    let end = d.len() as u32;
    d.extend_from_slice(&[0x33, 0xFC, 0x00, 0x0F, 0x00, 0xDF, 0xF0, 0x96, 0x4E, 0x75]);
    d.resize(d.len().next_multiple_of(4), 0);
    // the tag array: DTP_CustomPlayer, then the functions and the title
    let values = [1, name_ptr, range, init, int, end];
    for (i, (&t, &v)) in TAGS.iter().zip(&values).enumerate() {
        let at = tags + i * 8;
        d[at..at + 4].copy_from_slice(&t.to_be_bytes());
        d[at + 4..at + 8].copy_from_slice(&v.to_be_bytes());
        if i > 0 {
            relocs.push(at as u32 + 4);
        }
    }
    // the hunk file: header, one code hunk, its relocations
    let mut f = Vec::new();
    for v in [0x3F3, 0, 1, 0, 0, d.len() as u32 / 4, 0x3E9, d.len() as u32 / 4] {
        long(&mut f, v);
    }
    f.extend_from_slice(&d);
    for v in [0x3EC, relocs.len() as u32, 0] {
        long(&mut f, v);
    }
    for r in relocs {
        long(&mut f, r);
    }
    long(&mut f, 0);
    long(&mut f, 0x3F2);
    f
}
