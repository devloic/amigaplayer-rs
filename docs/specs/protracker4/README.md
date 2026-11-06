# Protracker4 (ProTracker 4 IFF, `mod3.`): unknowns and caveats

*Sanitised for publication: behaviour only.*


What it is: an IFF `FORM`/`MODL` container (ProTracker 3.6x/4.0) around an ordinary ProTracker
module, played by a ProTracker 2.x engine. A port can reuse the repo's ProTracker core and apply
the differences in SPEC section 6. The `ptm.` prefix of the same Eagleplayer accepts only FORM/MODL
files. All 81 `ptm.` files of the corpus are other formats (PolyTracker etc.) and must stay with
their ports.

No player data is needed: the period, vibrato and funk tables are the standard ProTracker ones,
identical to those in the repo's ProTracker core. No data/ folder.

1. Only one real module exists (MOD3.heaVen). It uses only 9xx, Cxx and Fxx. Everything else was
   validated on synthetic containers around real M.K. modules (LOGS.md), and the container layout
   was copied from heaVen. Other ProTracker 3.6x writers might set the chunk sizes differently.
   The reference's chunk walk (SPEC 2.2) would then fail, and the reference plays silence.
   Whether a port should be more lenient is the lead's decision. The behaviour as specified is the
   reference's.
2. LEN <= 1 written as 0 makes Paula play 65536 words from the repeat start of one-shot samples, so
   the reference sounds different from PT 2.3D at every one-shot sample end. It is what the
   reference does in this host (the host writes the poked value as is). On real hardware with the
   original Eagleplayer the same would happen if its poke-length function writes the value
   unchanged, but that was not checked.
3. Not observed in any run: E3x glissando; arpeggio past the end of the period table (SPEC 4.4
   describes what the player would read); Dxx with value 64; instrument numbers above 31; a song
   longer than 127 positions; tempo changes on NTSC.
4. 8 voices (`8CHN`): values and order are verified (s5ch8). The amplifier's mixing, output rate and
   the timing of the virtual-voice pokes were not examined. In 8-voice mode the host's voice mask
   repeats the mask of voices 0-3 for voices 4-7.
5. Timing (SPEC 5): the tick period and the DMA wait are exact rules. Write spacing is a measured
   table that covers the paths seen in the test modules (99.8 % of writes within 1 cc on its own
   data). It generalises only partly: 81 % on two held-out modules, and 19 % of their writes had
   key combinations it lacked. A compositional cost model (per event costs that add up) was not
   derived. Expect to calibrate with replay-cmp. 8-voice timing and the costs of E8x
   (Karplus-Strong loop length dependent) and EFx paths are not tabulated beyond what the table
   holds.
6. The first interrupt's delay after initialisation and the host's song-end stop are host
   behaviour, not the player's.
7. Info only, not specified: duration calculation, sample-info and note-info structures, next/
   previous pattern controls.
8. Validation honesty: the value model was written by the spec agent alongside this text, not by a
   separate reader. It encodes SPEC 2-4 as written, and every rule it needed is in the text.

## Errata found by the implementation

- The reference's first register write is data dependent (the duration calculation: for example 1,597,632 colour clocks for one test module); it is not modelled, so audio is SIMILAR rather than IDENTICAL.
- Costs add up per code path, they are not a lookup table. Rules fitted beyond the spec's table: a previous empty cell costs 368 cycles and then adds 196 (194 on a trigger) to the rest of the path; an instrument set-up costs 318 (+62 with a repeat start), also on a row with an instrument but no note; a vibrato parameter costs +47 per non-zero nibble; a tone portamento costs +28 when it takes a new speed, +46 when it reaches its target (+36 going down) and +-16 on set-up by direction; a volume slide costs +16 when it hits a limit, and up is 8 cycles cheaper than down; F costs 934 (speed), 850 (F00) or 1430 (tempo, +496 for the timer division); the interrupt enters one cycle into the colour clock. With these, 97 to 100 percent of the writes land within 1 cc.
- The arpeggio read past the period table returns the voice count, then voice 0's cell high and low words, then 0 (the spec says only "voice 0's state").
- A file that fails the container's chunk walk is not recognised by the open replayer; the original starts and plays silence.
- Costs that could not be derived and were taken from the nearest measured path: E3, E4, E7, EC details and the E8x cost per byte. About 0.5 to 1.5 percent of the writes are 1 cc off through rounding. The implementation does not reuse the ProTracker core (the deviations are many) except for its period, vibrato and funk tables.
