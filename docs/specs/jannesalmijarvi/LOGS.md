# Janne Salmijarvi Optimizer: reference logs

All logs are register-write logs of the reference 68k player run through the host with
DELI_WRITE_LOG (`--player 'Janne Salmijarvi Optimizer'`, `--wav` with `SDL_AUDIO_DRIVER=dummy`,
`--seconds 40`; the offline render stops at the first song-end notification). Each line is
`ADDRESS VALUE COLOUR-CLOCK`; the first line `# module BASE SIZE` gives the module's load address.
Paula writes are at 0xDFF0xx, CIA-A port A writes at 0xBFE001.

No real module of this format exists in the corpus. The seven modules in `modules/` are synthetic,
built from the format description (title, sample headers, position list, `JS92`, patterns, sample
data of simple waveforms). They contain no code. `modules/README.txt` lists what each one contains.

## Logs

| log | module | interrupts | first interrupt entry E0 (cc) | song end | exercises |
|---|---|---|---|---|---|
| `basic.ref.log` | `JS.basic` | 3479 | 25936 | 17.4 s, wrap | notes, sample changes, looped and one-shot samples, empty cells, command 8 on rows, C, F03, 3 positions |
| `fx.ref.log` | `JS.fx` | 1979 | 25931 | 9.9 s, wrap | arpeggio, slides with limits, tone porta with glissando, vibrato and tremolo (all waveforms, E4x/E7x), volume slides, 5, 6, D00 |
| `ecmd.ref.log` | `JS.ecmd` | 1419 | 25941 | 8.8 s, wrap | E0, E1, E2, E3, E5, E6 (loops), E8, E9, EA, EB, EC, ED (incl. ED0), EE, EF; 9xx (fits / too large / remembered); F00; tempos 0x40, 0x9A, 0xFF, 0x7D; D30 (raw row 48); retrigger and new notes in the same tick |
| `short.ref.log` | `JS.short` | 539 | 25931 | 2.7 s, wrap | speed 2, one position |
| `bjump.ref.log` | `JS.bjump` | 119 | 25936 | 0.6 s, B command | the first B command ends the render |
| `fresh.ref.log` | `JS.fresh` | 1539 | 25931 | 7.7 s, wrap | first slide of the song moves by 0 (slide mask), arpeggio before any note (special table word, last entry of the previous finetune row), E5 without a note |
| `cover.ref.log` | `JS.cover` | 419 | 25939 | 2.1 s, D at row 16 then wrap | retrigger on voices 0 and 2, tone porta without target / to the same period / reaching the target both ways, glissando with no matching entry (finetune 8), tremolo ramp with a negative vibrato position, invert-loop wrap on the 4-word loop of an 8-word sample |
| `cover.dma.log` | `JS.cover`, 3 s | | | | DELI_DMA_LOG of the same module (`CLOCK CHANNEL ADDRESS WORD`): shows the invert-loop changes to sample memory and the zeroed first word of the sample data |

Interrupt k enters at E0 plus the sum of the interrupt periods before it (17735 cc at tempo 125;
section 3 of SPEC.md for tempo changes). E0 and the clocks before it are host properties (see
README).

## What they confirm

- Every write (address, value, order) and its colour clock `E + floor(C / 2)` with C from SPEC
  section 8 equals the reference in all seven logs: 10 210 writes, 0 differences (pre-play
  writes excluded, see below).
- The DMA log's 54 526 audio fetches from module memory equal the sample memory predicted by
  SPEC 5.5 and 1.2 (4 fetches that fall inside an interrupt, while a byte is being changed, are
  ambiguous and were not counted).
- Interrupt spacing: 17735 cc at tempo 125 (70940 per tick); after tempo commands exactly the
  values of SPEC section 3, starting one interval late (the interval right after the tempo
  command's interrupt still has the old length).

## Start (pre-play writes)

Before the first interrupt every log has 14 writes: two host volume calls (each writes AUDxVOL = 0
for voices 0..3, about 128 to 140 cc apart), then init: CIA-A port A = 0x02, AUDxVOL = 0 for voices
0..3 and DMACON = 0x000F, 8 cc apart, starting 230 cc after the CIA write. The CIA write comes
3780 to 3782 cc after the init call and the first interrupt 21785 to 21787 cc after it (these
vary by a cc or two with the module data). An open replayer that starts at the host's fixed start
phase should reproduce the first interrupt time through the engine's usual start handling.

## Comparing

`python3 -I tools/replay-cmp.py REF.log PORT.log --tick-gap 5000`: the interrupts are 8695 to
34640 cc apart and the last write of an interrupt comes at most 4234 cc after its entry, so a gap
of 5000 puts each interrupt's writes in one group (the default 20000 would merge phases). The
tool compares Paula writes only; the CIA writes of E0 are not compared by it.

## Typical offsets (cc after the interrupt entry)

- Effect tick, all voices with a zero command word: AUDxPER at 288, 464, 656, 855.
- Phase 1 (notes started): DMACON at 209; phase 2: LC/LEN of voice 0 at 225/235, then +24 per
  voice (LC of voices 1..3 at 249, 273, 297; LEN at 259, 283, 307).
- Row tick (`basic`, first row), voice 0 note start with sample 1 (loop length > 1), command 0,
  the other three cells empty: DMACON at 637, LC 692, LEN 702, PER 712, VOL 951, then the
  row-command PER at 1125.
