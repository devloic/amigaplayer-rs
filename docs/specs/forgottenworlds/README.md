# Forgotten Worlds (FWMP) spec: caveats and unknowns

Written from the behaviour of the reference player; the player is a 600-byte routine that drives the audio
hardware directly (including polling INTREQR), and the module contains only data. The spec in SPEC.md was
validated against five logs with a private model (see LOGS.md). A private notes folder exists next to these
files; it is not part of the specification.

## Not determined or not verified

* The reference run hangs on the first tick of three of the five real modules (channel 0 starts a note in
  tick 0). The cause is the host's audio hardware model (the audio interrupt flag is not raised after a DMA stop
  following a single word); real hardware presumably behaves the same, i.e. the game must start music in a
  different beam phase. A port has to give its W1 wait a defined end in that situation (for example the
  end of the silence word); the complete logs of those tunes therefore come from variants that delay channel 0.
* Command 0x80 (end): behaviour after it is not matched. The reference keeps running the channels; a channel
  reads bytes past its stream (module data) and the logged writes diverge from the model from tick 577
  of the jingle variant on. The command also zeroes a 16-bit word at an address that depends on the host (not
  observed); treat it as "channel/song end" and stop.
* Envelope state at start (levels 0x30, 0x40, 0x28, 0x28; stages hold, invalid, attack, attack; hold counters 45
  and 50 for channels 0 and 1) was seen only for channel 3 of the jingle (attack stage with level 0x28: matches).
  Channel 0's hold stage and channel 1's invalid stage are unverified; channel 1's would jump through a null pointer.
* Volume/balance (spec 6) and the stop sequence (7.1) are from the player's code, not from logs. Restart
  behaviour (stale envelope state) likewise.
* Instrument numbers >= 32 (quirk in spec 1) and negative note/pattern bytes are untested.
* Timing: figures are fitted per path; their split into parts is not unique (only sums are meaningful). A
  handful of rare combinations (first tick of a tune; a pattern call followed by an instrument select in one turn)
  are 2 to 5 clocks off. The Paula-dependent waits W1/W2/W3 and the missed W4 case are described, not
  predicted exactly; the engine's Paula model must supply them (W4's normal case follows the line-boundary rule).
* Only one tick rate (50 Hz) exists; no tempo parameter in the module.

## Data

No player-resident data is needed beyond the 46-entry period table (given in the spec). No data/ files were
produced.

## Coverage

Real modules: game_end, level_1_and_3 (full 30 s logs), title, jingle, level_2_and_4 (only as variants with a
delayed channel 0 because of the hang). No synthetic modules were composed from scratch.
