# What stays unknown / caveats (David Hanney)

1. THE DATA PROBLEM (decide before implementing). The format carries only patterns. The 16
   instruments (about 1.5 KB) and the three PCM samples (4 KB) are inside the original player
   binary and are the same for every module. SPEC.md describes their structure but, by the
   clean-room rule, does not contain the bytes. A replayer without them cannot make sound.
   Options: (a) load them at run time from the user's installed UADE/Eagleplayer
   file (the extraction needs a loader or a data file supplied by the maintainer); (b) derive the
   instrument parameters from behaviour (envelopes, vibrato, sweep come out of the write logs) and the
   waves/samples from the wavs; the waves are 32-byte shapes, the samples need audio.
   The licensing question is the maintainer's.
2. Never exercised by a log (UNVERIFIED): instrument number above 15; period index above 68 (note
   above 70; the synthetic "hinote" module goes up to 130 and the reference matches a model that
   reads the bytes after the table, but that is not specified); block numbers missing from the
   file (the reference then reads outside the chunk list); a block with 0 rows (the end-of-block test
   never fires: the block would never end); state 16 (only the host API sets it); envelope case
   "below target, reached, stage 6" (its cost, 151, is inferred); `INFO` second word (read, never used).
3. Note value 2 (documented as "stop envelope" by its code but the stray write goes to the wrong
   place): in the reference it has no effect on the output (synthetic module note2 matches a model
   that ignores it). If the player's host kept different memory it could corrupt something; not
   observed.
4. Effects 1, 2, 4 and notes 1, 2 are not used by any known module (only synthetic coverage).
5. Subsongs: none (one song per file). There is no title/author in the format; the player's
   Eagleplayer name is "David Hanney". Only 8 modules of one game (Tearaway Thomas) exist in the module
   corpus; the player's range of other speeds/songs is from synthetic modules only (speeds 0, 3, 8).
6. Timing costs (SPEC 8) are exact on 14 modules but the individual terms are fitted (a gauge
   freedom exists); a port that composes them as stated reproduces every write, a port that needs
   a term in another context (an unseen pair of writes) has to assume the baseline.
7. Master volume: the table in SPEC 4.4 follows the host's initial volume (64 in the reference).
   The relation to the host's value was inferred from the identity table seen; other volumes
   are UNVERIFIED.
8. The first tick start (SPEC 3.4) and the start call times are the reference harness's start
   phase; a port's own framework will have its own phase. Everything from tick 0 on is relative.
9. The reference host stops 0 to 3 ticks after the song-end call (3 in most files, 0 for speeds 0 and 8); that is the host, not the player; the rule was not found.
10. Period table beyond index 68, location addresses and the host's DMA-wait stub are
    environment properties: the DMA-on call takes 10 video lines (SPEC 8.1) because of the
    host stub, measured, not derived.
