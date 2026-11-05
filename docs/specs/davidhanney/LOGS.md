# Logs and reproduction

The files named below are not shipped; regenerate them as described. Write log format: `ADDRESS VALUE COLOUR-CLOCK` per line (DELI_WRITE_LOG), clocks absolute from
reset. Note the first group of a log (5 lines at 9.6 M) is the start call (DMACON clear + four
volume 0 writes); ticks are separated by gaps of more than 30000 clocks (tick spacing is exactly
70940 clocks in this player).

| file | content |
|---|---|
| title.ref.log, jingles.ref.log, unused.ref.log, world_1_woodland.ref.log ... world_5_futureland.ref.log | reference write logs of the 8 known modules (Tearaway Thomas, `dh.*` files of the module corpus), 20 s each (title ends earlier, at its song end: 959 ticks) |
| title.dma.log, world_1_woodland.dma.log | DELI_DMA_LOG of the same runs: `CLOCK CHANNEL ADDRESS WORD`, one line per Paula DMA word fetch |
| title.gaps.tsv, world_1_woodland.gaps.tsv | per-write gap tables (gaps_tsv.py) |
| syn/dh.fx_mix, dh.port_mix, dh.speed0, dh.speed8, dh.hinote, dh.note2 | synthetic modules made for this spec (gen_syn.py), valid in the format: random notes/effects (fx_mix, port_mix, speed0 = speed 0, speed8 = speed 8, hinote = notes up to 130, note2 = note value 2) |
| syn/*.ref.log | their reference write logs (40 s cap) |
| cmpticks_public.py, gaps_tsv.py | tick-relative compare of two write logs; gap table |

## How the logs were made (68k reference, silent)

    SDL_AUDIO_DRIVER=dummy RUST_LOG=error DELIPLAYERS=EMPTY_DIR DELIGENIES=EMPTY_DIR DELI_NATIVE=0 \
    EAGLEPLAYERS=<UADE players dir> DELI_WRITE_LOG=out.log DELI_DMA_LOG=out.dma.log \
    amigaplayer-rs --wav out.wav --seconds 20 dh.title

(Do not pass `--player DavidHanney`: the player name in that binary is the Eagleplayer's, found by
detection. The Eagleplayer "David Hanney" in EP 2.06 is the same player. No `--subsong`: one song per
file.) The binary has `DELI_TRACE=file` (+ `DELI_TRACE_FLOW=1`): it records each call, each host
function call with the clock jump, and every taken jump with its clock; the timing model was fitted
from those traces and then verified on the plain write logs.

## What the logs confirm

1. Values and order of all register writes (SPEC 5, 6): a model built from the spec text
   reproduces every (register, value) of all 8 real and 6 synthetic logs, 0 differences (location
   values compared only by instrument/sample identity, because they are memory addresses).
2. The DMA logs confirm the pulse sweep (SPEC 6.8): all words the DMA fetched from the synth waves
   (154 563 words in title, 205 096 in world_1) equal the model's wave bytes at the tick of the
   fetch or the one before.
3. Timing (SPEC 8): the cycle model predicts the clock of every write of the 14 logs exactly.
4. Song end (SPEC 1.3): the reference stops 3 ticks after the song-end call in 12 of the 14 files
   (title 959 ticks; jingles 1534, futureland 1279), 0 ticks after in the speed-0 and speed-8 synthetic files.

## Compare a port against the reference

    python3 -I tools/replay-cmp.py title.ref.log port.log

Port log: `EAGLEPLAYERS=<empty dir> DELI_WRITE_LOG=port.log amigaplayer-rs --open-replayers --wav /dev/null --seconds 20 dh.title`.
The location values of the reference are RAM addresses (wave of instrument i = bank base + 0x5E * i + 10; the
bank base is the same for all modules); a port with another memory map has other location values.
