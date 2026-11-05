# Unknowns and decisions (dl2)

1. Unseen builds: W0..W4 wrappers are only predictable by family mode (LOO: 98/200 exact, median 4 cyc,
   p90 66, max 830). Exactness needs the build's own code cycles. Decision: let the repo's
   existing 68k core execute the module driver for timing (the reference does exactly that), versus
   family tables. Per-build gap tables are the thing that fails to transfer.
2. First-write offset: data-dependent via command count before the first note (sequence data); the
   per-command handler costs are known only for indices seen (1: 135, 3: 62, 8 with EP writes 342-837).
   Not tabulated for every index.
3. Master bit (item 3): per-voice per-build code constant, no header predictor; read the immediate.
4. Item 4: neither-form-found case not observed. Form 2 formula verified on synthetic modules only.
5. Item 6: S (init setup + reset burst) has no header predictor; X routines are module code. Defaults
   (5664, X=0) are guesses for unseen builds.
6. Item 7: Time Scanner ts term (37 vs 21) from one file, 3 runs. Stop-call durations measured per class
   only; Frontier II 60 is one file.
7. Item 8: the +100 fix removes lag everywhere but equal-sample % stays 41-92% in many files because of
   in-tick write spacing (item 1) and residual cases (Darius, SF2, Raffles, older_F1).
8. Held-out validation: items 3, 7, 8 are rules checked on all files; item 1 held-out via leave-one-out
   only for wrapper values, not yet as an end-to-end exact-write percentage.
