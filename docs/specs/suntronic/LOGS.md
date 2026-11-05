# SUN-Tronic: reference logs

All logs are register-write logs of the 68k reference (the Eagleplayer "SUN-Tronic" running the unmodified module
on the emulated machine, run through the host with `DELI_WRITE_LOG` set; audio to a null WAV with the dummy SDL
driver). One line per Paula register write: `ADDRESS VALUE COLOUR-CLOCK`; the first line is `# module BASE SIZE`
(load address and size of the module). A 32-bit AUDxLC write appears as two lines with the same clock. They were
captured with only the SUN-Tronic player installed. `tools/replay-cmp.py REF.log PORT.log` compares logs tick by
tick.

## Modules

| log | module | kind | ticks | writes |
|---|---|---|---|---|
| `shades.log` | `shades.sun` (variant A, 3 642 bytes) | real, from the demozoo archive; not shipped | 3 074 (to the song end; the host stops 2 ticks after the first wrap) | 34 687 |
| `msx-cracktro2.log` | `msx-cracktro 2.sun` (variant B, 4 186 bytes) | real, from the demozoo archive; not shipped; first 40 s | 1 999 | 19 935 |
| `a001.log`, `a004.log` | `modules/a001.sun`, `modules/a004.sun` (variant A) | synthetic (README) | 1 189 each (to the song end) | 19 335, 19 089 |
| `b001.log`, `b003.log` | `modules/b001.sun`, `modules/b003.sun` (variant B) | synthetic; 60 s | 2 999 each (only the first 1 272 / 1 434 ticks are defined, SPEC 7.6) | 37 299, 36 165 |

The other synthetic modules in `modules/` (`a002`, `a003`, `a005`, `a006`, `b002`) are shipped without logs;
reference logs are made with the host exactly as above.

## What the logs confirm

- Every register write (address, value, order, tick-relative time) of the logs above equals the value produced by
  SPEC 3 to 8, for the two real modules in full and for the synthetic runs, with the one exception of SPEC 8.4
  (a one-colour-clock deviation in the write after the song-end notification of two wrap ticks).
- The DMA stream (words fetched by the audio DMA) equals the buffer content of SPEC 5.5, 5.7 and 5.8 for
  `shades.sun` (1 900 ticks), `msx-cracktro 2.sun` (1 900 ticks) and `a001.sun` (all 1 189 ticks, all waveform
  types 0 to 4).
- Tick spacing of the host: nominally 70 940 cc, with deviations of a few cc that alternate; times relative to the
  call start are exact.
- Variant A idle ticks contain exactly nine writes (DMACON, 4 x VOL+PER, DMACON), variant B idle ticks eight.
- Effects 0x97, 0x96, 0x95, 0x93, 0x92, the fade, the sample-instrument start and the end-of-song marker never
  occur in the real modules; the first five are exercised by the synthetic modules, the last two are not
  specified.
- The song-end notification of variant A (SPEC 5.10) is reached in `shades.sun` and in every variant-A
  synthetic module; the real module wraps after 3 072 ticks.

## Typical timing (cc from the call start, variant A, idle tick)

| write | cc |
|---|---|
| DMACON (clear) | 219 |
| ch0 VOL, PER | 245, 255 |
| ch1 VOL, PER | 320, 330 |
| ch2 VOL, PER | 396, 406 |
| ch3 VOL, PER | 471, 481 |
| DMACON (set) | 2 200 .. 5 500 (SPEC 8.3) |
