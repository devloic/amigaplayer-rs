# Benn Daglish SID: notes for the implementer and the lead

- **Port approach.** Every BDS module carries its game's own 68000 sound driver (the Amiga port of
  Daglish's C64 routine used in early Teque games). The Eagleplayer has no replay routine of its
  own. The port runs the driver on the repo's 68000 core, as the Riff Raff port does, and
  reproduces the Eagleplayer's wrapper: loading, the audio-routine scan, five replaced code shapes
  as host traps with fixed cycle costs, the start-up timeline, the tick, audio-interrupt delivery
  and the two song-end paths (SPEC 2 to 7). No music algorithm has to be written. Unlike Riff Raff,
  the driver also runs on every audio interrupt (SPEC 5): the port overrides the per-channel
  audio-interrupt call of the replayer interface.
- **Modules.** Three real modules exist: Terramex (1 subsong), Chubby Gristle and The Flintstones
  (2 each); all are in `modules/`. They separate the per-module facts (entry points, subsong count,
  scanned length, site positions, volume bytes, song-end path: SPEC 1.5) from the player-wide ones
  (header layout, scan and search rules, costs, timeline). Terramex's driver is the older one (no
  samples, song end by restarting itself); the two 1988 drivers carry samples and end the song with
  the flag byte. Every log of the three is reproduced exactly by a model following the spec.
- **What a further module would confirm.** The scan cost per volume-load word (12 cc, derived
  from the player's code, every known module has exactly three), a SONGEND shape outside the init
  routine (it would report a song end each time it runs), more than one occurrence of a key, and
  init routines that read registers other than D0.
- **data/.** `data/site_keys.bin` holds the five search keys (for each key a byte with its word
  count, then the words) and `data/scan_words.bin` the three words of the audio-routine scan (end
  word, replacement, volume-load word). They are words of the modules' own driver code as the
  player matches them, not bytes of the player; the port embeds them on the project owner's decision.
- **Publication screening.** The spec names 68000 registers (D0-D6, A0, A4, A5) only as the
  calling state the host and the driver exchange (which a port must read or set). The lead may
  want to look at that and at the hex words in data/ before publishing.
- **Host properties.** The init-call clock `656 + 21 W + 12 m` (SPEC 4.1) and the hunk placement
  (SPEC 2.1) are the reference host's; the model reached them with the engine calling the port's
  start before that clock and the machine's allocator.
- **C64 files.** A C64 SID file (e.g. the C64 Terramex) cannot be turned into a BDS module: the
  Amiga player has no driver to feed. The first-pass comparison found the C64 data layout (song
  table, tracks, patterns, instruments) similar in kind to the Amiga driver's, which is irrelevant
  for a hosted port.
- The 'Benn Daglish SID_v2' Wanted Team source was read for the player's interface only.
- The spec author's private working notes are kept in a separate folder that implementers do not
  use.
