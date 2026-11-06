# CustomMade spec: notes for the implementer and the lead

*Sanitised for publication: behaviour only.*

## What CustomMade is

It is a module format, not a per-game player scheme. The files are raw memory dumps of Ron Klaren
/ Ivo Zoer game sound drivers with their music (Battle Squadron, Plague). The Eagleplayer finds the
music tables in the dump through byte markers of the driver code, translates the dump's absolute
pointers (Origin), and plays the data with its **own fixed replayer**. It never executes the dump's
code.

Prefixes `cm`, `rk`, `rkb`: the `rk` prefix is shared with the Ron Klaren hunk modules. The two
reference players split the files by container, with no overlap:
- hunk files with `RON_KLAREN_SOUNDMODULE!` (the `Eagleplayer_Versions/rk.*` copies): Eagleplayer
  "RonKlaren", already ported as `src/replay/ronklaren.rs`; CustomMade's check refuses them;
- raw dumps (all `cm.*` and the `Other_RK_Versions/rk.*` copies): only CustomMade. Today they get
  "unknown module format" from the open replayers (12 distinct files in the corpus). There are no
  `rkb.*` files in the corpus.

So the raw dumps are a distinct behaviour that no existing replayer covers. A CustomMade open
replayer should run its detection (SPEC 1.2) before or alongside Ron Klaren's. The two
detections cannot both match one file.

## Does the hosted-driver pattern fit?

No. The reference does not run the game's code. The dumps are absolute-addressed (three have
Origin 0x20, inside the exception vector area), and the game drivers differ from the Eagleplayer's
fixed replayer (for example the newer drivers' voice backup and restore at 0x83, and their
70-entry period table). Running the embedded driver as `riffraff.rs` / `markii.rs` do would not
match the reference. A plain data-driven replayer is the right shape. It is small: one replayer
with no variants, about 300 lines plus the timing table. Some format knowledge overlaps with
`ronklaren.rs` (same track, instrument and envelope semantics), but the replay rules and timing
are this spec's.

## Unknowns and caveats

- Period lookups outside the 84-entry table read the player's own memory: index 84 is the status
  word (0x003F while playing), the next 124 words are the player's four voice records (live
  state, partly addresses). Negative indices read player code. No corpus run does this. An
  implementation can return 0 or emulate index 84 only.
- Zero-duration notes use a scratch high byte (SPEC 5.6) that can push the lookup far out of the
  table. This is described but never exercised.
- A dump without the `ptoff` marker, or without a timer marker (timer 0), would behave in an
  undefined way in the reference.
- Timing (SPEC 8) is a fitted cycle model. Individual step costs are not meaningful alone; only the
  sums along a path are. It is exact on all corpus ticks. Paths never executed (listed in LOGS.md)
  have no measured cost beyond the step costs they share.
- Absolute start times are host business. They are given as fits (SPEC 8.4), exact within 1 cc
  from the last host volume write on, and within 11 cc for the first host write after loading.
- The host's way of starting subsong N (running InitSound for 1..N with stops in between) is a
  property of the reference host. It shows up in the logs (one batch of zero volume writes and
  12 sample writes per extra init).
- The phase modulation and the 0x270F patch change the module image permanently, also across
  subsong changes. A replayer that reloads the image per subsong would differ from the reference
  after a subsong switch in mid-play (not covered by the logs: every log starts from a fresh
  load).
- Player data: the only table taken from the player is the 84-entry period table. It is given as
  values in SPEC 2 (a small format table), so no `data/` directory was produced.

## Errata found by the implementation

- SPEC 8.4, counting E: the entries are counted with the 12-byte stride up to a long of `0xFFFFFFFF`, not up to the first byte with bit 7 set. Battle Squadron `cm.ingame` subsong 3, voice 3 has no end marker and runs on 10 entries, which gives a 228 cc gap at init.
- SPEC 8.4, first host write: the fit misses a constant of about 430 to 470 cc. The implementation uses `427 + 0.862 * file size`; the result is within -9 to +11 cc of the reference per module, and this is the main remaining inexactness.
- The patch of the value 0x270F costs 9 extra cycles at init (18 cycles); the spec does not mention it.
- Subsong numbering is not stated: the implementation counts from 1, like the Ron Klaren port.
- The stop sequence (DMACON 0x000F, then four volume writes) is timed from the logs, not from the spec. Six init writes in subsongs 3 and up are 1 cc off (a parity of 387/388).
- Not exercised by any log: period lookups outside the table (the implementation returns 0), zero-duration notes with an odd scratch high byte, and a row that never ends (the song stops after 4096 passes). Four of the twelve logged cases have no module file and could not be checked.
