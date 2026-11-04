//! Per-build wrapper constants of the `dl` driver's tick (68000 cycles), measured from the reference's
//! execution of each corpus module (spec dl2: the three-layer cycle sum). The part of a voice slot that
//! is module code and not the Eagleplayer's intercept or the universal driver core path: `w` = the cycles
//! before voice 0's core (the Eagleplayer's tick wrapper included), and from the previous voice's
//! volume write to each of voices 1..3's core; the fifth is the tail to the end of the tick. `core` is the
//! build's idle core path per voice where it differs from the universal 372. -1 = not measured.
//! Generated data; the leave-one-out family prediction below serves modules outside the table.

pub struct Build {
    pub key: (u32, u32, u32, u32),
    /// Code family: b'A' when the play routine lies before the init routine, else b'B'.
    pub fam: u8,
    pub w: [i32; 5],
    pub core: [i32; 4],
    /// The note-start preamble of each voice relative to the universal one (0 in most builds).
    pub na: [i32; 4],
}

/// The idle core path per voice, whatever the build (cycles from the voice's core start to the
/// volume write, the Eagleplayer's pre-write cost not included).
pub const CORE: i32 = 372;

pub static BUILDS: &[Build] = &[
    Build { key: (21249, 0xa36, 0x106, 0x96e), fam: b'A', w: [473, 103, 233, 219, 443], core: [372, 372, 372, 372], na: [-64, -55, -55, -55] },
    Build { key: (27436, 0x83a, 0x102, 0x772), fam: b'A', w: [472, 99, 229, 215, 431], core: [387, 384, 384, 376], na: [-11, -11, -11, -11] },
    Build { key: (19031, 0xa5c, 0x102, 0x994), fam: b'A', w: [469, 99, 229, 215, 443], core: [372, 372, 372, 372], na: [-39, -39, -39, -39] },
    Build { key: (45270, 0x3ce, 0x4da, 0x2cc), fam: b'B', w: [618, 261, 261, 261, 571], core: [291, 280, 280, 280], na: [-51, -51, -51, -51] },
    Build { key: (39086, 0x40e, 0x51a, 0x30c), fam: b'B', w: [539, 261, 261, 261, 571], core: [292, 280, 280, 280], na: [-39, -39, -51, -51] },
    Build { key: (12938, 0x256, 0x350, 0x188), fam: b'B', w: [486, 99, 231, 257, 583], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (10344, 0x1a8, 0x270, 0xe0), fam: b'B', w: [469, 99, 157, 157, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (9289, 0x3b4, 0x4c8, 0x2b2), fam: b'B', w: [622, 265, 265, 265, 583], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (17267, 0x20a, 0x31e, 0x108), fam: b'B', w: [560, 203, 203, 203, 583], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (14178, 0x3a6, 0x4ba, 0x2a4), fam: b'B', w: [560, 203, 203, 203, 583], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (13146, 0x76c, 0xe6, 0x6a4), fam: b'A', w: [393, 95, 225, 211, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (21589, 0x37c, 0x490, 0x27a), fam: b'B', w: [622, 265, 265, 265, 583], core: [372, 372, 372, 372], na: [-39, -39, -39, -39] },
    Build { key: (27024, 0x518, 0x62c, 0x416), fam: b'B', w: [622, 265, 265, 265, 583], core: [372, 372, 372, 372], na: [-39, -39, -39, -39] },
    Build { key: (39220, 0xcb4, 0xde6, 0x0), fam: b'B', w: [921, 289, 289, 289, 571], core: [322, 312, 310, 312], na: [-39, -51, -51, -51] },
    Build { key: (23655, 0x78e, 0xec, 0x6c6), fam: b'A', w: [401, 103, 227, 219, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (22044, 0xa6c, 0x102, 0x9a4), fam: b'A', w: [473, 103, 233, 219, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (21337, 0x824, 0xfa, 0x75c), fam: b'A', w: [473, 103, 233, 219, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (26387, 0x820, 0x102, 0x758), fam: b'A', w: [473, 103, 161, 161, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (19840, 0xa6c, 0x102, 0x9a4), fam: b'A', w: [473, 103, 233, 219, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (24389, 0x818, 0xfa, 0x750), fam: b'A', w: [473, 103, 233, 219, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (22471, 0x818, 0xfa, 0x750), fam: b'A', w: [473, 103, 233, 219, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (20320, 0xa6c, 0x102, 0x9a4), fam: b'A', w: [473, 103, 233, 219, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (7839, 0x814, 0xfa, 0x74c), fam: b'A', w: [473, 103, 233, 219, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (25572, 0x7f0, 0xf8, 0x74a), fam: b'A', w: [473, 103, 233, 219, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (21828, 0x812, 0xf8, 0x74a), fam: b'A', w: [473, 103, 233, 219, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (21860, 0x820, 0x102, 0x758), fam: b'A', w: [473, 103, 161, 161, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (11955, 0xa48, 0xfa, 0x980), fam: b'A', w: [489, 99, 215, 215, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (31444, 0x7b4, 0x120, 0x6ec), fam: b'A', w: [401, 103, 233, 219, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (25747, 0xa78, 0x10e, 0x9b0), fam: b'A', w: [473, 103, 233, 219, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (47219, 0xa90, 0x126, 0x9c8), fam: b'A', w: [473, 103, 233, 219, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (11705, 0x132, 0x19c, 0xd2), fam: b'B', w: [311, 65, 71, 91, 431], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (29840, 0x172, 0xf8, 0x202), fam: b'A', w: [-1, -1, -1, -1, 571], core: [-1, -1, -1, -1], na: [0, 0, 0, 0] },
    Build { key: (14024, 0x37a, 0x48c, 0x270), fam: b'B', w: [556, 199, 199, 199, 583], core: [372, 372, 372, 372], na: [-39, -39, -39, -39] },
    Build { key: (20805, 0x172, 0x266, 0xd2), fam: b'B', w: [533, 83, 83, 83, -1], core: [-1, -1, -1, -1], na: [0, 0, 0, 0] },
    Build { key: (19389, 0x3a4, 0x4b8, 0x2a2), fam: b'B', w: [560, 203, 203, 203, 583], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (8278, 0x392, 0x4a6, 0x290), fam: b'B', w: [556, 199, -1, -1, 583], core: [372, 372, -1, -1], na: [0, 0, 0, 0] },
    Build { key: (19919, 0x7ea, 0x132, 0x722), fam: b'A', w: [397, 99, 229, 215, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (21924, 0x8f4, 0x178, 0x82c), fam: b'A', w: [493, 103, 233, 219, 443], core: [354, 354, 354, 354], na: [0, 0, 0, 0] },
    Build { key: (19827, 0x3b8, 0x4cc, 0x2b6), fam: b'B', w: [622, 265, 265, 265, 583], core: [372, 372, 372, 372], na: [-39, -39, -39, -39] },
    Build { key: (11428, 0x86c, 0xf0, 0x7a4), fam: b'A', w: [-1, -1, -1, -1, 443], core: [-1, -1, -1, -1], na: [0, 0, 0, 0] },
    Build { key: (32887, 0x348, 0x45c, 0x256), fam: b'B', w: [552, 203, 203, 203, 607], core: [-1, -1, -1, -1], na: [0, 0, 0, 0] },
    Build { key: (25434, 0x3ec, 0x500, 0x2ea), fam: b'B', w: [622, 265, 265, 265, 583], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (19522, 0x392, 0x4a6, 0x290), fam: b'B', w: [560, 203, 203, 203, 583], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
    Build { key: (10807, 0xa0e, 0xe0, 0x946), fam: b'A', w: [473, 109, 239, 194, 1273], core: [372, 372, 372, 372], na: [-64, -64, -64, -64] },
    Build { key: (14623, 0x79e, 0xe6, 0x6d6), fam: b'A', w: [397, 99, 229, 215, 443], core: [372, 372, 372, 372], na: [0, 0, 0, 0] },
];

/// The wrapper constants used for a module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wrap {
    pub w: [i32; 5],
    pub core: [i32; 4],
    pub na: [i32; 4],
}

/// The most frequent value among the measured ones (the smaller on a tie).
fn mode(v: impl Iterator<Item = i32>) -> Option<i32> {
    let mut c: Vec<(i32, u32)> = Vec::new();
    for x in v {
        match c.iter_mut().find(|e| e.0 == x) {
            Some(e) => e.1 += 1,
            None => c.push((x, 1)),
        }
    }
    c.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|e| e.0)
}

/// The wrapper constants of the module of this build (`play` and `init` header offsets give its family).
/// Slots the table does not hold, and every slot of a build outside it (or all of them when `loo`
/// is set: the leave-one-out check), take the most frequent value of the family's other builds.
pub fn wrapper(key: (u32, u32, u32, u32), play: u32, init: u32, loo: bool) -> Wrap {
    let own = if loo { None } else { BUILDS.iter().find(|b| b.key == key) };
    let fam = if play < init { b'A' } else { b'B' };
    let mut w = [0i32; 5];
    let mut core = [CORE; 4];
    let mut na = [0; 4];
    for s in 0..5 {
        w[s] = match own.map(|b| b.w[s]).filter(|&x| x >= 0) {
            Some(x) => x,
            None => mode(BUILDS.iter().filter(|b| b.fam == fam && b.key != key).map(|b| b.w[s]).filter(|&x| x >= 0)).unwrap_or(0),
        };
    }
    if let Some(b) = own {
        for s in 0..4 {
            if b.core[s] >= 0 {
                core[s] = b.core[s];
            }
        }
        na = b.na;
    }
    Wrap { w, core, na }
}
