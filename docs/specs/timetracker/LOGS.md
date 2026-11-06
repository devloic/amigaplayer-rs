# Logs and reproduction

All logs are of the reference 68k player (the UADE players directory's `TimeTracker`, V1.0 of 7 Mar 2014)
run through the host with DELI_WRITE_LOG, `--wav` to a file or `/dev/null`, `SDL_AUDIO_DRIVER=dummy`;
nothing was played through speakers. Format: `ADDRESS VALUE COLOUR-CLOCK` per line, first line
`# module BASE SIZE` (load address and size of the song file). Sample addresses are absolute: the
samples file is loaded separately (bases 0x7280 Erematix, 0x7598 Still Alive, 0x76F8 Outlaw Is Dead,
0x4DE0 synthetic fx, 0x9B88 synthetic two-subsong). The first group of each log (before the first tick)
is the host's start: two volume passes of the four voices (cached values; voice 3 shows 0x65, a
leftover byte, harmless because DMA is off) and the LED-off write; for a subsong other than the first
the start runs twice.

## Modules

Real modules (single-file form, public collections): `Still Alive.tmk` (md5 157f4d64c0f3...),
`Outlaw Is Dead.tmk` (87df350562ee...), `Erematix.tmk` (bc993cedff45..., delta-coded samples). The
Eagleplayer needs the two-file form, so each was split at `file length - total sample bytes` (= end of
row data) into `TMK.<name>` (song) and `SMP.<name>` (samples), names with `_` for spaces.

Synthetic modules (built for this spec, Erematix's samples):
- `TMK.fx` / `SMP.fx` (and the single-file `fx.tmk`): two patterns exercising effects the real modules
  do not use: first portamento after start, E1x/E2x, tremolo with E7x waveforms, sample offset (with the
  double application and a following note without sample), E0x, E3x/E4x glissando, 5xy, 6xy, E9x with
  and without note, ECx, EDx, E5x, EAx/EBx, EEx, EFx funk with a looped sample, F21 (a 33-tick row),
  E6x loop, Dxx, Bxx, the short note+sample form, a sample-only entry, arpeggio at the top note, tone
  portamento to low notes with finetune 3 and 9 (6.5).
- `TMK.two` / `SMP.two`: two subsongs (Erematix's block twice, the second with the position list rotated
  by 13). The first block is padded so that byte 5 of the file reads as 6 (see README, Eagleplayer quirk).

## Files in logs/

| file | content |
|---|---|
| Erematix.ref20.log, .ref60.log, .ref400.log | 20 s, 60 s, whole song (song end after 112.7 s, the host then stops) |
| Still_Alive.ref20/60/400.log | same; song end after 174.2 s |
| Outlaw_Is_Dead.ref20/60/400.log | same; song end after 148.6 s |
| synth_fx.ref40.log | synthetic effect module; ends at its Bxx (song end report) after 154 ticks |
| synth_two.sub0.ref30.log, synth_two.sub1.ref30.log | two-subsong module, `--subsong 0` and `--subsong 1`, 30 s |
| gaps_summary.tsv | per-write gaps over the four long logs: tick kind (row = has DMACON on), previous write, write, count, median, most common values |

Reproduce (silent):

    SDL_AUDIO_DRIVER=dummy RUST_LOG=error DELIPLAYERS=EMPTY_DIR DELIGENIES=EMPTY_DIR DELI_NATIVE=0 \
    EAGLEPLAYERS=<UADE players dir> DELI_WRITE_LOG=out.log \
    amigaplayer-rs --player TimeTracker --wav /dev/null --seconds 60 /ABSOLUTE/PATH/TMK.Erematix

(the `SMP.` file must sit next to the `TMK.` file). Beam trace: add DELI_TRACE=trace.txt.
Port side: `EAGLEPLAYERS=EMPTY_DIR DELI_WRITE_LOG=port.log amigaplayer-rs --open-replayers --wav /dev/null ...`
on the single-file `.tmk`; compare with `tools/replay-cmp.py REF.log PORT.log`.

## Validation of SPEC.md

A value model written from SPEC.md (sections 1-6) was run against every log above, tick by tick,
comparing the complete ordered list of (address, value) writes of each tick (audio registers and the LED
writes):

| log | ticks | differing ticks |
|---|---|---|
| Erematix 20 / 60 / 400 s | 999 / 2999 / 5634 | 0 / 0 / 0 |
| Still Alive 20 / 60 / 400 s | 999 / 2999 / 8709 | 0 / 0 / 0 |
| Outlaw Is Dead 20 / 60 / 400 s | 999 / 2999 / 7429 | 0 / 0 / 0 |
| synth_fx 40 s | 154 | 0 |
| synth_two subsong 0 / 1 | 1499 / 1499 | 0 / 0 |

The model also reports song end at the same point as the reference (once per whole-song log). Mutation
check: replacing one TimeTracker deviation by the ProTracker behaviour makes the model disagree with the
logs: base-period rewrite in 5xy/6xy removed (Erematix 522, Outlaw 860, fx 10 differing ticks); tone
portamento target with the 36-entry stride (fx 5); portamento mask starting at 0xFF (Still Alive 3,
Erematix 3, Outlaw 21, fx 18); E4x setting the vibrato waveform (fx 10); a period write at the start of
every row (about 1150 per real module).

Timing (section 7) is measured, not modelled: the constants and the raster-wait behaviour come from
gaps_summary.tsv and a beam trace of the first row ticks of Erematix. Not every per-path cost was
extracted; expect to calibrate the remaining gaps with replay-cmp.

## The current repository playback compared (built-in ProTracker engine via the TMK converter)

Effective per-tick Paula state (LC/LEN/PER/VOL after each tick and the voices restarted), reference vs the
current playback of the single-file modules, whole songs: Erematix 617 of 5634 ticks differ, Still Alive
32 of 8709, Outlaw Is Dead 742 of 7429, all in AUDxPER only; sample pointers, lengths, volumes and
restarts agree. tools/replay-cmp.py finds 0 identical ticks in all three (write order and the extra
writes of 7.1). The synthetic fx module differs in 120 of 128 compared ticks and in length (F21 is taken
as a tempo there). Causes: the row-start period rule (5.1 step 1), 6xy/5xy (6.7), the portamento mask
(6.3), plus, not visible in the write logs, the LED (the reference keeps the filter off from the start;
the current playback switches it on) and the delta decoding (1.4).
