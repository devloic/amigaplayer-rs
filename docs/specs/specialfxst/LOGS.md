# Reference logs

All logs: the reference 68k player run through the host with DELI_WRITE_LOG (lines `address value colour-clock`),
silent rendering, default volume and balance (gain 64), all voices on. All modules are synthetic (the archive
holds no DODA file; see README); they are in `logs/mods/` (`DODA.<name>`). Each is 2400 bytes: a fake driver
skeleton that carries only the byte patterns of SPEC 1 and 2.1 (all table references by displacement), the
tables, and the period-table marker at offset 2000.

In all runs the sample base `B` (SPEC 4.1) is 0x68AF00: noise 0x68AF00, pulse 0x68F302, silent 0x68F304.
The tick routine is called at `C0 + 70940 n`. The first line of each log gives the module address and size.

| log | C0 | ticks | content |
|---|---|---|---|
| t1.log | 1922884 | 49 | voice A only, tone, notes and a rest; B and C unused (silent voices restarted every tick) |
| t2.log | 1923113 | 399 | tone / noise / tone+noise voices, fixed duration, rest, tie |
| t3.log | 1923113 | 399 | arpeggio, slide, slide list (0x8C), note kick, envelope trigger, drums, envelope loops |
| t4.log | 1922900 | 399 | sequence repeat, transpose, extra pattern, sequence jump, stop; subsong 1 of 3 |
| t5.log | 1923113 | 399 | noise mixing with 3, 2, 1 and 0 tone+noise voices |
| t6.log | 1922982 | 54 | voices stop (0x80 in pattern and in sequence), 0x8D, song end with no active voice |
| t7.log | 1923058 | 24 | song end through sequence jumps on all voices |
| t8.log | 1923017 | 399 | sequence commands at subsong start (0xFF, 0x81, 0x80), pattern jump 0x81 |
| t4s2.log | 1926680 | 399 | the same module, subsong 2 (host started at 1 and advanced once) |
| t4s3.log | 1930619 | 399 | the same module, subsong 3 (advanced twice) |
| f0.log | 1922954 | 499 | random module (fuzz): random patterns, sequences and tables of every kind |
| f1.log | 1922954 | 499 | random module (fuzz): random patterns, sequences and tables of every kind |
| f2.log | 1923041 | 499 | random module (fuzz): random patterns, sequences and tables of every kind |
| f3.log | 1922900 | 499 | random module (fuzz): random patterns, sequences and tables of every kind |
| f4.log | 1923015 | 124 | random module (fuzz): random patterns, sequences and tables of every kind |
| f5.log | 1922900 | 499 | random module (fuzz): random patterns, sequences and tables of every kind |
| f6.log | 1922961 | 499 | random module (fuzz): random patterns, sequences and tables of every kind |
| f7.log | 1923024 | 499 | random module (fuzz): random patterns, sequences and tables of every kind |
| f8.log | 1922878 | 189 | random module (fuzz): random patterns, sequences and tables of every kind |
| f9.log | 1923022 | 499 | random module (fuzz): random patterns, sequences and tables of every kind |
| f10.log | 1922884 | 499 | random module (fuzz): random patterns, sequences and tables of every kind |
| f11.log | 1922900 | 499 | random module (fuzz): random patterns, sequences and tables of every kind |
| f12.log | 1922961 | 499 | random module (fuzz): random patterns, sequences and tables of every kind |
| f13.log | 1922900 | 499 | random module (fuzz): random patterns, sequences and tables of every kind |
| f14.log | 1922823 | 499 | random module (fuzz): random patterns, sequences and tables of every kind |
| f15.log | 1923015 | 284 | random module (fuzz): random patterns, sequences and tables of every kind |

A run ends at the host's song end (or after 8-10 s). Writes before `C0` belong to the subsong change
(end-sound writes, SPEC 5).

Validation: a private model written from SPEC.md reproduces every write of all 26 logs (value, order and
colour clock). f6 and f9 reach notes outside the period table (index -1); they match only with the
bytes in `data/` (README).
