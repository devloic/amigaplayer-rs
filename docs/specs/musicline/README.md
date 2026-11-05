# What remains unknown (MusicLine timing)
Exact and validated: clock/stretch model, CIA entry, loader stretches, start-clock
prediction (given init stretch), idle tick (10885 cyc), write order.
Not solved: (1) closed form of the first sequencer row cost (init stretch is
372668 + that row); (2) per-signature constants of the five instrument-effect
functions (per-iteration 232..393 cyc, data dependent); (3) the wave-vs-sample
retrigger pricing rule; (4) +64 and +518 cyc init outliers on a few modules and
module 127's +202; (5) 8-voice modules. Section 4 values are measured ranges,
not derivations. The implementer should treat sections 1-3 and 5 as exact and
section 4 as calibration targets.
