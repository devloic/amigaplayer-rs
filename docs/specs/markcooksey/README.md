# What stays unknown / caveats

1. Timing is only partly specified. Specified: raster-wait primitives, the channel-dependent constants of
   the note-start sequence (N: attack sample case; O: complete), tick period (nominal 70 940 cc), a
   handful of steady-state maintenance gaps. Not extracted: the cycle cost of every path of the voice
   loop that lies between writes (command parsing per command type, an idle voice, an envelope-wait
   voice, slide/pitch-table steps, voices that have no track, the four sound-effect slots visited first
   each tick) and of the note-start without attack sample. The gap tables and timelines in logs/ show
   them in aggregate; a regression over per-command features was not done. Expect to calibrate
   with replay-cmp. In the steady-state tables only 7 maintenance keys were seen to be exactly
   deterministic (single value +-1 cc), more combinations exist.
2. The time from the volume write to the first beam read differs between N-A and N-B and by channel; only
   the first-tick values (and the ones from later ticks' statistics) are given. The loop-sample-present
   constants for channel 3 and the no-loop-sample constants for channels 2..0 were not seen in the
   traced tick.
3. Interrupt-flag waits (N): the waiting time depends on Paula's audio channel state (period counter of
   the running channel; period 1 was just written, but the old period must expire first). The wait loop
   period (14.5 cc) and the observed set of delays are given; the actual flag timing has to come from a
   Paula model. The same for the flag after DMA start (165 vs 180 cc).
4. Third header kind (Eagleplayer "Third", file starts with 6000 branches) and the old Eagleplayer
   "Mark_Cooksey_Old" (prefix mco) were not examined: no sample files were found in the module corpus
   under mc/mcr (the `fw.*` files are a different format). The `mc.master.tune.mod` entry of the
   corpus list was missing from the archive.
5. Deep pattern nesting (more than about 3 levels of 0x64/0x65 calls) would, in the original, overwrite
   other fields of the voice state (the frame stack sits in the voice's own state block). Not modelled,
   not seen. Unknown commands loop forever in the original (never seen).
6. The table-location rule (SPEC 4.1) was validated on 8 N modules (both variants); other N modules might
   place the tables differently. Layout facts listed there are checks. For O the period table pointer
   comes from the init routine; verified on 3 mcr files (Aquablast / QoS share the same, Overlander
   differs).
7. Volume: the reference replaces the module's own volume scaling by "level >> 1, clamp 64" (SPEC 4.5);
   it is the observed output (it is louder than the module's own scaling would give for large levels).
   The master-volume word (entry "set master volume") has no effect on the replay path in the test
   modules.
8. Song end: the report is a host callback; the reference's own stopping after the report is the host's
   behaviour (a few ticks later). A subsong that uses fewer than 4 voices never reports song end.
9. Silence buffer address (O): 0x0075A630 in the reference run is the Eagleplayer's own buffer, not a
   module address; N uses the first bytes of the sample area as silence.
10. Positions such as GetPosition (current step in the track) exist in the reference for O/N-B variants
   of the info display but are not specified (display only).
11. Game-use features of the driver (sound-effect voice set, master volume entry, second start routine)
   are not reachable from the player and not specified.
