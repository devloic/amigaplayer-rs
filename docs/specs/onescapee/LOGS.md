# Logs and reproduction

Write-log format: `ADDRESS VALUE COLOUR-CLOCK` per line (DELI_WRITE_LOG), absolute colour clock from
reset, first line `# module BASE SIZE`. Audio DMA logs (DELI_DMA_LOG): `CLOCK CHANNEL ADDRESS WORD`, one
line per fetched word. All logs are of the reference 68k player (the UADE `onEscapee` Eagleplayer) run
through the host with `--wav` to a dummy output, `SDL_AUDIO_DRIVER=dummy`; nothing was played through
speakers. The run ends at the first 0.1 s render step after the player reports its song end, so the
tail of each log runs up to 0.1 s past the song end interrupt (this is why some logs show the loop).

## No real module

The module archive holds only two files with the `one.` prefix (`ONE.MOD`, `one.mod`); both are
ProTracker modules ("two, three, four!") and the player rejects them. No real onEscapee module was
found, so all logs are of SYNTHETIC modules in `modules/`, built by encoding a signed 8-bit test
signal (two sines plus steps) with the key of SPEC 1.1; the first 96 bytes are zero (24 x AA55FF00 in
the file) so detection passes.

| module | size (bytes) | what it exercises |
|---|---|---|
| one.a | 20 000 | 19 full buffers + 544-byte tail (song end in the middle of a buffer) |
| one.b | 8 192 | exact multiple of 1024: last full buffer (d = 6639), then clear-only buffer |
| one.d | 5 001 | size not a multiple of 4 (last byte ignored), 904-byte tail |
| one.e | 3 000 | short, 952-byte tail |
| one.r0 / r4 / r100 / r512 / r1020 | 2 048 + 0/4/100/512/1020 | tail lengths 0, 4, 100, 512, 1020 (fits the partial-buffer cost) |
| one.r3072 / one.r4096 | 3 072 / 4 096 | exact multiples; the log continues after the song end and shows the loop |
| one.t_* | 4 096 | detection tests only (SPEC 1.2): all24A, j5, j0, allB are accepted |

## Files

`logs/one.<m>.ref.log` for each module above; `logs/one.e.ref.dma`, `logs/one.r4096.ref.dma`
(audio DMA words, channels 0 to 3). Reproduce with:

    SDL_AUDIO_DRIVER=dummy RUST_LOG=error EAGLEPLAYERS=<UADE players dir> DELIPLAYERS=<empty dir>
    DELIGENIES=<empty dir> DELI_WRITE_LOG=out.log DELI_DMA_LOG=out.dma
    amigaplayer-rs --player onEscapee --wav out.wav --seconds 5 modules/one.a

Use `tools/replay-cmp.py REF.log PORT.log` to compare a port against a reference log. The absolute
times assume the host's fixed start phase (DMA enable at cc 7626); relative times (SPEC 2, 3) are what
matters.

## Facts checked in the logs

* Handler cost table (SPEC 3.5) for every handler in the 11 logs.
* Interrupt times E+160, E+227271 + (n-1)*227328.
* Pointer alternation B0, B1, B0, ...; buffer addresses B0 = 0x646D58, B1 = 0x647158 in these runs.
* The DMA words of channel 0 equal the decoded module bytes in order (chunk sequence of SPEC 3.3/3.4,
  zero-padded last buffer, loop), and all four channels fetch identical words.
* Word fetch spacing in the reference Paula: 454 cc (mostly) or 227 cc, average 444 = 2 * period.
