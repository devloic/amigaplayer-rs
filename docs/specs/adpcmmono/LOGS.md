# Reference logs (ADPCM mono)

All logs come from the reference 68k player run through the host with DELI_WRITE_LOG and DELI_DMA_LOG,
player selected with `--player 'Adaptative Delta Pulse Code Modulation'` from the UADE players directory,
`--wav out.wav --seconds 3..5`, SDL_AUDIO_DRIVER=dummy, empty DeliPlayer/DeliGenie directories.

No real `.ADPCM` module exists in the corpus (the files whose names contain "adpcm" are RIFF WAV with
IMA ADPCM and XM modules: other formats). All modules here are SYNTHETIC; `logs/modules/make_modules.py`
regenerates them byte for byte (run it in a directory with a `mods/` subdirectory).

| module | size | content | samples L | interrupts logged | notes |
|--------|------|---------|-----------|-------------------|-------|
| sine.ADPCM | 42 562 | IMA-encoded 440 Hz sine, gated loud/soft | 85 124 | 84 | no saturation; end in a partial buffer (132 samples) |
| random.ADPCM | 20 001 | random bytes, odd size | 40 000 | 39 | many saturations / index clamps; odd byte ignored; partial end (64) |
| short.ADPCM | 777 | random, odd | 1 552 | 3 | ends in the first interrupt (528 samples + zeros), then restart |
| exact.ADPCM | 1 024 | random | 2 048 | 3 | exact multiple: a full silent buffer, cheap fill (4068 cc), then restart |
| mixed.ADPcm | 3 000 | random | 6 000 | 6 | mixed-case suffix accepted; partial end (880) |
| zeros.ADPCM | 8 192 | all zero | 16 384 | 17 | index clamped low on every sample |
| loud.ADPCM | 8 192 | codes 7/F only | 16 384 | 17 | index stuck at 88, heavy saturation |
| mix2.ADPCM | 16 384 | random with quiet runs | 32 768 | 34 | mixed clamp/saturation counts |
| tiny.ADPCM | 300 | random | 600 | 84 | shorter than one buffer: no song end, plays memory past the module (README) |
| sine.adpcm | 4 000 | | | – | REJECTED by the player (lower-case suffix) |
| ADPCM2hdr.ADPCM | 1 006 | starts with `ADPCM2` | | – | REJECTED (ADPC magic) |

Files: `<module>.write.log` (lines `ADDRESS VALUE COLOUR-CLOCK`, first line `# module BASE SIZE`),
`<module>.dma.log.gz` (lines `CLOCK CHANNEL ADDRESS WORD`: every audio DMA word fetched).

## Measured timing (colour clocks)

- Init: VOL0 → VOL1 6, → VOL2 8, → VOL3 6; VOL3 → AUD0LC = init decode cost; LC to LC 8; AUD3LC → AUD0LEN
  18; LEN to LEN 6; AUD3LEN → AUD0PER 8; PER to PER 6; AUD3PER → INTENA 0x8080 85; → DMACON 0x800F 10.
  Each gap was the same in all 9 runs.
- Interrupt: INTENA 0x0080 → INTREQ 0x0080 8 (287 of 287); INTREQ → AUD0LC = fill cost; LC to LC 8;
  AUD3LC → INTENA 0x8080 16 (287 of 287).
- First INTENA 0x0080 = first channel-0 DMA fetch + 68 (7 of 7 runs checked against the DMA log). The
  DMACON write to first fetch gap varies with the raster position (122..212 cc observed).
- Interrupt spacing: 128 000 − 23 after the first, then exactly 128 000.
- Full decode: 63 507 + 10 per saturation + 6.5 per index clamp: 190 of 194 full decodes exact to
  the cc after rounding, 4 off by 2.
- Partial end: decode cost + 133 + 15 per zeroed longword (fits all 5 partial cases exactly).
- Silent end buffer: 4068 (4 of 4).

## Validation

A private model written from SPEC.md (decoder + timing rules; it takes from each reference run only the
time of the first volume write and of the first DMA fetch) reproduces:

- the audio DMA word stream of all four channels byte for byte for the 8 normal modules (including the
  zero padding, the silent buffer and the restart after the song end);
- every register write (address and value) of the 8 normal write logs, in order, and every write time
  within 3 cc (max 1 cc on 5 of 8 modules, 2-3 cc on exact/zeros/loud/mix2).

`tiny.ADPCM` is reproduced only up to the end of the module's data (byte 600 of the stream): past it the
reference decodes host memory.
