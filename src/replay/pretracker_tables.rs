//! PreTracker's tables, from Raspberry Casket (Copyright (c) 2022 Chris 'platon42'
//! Hodges) and rePlayer's PreTracker C player (Copyright (c) 2026 Daniel Collin), MIT
//! licence: see LICENSE-PreTracker.

/// The length of one period of the oscillators' waveforms for each note of an
/// octave ($400000 / 2^(n/12) * $8000, rounded down).
pub const LOG12: [u8; 12] = [128, 121, 114, 107, 102, 96, 90, 85, 80, 76, 72, 67];

pub const VIB_SPEED: [u8; 16] = [2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 20, 40, 80];
pub const VIB_DEPTH: [u8; 16] = [0, 8, 9, 10, 11, 12, 13, 14, 18, 20, 28, 40, 50, 70, 160, 255];
pub const VIB_DELAY: [u8; 16] = [0, 4, 8, 10, 12, 14, 16, 18, 20, 24, 32, 40, 56, 96, 150, 255];
pub const RAMP_UP_16: [u8; 16] = [0, 1, 3, 6, 7, 9, 10, 11, 12, 13, 14, 16, 19, 35, 55, 143];
pub const FAST_ROLL_OFF_16: [u16; 16] =
    [0x400, 0x200, 0x80, 0x64, 0x50, 0x40, 0x30, 0x20, 0x10, 14, 12, 10, 8, 4, 2, 1];

/// The ADSR's and the volume envelope's decay steps (144 entries).
pub const ROLL_OFF: [u16; 144] = [
    0x400, 0x200, 0x180, 0x140, 0x100, 0xC0, 0xA0, 0x80, 0x78, 0x74, 0x6E, 0x69, 0x64, 0x5A, 0x46, 0x40, 0x38,
    0x30, 0x28, 0x20, 0x1F, 0x1E, 0x1D, 0x1C, 0x1B, 0x1A, 0x19, 0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11,
    0x10, 15, 14, 13, 13, 12, 12, 11, 11, 10, 10, 9, 9, 8, 8, 8, 8, 7, 7, 7, 7, 6, 6, 6, 6, 5, 5, 5, 5, 4, 4, 4,
    4, 4, 4, 4, 4, 4, 4, 3, 4, 4, 3, 4, 4, 3, 4, 3, 4, 3, 4, 3, 4, 3, 3, 3, 3, 3, 3, 3, 3, 3, 2, 3, 3, 3, 2, 3,
    3, 2, 3, 3, 2, 3, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 2, 2, 2, 2, 2, 2, 2, 1, 2, 1, 2, 1, 2, 1, 2, 1, 1, 2,
    1, 1, 1, 2, 1,
];

/// By (pitch - $219) / 64: how many octaves down the sample is taken, and by
/// how many fine steps (16 a note) the pitch goes down for it.
pub const OCTAVE_SELECT: [u8; 24] = [1, 1, 1, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3];
pub const OCTAVE_NOTE_OFFSET: [u8; 24] = [
    48, 48, 48, 96, 96, 96, 144, 144, 144, 144, 144, 144, 144, 144, 144, 144, 144, 144, 144, 144, 144, 144, 144,
    144,
];

/// The track delay's period shift, by the delay's distance.
pub const MINUS4_PLUS4: [i8; 8] = [-4, -5, -1, 1, 2, 3, 4, 0];

pub const RAMP_UP_DOWN_32: [u16; 32] = [
    0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448, 480, 480, 448, 416, 384, 352, 320, 288,
    256, 224, 192, 160, 128, 96, 64, 32, 0,
];

pub const MODULATOR_RAMP_8: [u16; 8] = [0x4D, 0x125, 0x21B, 0x437, 0x539, 0x755, 0x96D, 0xBD7];

/// Three octaves of Paula periods, and the last one again.
pub const PERIODS: [u16; 37] = [
    0x350, 0x320, 0x2F2, 0x2C8, 0x2A0, 0x279, 0x256, 0x236, 0x216, 0x1F8, 0x1DC, 0x1C0, 0x1A8, 0x190, 0x179,
    0x164, 0x151, 0x13E, 0x12C, 0x11B, 0x10B, 0xFC, 0xEE, 0xE0, 0xD4, 0xC8, 0xBD, 0xB2, 0xA8, 0x9F, 0x96, 0x8D,
    0x86, 0x7E, 0x78, 0x71, 0x71,
];
