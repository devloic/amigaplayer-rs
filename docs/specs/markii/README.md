# Mark II: notes for the implementer and the lead

- **Port approach.** Every Mark II module carries its own 68000 driver (position independent, a
  single entry point with a command in D0). The Eagleplayer has no replay routine of its own: it
  checks the module, replaces five or six driver instructions with calls into itself and calls the
  entry point once per tick. The port runs the driver on the repo's 68000 core, as the Riff Raff and
  Benn Daglish SID ports do, with the sites of SPEC 5 as host traps and the timeline of SPEC 4. No
  music algorithm has to be written.
- **Relation to Darius Zendeh.** The repo's Darius Zendeh port (the "old Mark II" driver, prefixes
  `dz`, `mkiio`) reads its modules' tables and does not run them. The Mark II driver is a later,
  larger build of the same design (alternating gate and row ticks, 4-byte record steps, retrigger
  by DMACON mask); SPEC 8 lists the differences. Neither player accepts the other's modules (the
  Darius Zendeh detection string is absent from all 24 `.mk2` files; the Mark II check rejects the
  three Darius Zendeh modules, which have no 0xE742 word within its scan budget, and none of them
  matches a site table). The earlier note in the Darius Zendeh README that Mark II
  modules have "sub-song tables with per-song row counts" is wrong: the parameter block holds one
  row count and the player has one subsong.
- **Three builds.** The 24 known modules use three driver builds (A: 4, B: 18, C: 2), identical
  within a build. A port could therefore also be written data-only from SPEC 8, but register-exact
  timing comes for free only by running the code.
- **Player bug kept.** The song-end site clears the wrong word (SPEC 6): after the song end one
  voice (voice 2 in every log) writes to 0xDFF000..0xDFF008 instead of Paula and keeps sounding
  its last note. This is what the reference does; the port reproduces it. Whether the project wants
  to keep it for playback that loops past the song end is the lead's choice; the logs end shortly
  after the report.
- **Host interface.** The start-call clock (SPEC 4.1) includes the player's check and InitPlayer
  costs; the check cost depends on the module (formula in SPEC 1.3), the InitPlayer cost only on
  the build. The host bug that made the player fail earlier (calling unset function entries) is
  fixed in the host; the player's volume/balance entries are never called.
- **data/.** `data/markii_sites.bin` holds the three site tables (module offset, the three code
  words compared there, kind). The words are the modules' own driver code as the player matches
  them; the table layout and kind codes are this spec's. The port embeds them on the project owner's decision.
- **Publication screening.** SPEC 5 names 68000 registers (D1, D2, A0, A1, A3) because the
  sites exchange values with the driver through them and must leave them as the replaced
  instructions would; the lead may want to look at that and at the hex words in data/.
- **What more modules would confirm.** A build-B module whose gate tick slides (PERTAB in build B),
  modules of other builds, and a module saved with a different parameter block layout.
