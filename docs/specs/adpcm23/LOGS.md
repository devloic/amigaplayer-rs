# Reference logs (ADPCM2 / ADPCM3)

All logs come from the reference 68k Eagleplayer `ADPCM` run through the host with DELI_WRITE_LOG and
DELI_DMA_LOG, `--player ADPCM`, `--wav ... --seconds N`, SDL_AUDIO_DRIVER=dummy, empty DeliPlayer and
DeliGenie directories.

Important: the stock host renders silence for this player (it never calls the AudioStruct interrupt, SPEC 3):
`dancing.adpcm.stock-host.write.log` shows the start writes and nothing else. Every other log here was made
with the host extended by the one missing piece of SPEC 3 (on each channel-0 audio interrupt the host calls
the player's AudioStruct-0 interrupt; the host was also told to keep rendering after song end for the
synthetic files). The 68k player code itself is unchanged and does all the decoding, so the DMA logs are the
original decoder's output.

Files: `<module>.write.log` (lines `ADDRESS VALUE COLOUR-CLOCK`, first line `# module BASE SIZE`),
`<module>.dma.log.gz` (lines `CLOCK CHANNEL ADDRESS WORD`: every audio DMA word fetched; for the three real
files only the first 5 s, `.dma-5s.log.gz`, the write logs are complete).

## Modules

Real files (local test copies, not in the repo): `dancing.adpcm` (ADPCM3, 16 726 Hz, 1 368 925 bytes, Dancing
Moon by Sidewinder, 1996) and `ParaMiAmiga1.PCM` / `ParaMiAmiga2.PCM` (ADPCM2, 8 526 Hz, 369 834 and 371 075
bytes, Para Mi Amiga by David Pleasance, split in two by the uploader). No other ADPCM2/3 file was found.
Synthetic files: `logs/modules/make_modules.py` regenerates them byte for byte.

| module | type | rate | per | data bytes | content | logged |
|--------|------|------|-----|------------|---------|--------|
| dancing.adpcm | 3 | 16 726 | 212 | 1 368 915 | music, 218.2 s; last block partial (339 bytes) | to song end (3566 refills) |
| ParaMiAmiga1.PCM | 2 | 8 526 | 416 | 369 824 | music, 173.5 s; partial end (160 bytes) | to song end (1445) |
| ParaMiAmiga2.PCM | 2 | 8 526 | 416 | 371 065 | music, 174.1 s; partial end (121 bytes) | to song end (1450) |
| syn3_exact | 3 | 16 726 | 212 | 1 536 | random, 4 whole blocks | 2 s, 8 restarts |
| syn3_part100 | 3 | 16 726 | 212 | 1 252 | random, partial 100 (99 decoded) | 2 s |
| syn3_part2 | 3 | 16 726 | 212 | 1 154 | random, partial 2 (one group read past the end) | 2 s |
| syn3_loud | 3 | 22 050 | 160 | 2 304 | all codes 7 (heavy clamping) | 2 s, overloaded (SPEC 4.4) |
| syn3_quiet | 3 | 11 025 | 321 | 1 536 | zeros then random | 2 s |
| syn2_exact | 2 | 8 526 | 416 | 1 536 | random, 6 whole blocks | 2 s |
| syn2_part101 | 2 | 8 526 | 416 | 1 381 | random, partial 101 (99 decoded) | 2 s |
| syn2_loud | 2 | 8 000 | 443 | 2 048 | all codes 3 | 2 s |
| syn2_alt | 2 | 12 000 | 295 | 2 048 | alternating patterns | 2 s |

## Measured timing (colour clocks)

- Start: DMACON 0x0003 first (3028 / 3092 cc into the run for ADPCM2, 3176 / 3304 cc for ADPCM3: it
  depends on the module), then AUD0LC +7787, LEN +89, PER +95, VOL +85, AUD1LC +91, LEN +89, PER +95,
  VOL +85, DMACON 0x8003 +102. Identical in all 13 runs.
- First channel-0 DMA fetch F: 31–209 cc after DMACON 0x8003 (raster).
- Refill k ≥ 1: AUD0LC at `F + 227 + (1022 + 1024·(k−1))·per + 117`, AUD1LC 19 cc later (all refills of
  all non-overloaded runs, 0 cc error). Refill 0: AUD0LC 2232–2243 cc after F (host's first service).
- Refill spacing: exactly 1024·per (217 088 / 425 984 / ...), except refill 0 → 1.
- Fill cost (extra overload runs at per 124, not in logs/): ADPCM2 144 500–158 800 cc per 1024 samples, ADPCM3
  158 400–176 700 (all-zero codes cheapest, all-maximum codes dearest).

## Validation

A private model written from SPEC.md (decoder of SPEC 2, stream rules of SPEC 4.2, timing of SPEC 4.4; it
takes from each reference run only the time of the first write and of the first channel-0 DMA fetch, and the
time of refill 0) reproduces:

- the channel 0 and channel 1 DMA byte streams byte for byte: all three real files from the start to song end
  (3 651 354, 1 479 300 and 1 484 416 bytes per channel, including the partial last blocks) and all 9
  synthetic files over 2 s with several restarts, except `syn3_loud` from its 15th block on, where the
  reference is overloaded and plays a buffer twice (SPEC 4.4);
- every write (address, value and clock) of 11 of the 12 interrupt-driven write logs exactly (14 276 writes for
  Dancing Moon); `syn3_loud` matches in addresses and values, its times slip by the overload.

The plain one-pass decode (SPEC 5) of the real files gives 3 650 440 samples (218.25 s at 16 726 Hz) and
1 479 296 / 1 484 260 samples (173.5 s / 174.1 s at 8 526 Hz). Readme durations: "4 minute" for Dancing Moon;
none for Para Mi Amiga. The decoded audio is plausible: rhythmic broadband transients for the dance track,
harmonic lines for the guitar (spectrograms), lag-1 correlation 0.54 / 0.87, RMS 42 / 14. As an independent
check of the codec identification, an AVM ZyXEL 2-bit voice file (16-byte ZyXEL header skipped) decodes with
the same decoder to plausible speech.

No log exists for the standalone PlayADPCM: its Paula writes are audio.device's, and the host cannot run a
CLI program.
