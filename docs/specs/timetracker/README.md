# TimeTracker spec: summary, unknowns, caveats


## Is it ProTracker?
No, though close. The effect set and most effect arithmetic are ProTracker 2.x's (same period, sine and
funk tables, arpeggio by tick mod 3, vibrato >> 7, tremolo >> 6, the double sample offset, EEx/E6x
logic). The differences, all confirmed with reference logs (LOGS.md), are:

Audible in the real modules:
1. Row start: the period is rewritten only when the voice's previous entry was completely empty; an
   offset left by vibrato/arpeggio stays over the row tick (SPEC 5.1 step 1).
2. 5xy/6xy rewrite the unmodulated period after the effect: 6xy has no audible vibrato (SPEC 6.7).
3. The first 1xx/2xx after song start slides by 0 for one tick (global mask, SPEC 6.3).
4. LED filter off at song start (the current repository playback turns it on).
5. Fixed 50 Hz timing; Fxx is always a speed (no BPM).
6. Delta-coded samples decode one byte later than the existing single-file converter produces
   (SPEC 1.4), and the first 4 bytes of each sample are zeroed (not checkable with write logs).

Not used by the three real modules, confirmed with a synthetic module:
7. E4x sets glissando (vibrato waveform is fixed to sine with retrigger).
8. Tone-portamento target lookup with a 37-entry row stride (SPEC 6.5).
9. EFx is the old funk repeat (moves the loop pointer), not ProTracker's invert loop.
10. Subsongs (byte 3 = count, block size table).

Loggable only: every tick starts with a volume write of all four voices; on effect ticks every voice
without an effect writes its period; the sample volume is written before the DMA-off of a note start;
the DMA restart and retrigger wait 9 raster lines (about 2 050 cc) each, longer than the current
ProTracker path.

## Unknowns / not determined
1. Timing is only partly specified (SPEC 7): the volume block, the common per-voice gaps, the note-start
   constants, the raster-wait mechanism and the loop block are given; the cost of every code path between
   writes (each effect, empty voices, row advance) is only available as the measured distributions in
   logs/gaps_summary.tsv. Expect calibration with replay-cmp.
2. Reads past the period table (tone portamento with finetune 15 and very low target, arpeggio offsets past
   the last row, note numbers above 36) hit other player memory in the original; their values are not
   specified. Not seen in any module.
3. Effect-only entries with an effect number above 15 (`0xD0..0xFF` first byte) would make the original
   jump outside its effect table; sample numbers above the sample count or a finetune byte above 15 read
   undefined memory. Not specified.
4. Multi-subsong modules: none exists in the corpus; the layout was confirmed with a synthetic two-subsong
   file only. Eagleplayer quirk: its load check and sample-info display take the sample count from file
   byte 5, which is the first block's count byte only when there is one subsong; with more subsongs it is
   the low byte of the first block-size word (a real two-subsong file is therefore likely to be rejected
   as "too short" unless that byte happens to fit). The replay itself uses the first block's count byte
   correctly. An implementation should use the first block's count.
5. Host start group: two volume passes before the first tick, voice 3 showing 0x65 (a stale byte in the
   player's variable area); harmless. For a subsong other than the first the host starts the song twice.
6. Song end: reported on every Bxx (also forward jumps) and at the position wrap; what the host does after
   the report (stopping a few ticks later) is host behaviour.
7. The Eagleplayer's amplifier variant (V2.0, "TimeTracker.AMP") was not examined; the spec is of the
   DeliTracker-compatible V1.0 found in the UADE players directory.
8. Host volume/balance scaling (SPEC 5 step 1) was read, not logged (all logs at full volume).

## data/
Not produced: the only player tables (period, sine, funk) are the standard ProTracker ones, given in SPEC 2
(the period table is the one the repository's ProTracker-style converters already use), and the volume
table is the identity.

## Errata found by the implementation

- The spec gives no model for the timing between writes inside a tick; the port fits a gap table and the
  raster-wait rules (nine lines on, plus 61 and a 13-clock poll phase for the second wait) from the logs.
  About 57-68% of writes inside a tick land on the exact colour clock; ticks start exactly on time.
- No behavioural spec errors were found: every tick of every reference log is identical in address, value
  and order. Multi-subsong single-file layout is unknown (treated as two-file only).
- Claiming: detection is a strict structure check, so a ProTracker module titled "TMK" is not taken, and
  the generic ProWizard TMK conversion is skipped for these files when the open replayers run.
