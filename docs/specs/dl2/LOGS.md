# Logs and commands (dl2)

None of the logs, scripts or traces named here is shipped (see `docs/specs/README.md` for how to regenerate reference logs).

Wrappers: a full instruction trace + write log per module, and phase runs. Scripts (names as used by the spec team): trlib, segs, pairs, cover,
landmarks, wrapper, epcost, master, wavlag, subcheck, first, ...

| Log | Producer | Content |
|---|---|---|
| logs/wrapper_table.json | wrapper.py, wtable.py | per-build wrapper W0..W4 |
| logs/family.json | pairs.py | family A/B per build |
| logs/pair_medians.json | pairs.py | write-pair gap medians per build |
| logs/first_tick.txt/.json | first.py | first write offset (item 2) |
| logs/master_rule.txt | master2.py | DMA-on word per voice vs code (item 3) |
| logs/subsong_check.txt | subrun.sh, subcheck.py | 64 subsong runs vs formula (item 7), "ok 61 bad 3" |
| logs/lag_adj_all.txt | lagall.py (DELI_START_ADJ=100) | tick-1 lag per file after fix (item 8) |
| logs/lag_adj_older.txt | lagall.py | same for older_* |
| logs/syn/ | synthetic patched modules | item 4 scan-form tests |
