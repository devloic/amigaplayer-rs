# Sound Programming Language: caveats, unknowns

Files: SPEC.md (behaviour spec), LOGS.md (logs and reproduction), logs/ (reference write logs). A folder
with the reading notes exists next to these files; it is not part of the published spec.
No data/ folder: everything the replay needs lives in the module (it embeds the driver, tables, streams
and samples); the host contributes only the detection, the title/subsong/timer fields and the volume
stage.

## What is covered

All four known modules (Boulder Dash, Clever & Smart, Graffiti Man, Hollywood Poker; three driver
generations). Register values and order are exact for all of them over the tested spans (25 s each, 90 s
for most), and for two synthetic variants that use the envelope flag.

## Unknowns and caveats

- Module-specific layout: the offsets of the driver's variables and tables are listed per module
  (SPEC section 9). A port that wants to play other modules of this family (none are known) would have to
  find them by analysing the code part of the hunk; no rule is given for that. The lead may key the four
  layouts by file size/title, or read the table offsets by scanning; the three families have different
  record layouts.
- Load address: the audible and register-visible behaviour depends on the load address in exactly one
  place, Clever & Smart's stack overflow (SPEC 4.9); a port that places the hunk elsewhere will write
  different AUDxLEN values for notes of samples 0 and 1 played after the overflow. That also means the
  reference output is not independent of the host's memory layout.
- Not exercised by any real module: envelope flag (checked on synthetic modules), zero note byte in
  family 2, instrument volumes that are not multiples of 8 with the decay flag (the volume would wrap and
  be masked to 7 bits), a stack deeper than the layouts allow in families 1 and 3, a Graffiti Man song
  number outside 1..8, a Hollywood Poker song number outside 1..2 (treated as 2).
- Start of play: the first tick's phase, the tick spacing and the host's own start-up writes belong to the
  host framework (SPEC section 7). The position of the first tick after the start writes is not specified
  (within one tick period of the start group in the logs, host dependent).
- Timing is measured, not derived: numbers of SPEC section 8 are medians/fits of the reference traces
  (+-5 cc inside a note-on, up to +-60 cc for the first write after parser work). The raster wait is
  modelled as "9 line changes of the beam counter, polled every 12.5 cc, line = 227 cc"; the exact beam
  position at tick start is the host's.
- The information longs at header offsets 0x24..0x30 are not used by the replay; their meaning was not
  determined (they look like size values).
- The song-end of Graffiti Man songs 7 and 8: the host does not stop at that point; the driver just stops
  writing (no end flag is visible to the host).
- The title/author strings come from the Wanted Team header, not from the original game files.
