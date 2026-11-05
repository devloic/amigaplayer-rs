# Maximum Effect: what stays unknown and caveats

(An analysis folder with the reading notes and scripts exists next to this file; it is not part of the
spec.)

1. **No real modules.** The module index has no file of this format (`max.*` hits are other formats: PC
   trackers, an amp collection of unrelated files; `smp.*` hits are a different format). Everything here
   was derived from the reference behaviour on **synthetic** modules built from the structure the
   player reads. Real modules of the game ('Allo 'Allo!, Alternative Software 1993, music by Jonathan
   Scarcliffe and Alastair Dukes) were not seen. Things a real module might do that the synthetic ones do
   not: songs with fewer than 4 voices (they do not play correctly in the reference, SPEC 4.3, so
   probably none exist), shared track lists, note indexes above 35, commands above 8, patterns whose
   event counter does not match the stream (these read on through the stream, SPEC 2.4), or a sample
   bank whose table is not directly followed by the data (the load check then reads the wrong place).
2. **Detection collision.** `max` is the prefix of this Eagleplayer alone in the UADE configuration.
   Detection is by content (SPEC 1.2). The rule was run (in my own re-implementation) on the 16 files of
   the module index whose names start with or end in `max` (ProTracker modules, an XM-like file, Digital
   Mugician files, two `MOD.max` files): none passes. A file passes only if its first long is at least 15,
   P1 (if non-zero) is even, inside the file and 2 + a multiple of 18, and every non-zero P2..P4 is even,
   inside the file and preceded 6 bytes earlier by a zero long. No collision could be tested with real
   Maximum Effect files.
3. **Absolute phase of the first tick.** The reference calls the first tick 70 940 cc after the end of its
   start-up, and the start-up has a fixed cost plus module-dependent parts. For tick-exact comparison the
   line grid (multiples of 227 cc of the absolute clock) matters, because the two raster waits end at
   the next line boundary. The measured first-call clock, in cc, of the logs is in LOGS.md. A fit that
   reproduced all 28 runs exactly (an observation of the host, not part of the format): first call =
   100 + C + X + I + 270 + sum over sub-songs j = 0..s of (J_j + (j > 0 ? 284 : 0)) + 70 940, where
   C = (654 - 65 z) / 2 (z = number of zero pointers among P2..P4 of song 0), X = (2254 + 22 x (length of
   the file name including `max.` minus 6) + 17 x (length of the directory part of the path, with its trailing
   slash, minus 96)) / 2, I = (499 + 416 S + 61 E + 143 N) / 2 (S = songs, E = list
   entries not counting end markers, over all songs, N = samples), J_j = (2537 + 156 k + L) / 2 with k =
   voices of song j and L = 47 x (entries of its voice 0 list) when the pointer is non-zero, -29 when it
   is zero; divisions floor. Tested with file names of 6 and 7 characters, three directory lengths (92, 95, 96) and synthetic
   structure only (the log first lines in logs/ were made with a directory part of 96 characters; a run from
   another directory starts earlier or later by 8.5 cc per character of difference). Treat as a starting point; replay-cmp
   will show a wrong phase as shifted raster-wait results.
4. **Timing of unexercised paths.** The cycle table of SPEC 8 covers every code path the synthetic modules
   reach. Not reached: commands 9 and up, a note index that reads outside the period table, an
   unassigned processed channel whose counter wraps (after 32 768 ticks), a restart offset that points at
   the end marker. The shift in the vibrato step costs a fixed number of cycles in the reference
   (the emulator's static table), not the 68000's data-dependent one; SPEC 8.3 uses the fixed figure.
5. **Song-end tail.** After the song-end report the reference stops 0 to 4 ticks later; that is host
   behaviour and not specified. For songs that never report the end (play-once, fewer than four voices)
   the logs are cut at 30 s.
6. **Volume scaling.** The reference multiplies the volume by a balance and a master value that are 64 and
   64 in all runs (neutral); the written volume is then the volume AND 0x3FF (no clamp to 64: command 3 or a
   sample record can give 65..255 and they are written as is). The host's own volume/balance
   controls (not used in the runs) would scale the value as (volume x balance) >> 6 per channel
   pair (channels 0 and 3 one balance, channels 1 and 2 the other).
7. **CIA writes.** Commands 1 and 2 write the CIA-A port A byte; the log shows the value written (0x02,
   0x00). On the host this switches the audio filter / power LED; the effect on the audio is the host's
   filter and is not part of the write sequence. A port should emit the same two values if its log is
   compared.
8. **Pointer sharing, double relocation.** SPEC 2.5. A port that treats pointers as offsets will play a
   file whose lists are shared; the reference would break. No file is expected to do this.
9. **No `data/` folder** was produced. The only player-resident data are the 36-value period table and the
   64-value sine table; both are given in SPEC section 7 (the sine table as a formula, verified equal to
   the player's). The lead need not embed anything.
10. **Song length.** The "length" the reference displays is the number of entries of the first voice's
    track list (display only). The song's length in ticks is not stored.
