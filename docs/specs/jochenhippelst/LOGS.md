# Logs and reproduction

Write-log format: `ADDRESS VALUE COLOUR-CLOCK` per line (reference 68k player run through the host with
DELI_WRITE_LOG); first line `# module BASE SIZE`. Runs: silent (`--wav` to a null file,
SDL_AUDIO_DRIVER=dummy), 4-10 s, subsong 1 (`--subsong 1`, which the host treats as relative, so logs
start with a few zero-volume init writes; ignore writes before the first AUDxPER=1 write on $DFF0A6).
Ticks are 70940 cc apart; a tick's first write is about 1800-2000 cc after the entry.

| file | content |
|---|---|
| hst.sog.unicorn.sub1.ref.log | real module (raw TFMX with prefix code, tag at file offset 2708; Dragonflight, md5 13a52f2be23e90aafeaab841a3e93fc3) |
| hst.hipc.MUSUNI.sub1.ref.log | real module (COSO image, md5 e09616526b70c7e4686d2e720071fe29) |
| hst.syn*.sub1.ref.log + hst.syn* | synthetic COSO modules (my own generated files, see gen_synthetic.py), with their module files |
| hst.lsmp1(.sub1.ref.log), SMP.lsmp1 | synthetic two-file (LSMP) module and its companion |
| gen_synthetic.py | generator of the synthetic modules |

The corpus holds no real `hst.`/`sdc.` files; the two real modules are Amiga-family files that this
player's detection accepts. No real Atari ST module was available.
