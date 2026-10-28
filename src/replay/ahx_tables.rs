// AHX tables, from 8bitbubsy's ahx2play (Copyright (c) 2021-2024, Olav
// Sørensen, BSD 3-Clause licence: see LICENSE-ahx2play).

/// A note's period. The original's table is followed by its vibrato table's
/// first entries (below), which notes 61 to 63 read in a portamento's limit.
const PERIODS: [u16; 64] = [
    0, 3424, 3232, 3048, 2880, 2712, 2560, 2416, 2280, 2152, 2032, 1920, 1812, 1712, 1616, 1524, 1440, 1356, 1280,
    1208, 1140, 1076, 1016, 960, 906, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360,
    339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113, //
    0, 24, 49,
];

const VIBRATO: [i16; 64] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197,
    180, 161, 141, 120, 97, 74, 49, 24, 0, -24, -49, -74, -97, -120, -141, -161, -180, -197, -212, -224, -235, -244,
    -250, -253, -255, -253, -250, -244, -235, -224, -212, -197, -180, -161, -141, -120, -97, -74, -49, -24,
];

/// Where a wavelength's triangle or sawtooth starts among the six.
const WAVE_OFFSETS: [u32; 6] = [0x00, 0x04, 0x0C, 0x1C, 0x3C, 0x7C];

/// The 45 waveforms each filter pass runs over: six triangles, six sawtooths,
/// 32 squares and the noise.
const FILTER_LENGTHS: [usize; 45] = [
    0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, //
    0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, //
    0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, //
    0x780,
];
