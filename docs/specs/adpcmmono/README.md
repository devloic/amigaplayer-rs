# ADPCM mono — spec folder

Behaviour spec of the Eagleplayer "Adaptative Delta Pulse Code Modulation" (file ADPCM_mono, prefix
adpcm in the UADE player list): SPEC.md (format, decoder, register writes and timing), LOGS.md (reference
runs and measurements), logs/ (write logs, DMA logs, the synthetic modules and their generator).
A private folder with the reading notes and validation scripts exists for the lead only.

Feasibility: fully feasible and small. The format is a raw IMA/DVI-style 4-bit ADPCM stream; the player
decodes it in 68k code into two 1024-byte buffers. The repo does not decode this format anywhere
(`adpcm` appears only in the engine's list of PC formats, see below). No player-side data is needed: the
only table is the standard 89-entry IMA step table (given in SPEC 2) and the difference table the player
uses equals the formula in SPEC 2 exactly (checked for all 89 × 16 entries). No data/ folder.

## Unknowns and caveats

1. No real module: the corpus has no `.ADPCM` file. Everything is validated on synthetic modules only.
   The detection rule needs the `.ADPCM` SUFFIX in the file name; the player list's "prefix adpcm"
   (`adpcm.name`) does not pass the player's own check.
2. Lower-case quirk: the player rejects `name.adpcm` (all lower case) because of its `p` comparison
   (SPEC 1). A port may choose to accept it; the reference does not.
3. Files shorter than 512 bytes (fewer than 1024 samples): the init decode does not check the length, so
   the first buffer holds the module's samples followed by decoded host memory past the module, and the
   remaining-sample counter wraps to a huge value: the reference never reports song end and keeps
   decoding memory (`tiny.ADPCM`). Not worth reproducing; a port should treat such a file as one zero-padded
   buffer (a choice, not the reference).
4. Over-read on the last buffer: the timing of the final partial fill depends on the bytes that follow
   the module in memory (zeros in the host). The audio does not (they are overwritten with zeros).
5. Interrupt latency: 68 cc from Paula's request to the first write was measured on the first interrupt
   only (where the request time is visible as the first DMA fetch). For later interrupts the timeline is
   consistent with the same latency and the request every 128 000 cc, but the request time itself is not
   in the logs.
6. Song end: reported during the fill of the last buffer; what happens after it (the host stops a few
   thousand cc later) is the host's behaviour. The player itself would loop forever (restarting the
   stream, state reset). The stop/end writes (SPEC 6) are not in any log.
7. Output stage: the engine lists `adpcm` among the PC formats played without Amiga filters, but the
   host matches by format name and this player's format name is its player name, so the reference run
   used the A500 filter (`filter=a500(auto)`). The lead decides whether that list entry should apply.
8. Timing granularity: half-cc costs (6.5 per index clamp) are rounded once per fill; this leaves up to
   3 cc error on some write times.

## Errata found while implementing

- The 0.5 cc in the decode cost rounds half-down (fits better than half-up).
- The first VOL write start phase varies per module in the reference (-8 to +21 cc); the port fixes it at 618.
