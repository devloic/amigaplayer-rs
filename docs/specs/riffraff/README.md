# Riff Raff: notes for the implementer and the lead

- **Port approach.** Each module carries its game's own 68000 sound driver. As decided by the lead,
  the port runs that driver on the repo's 68000 core (as the Fred Gray and Dave Lowe hosted ports
  do) and reproduces the Eagleplayer's wrapper: loading, fix-ups, replaced code shapes as host traps
  with fixed cycle costs, start-up timeline, tick sequence and song end (SPEC 2 to 7). No music
  algorithm has to be written.
- **Publication screening.** SPEC 2.5 gives the ten search keys as hex words: they are bytes of the
  modules' own driver code (which the implementer has), not of the Eagleplayer, and no mnemonics
  are used. SPEC 6 names the driver's calling-state registers by neutral names (VREC, VCHIP, X0, X1, X2) as the driver's calling state at the
  sites, because the port must read or write them. The lead may want to look at both before
  publishing.
- **Modules.** Four real modules (Bomb Fusion, Protector, Side Winder II, T'Bird) were provided by
  the lead and are not copied here. The three synthetic modules of the first pass remain in
  `modules/` (they contain a small hand-written routine).
- **No data/.** The format needs no bytes from the player. The only player-owned object is a 4-byte
  zero buffer whose address (0x006AC650) is given in SPEC 2.2.
- **Hunk placement** (SPEC 2.1) is a host property. The rule given fits the four real modules but not
  the 676-byte synthetic files (0x158 higher). The log comparison tolerates a constant offset, but
  the zero-buffer address must be identical.
- **Absolute start time** (SPEC 4.1) is the reference's clock: 476 + floor(C_load / 2) for the first
  host write. How the port's host start relates to the reference's clock origin is up to the
  implementer (see the start-phase recipes of the earlier hosted ports).
- **Not exercised** by any log: master volume or balance other than 64 and voice muting (SPEC 8),
  subsong changes during playback (only the start-time switch of SPEC 4.2 is logged), a voice-3
  restart without song end and the "X2 smaller" branch of MINVOLUME. The costs of those two paths
  were measured by running the player's own handlers privately, not from logs.
- **Corrections to the first pass:** the header field at 0x18 names the routine called first in a
  tick (the driver's main routine), not a small pre-tick hook; the raw shadow volumes are not
  cleared by an init; costs are now exact per site kind and voice; the start-up timeline is now
  explained.
- The 'Riff Raff_v2' loader files that came with the modules were not used.
- The spec author's private working notes are kept in a separate folder that implementers do not
  use.
