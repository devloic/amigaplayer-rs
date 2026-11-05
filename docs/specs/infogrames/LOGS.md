# Logs and reproduction

Folder: `logs/` next to this file. Write-log format: `ADDRESS VALUE COLOUR-CLOCK` per line
(DELI_WRITE_LOG), absolute colour clock from reset; first line `# module BASE SIZE` (load address of
the song file in the reference's memory; 0x45E0 in all runs). Files are xz-compressed
(`xz -dc logs/NAME.ref.log.xz > NAME.ref.log`).

All logs are of the reference 68k player (the UADE players directory, player `Infogrames`) run
through the host with DELI_WRITE_LOG, `--wav /dev/null`, `SDL_AUDIO_DRIVER=dummy`; nothing was
played through speakers. Length: 120 s (12 787 ticks) for the real song files, 40 s for the two
constructed ones.

**Instrument files are synthetic.** No real `.ins` bank was available, so every run used the same
constructed bank `modules/synthetic.ins` (copied next to the song file under the matching name,
1.4 of SPEC). It has 32 entries with distinct lengths and three kinds of loops (one-shot with a
1-word loop on a trailing zero word, a short loop at the end, the whole sample looping); it is made
by `modules/mkins.py`. Register values other than sample addresses/lengths do not depend on the
bank. The bank's load address is not in the log header: in these runs it is
`BASE + (SIZE rounded up to 8) + 0x400` for files over about 1 KB (e.g. 0x5040 for us.dum) and
0x4DE4 for bob4e/bob4f (the host reserves a minimum for the song file); safest is to derive it from
the first sample LC write. tools/replay-cmp.py treats it as one constant offset.

## Files

| log | song file (md5 prefix) | notes |
|---|---|---|
| us | Bivouac / Chamonix Challenge / Final Assault `us.dum` (892a8aa4c38f) | all voices stopped from tick 3 863 |
| bivouac | same games, `bivouac.dum` (022f54c6bdf0) | 6 625 bytes, speed 2; plays past 120 s |
| bivouac3 | same games, `bivouac3.dum` (4a4ed3a64e75) | speed 6; stopped from tick 6 143 |
| bob4a .. bob4f | Bob Morane Ocean / Operation Neptune `bob4a.dum`, `BOB4B..F.DUM` (5f8af8efc84d, c25c9bfeb22b, f6d40b89f1a8, f3190b06a712, 071b7971a1ea, d68b174d48f6) | speed 3 or 5; all stop within 120 s (4e, 4f at tick 1 727) |
| rh2.blood | Captain Blood `rh2.blood.dum` (5204b3eeeb51) | plays past 120 s |
| rh2.bobodors, .bobofil, .bobolav3, .bobomang3, .bobopat3, .bobotram, .interlude, .scores | Stir Crazy featuring Bobo (cf4ff5aa13f2, d6ea4ede32c6, 422a250b153e, f27bee083127, 8817fd8b7bbb, 9e9cc7747fd9, b864467892ed, c9563fb4b27a) | interlude stops at tick 323; bobopat3 plays past 120 s |
| syn1 | `modules/syn1.dum` (constructed, `modules/mksyn.py`) | two songs (the second is played), pitch envelope one-shot (E2) and looping, command aliases E8/E9, no-op E3, volume envelope going below 0 (clamp) and above 64 (written as 80), looping volume envelopes with step, a row spanning a pattern end, note 0, a track that ends early |
| syntempo | `modules/syntempo.dum` (us.dum padded to 2 850 bytes with a filler word so that its checksum is in the tempo list; `modules/mktempo.py`) | ticks 47 360 cc apart |

The Chamonix Challenge, Final Assault, Operation Neptune and Project Neptune copies of these files are
identical to the ones listed. The module index lists two real banks (`rh.ins`, Captain Blood and Stir
Crazy) that
were not available here and that the reference would not find by name anyway (SPEC 1.4).

## Reproduce a reference log (silent)

Put the song file and an instrument file named after it (`NAME.dum` -> `NAME.ins`) in one
directory, then:

    E=/path/to/an/empty/dir
    SDL_AUDIO_DRIVER=dummy RUST_LOG=error DELIPLAYERS=$E DELIGENIES=$E \
    EAGLEPLAYERS=$HOME/.local/opt/uade/share/uade/players DELI_WRITE_LOG=/path/ref.log \
    <reference amigaplayer-rs binary> --player Infogrames --wav /dev/null --seconds 120 /ABS/DIR/NAME.dum

Port log: `EAGLEPLAYERS=$E DELI_WRITE_LOG=/path/port.log <binary> --open-replayers --wav /dev/null
...`, then `python3 -I tools/replay-cmp.py REF.log PORT.log`. A tick with sample starts lasts more
than 20 000 cc (each start waits two times 10 raster lines), so gap-based tick grouping can merge a
long tick with the next one; grouping at each voice-0 volume write (0xDFF0A8, always the first write
of a tick) is exact.

## Timing facts measured

- Calls every 33 285 cc (CIA 0x1A00), 47 360 cc for the tempo-list file.
- The first call comes exactly one tick period after the end of the player's song initialisation
  (checked on 4 files); the initialisation costs 2 259 cc + 27.5 cc per 32-bit word of the song
  file (us.dum 13 479 cc, bivouac.dum 47 799 cc, rh2.interlude.dum 9 409 cc).
- Within a tick, every write time follows the cycle model of SPEC 6 exactly (below).

## What was checked against the logs

A private value-and-timing model written from the behaviour in SPEC.md was run against every log,
tick by tick, comparing the address, value and colour clock of every write (call times: the
emulator's call trace for 8 runs, otherwise first call fitted from the first write and then one
period per tick):

| logs | ticks compared | differing values | differing times |
|---|---|---|---|
| 18 real song files | 18 × 12 787 = 230 166 | 0 | 0 |
| syn1 | 4 262 | 0 | 0 |
| syntempo | 2 995 | 0 | 0 |

All paths of SPEC 4.3 and 3.3 except the undefined ones were exercised (syn1 covers the ones the
real files do not use).

## Addendum from the first implementation (start-up phase)
- The reference's run origin depends on the full path of the song file: about 681.5 cc plus 8.5 cc per character of directory and file name (fitted on 20 logs and on one differently-pathed run). Without it the DMA waits land at a different phase and the audio is only SIMILAR.
- A song found in the tempo list starts earlier: 333 cc for entry 0; the saving per entry, (11 - index) x 333/11, is an interpolation (only entry 0 was observed).
