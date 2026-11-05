# Logs and reproduction

Write-log format: `ADDRESS VALUE COLOUR-CLOCK` per line, first line `# module BASE SIZE`. Logs come from
the reference 68k player run through the host with DELI_WRITE_LOG (player name "TFMX ST"), silent
(`--wav`, SDL_AUDIO_DRIVER=dummy), 5-10 s per run. Logs over 40 KB are xz-compressed.

Each log starts with the host's stop-sound writes (DMACON $000F, AUDxVOL = 0); skip them up to the
first AUDxPER = 1 write. Sample addresses in AUDxLC writes are the host's buffer addresses: compare the
relative layout (pulse, silent = pulse + 4, noise = pulse - 1024, companion sample data).
The host plays a few ticks past the song-end signal; those writes are part of the logs.

| path | content |
|---|---|
| hc/mdst.h1, h1m | pattern loop / call / return, $FE row, track $FE with channel 6, mutes (h1m mutes track 1), track fade and stop |
| hc/mdst.h2 (+ .s1, .s2 logs) | three songs; `.s1`/`.s2` = `--subsong 1` / `--subsong 2` |
| hc/mdst.h3 | explicit-offset table layout |
| hc/mdst.h4, h4b + smpl.* | digital samples, 4-bit and 8-bit companion files |
| hc/mdst.h5 | track loop with count 0 |
| hc/mdst.h6 | all tracks stopped |
| hc/mdst.h7 | fades |
| fuzz/mdst.f* (+ smpl.*) | random modules from fuzz.py (seed = number); those with a smpl file use digital samples |
| gen.py, hc.py, fuzz.py | my module builders (hc.py writes the hc set, `fuzz.py OUT A B` writes seeds A..B-1) |

All modules are synthetic: no real TFMX ST module was available. The fuzz generator avoids the
beam-random riff variant, returns without a prior call, and unterminated zero-wait loops.
