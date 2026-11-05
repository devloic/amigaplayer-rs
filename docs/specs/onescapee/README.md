# onEscapee spec - unknowns and caveats

* Files: SPEC.md (format, detection, replay), LOGS.md (logs and modules). A private working folder with the
  reading notes and the validation scripts exists and is not part of the spec.
* No real module. The module archive's two `one.*` files are ProTracker modules rejected by the player.
  All validation is on synthetic modules (LOGS.md). The format has no parameters apart from the file
  content, so the synthetic modules cover everything the player does; real modules differ only in
  size and sample data.
* Validation: a model written only from SPEC.md (init writes, interrupt times, buffer filling, cost
  table, pointer pattern, loop) reproduces all register writes (address, value, colour clock) of all 11
  synthetic reference logs exactly (0 mismatching writes), and the decoded byte stream equals the
  audio DMA words of channel 0 in the 8 DMA logs checked (0 mismatching words; all four channels
  identical).
* Not determined / caveats:
  - Behaviour for files shorter than 1024 bytes (the original misbehaves) and shorter than 100 bytes
    (detection reads past the end).
  - The absolute interrupt times come from the reference's Paula; the implementer's Paula must produce
    the 160 / 227 271 / 227 328 cc pattern by itself (period 222, length 512 words, restart after each
    buffer). The first interrupt after the DMA start is the usual length-load request.
  - The handler cost (6641 etc.) is the reference CPU's; it only matters for the cc of the pointer
    writes, not for the sound, as long as the handler finishes before the next buffer restart.
  - Stop (end of play) behaviour is from the player's source, not observed.
  - The host stops at the song end; the loop (restart from the first byte with a zero-padded final
    buffer) was observed on r3072 and r4096 only because the host's render step runs a little past the
    song end.
  - Real modules' own song end/looping and volume balance are as described; nothing in the original
    scales the volume per module.
* No data/ folder was produced: the format needs no bytes from the player.
