//! Per-module data of the Dave Lowe replayer, written out from the specification's tables
//! (`dl-variants.tsv`, `dl-initplayer-patches.tsv`); generated data, not original code.

/// One row: hunk size in longs, the three header offsets (init, play, stop), then
/// envelope header words, held value at the terminator, rest returns, volume boost,
/// one-state blocks (IK+), DMA every tick, fade-in, sample-table fix-up id (0 = none).
pub struct Row {
    pub longs: u32,
    pub init: u32,
    pub play: u32,
    pub end: u32,
    pub env_hdr: u8,
    pub held: bool,
    pub rest_returns: bool,
    pub boost: bool,
    pub ik: bool,
    pub dma_tick: bool,
    pub fade: bool,
    pub rest64: bool,
    pub held1: bool,
    pub rest_env: u32,
    pub patch: u8,
}

pub const ROWS: &[Row] = &[
    Row { longs: 21249, init: 0xa36, play: 0x106, end: 0x96e, env_hdr: 1, held: true, rest_returns: false, boost: false, ik: false, dma_tick: false, fade: false, rest64: true, held1: false, rest_env: 0x7ca, patch: 0 },
    Row { longs: 27436, init: 0x83a, play: 0x102, end: 0x772, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x51a0, patch: 0 },
    Row { longs: 19031, init: 0xa5c, play: 0x102, end: 0x994, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x4ea2, patch: 0 },
    Row { longs: 45270, init: 0x3ce, play: 0x4da, end: 0x2cc, env_hdr: 2, held: true, rest_returns: false, boost: false, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x1d24, patch: 1 },
    Row { longs: 39086, init: 0x40e, play: 0x51a, end: 0x30c, env_hdr: 2, held: true, rest_returns: false, boost: false, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x239e, patch: 2 },
    Row { longs: 12938, init: 0x256, play: 0x350, end: 0x188, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x1658, patch: 0 },
    Row { longs: 10344, init: 0x1a8, play: 0x270, end: 0xe0, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x168e, patch: 0 },
    Row { longs: 9289, init: 0x3b4, play: 0x4c8, end: 0x2b2, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x253e, patch: 0 },
    Row { longs: 17267, init: 0x20a, play: 0x31e, end: 0x108, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x10cc0, patch: 0 },
    Row { longs: 14178, init: 0x3a6, play: 0x4ba, end: 0x2a4, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x1884, patch: 0 },
    Row { longs: 13146, init: 0x76c, play: 0xe6, end: 0x6a4, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x1a6c, patch: 0 },
    Row { longs: 21589, init: 0x37c, play: 0x490, end: 0x27a, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x1774, patch: 0 },
    Row { longs: 27024, init: 0x518, play: 0x62c, end: 0x416, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x2778, patch: 3 },
    Row { longs: 39220, init: 0xcb4, play: 0xde6, end: 0x0, env_hdr: 2, held: true, rest_returns: false, boost: false, ik: false, dma_tick: false, fade: true, rest64: false, held1: false, rest_env: 0x26272, patch: 0 },
    Row { longs: 23655, init: 0x78e, play: 0xec, end: 0x6c6, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x5234, patch: 0 },
    Row { longs: 22044, init: 0xa6c, play: 0x102, end: 0x9a4, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x199c, patch: 0 },
    Row { longs: 21337, init: 0x824, play: 0xfa, end: 0x75c, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x18c6, patch: 0 },
    Row { longs: 26387, init: 0x820, play: 0x102, end: 0x758, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x400e, patch: 0 },
    Row { longs: 19840, init: 0xa6c, play: 0x102, end: 0x9a4, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x190e, patch: 0 },
    Row { longs: 24389, init: 0x818, play: 0xfa, end: 0x750, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x1d9c, patch: 0 },
    Row { longs: 22471, init: 0x818, play: 0xfa, end: 0x750, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x1866, patch: 0 },
    Row { longs: 20320, init: 0xa6c, play: 0x102, end: 0x9a4, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x1fe0, patch: 0 },
    Row { longs: 7839, init: 0x814, play: 0xfa, end: 0x74c, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x160a, patch: 0 },
    Row { longs: 25572, init: 0x7f0, play: 0xf8, end: 0x74a, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x1a0e, patch: 0 },
    Row { longs: 21828, init: 0x812, play: 0xf8, end: 0x74a, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x152c, patch: 0 },
    Row { longs: 21860, init: 0x820, play: 0x102, end: 0x758, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x45a6, patch: 0 },
    Row { longs: 11955, init: 0xa48, play: 0xfa, end: 0x980, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x30dc, patch: 0 },
    Row { longs: 31444, init: 0x7b4, play: 0x120, end: 0x6ec, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x1a0e, patch: 0 },
    Row { longs: 25747, init: 0xa78, play: 0x10e, end: 0x9b0, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0xf1a, patch: 0 },
    Row { longs: 47219, init: 0xa90, play: 0x126, end: 0x9c8, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x1704, patch: 0 },
    Row { longs: 11705, init: 0x132, play: 0x19c, end: 0xd2, env_hdr: 0, held: false, rest_returns: true, boost: false, ik: true, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x3de, patch: 0 },
    Row { longs: 29840, init: 0x172, play: 0xf8, end: 0x202, env_hdr: 0, held: false, rest_returns: true, boost: false, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x3c2, patch: 0 },
    Row { longs: 14024, init: 0x37a, play: 0x48c, end: 0x270, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x1ffe, patch: 0 },
    Row { longs: 20805, init: 0x172, play: 0x266, end: 0xd2, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: true, fade: false, rest64: false, held1: false, rest_env: 0x143dc, patch: 0 },
    Row { longs: 19389, init: 0x3a4, play: 0x4b8, end: 0x2a2, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x518c, patch: 0 },
    Row { longs: 8278, init: 0x392, play: 0x4a6, end: 0x290, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x1ece, patch: 0 },
    Row { longs: 19919, init: 0x7ea, play: 0x132, end: 0x722, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x6108, patch: 0 },
    Row { longs: 21924, init: 0x8f4, play: 0x178, end: 0x82c, env_hdr: 1, held: true, rest_returns: true, boost: false, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x66a, patch: 4 },
    Row { longs: 19827, init: 0x3b8, play: 0x4cc, end: 0x2b6, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x318c, patch: 0 },
    Row { longs: 11428, init: 0x86c, play: 0xf0, end: 0x7a4, env_hdr: 1, held: true, rest_returns: true, boost: false, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x5e2, patch: 0 },
    Row { longs: 32887, init: 0x348, play: 0x45c, end: 0x256, env_hdr: 2, held: true, rest_returns: false, boost: false, ik: false, dma_tick: false, fade: false, rest64: false, held1: true, rest_env: 0x200dc, patch: 0 },
    Row { longs: 25434, init: 0x3ec, play: 0x500, end: 0x2ea, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x258a, patch: 0 },
    Row { longs: 19522, init: 0x392, play: 0x4a6, end: 0x290, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x4382, patch: 0 },
    Row { longs: 10807, init: 0xa0e, play: 0xe0, end: 0x946, env_hdr: 1, held: true, rest_returns: false, boost: false, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x2592, patch: 0 },
    Row { longs: 14623, init: 0x79e, play: 0xe6, end: 0x6d6, env_hdr: 2, held: true, rest_returns: false, boost: true, ik: false, dma_tick: false, fade: false, rest64: false, held1: false, rest_env: 0x2834, patch: 0 },
];

