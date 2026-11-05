# What stays unknown / caveats

1. No real module. The archive holds no SynTracker file (see LOGS.md); every test module is
   synthetic. Things real files may do that no test covers: note indices above 71 (the period is
   then read from whatever follows the table in the player's memory; SPEC.md 3 gives the first
   16 words for index 72..87 as read from the sine table, beyond that unknown), instrument bytes
   0x80..0xFE (reference reads outside the instrument table), starting instrument 0 before any
   instrument was set (reads record 255, garbage), pattern numbers above the highest pattern,
   positions lists longer than nPos, sample offsets pointing outside the file, instrument +4 length
   not 16 for synth instruments (a different LEN is written but the buffer is still 32 bytes).
2. The text block at 0x14 (32 x 32 bytes) is handed to the host as info text; that it holds the
   instrument names is a guess (size and layout fit). The player has no title, author or
   subsong data.
3. Timing is approximate (SPEC.md 10): the tick start (host), the whole row-tick processing
   (every effect and instrument-start variant has its own cost; only a few observed values are
   listed in 10.3), the end-of-tick copy and the delay between DMACON and the end of the tick.
   Plain-tick gaps are reproducible to +-1 cc for PER and VOL, the morph/scan sections within
   +-15 cc typically (max about 32). A cycle accurate 68000 model of the original (own estimate,
   private) was tried: with textbook cycle counts it is 1..8 cc too fast per segment, and MULU/DIVU
   in the reference are NOT data dependent (the time of a vibrato period is constant, only the
   branch taken matters), so a plain "sum the 68000 cycles" model needs per-instruction fixes; the
   spec therefore gives measured figures instead.
4. The CIA effect F (LED filter) writes CIA-A PRA (bfe001): the logged byte value (0x00 / 0x02)
   depends on the host's CIA contents, only bit 1 is meaningful.
5. Song end: the reference host calls its song-end routine when the position index wraps to 0
   (in the same row tick, after the last row); what the host then does (stop, loop) is not part of
   the player. Jumps by B never signal the end, so modules that jump back never end.
6. Master volume per channel is 64 in these logs (UADE default); other hosts may pass other
   balance values; the formula is `(value * master) >> 6`.
7. The chip buffer address (written into AUDxLC) is chosen by the host; under UADE 0x006861F8 for the
   buffer start. Sample instrument LC values are module address + D + offsets.
8. DMA words fetched while the 128-byte end-of-tick copy runs may see old or new bytes (real
   Paula timing); the copy takes about 370 cc (10.2).
9. The speed byte of effect S is 8 bit; speeds above 255 impossible; speed 0 is ignored.
10. Effect A with the note index 0 is ignored by the reference (a quirk kept in SPEC 7).
