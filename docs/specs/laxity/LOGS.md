# Laxity: reference logs

All logs are register-write logs of the 68k reference (the Eagleplayer "Laxity-Player" from the
UADE player set, run through the host with DELI_WRITE_LOG), one line per Paula register write or
CIA-A PRA write (`ADDRESS VALUE COLOUR-CLOCK`), first line `# module LOAD-ADDRESS SIZE`. Captured
with `--player Laxity-Player --wav OUT.wav --seconds 30..60`, `SDL_AUDIO_DRIVER=dummy`, empty
DeliPlayer and DeliGenie directories. The host stops the render shortly after the first song end,
so each log ends there (syn2 never reaches the song end: it loops by a position jump and is cut at
60 s). Runs with `DELI_NATIVE=0` (interpreted 68k) give byte-identical logs.

## Modules

No module of this format exists in the archive that was searched (README), so all modules are
synthetic, built by `mods/mk.py` (`mods/gen1.py`, `mods/gen2.py`; run from `mods/`, output in
`mods/syn/`). Every module carries the `YET ANOTHER COOL` signature and two synth instruments 1
and 2 (required by the detection).

| log | module | writes | ticks | exercises |
|---|---|---|---|---|
| syn1 | powt.syn1 | 390 | 188 | basic notes, synth and sample instruments, one-shot sample, vibrato, arpeggio, slides, empty cells with volume 0, song end |
| syn2 | powt.syn2 | 17 385 | 2 999 | all commands 0..15, volume column 0x8X/0x9X/0xA5, hold (0xFE) and mute (0xFF) positions (so the sample start skips 256 patterns), zero-length sample note, notes 1..63, position jump looping forever |
| syn3 | powt.syn3 | 237 | 39 | speed changes, pattern break, volume column 0xFE (break on every tick), position jump with parameter 0, synth widths |
| syn4 | powt.syn4 | 437 | 49 | instruments 17..22 (high instrument bits), finetune -3..+3, note without instrument (finetune from file offset 6), instrument without note, speed 1 and 0, voices without an instrument on the first row (stale loop registers) |
| syn5 | powt.syn5 | 1 057 | 236 | period clamp at 0x71, volume clamps, slow slide, retrigger, mute/hold positions, tone portamento downward |
| syn6 | powt.syn6 | 427 | 69 | notes 61..63 (tables read past their end), arpeggio 0xFE/0xEF, instrument 36, speed 0, volume 0x7F/0x8F |
| syn7 | powt.syn7 | 184 | 92 | tone portamento running below period 0 (target 0 written, direction cleared) |
| syn8 | powt.syn8 | 238 | 44 | volume byte wrapping past 0x7F without clamp, slide down from a negative volume, command 12 with parameter above 0x40 |

## What they confirm

- Every write (address, value, order and absolute colour clock) of all eight logs equals the
  output of a private model built from SPEC.md, given only the first call's clock (first write
  - 427 cc), the module's load address from the log's first line and the one-shot zero-word
  address 0x0095505C: 20 355 writes over 3 716 ticks, no difference. `tools/replay-cmp.py`
  reports "no difference" as well.
- Every executable path of the play routine ran in at least one log (checked with the host's
  instruction trace); the paths that cannot run are the upward tone portamento and a negative
  result of the volume bias (README).
- DMA logs (`DELI_DMA_LOG`, not shipped) of syn1, syn3 and syn6: 47 930 audio words fetched from the
  synth buffers equal the model's buffer contents at that time; 33 further fetches differ and all
  of them are either a fetch the host logged a few hundred cc before the synth update that it
  already reads (the host evaluates audio DMA lazily, so the logged clock of a fetch can precede
  the memory access) or a fetch after the model's last tick; 219 fetches that fall inside a synth
  update were not judged.
- Tick spacing 70 940 cc; the first write of every log is 427 cc after the first call.
