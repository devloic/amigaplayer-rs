# Rob Hubbard: reference logs

The files named below are not shipped; regenerate them as described. All logs are register-write logs of the 68k reference (the Eagleplayer "Rob Hubbard" running the
unmodified module on the emulated machine), one line per Paula register write with its colour clock.
Captured with a script `runref.sh MODULE SUBSONG OUT.log [seconds]` (sets `DELI_WRITE_LOG`; audio goes to
`--wav /dev/null` with `SDL_AUDIO_DRIVER=dummy`; release build of this repository, default 20 s).
`rundma.sh` is the same with `DELI_DMA_LOG` (audio DMA word fetches), used for the waveform
modulation check. `gaps_tsv.py` turns a log into per-write gaps; `cmpticks_public.py`
compares two logs tick by tick (tick-relative times; `tools/replay-cmp.py` does the same).

## Modules

| name | generation | subsongs logged |
|---|---|---|
| Budokan `rh.Budokan` | C | 0 to 5 |
| Populous `rh.Populous` | B | 0 |
| Bards_Tale_3 `rh.driver` | A | 0 to 7 |
| PGA_Tour_Golf `rh.FLYMUS` | A | 0, 1 |
| PGA_Tour_Golf `rh.GMUSIC` | A | 0 |

Synthetic modules (edited copies of corpus modules; they change only data bytes in streams,
instrument records and the song table, never code), made to execute paths the originals never use:

| log | edit | exercises |
|---|---|---|
| synA_budokan_s0, s2; synA2_budokan_s0, s2 | slide, vibrato and waveform-modulation instruments and commands inserted | slide, vibrato (with and without wrap), modulation up/down, 0x83, 0x86, unknown command, 0x87, zero loop offsets |
| synB_populous_s0 | same idea on generation B | rest command (0x82) with the 0x100-word silent area, effects of gen B |
| synC_bards3_s0, s3 | same on generation A | effects and 0x80 with operand >= 0x80 (default instrument), no-instrument start |

## What they confirm

- Every register write (address, value, order) of every log above equals the value produced by the
  rules of SPEC.md sections 2 to 7, for all real logs and all synthetic ones except the item below.
- Tick-relative time (colour clock minus the clock of the tick's first write) equals the SPEC.md
  section 8 model exactly for all of them: about 61 000 writes in the corpus logs and 58 000 in the
  synthetic logs, zero differing ticks.
- Tick spacing is 70 940 cc, over about 1 300 ticks.
- Waveform modulation: the DMA log of synA_budokan_s0 (104 719 audio words) equals the model's
  sample memory word for word.
- Real-module coverage: slide, vibrato, modulation, commands 0x83, 0x86, unknown commands and the
  generation B rest never execute in the five real modules; their behaviour and timing are confirmed
  only on the synthetic logs.
- The 0x85 to 0x84 rewrite (SPEC 7.1) is visible in Budokan subsong 2.
- Default instrument = number of entries of voice 3's list (SPEC 7.2) is visible in Bards_Tale_3
  subsongs 3 and 7.
- The `Hardware` (position) voice costs 32 more cycles on its 0x84; without this, Budokan shows
  +-16 cc deltas.

## Not matched

synC_bards3_s3 reads the volume of a voice that starts a note with no instrument selected from
memory beyond the 8-byte zero block of the host; the model cannot reproduce that value. Only that
one volume write differs.

## Typical gaps (cc), Budokan subsong 3

| channel | LC to LEN | LEN to VOL | VOL to PER | PER to DMACON |
|---|---|---|---|---|
| 0 | 62/63 | 163 | 190/191 | 74/75 |
| 1 | 78 | 196 | 206 | 90 |
| 2 | 93/94 | 217 | 221/222 | 105/106 |
| 3 | 97/98 | 216 | 225/226 | 109/110 |

Generation A and B have LEN to VOL of about 150, 177/178, 204/205, 204 for channels 0 to 3.
Gap TSVs: `budokan_s0.gaps.tsv`, `populous_s0.gaps.tsv`.
