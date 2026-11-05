# Blade Packer: unknowns and caveats

Blade Packer is a self-contained tracker format (song file plus sample file), not a converter of
another module format. SPEC.md describes it completely as far as the reference runs show.

## Unknowns

- Behaviour after song end: the host stops or steps to the next subsong about two ticks after, so
  the restart position was only partly observable.
- Undefined (depends on host memory in the reference): an arpeggio or note before any sample was
  selected on a channel; finetune values above 15; sample numbers beyond the sample table;
  command bytes that are not multiples of 4; jumps beyond the position list.
- Bus-contention jitter of up to 9 colour clocks on write times cannot be predicted.
- The optional sample-file header (four letters SPLS) was tested only on a synthetic prefix.
- Only the first four of eight tracks per position are played; 8-track layout tested synthetically.
- Several effect behaviours are quirks (inverted E1/E2, command 8 slot shift, signed arpeggio
  parameter); they are as observed and verified, and must not be "fixed".

## Data

`data/periods.bin` (finetune period table, 16 rows of 36 big-endian words) and `data/waves.bin`
(three 64-byte waveforms) hold tables that live in the player. The lead decides on embedding.

## Synthetic modules

Edited corpus modules in `logs/syn` are synthetic; the lead decides whether they are shipped.

The folder a private working folder holds reading notes and scripts and is not part of the specification.
