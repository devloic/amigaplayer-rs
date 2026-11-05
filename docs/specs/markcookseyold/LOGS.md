# Logs and reproduction

All logs in `logs/` come from the reference 68k player ("MarkCookseyOld", the UADE players directory)
run through the host with DELI_WRITE_LOG. Each run lasted 60 s, or until the reference stopped at song
end. Output went to `--wav /dev/null` with `SDL_AUDIO_DRIVER=dummy`, so nothing was played through
speakers. Subsong s was selected with `--subsong s` (the host calls the player's next-song function s
times before the first tick).

Format: `ADDRESS VALUE COLOUR-CLOCK`, one line per register write. A long write appears as two lines
with the same clock. The first line is `# module BASE SIZE` (the file's load address in the
reference's memory; sample addresses in AUDxLC writes are BASE + offset). A tick that writes
nothing has no lines; ticks are 70 940 cc apart.

Reproduce (player directory = the UADE `players` directory, the empty directory for the others):

    SDL_AUDIO_DRIVER=dummy RUST_LOG=error DELI_WRITE_LOG=out.log DELIPLAYERS=<empty> DELIGENIES=<empty> \
      EAGLEPLAYERS=<uade players> amigaplayer-rs --player MarkCookseyOld --wav /dev/null --seconds 60 \
      --subsong S mco.NAME

The real modules are the `mcs.*` files of the UnExoticA Mark Cooksey game directories, renamed
`mco.*` (Live and Let Die's `mcs` file is identical to Aquablast's). No file with the `mco` prefix
exists in the module index.

| log | module | writes | ticks with writes | song end |
|---|---|---|---|---|
| mco.Aquablast.sub0/1 | Aquablast (= Live and Let Die) | 14 821 / 17 159 | 501 / 500 | no |
| mco.Overlander.sub0..4 | Overlander | 10 407, 22 099, 685, 2 513, 1 052 | 365, 750, 42, 138, 41 | subsongs 2, 3, 4 |
| mco.A_Question_of_Sport.sub0..2 | A Question of Sport | 23 674, 356, 2 532 | 1 334, 8, 94 | subsong 1 |
| mco.Mike_Reads_Comp_Pop_Quiz.sub0..2 | Mike Read's Computer Pop Quiz | 16 866, 736, 707 | 508, 34, 34 | subsongs 1, 2 |
| mco.syn1.sub0 | synthetic (logs/synthetic/mco.syn1) | 5 940 | 573 | no (voices loop) |
| mco.syn2.sub0/1 | synthetic (logs/synthetic/mco.syn2) | 52 / 53 | 3 / 3 | yes, tick 6 |

Synthetic modules (made for this spec, 174 and 176 bytes; 2 instruments of 8 and 16 sample bytes):
- syn1: voice 0: instrument, note, period command (0x04), rest, mark, loop begin 3 / note / loop end,
  period, loop to mark. Voice 1: note before any volume command (initial volume word 0x14), note off,
  note on a stopped channel, volume, note, stop. Voice 2: transposed call (-2) of a pattern that ends
  with return, plain call, volume 0x40, note, rest 0xFF (dead). Voice 3: volume, instrument 1, note, mark,
  note, period command, rest 0, loop to mark.
- syn2: two subsongs in which every voice dies (rest 0x80, note with duration 0, stop, rest 0), so the
  song ends.

Validation: a private model written from SPEC.md reproduces every log above exactly (address, value
and colour clock of every write, 119 652 writes). `tools/replay-cmp.py REF MODEL` reports "no
difference" (checked on Overlander subsong 1, 750 ticks).
