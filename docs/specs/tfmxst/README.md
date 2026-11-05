# TFMX ST - caveats

Private notes, scripts and the validation model exist in a private working folder (not for the implementer).

Validation: a private model written from this behaviour reproduces the exact write VALUES and ORDER of
the reference for 186 of 200 random synthetic modules (the other 14 use the beam-random riff variant),
all 9 handcrafted modules, and subsongs 1 and 2 of hc/mdst.h2; the subsong-list rule of section 4
matched the reference count on 60 of 60 test modules. Timing: the per-tick cost table (9.3) predicts
interrupt entry to conversion entry within 1 cc on 99.1% of 28,693 ticks. The conversion-step gaps (9.2)
are measured values; an end-to-end timing simulation was NOT built or validated.

Unknowns:
* No real modules: the corpus has no `mdst.` files and no other TFMX header file passes detection.
* Riff mode bit 0 and riff notes summing to 0 take a variant that reads the beam position; not specified.
* A macro return ($16) or pattern return without a prior call jumps to host low memory.
* The INTREQR wait exits depend on Paula's internal period counter; poll a Paula model.
* Cost-table entries for paths missing from the fitted set (listed in 9.3).
* Digital sample length 0 (= 65536 words) is what the reference writes; untested with long samples.

No data/ folder: all tables are small and given inline, and the noise buffer is generated (section 9).
The lead decides on embedding if a data/ file is ever added.