/// Sample-table fix-ups: (hunk offset, is 32-bit, new value), per id (1-based).
pub const PATCHES: &[&[(u32, bool, u32)]] = &[
    // Beneath_a_Steel_Sky__game_complete
    &[(0x1bd6, true, 0x293f4), (0x1be4, true, 0x1ed4a), (0x1bea, true, 0x1fd97), (0x1c00, true, 0x1daa6), (0x1c0e, true, 0x1c76c), (0x1c1c, true, 0xe680), (0x1c2a, true, 0xfb0a), (0x1c38, true, 0x19574), (0x1c46, true, 0x8c64), (0x1c54, true, 0x81c0), (0x1c62, true, 0x98b4), (0x1c70, true, 0x10fda), (0x1c7e, true, 0xc3e8), (0x1c8c, true, 0x4108), (0x1c9a, true, 0x1e0c), (0x1ca8, true, 0xa358), (0x1cb6, true, 0x6132), (0x1cc4, true, 0x1a686), (0x1cd2, true, 0x27f96), (0x1ce0, true, 0x2a9a4), (0x1ce6, true, 0x2a9a4), (0x1cee, true, 0x252d8), (0x1cf4, true, 0x2614e), (0x1cfc, true, 0x210be), (0x1d0a, true, 0x134e6), (0x1d18, true, 0x15ba0)],
    // Beneath_a_Steel_Sky__intro
    &[(0x220a, true, 0x2070a), (0x2218, true, 0x17b6c), (0x221e, true, 0x18bb9), (0x2234, true, 0x14f7e), (0x2242, true, 0x12164), (0x2250, true, 0x7440), (0x225e, true, 0x88ca), (0x226c, true, 0x10f3a), (0x227a, true, 0xfe28), (0x2288, true, 0x34da), (0x2296, true, 0x2486), (0x22a4, true, 0x678a), (0x22b2, true, 0x1aa32), (0x22c0, true, 0x19ee0), (0x22ce, true, 0x1349e), (0x22dc, true, 0x1428e), (0x22ea, true, 0x16e72), (0x22f8, true, 0x16222), (0x2306, true, 0x5096), (0x2314, true, 0x45f2), (0x2322, true, 0x5ce6), (0x2330, true, 0x1e2ec), (0x233e, true, 0x236d6), (0x234c, true, 0x24c56), (0x235a, true, 0x21cba), (0x2360, true, 0x21cba), (0x2368, true, 0x1b62e), (0x236e, true, 0x1c4a4), (0x2376, true, 0x1f74a), (0x2384, true, 0x9d9a), (0x2392, true, 0xc454)],
    // F-15_Strike_Eagle_2__F-15_Strike_Eagle_II
    &[(0x25da, true, 0x278a), (0x25e0, true, 0x1a596), (0x25e8, true, 0x7117), (0x25ee, true, 0x1a596), (0x25f6, true, 0x78a9), (0x25fc, true, 0x1a596), (0x2604, true, 0x7f09), (0x260a, true, 0x1a596), (0x2612, true, 0x3f43), (0x2618, true, 0x1a596), (0x2620, true, 0x4e1c), (0x2626, true, 0x1a596), (0x262e, true, 0x66d1), (0x2634, true, 0x1a596), (0x263c, true, 0x2a23), (0x2642, true, 0x2a23), (0x264a, true, 0x8612), (0x2650, true, 0x1a596), (0x2658, true, 0x988a), (0x265e, true, 0x1a596), (0x2666, true, 0xb3ca), (0x266c, true, 0x1a596), (0x2674, true, 0xc55e), (0x267a, true, 0x1a596), (0x2682, true, 0xd8f6), (0x2688, true, 0x1a596), (0x2690, true, 0xf0c0), (0x2696, true, 0x1a596), (0x269e, true, 0x108b2), (0x26a4, true, 0x1a596), (0x26ac, true, 0x138e2), (0x26b2, true, 0x1a596), (0x26ba, true, 0x14444), (0x26c0, true, 0x1a596), (0x26c8, true, 0x14b54), (0x26ce, true, 0x14b54), (0x26d6, true, 0x14fbe), (0x26dc, true, 0x14fbe), (0x26e4, true, 0x1590a), (0x26ea, true, 0x1590a), (0x26f2, true, 0x17c02), (0x26f8, true, 0x1a596), (0x2700, true, 0x1903a), (0x2706, true, 0x1a596), (0x270e, true, 0x1a34c), (0x2714, true, 0x1a34c), (0x271c, true, 0x1a62e), (0x2722, true, 0x1a62e), (0x2728, true, 0x1a596)],
    // Raffles__Raffles
    &[(0x3102, true, 0x4e7d), (0x3110, true, 0x35c8), (0x311e, true, 0x5d56), (0x312c, true, 0x6566), (0x313a, true, 0x6c6f), (0x3140, true, 0x6c6f), (0x3148, true, 0x7f0f), (0x314e, true, 0x7f0f), (0x3156, true, 0x7b0f), (0x315c, true, 0x7b0f), (0x3164, true, 0x728f), (0x3172, true, 0x9432), (0x3180, true, 0xac47), (0x318e, true, 0xc029), (0x319c, true, 0xe185), (0x31aa, true, 0xe50e), (0x31b8, true, 0xe940), (0x31c6, true, 0xedab), (0x31d4, true, 0xf79d), (0x31e2, true, 0x10867), (0x31f0, true, 0x10cc5), (0x31fe, true, 0x1152a), (0x320c, true, 0x13533), (0x321a, true, 0x1489f)],
];
