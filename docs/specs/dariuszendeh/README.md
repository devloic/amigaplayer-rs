# Darius Zendeh (old Mark II) -- unknowns and caveats

The private working folder holds the reading notes, scripts and the private model (implementer: do not open).

Scope decision. The two UADE players DariusZendeh and MarkII serve different drivers. The module
family is related (same author lineage; the Darius Zendeh player says the first Mark II editor was
written by Darius Zendeh and improved by Andreas Scholl), and the opening bytes of the programs look
alike (`48E700F0 41FA .... 4CD80600 4A00`), but the driver bodies differ: the Darius Zendeh player's
detection rejects the three `.mk2` modules it was run on, and the byte string it needs is absent from all seven inspected, and the `.mk2` driver has a richer state machine.
This spec therefore covers Darius Zendeh only. No shared driver, so no combined spec "mark2".

## Short note on MarkII (not specified)

Eagleplayer MarkII (player "Mark II player module V1.2, Cachet Software, adapted by Eagleeye", prefixes
mk2, mkii). It does not run its own copy of the data tables: it checks the module, patches the
module's code in place (it overwrites a few call sites inside the module and adds its own volume
hook), then calls the module. In the host used here (amigaplayer-rs reference) the player fails on all
tried `.mk2` files (error: "returned through sentinel with unbalanced stack"), so no reference logs
could be made; a port needs a different run of the reference (UADE proper). From reading one module
(atron.mk2): ticks run per-channel envelope state (attack and decay ramps that count up to 64 and down),
period slides (add or subtract with a direction flag), an optional swap of the voice order per record
entry, variable record size and "sub-song" tables with per-song row counts. The row routine is
much larger than the Darius Zendeh one (about 600 bytes against 250). About 24 `.mk2` modules exist in the
header index (demozoo full2: defcon1-*, quadlite-*, hotel, pixeldots ... and `pumpupthejet`), so a MarkII spec is
worth a separate round with a working reference.

## Unknowns and caveats for Darius Zendeh

1. Only three real modules exist (one per variant). Locating rules (SPEC section 2) are checked on
   these; other modules might place constants differently. Variants identified by constants only.
2. The "4A44" kind (sub-songs) and the command values of SPEC section 7 (fade, sound effects, sub-song
   select) were not exercised by any module or by the player in the runs; unspecified. The reference player's
   sub-song number is always 0 in the runs.
3. Timing: the +-1 / +-2 jitter in gaps is not modelled (it comes from DMA slot contention);
   the C gaps are given as fractional means. Absolute timing of the start-up depends on the host scan of
   the module header and is given as measured values and a fitted formula.
4. The base of the sample area and tables is in the module; nothing is needed from the player, so
   there is no data/ folder.
5. The note/period table index is not range checked in the original; out-of-table indexes read
   neighbouring module bytes (not seen).
6. Song end: none. The loop restarts at row 0 without any state reset (the C slide "previous period" persists).
