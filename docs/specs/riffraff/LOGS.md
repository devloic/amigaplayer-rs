# Riff Raff: reference logs

All logs are register-write logs of the 68k reference (the UADE player "RiffRaff", internal name
"Riff Raff", run through the host with DELI_WRITE_LOG; audio to a WAV with
`SDL_AUDIO_DRIVER=dummy`). Each line is ADDRESS VALUE COLOUR-CLOCK; the first line
`# module 0045e0 SIZE` gives where the host loaded the module file. They can be reproduced with
`logs/runref.sh MODULE OUT.log [seconds] [subsong]` (it reads REF, PLAYERS and EMPTY from the
environment). Note that `--player` needs the name with the space: `'Riff Raff'`. The offline render
stops at the first song end.

## Real modules

Four genuine modules (hunk executables with the `RIFFRAFF` header) were provided by the lead. They
are not copied into the spec folder.

| module file | log name | logs | song end |
|---|---|---|---|
| `RIFF.BombFusion` (45016 bytes) | `bombfusion` | 10 s, 60 s, full | 169.0 s |
| `RIFF.Protector` (36800 bytes) | `protector` | 10 s, 60 s, full | 115.3 s |
| `RIFF.SideWinder2` (20976 bytes) | `sidewinder2` | 10 s, 60 s, full | 84.6 s |
| `RIFF.T'Bird` (54044 bytes) | `tbird` | 10 s, 60 s, full (subsong 0) | 161.4 s |
| `RIFF.T'Bird` subsong 1 | `tbird.sub1` | 60 s, full | 188.2 s |

Files in `logs/`: `<name>.10s.ref.log` (the lead's 10 s logs, unchanged), `<name>.60s.ref.log`, and
`<name>.full.ref.log.xz` (to song end, xz-compressed; decompress before comparing).

What they show:

- Start-up (SPEC 4.1): 8 host volume writes of 0, the init routine's own writes (DMA off, location,
  period, length, volume 0x40 for all voices, plus the zero-buffer locations in the three modules
  with clearing sites), then the first tick 70940 cc after the init ends.
- Ticks every 70940 cc. Bomb Fusion (no routine A): volume, period and audio-control writes per voice,
  note starts (DMA off, location, length, period), a wait, DMA on, a wait, loop location and length.
  The other three: routine A's writes, the waits around the wrapper's DMACON write, then routine B's
  loop location and length writes.
- Location values: inside the hunk (hunk address + offset) or the zero buffer 0x006AC650.
- T'Bird subsong 1 (SPEC 4.2): the subsong-0 init, the stop routine (DMACON 0x000F, four volume 0
  writes), two host volume rounds, the subsong-1 init; it is a 3-voice song (no voice-3 writes
  during play, voice 3 not counted for song end, 12 more cycles at the end of the init call).
- Song end (SPEC 7): each full log ends in the tick where the last tracked voice restarts its
  sequence. In Bomb Fusion the writes after the report are one colour clock earlier than an
  un-rebased count would give (the re-basing of SPEC 3).

## Synthetic modules (first pass)

`modules/` holds three synthetic executables written for the first pass (not Riff Raff's driver: a
minimal routine using some of the replaced shapes), with logs `riff.synth4.ref.log`
(+ `riff.synth4.dma.log`, DELI_DMA_LOG), `riff.synth3.ref.log` and `riff.synthnp.ref.log`. In
these, the header field at 0x18 was used as a routine returning a DMA value (SPEC 5), voice 3 is
tracked or not by bit 3 of the mask word, and song end comes after 48, 24 and 24 ticks. Their hunk
data was placed 0x158 above the placement rule of SPEC 2.1 (file length 676).

## Validation

A private model written from SPEC.md alone (driver run on the repo's 68000 core with the cycle
table, sites as host traps) was compared with every log: all writes equal in address, value, order
and colour clock, for all 10 s, 60 s and full-length logs of the four real modules, both T'Bird
subsong 1 logs, and the three synthetic logs (given their hunk address). The repo's
`tools/replay-cmp.py` also reports "no difference" for a run with the hunk at a different address
(it reports the constant offset).

## Not covered

Master volume or balance other than 64, voice muting, subsong changes during playback, and modules
with more than one hunk.
