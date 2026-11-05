# Mark II: reference logs

All logs are register-write logs of the 68k reference (the UADE Eagleplayer "MarkII", internal
name `Mark_II`, run through the host with DELI_WRITE_LOG; audio to a temporary WAV with
`SDL_AUDIO_DRIVER=dummy`, nothing played on speakers). Each line is ADDRESS VALUE COLOUR-CLOCK;
the first line `# module 0045e0 SIZE` gives where the host loaded the module file. Reproduce
with `logs/runref.sh MODULE OUT.log [seconds]` (it reads REF = the amigaplayer-rs binary,
PLAYERS = the directory holding the Eagleplayer, EMPTY = an empty directory, from the
environment). Runs are deterministic.

## Modules

Six of the 24 real modules (demozoo `.mk2` files), copied into `modules/`, chosen to cover the
three driver builds, both start phases of the gate/row flag, four record sizes and the slide
update of the gate tick:

| module file | bytes | build | rows x steps | first tick | writes 10 s | writes full | song end |
|---|---|---|---|---|---|---|---|
| `quadlite-wabb.mk2` | 20122 | A | 48 x 32 | gate | 5501 | 33836 | 61.44 s |
| `movement_oldskool.mk2` (demozoo `movement oldskool.mk2`) | 13174 | A | 24 x 32 | row | 5521 | 16961 | 30.70 s |
| `atron.mk2` | 56692 | C | 150 x 24 | gate | 5501 | 79314 | 144.01 s |
| `defcon1-contact.mk2` | 23282 | B | 39 x 32 | row | 5521 | 27521 | 49.90 s |
| `theend.mk2` | 93808 | B | 81 x 64 | gate | 5501 | 114071 | 207.37 s |
| `psychotic.mk2` | 94524 | B | 146 x 48 | row | 5521 | 154241 | 280.31 s |

Files in `logs/`: `<name>.10s.ref.log` and `<name>.full.ref.log.xz` (run to 10 s past the song
end; the render stops at the end of the 0.1 s chunk holding the report; decompress before
comparing). One subsong each (asking for subsong 1 gives the same log).

## What they show

- Start (SPEC 4.1): no host write before the driver's; the first write is DMACON 0x000F of the
  start call (quadlite-wabb 4052, theend 5109, atron 5368), then the four voices' length,
  location (two lines, same clock), period, volume, then DMACON 0x820F. The first tick follows
  70940 cc after the start call's end.
- Gate ticks: a single DMACON write (the retrigger mask, bit 15 clear), plus period writes for
  sliding voices in atron (build C; 24 such ticks in the first 60 s).
- Row ticks: 21 writes: per voice 0..3 length, location pair, period, volume, then DMACON 0x820F.
  The gaps between them come from the driver's code and the site costs (SPEC 5); per voice they
  grow because the sites' voice test costs more for later voices.
- Song end (SPEC 6): from the row of the report on, the writes of voice 2 go to 0xDFF000..0xDFF008
  (all six full logs).

## Validation

A private model following SPEC.md was compared with every log: all writes equal in address,
value, order and colour clock for the six 10 s logs, the six full logs, and 60 s logs of all 24
known modules (not published); `tools/replay-cmp.py` reports "no difference" for the full logs.

## Not covered

The build-B slide update (PERTAB site in build B), the fade command, the player's volume and
balance functions, a module without the 0xD1FA word (SPEC 1.3), and playback continued after the
song end beyond the end of the render chunk.
