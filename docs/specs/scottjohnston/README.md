# Scott Johnston: unknowns and caveats

- No real module: the corpus has no file starting with `Scott Johnston 1`; the only `sj`-named file
  (`MOD.sj`) is a ProTracker module and is rejected by this player. Everything here is checked on five
  synthetic modules (LOGS.md). Real modules (from the game this player was made for) may use commands or
  instrument files in ways the synthetic ones do not; the format rules come from the player itself, so
  they apply, but nothing was heard from a real song.
- No real instrument files either: the player expects 8SVX files with the 95 names of
  `data/instrument_names.bin` in an `Instruments` folder. The synthetic ones are simple 8SVX files. Real
  files may have other chunks before VHDR/BODY; only the search limits of SPEC 2.3 apply.
- data/: `data/instrument_names.bin` holds the player's instrument file name table (95 entries of 12
  bytes, exact bytes of the player). A replayer needs these names to find the files; whether to embed the
  table (as data/*.bin with include_bytes!) or to describe it differently is the lead's decision. The
  period table (39 values) is given in SPEC 7 as values.
- Undefined inputs, specified only as undefined: slot 1 = 0 (the reference uses an unrelated memory
  address as the file), slot bytes above 95, note values above 39, instrument numbers above 38, songs
  without 0xDE or 0xDD (rows run into the bytes after the module), VHDR/BODY not within 0x400/0x800 bytes.
- Not observed: the player's stop routine (fade-out and lock of all voices, DMA off) is never called during
  the logged runs; the `lock` flag therefore never matters. The player's own song-end call exists but is
  never used by the Eagleplayer, so no song end is ever reported.
- The init group (SPEC 4.1, 9.5): order and values are exact; the time from the player init call to its
  first write depends on scanning the instrument files and is given only as a range. Tick timing (SPEC 9)
  is exact relative to the interrupt calls; the absolute call clocks come from the host (first call =
  end of the song start call + one period).
- Addresses: the logged sample addresses are the reference host's guest addresses ($4DE0 onwards for the
  files, $A0CF84 for the empty word); a port must lay out its memory with the same distances for
  tools/replay-cmp.py to accept them (LOGS.md).
- Filter commands (0x01/0x02) change the LED state only; they leave no trace in the write log, so their
  effect was checked only through the host's LED report (one switch to off in sj2-effects at 0.17 s, then
  changes on each 0x01/0x02).
- My reading notes, disassembly and scripts are in a private working folder (not for the implementer).
