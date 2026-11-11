# Fred Gray - caveats

- Modules used: the four real modules in the archive (Black Lamp, Star Goose,
  Victory Road, Eco), 20 subsongs in total. No synthetic modules.
- Register values and write order, including DMA pointer/length writes, were
  reproduced exactly by a private model for all subsongs over 400 s or until
  song end. Colour-clock timing is not register-exact: the cost model is
  standard cycles / 2 plus up to 25 colour clocks of unexplained jitter.
- The start phase (time to the first init and to the first tick) is given as a measured rule in SPEC section 12, residual at most 2 cc for the first init and about 5 cc for the first tick.
- Unknown: origin of that jitter; exactly when the host stops after song end
  (0-2 ticks after section 0 becomes current).
- The code that runs the sections lives in each module, so an implementer needs
  a small 68000-subset interpreter (SPEC section 7). A fifth module using the
  same driver is expected to work if its tables are located as in SPEC 2.6, but
  none was available to test.
- The wrapper's glide patch is reproduced (glide bit has no effect).
- No data/ files were produced; nothing needed from the player binary is
  absent from the modules. The lead decides on any embedding.
- Private analysis notes, scripts and the disassembly exist in a separate
  folder that the implementer must not open.

## Errata found by the implementation

Measured afterwards against reference logs that carry the half colour clock (68000 cycle), relative
to the song start; with them every write of the 20 subsongs (60 s each) falls on the reference's
cycle. They replace the "12 to 25 cc of jitter" of SPEC 11 / 12.3 and LOGS.md:

- There is no jitter. The wrapper takes a fixed number of 68000 cycles at each store of the play
  routine to an audio register, on top of the module's own instructions. Builds that store
  through `a5` (Black Lamp, Star Goose, Victory Road): 12 after the period stores of channels 0
  and 1, 116 after channel 2's, 104 after the volume stores of channels 0 and 1, 128 after channel
  2's, 20 after the low word of a pointer, 183 after a length (an odd number: the stores behind it
  fall on the other half of the colour clock). The build with absolute stores (Eco): 68 before
  channel 0's period store, then 44, 60 and 148 after the three period stores, 104 after the volume
  stores of channels 0 and 1; its pointer and length stores cost nothing.
- The timer interrupt comes exactly 70940 cc after the return of the last init and every 70940 cc
  after that; the play routine is entered 140 cc (first builds) or 120 cc (Eco) after the
  interrupt (SPEC 12.2 gives 126 +/- 5 after the final DMACON write, which is 8 cc before the
  return). An interrupt that falls due while the host changes the subsong is served when the new
  init returns, with the same 140 / 120 cc before the play routine; the following ones keep the
  old phase.
- Every tick on which section 0 is current takes 116 cycles more than a plain return before the
  rest of the tick (the song-end call into the host).
- The glide patch does not look at what follows the status register restore (0x46DF): the restore
  and the word behind it are overwritten with no-ops, and one word more when that word's low byte
  is zero (the same rule as the patched area of SPEC 12.1). In a test copy with another instruction
  behind the restore, the reference's note starts are shorter by that instruction's cycles less a
  no-op's.
- Eco rewrites its wave buffer in the play routine before the first register store of a tick. What
  the audio DMA reads on the lines between the interrupt and that store is the buffer as it is at
  the store (the engine fetches lazily), so a port must take its tick event at the interrupt, not
  at the play routine's entry, to play the same words.
