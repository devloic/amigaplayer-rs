# Leggless Music Editor spec: unknowns and caveats

1. What is in this folder: SPEC.md (the behaviour spec), LOGS.md (how the reference logs were made and
   what was checked), logs/ (reference write logs of the reference 68k player run through the host
   with DELI_WRITE_LOG). A private folder with the author's notes exists and is not part of the spec.
2. No data/ is needed: the format keeps everything (instruments, patterns, samples) in the module, and
   the only table the format needs, the 37-word period table, is in SPEC 2.5. Synth voices use a
   128-byte buffer per voice which the player (not the file) provides; the lead decides where it
   lives. Nothing to decide about embedding.
3. Reference timing is given in cycles of the host's static 68000 cost model; the spec is exact for
   that model. A port that models time differently needs the same figures at the same moments or it
   will not reproduce the log colour-clock-exact (the values and order of the writes do not depend on
   timing, except for the cc spacing inside a tick and the raster-wait quantisation).
4. The first tick's entry time (T0) and its phase against the 227-cc line grid belong to the host;
   they change the final DMACON write of every tick (SPEC 9.4) by up to one poll (12.5 cc) and the
   whole tick by nothing else. The gaps between ticks are exactly 70 940 cc in the reference.
5. Not specified (never occurs in the three real modules, or reads unrelated data in the original):
   note/transposition indices outside 0..36; negative or very large transpositions; a pattern whose
   first word is already an end marker; a slide with count 0 (division by zero); pitch tables with
   more than 9 listed notes (n > 10; the record overruns into the neighbouring voice's data);
   instrument numbers beyond the table (the copy reads past it); synth widths above twice the sample
   length (the buffer fill runs past the buffer); a non-zero initial LFO direction word (SPEC 5.2);
   hold, delay or retrigger fields with unusual values were only fuzzed within small ranges.
6. Not covered: the Eagleplayer's other functions (position display, sample-info display, user voice
   switches: the voice masks are all on), and the "amplifier" version of the Eagleplayer for the same
   format, which reads the same data and was not run.
7. Starting a second subsong in the same run without a restart would leave the first subsong's
   state (instrument copies, slide records, pitch tables, the tie flag) in place; the references
   always start from a fresh process, so this was not observed.
8. The song-end behaviour after the report (the host stops three ticks later) is the host's, not the
   format's. A subsong in which one voice never reaches the end of its track list never reports.
9. The audio content of synth instruments (buffer contents) cannot be seen in the write logs. The
   formula of SPEC 5.6 was compared with the memory of the reference player run in a private
   interpreter (4 095 voice-ticks, identical), not with audio.
