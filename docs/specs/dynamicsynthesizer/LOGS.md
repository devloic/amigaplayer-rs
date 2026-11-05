# Reference write logs - Dynamic Synthesizer

All logs were produced by the reference 68k player run through the host with DELI_WRITE_LOG,
audio to a dummy driver, 20 s per subsong, with the module given as a short relative path
(`dns.title`, `dns.ingame`) next to its sample file. Line format: `ADDRESS VALUE CC`, with
ADDRESS a hardware register (hex, e.g. `dff096`, or `bfe001`), VALUE hex 16-bit, CC the colour
clock stamp. The first line is a comment `# module ADDR SIZE`.

| file | module | subsong | writes |
|---|---|---|---|
| title.sub0..7.ref.log | title tune (dns.title, 10184 bytes) | 0..7 | 5724-6859 |
| ingame.sub0..4.ref.log | in-game tune (dns.ingame, 17288 bytes) | 0..4 | 5679-7902 |

Each log covers 999 ticks after the optional init groups. They contain 20 s of audio; none of
them reaches the song end.

Things to know when comparing:
- The sample base in the LC values is host dependent (0x71A8 for title, 0x8D68 for in-game).
- A write is stamped at the end of the writing instruction; the stamp is
  floor((tick start in cycles + offset in cycles) / 2). See SPEC.md section 4.
- The absolute stamp of tick 0 depends on the module path string length and module size (about
  2 cc per path character); compare timings relative to the first write.
- Subsong s > 0 starts with s init groups (SPEC.md 4.1); subsong 0 has none.
- Long runs (about 136 s) of title 5 and in-game 0 and 3 were also used privately to measure
  the song-end tick and its extra 221-222 cycles. Those are not shipped (size).

Measured agreement of the private model (values, order and stamps) with these 13 logs: exact
for every write of every log (see README).
