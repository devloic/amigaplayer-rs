//! The EaglePlayer side of the player interface (see EaglePlayer.i in the
//! Eagleplayer 2.06 developer kit). Eagleplayers are DeliPlayers with extra
//! `EP_*` tags. They get their own globals block: `EaglePlayerGlobals` is the
//! DeliTracker structure only up to `dtg_NotePlayer`, where the `EPG_*` fields
//! begin, and below it (negative offsets from A5) lies a table of 6-byte
//! `ENPP_*` entry points.
//!
//! Most of these players use the "amplifier" interface, which is not in the
//! kit: `EP_InitAmplifier` hands the host a tag list, and from then on the
//! player sends its Paula writes through `ENPP_PokeAdr/Len/Per/Vol` and
//! `ENPP_DMAMask`. Like UADE's score, the host writes them straight to Paula,
//! unless the player has more than 4 voices or leaves them in AudioStructs
//! (no `EPAMB_Direct`): then they go to the mixing amplifier (amplifier.rs).

use crate::deli::{self, dtg, TAG_USER};
use crate::machine::{Machine, Trap};
use m68k::AddressBus;
use std::collections::HashMap;

pub const EP_BASE: u32 = TAG_USER + 0x4550; // TAG_USER + "EP"
pub const MI_BASE: u32 = TAG_USER + 0x4D49; // TAG_USER + "MI"
pub const EPAMT_BASE: u32 = TAG_USER + 0x414D; // TAG_USER + "AM"

#[allow(dead_code)]
pub mod tag {
    use super::EP_BASE as B;
    pub const GET_MODULE_INFO: u32 = B;
    pub const VOICES: u32 = B + 2;
    pub const SAMPLE_INIT: u32 = B + 3;
    pub const MODULE_CHANGE: u32 = B + 6;
    pub const STRUCT_INIT: u32 = B + 8;
    pub const GET_POSITION_NR: u32 = B + 12;
    pub const SET_SPEED: u32 = B + 13;
    pub const FLAGS: u32 = B + 14;
    pub const PLAYER_VERSION: u32 = B + 16;
    pub const CHECK3: u32 = B + 20;
    pub const NEW_MODULE_INFO: u32 = B + 22;
    pub const CHECK5: u32 = B + 28;
    pub const INIT_AMPLIFIER: u32 = B + 35;
    pub const EAGLE_BASE: u32 = B + 42;
    pub const SET_PLAY_FREQUENCY: u32 = B + 45;
    /// One past the last tag the 2.06 kit defines (EP_SamplePlayer).
    pub const END: u32 = B + 47;
}

/// ModuleInfo tags (`EP_NewModuleInfo`, `EP_Get_ModuleInfo`).
pub mod mi {
    use super::MI_BASE as B;
    pub const SONG_NAME: u32 = B;
    pub const AUTHOR_NAME: u32 = B + 1;
    pub const SUB_SONGS: u32 = B + 2;
}

/// Amplifier tags (`EPG_AmplifierTagList`).
pub mod epamt {
    use super::EPAMT_BASE as B;
    pub const NUM_STRUCTS: u32 = B;
    pub const AUDIO_STRUCTS: u32 = B + 1;
    pub const FLAGS: u32 = B + 3;
}

/// EPAMT_Flags bits, as the players' InitAmplifier routines set them.
pub mod epamb {
    /// The player pokes Paula through ENPP_Poke*: no mixing needed.
    pub const DIRECT: u32 = 1;
    /// Signed 8-bit samples.
    pub const BIT8: u32 = 2;
    /// The next kind of samples: the 8 Bit Amplifier takes them as unsigned
    /// 8-bit ones, and PS3M sets it for S3M modules with unsigned samples.
    pub const BIT16: u32 = 4;
    pub const CHIP_RAM: u32 = 0x10;
    /// AudioStruct interrupts (`aus::INT`).
    pub const AUDIO_INTS: u32 = 0x40;
}

/// EaglePlayerGlobals offsets past the DeliTracker part.
pub mod epg {
    pub const SAVE_MEM: u32 = 0x80; // first of the 14 function pointers ending at ModuleRestore
    pub const CIAB_BASE: u32 = 0x108;
    pub const IDENTIFIER: u32 = 0x118;
    pub const EAGLE_VERSION: u32 = 0x11C;
    pub const VOICES: u32 = 0x146;
    pub const VOICE1_VOL: u32 = 0x148;
    pub const FIRST_SND: u32 = 0x160;
    pub const SUB_SONGS: u32 = 0x162;
    pub const ARGN: u32 = 0x122;
    pub const ARG1: u32 = 0x126;
    pub const ARG2: u32 = 0x12A;
    pub const ARG3: u32 = 0x12E;
    pub const ARG4: u32 = 0x132;
    pub const ARG5: u32 = 0x136;
    pub const PLAYER_TAG_LIST: u32 = 0x16C;
    pub const MODULE_SIZE: u32 = 0x1BC;
    pub const AMPLIFIER_TAG_LIST: u32 = 0x1D8;
    pub const EAGLEPLAYER_DIR_ARRAY_PTR: u32 = 0x1F0;
    pub const SONG_NAME: u32 = 0xF8;
    pub const SIZE: u32 = 0x1F4;
}

/// EAGLEVERSION of the 2.06 kit's EaglePlayer.i.
pub const EAGLE_VERSION: u32 = 13;

/// The ENPP_ jump table, in order: entry `i` is at A5 - 6 * (i + 1).
const ENPP_NAMES: [&str; 65] = [
    "AllocSampleStruct", "NewLoadFile2", "MakeDirCorrect", "TestAufHide", "ClearCache", "CopyMemQuick",
    "GetPassword", "StringCopy2", "ScreenToFront", "WindowToFront", "GetListData", "LoadFile", "CopyDir",
    "CopyFile", "CopyString", "AllocAudio", "FreeAudio", "StartInterrupt", "StopInterrupt", "SongEnd",
    "CutSuffix", "SetTimer", "WaitAudioDMA", "SaveMem", "FileReq", "TextRequest", "LoadExecutable",
    "NewLoadFile", "ScrollText", "LoadPlConfig", "SavePlConfig", "FindTag", "FindAuthor", "Hexdez", "TypeText",
    "ModuleChange", "ModuleRestore", "StringCopy", "CalcStringSize", "StringCMP", "DMAMask", "PokeAdr",
    "PokeLen", "PokePer", "PokeVol", "PokeCommand", "Amplifier", "TestAbortGadget", "GetEPNrfromMessage",
    "InitDisplay", "FillDisplay", "RemoveDisplay", "GetLocaleString", "SetWaitPointer", "ClearWaitPointer",
    "OpenCatalog", "CloseCatalog", "AllocAmigaAudio", "FreeAmigaAudio", "RawToFormat", "FindAmplifier",
    "UserCallup5", "GetLoadListData", "SetListData", "GetHardwareType",
];
pub const ENPP_COUNT: u16 = ENPP_NAMES.len() as u16;
const ENPP_ALLOC_SAMPLE_STRUCT: u16 = 0;
const ENPP_GET_LIST_DATA: u16 = 10;
const ENPP_WAIT_AUDIO_DMA: u16 = 22;
const ENPP_SAVE_MEM: u16 = 23;
const ENPP_LOAD_EXECUTABLE: u16 = 26;
const ENPP_MODULE_CHANGE: u16 = 35;
const ENPP_NEW_LOAD_FILE: u16 = 27;
const ENPP_DMA_MASK: u16 = 40;
const ENPP_POKE_ADR: u16 = 41;
const ENPP_POKE_VOL: u16 = 44;
const ENPP_POKE_COMMAND: u16 = 45;
const ENPP_AMPLIFIER: u16 = 46;
const ENPP_GET_HARDWARE_TYPE: u16 = 64;
/// The jump table, rounded up so the globals stay longword aligned.
const JUMP_TABLE: u32 = (ENPP_COUNT as u32 * 6 + 3) & !3;
/// EPS_SizeOf (one SampleInfo structure).
const SAMPLE_STRUCT_SIZE: u32 = 40;

pub fn enpp_offset(i: u16) -> i32 {
    -6 * (i as i32 + 1)
}

#[derive(Default)]
pub struct EagleHost {
    /// The EaglePlayerGlobals (0 until the first Eagleplayer is loaded).
    pub globals: u32,
    /// Memory handed out for the current module (SampleInfo structures,
    /// executables).
    pub allocs: Vec<u32>,
    /// ENPP_ entries already reported as unimplemented.
    logged: u128,
    /// A player poking Paula itself (EPAMB_Direct, 4 voices or fewer) that asked for
    /// AudioStruct interrupts: (AudioStructs, how many). The Interrupt of voice v's struct is
    /// Caused when Paula channel v starts a block, as when the amplifier plays the voices.
    pub direct_ints: Option<(u32, u32)>,
}

/// Whether a player's tags make it an Eagleplayer.
pub fn has_ep_tags(tags: &HashMap<u32, u32>) -> bool {
    tags.keys().any(|&t| (EP_BASE..tag::END).contains(&t))
}

/// Sets up the EaglePlayerGlobals and the ENPP_ table below them. Needs the
/// DeliTracker globals, whose first part it copies.
pub fn init_globals(m: &mut Machine) -> u32 {
    if m.deli.eagle.globals != 0 {
        return m.deli.eagle.globals;
    }
    let top = std::mem::replace(&mut m.alloc_top, true);
    let blk = m.alloc(JUMP_TABLE + epg::SIZE + 0x10);
    m.alloc_top = top;
    let g = blk + JUMP_TABLE;
    m.deli.eagle.globals = g;
    let dg = m.deli.globals;
    for off in (0..dtg::NOTE_PLAYER).step_by(4) {
        let v = m.bus.r32(dg + off);
        m.bus.w32(g + off, v);
    }
    for i in 0..ENPP_COUNT {
        let a = g.wrapping_add_signed(enpp_offset(i));
        m.stub_at(a, Trap::Eagle(i));
        m.bus.w16(a + 4, 0x4E71); // nop
    }
    // EPG_SaveMem .. EPG_ModuleRestore are the same functions as ENPP_SaveMem ..
    for k in 0..14 {
        let a = g.wrapping_add_signed(enpp_offset(ENPP_SAVE_MEM + k));
        m.bus.w32(g + epg::SAVE_MEM + 4 * k as u32, a);
    }
    let ciab = m.os.libs.iter().find(|l| l.name == "ciab.resource").map(|l| l.base).unwrap_or(0);
    m.bus.w32(g + epg::CIAB_BASE, ciab);
    m.bus.write_bytes(g + epg::IDENTIFIER, b"EPGL");
    m.bus.w32(g + epg::EAGLE_VERSION, EAGLE_VERSION);
    m.bus.w16(g + epg::VOICES, 15);
    for v in 0..4 {
        m.bus.w16(g + epg::VOICE1_VOL + 2 * v, 64);
    }
    // the directory the Eagleplayers were loaded from, as the volume `EAGLE_DIR_VOLUME` (the
    // block's spare bytes after the globals hold it): PlayAY locks it and makes it the current
    // directory to LoadSeg its sub-player "AYPlayers/ZXAY<type>" when it is not found in the
    // current one (EP206 has AYPlayers/ in its Eagleplayers directory)
    m.bus.write_cstr(g + epg::SIZE, EAGLE_DIR_VOLUME);
    m.bus.w32(g + epg::EAGLEPLAYER_DIR_ARRAY_PTR, g + epg::SIZE);
    g
}

/// The name EPG_EagleplayerDirArrayPtr gives the Eagleplayers' directory (`OsState::eagle_dir`).
pub const EAGLE_DIR_VOLUME: &str = "Eagleplayers:";

/// Per-module EPG_ fields, set before the player is initialised.
pub fn prepare(m: &mut Machine, tag_list: u32, size: u32) {
    let g = m.deli.eagle.globals;
    m.bus.w32(g + epg::PLAYER_TAG_LIST, tag_list);
    m.bus.w32(g + epg::MODULE_SIZE, size);
    m.bus.w32(g + epg::SONG_NAME, m.deli.file_buf);
    m.bus.w16(g + epg::FIRST_SND, 0xFFFF);
    m.bus.w16(g + epg::SUB_SONGS, 0xFFFF);
    m.bus.w32(g + epg::AMPLIFIER_TAG_LIST, 0);
}

/// Takes what the player asked for in EP_InitAmplifier. With EPAMB_Direct and
/// at most 4 voices the pokes go to Paula; otherwise the amplifier mixes the
/// voices, poked or left in the AudioStructs.
pub fn amplifier_ready(m: &mut Machine, player: &str) {
    let tl = m.bus.r32(m.deli.eagle.globals + epg::AMPLIFIER_TAG_LIST);
    if tl == 0 {
        log::info!("{player}: InitAmplifier left no amplifier tag list");
        return;
    }
    let get = |t| find_tag(&m.bus, tl, t);
    let (n, structs, flags) =
        (get(epamt::NUM_STRUCTS).unwrap_or(0), get(epamt::AUDIO_STRUCTS).unwrap_or(0), get(epamt::FLAGS).unwrap_or(0));
    log::info!("{player}: amplifier with {n} AudioStructs at ${structs:06x}, flags ${flags:x}");
    let direct = flags & epamb::DIRECT != 0;
    if direct && n <= 4 {
        // (IFF 8SVX, V0.84: flags $83, voice 0's struct holds the Interrupt that has its own
        // process refill the double buffer; without it the buffers stay silent)
        if flags & (epamb::AUDIO_INTS | 0x80) != 0 && structs != 0 && (structs as usize) + (n as usize) * 100 <= crate::bus::RAM_SIZE {
            m.deli.eagle.direct_ints = Some((structs, n));
        }
        return;
    }
    let structs_ok = structs != 0 && structs as usize + n as usize * 100 <= crate::bus::RAM_SIZE;
    if n == 0 || n as usize > crate::amplifier::MAX_VOICES {
        log::warn!("{player}: {n} amplifier voices, not mixed");
    } else if !direct && (!structs_ok || flags & (epamb::BIT8 | epamb::BIT16) == 0) {
        log::warn!("{player}: AudioStructs ${structs:06x} with flags ${flags:x}, not mixed");
    } else {
        crate::amplifier::start(m, n as usize, if structs_ok { structs } else { 0 }, flags);
    }
}

/// Frees what the host allocated for the current module.
pub fn stop(m: &mut Machine) {
    m.deli.eagle.direct_ints = None;
    for a in std::mem::take(&mut m.deli.eagle.allocs) {
        m.heap.free(a);
    }
}

/// utility.library FindTagItem() on a guest tag list: the data of `t`.
pub fn find_tag(bus: &crate::bus::Bus, list: u32, t: u32) -> Option<u32> {
    let mut p = list;
    for _ in 0..1000 {
        if p == 0 {
            return None;
        }
        let (tag, data) = (bus.r32(p), bus.r32(p + 4));
        match tag {
            0 => return None,                // TAG_DONE
            1 => p += 8,                     // TAG_IGNORE
            2 => p = data,                   // TAG_MORE
            3 => p += 8 * (data + 1),        // TAG_SKIP
            _ if tag == t => return Some(data),
            _ => p += 8,
        }
    }
    None
}

/// ENPP_LoadExecutable: LoadSegs the file named in EPG_ARG1 (PowerPacker
/// crunched or not) and returns its seglist in EPG_ARG1, D0 = 0 if it worked.
/// Ron Klaren modules are executables.
fn load_executable(m: &mut Machine) {
    let g = m.deli.eagle.globals;
    let name = m.bus.read_cstr(m.bus.r32(g + epg::ARG1));
    let (p, data) = deli::read_companion(m, &name);
    let Ok(data) = data else {
        log::info!("LoadExecutable: {} not found", p.display());
        m.set_d0(1);
        return;
    };
    match deli::load_seg(m, &crate::decrunch::decrunch(data)) {
        Ok((seg, _)) => {
            let mut s = seg;
            while s != 0 {
                m.deli.eagle.allocs.push((s << 2) - 4);
                s = m.bus.r32(s << 2);
            }
            m.bus.w32(g + epg::ARG1, seg);
            log::debug!("LoadExecutable {}", p.display());
            m.set_d0(0);
        }
        Err(e) => {
            log::info!("LoadExecutable {}: {e}", p.display());
            m.set_d0(1);
        }
    }
}

/// ENPP_ModuleChange in search mode: EPG_ARG1 = start and EPG_ARG2 = length of
/// the code to patch, EPG_ARG3 = a patch table of (code offset, word count - 1,
/// patch offset) word triples (offsets from the table) ended by a zero word.
/// Each place in the code where a table entry's code is found is replaced by a
/// `JSR` to the entry's patch (which runs the replaced code and returns),
/// padded with NOPs.
///
/// Wildcards: when an entry's code holds $FFFF words and the table leaves one
/// long per such word between the code and its patch, each $FFFF word matches
/// any word, and the long after the code for the k-th of them receives the
/// address that word resolves to as a 68000 PC-relative displacement (the
/// address of the word plus the word, sign-extended). The Eagleplayer 2.06
/// Hippel player's table is written that way: its patches load these longs
/// (`MOVEA.L slot(PC),An` for a `LEA d16(PC)` or `BSR.W` target, or the long
/// minus the `JSR`'s return address for the displacement of a `MOVE.L Dn,d16(A0)`).
/// An entry without that room matches its $FFFF words as they are.
fn module_change(m: &mut Machine) {
    let g = m.deli.eagle.globals;
    let (start, len, table) = (m.bus.r32(g + epg::ARG1), m.bus.r32(g + epg::ARG2), m.bus.r32(g + epg::ARG3));
    let (mode, arg5) = (m.bus.r32(g + epg::ARG4), m.bus.r32(g + epg::ARG5));
    log::debug!("ENPP_ModuleChange: {start:#x} {len} table {table:#x} mode {mode} arg5 {arg5:#x} n {}", m.bus.r32(g + epg::ARGN));
    if mode != 1 || arg5 != 0xFFFF_FFFE {
        // only the search mode of the Wanted Team players is known; the others
        // leave the code unpatched
        log::warn!("ENPP_ModuleChange: mode {mode} arg5 {arg5:#x} not handled, code left unpatched");
        m.set_d0(0);
        return;
    }
    let mut hits = 0;
    let mut t = table;
    while m.bus.r16(t) != 0 {
        let (code, words, patch) = (table + m.bus.r16(t) as u32, m.bus.r16(t + 2) as u32 + 1, table + m.bus.r16(t + 4) as u32);
        t += 6;
        let n = words * 2;
        if n < 6 || len < n {
            continue;
        }
        let wild: Vec<u32> = (0..words).filter(|&i| m.bus.r16(code + 2 * i) == 0xFFFF).collect();
        let wild = if !wild.is_empty() && patch.wrapping_sub(code + n) == 4 * wild.len() as u32 { wild } else { vec![] };
        let mut a = start;
        while a + n <= start + len {
            if (0..words).all(|i| wild.contains(&i) || m.bus.r16(a + 2 * i) == m.bus.r16(code + 2 * i)) {
                for (k, &i) in wild.iter().enumerate() {
                    let w = a + 2 * i;
                    m.bus.w32(code + n + 4 * k as u32, w.wrapping_add(m.bus.r16(w) as i16 as i32 as u32));
                }
                m.bus.w16(a, 0x4EB9);
                m.bus.w32(a + 2, patch);
                for i in 3..words {
                    m.bus.w16(a + 2 * i, 0x4E71);
                }
                log::debug!("ENPP_ModuleChange: code +{:#x} patched at {:#x} (+{:#x}) by table +{:#x}", code - table, a, a - start, patch - table);
                hits += 1;
                a += n;
            } else {
                a += 2;
            }
        }
    }
    log::debug!("ENPP_ModuleChange: {hits} places patched");
    m.set_d0(0);
}

fn custom_w16(m: &mut Machine, reg: u32, v: u16) {
    m.bus.write_word(0xDFF000 + reg, v);
}

pub fn host_call(m: &mut Machine, f: u16) {
    let name = ENPP_NAMES.get(f as usize).copied().unwrap_or("?");
    log::trace!("ENPP_{name}");
    match f {
        // the DeliTracker functions, in the same order
        ENPP_GET_LIST_DATA..=ENPP_WAIT_AUDIO_DMA => deli::host_call(m, f - ENPP_GET_LIST_DATA),
        ENPP_NEW_LOAD_FILE => deli::host_call(m, 1),
        ENPP_LOAD_EXECUTABLE => load_executable(m),
        ENPP_MODULE_CHANGE => module_change(m),
        ENPP_ALLOC_SAMPLE_STRUCT => {
            let a = m.alloc(SAMPLE_STRUCT_SIZE);
            if a != 0 {
                m.deli.eagle.allocs.push(a);
            }
            m.set_d0(a);
        }
        ENPP_DMA_MASK if m.bus.amp.is_some() => {
            let (mask, on) = (m.d(1), m.d(0) & 0x8000 != 0);
            for v in (0..32).filter(|v| mask & 1 << v != 0) {
                m.bus.amp_poke(v, crate::amplifier::Poke::Dma(on));
            }
            if on {
                deli::wait_audio_dma(m);
            }
        }
        ENPP_DMA_MASK => {
            // d1 = channel mask; d0.w negative switches them on
            let mut v = m.d(1) as u16 & 0xF;
            if m.d(0) & 0x8000 != 0 {
                v |= 0x8000;
            }
            custom_w16(m, 0x096, v);
            if v & 0x8000 != 0 {
                // UADE's score waits for the DMA here, as dtg_WaitAudioDMA does
                deli::wait_audio_dma(m);
            }
        }
        ENPP_POKE_ADR..=ENPP_POKE_VOL => {
            // d0 = value, d1.w = channel
            let ch = m.d(1) as u16;
            let v = m.d(0);
            if m.bus.amp.is_some() {
                use crate::amplifier::Poke;
                let p = [Poke::Loc(v), Poke::Len(v as u16), Poke::Per(v as u16), Poke::Vol(v as u16)];
                m.bus.amp_poke(ch as usize, p[(f - ENPP_POKE_ADR) as usize]);
                return;
            }
            if ch >= 4 {
                log::debug!("ENPP_{name}: channel {ch}");
                return;
            }
            let reg = 0x0A0 + 16 * ch as u32;
            match f - ENPP_POKE_ADR {
                0 => {
                    custom_w16(m, reg, (v >> 16) as u16);
                    custom_w16(m, reg + 2, v as u16);
                }
                1 => custom_w16(m, reg + 4, v as u16),
                2 => custom_w16(m, reg + 6, v as u16),
                _ => custom_w16(m, reg + 8, v as u16),
            }
        }
        ENPP_POKE_COMMAND => {
            // command 1: the LED filter (d1 = 0 off, 1 on, -1 toggle)
            match (m.d(0), m.d(1) as i32) {
                (1, 0) => m.bus.ciaa.pra |= 2,
                (1, 1) => m.bus.ciaa.pra &= !2,
                (1, -1) => m.bus.ciaa.pra ^= 2,
                (c, a) => log::debug!("ENPP_PokeCommand {c} {a}"),
            }
        }
        // the pokes take effect at their time, so there is nothing to flush
        ENPP_AMPLIFIER => {}
        ENPP_GET_HARDWARE_TYPE => m.set_d0(0), // EPHT_Amiga
        _ => {
            if m.deli.eagle.logged & (1u128 << f) == 0 {
                m.deli.eagle.logged |= 1u128 << f;
                log::warn!("ENPP_{name} not implemented");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Bus;

    /// EaglePlayerGlobals as declared in EaglePlayer.i, from dtg_NotePlayer on
    /// (FPTR/APTR/LONG = 4, WORD = 2; the STRUCTURE macros do not align).
    const LAYOUT: &[(&str, u32)] = &[
        ("FTPRReserved1", 4), ("FTPRReserved2", 4), ("FTPRReserved3", 4), ("SaveMem", 4),
        ("FileRequest", 4), ("TextRequest", 4), ("LoadExecutable", 4), ("NewLoadFile", 4),
        ("ScrollText", 4), ("LoadPlConfig", 4), ("SavePlConfig", 4), ("FindTag", 4), ("FindAuthor", 4),
        ("Hexdez", 4), ("TypeText", 4), ("ModuleChange", 4), ("ModuleRestore", 4), ("FTPRReserved8", 4),
        ("XPKBase", 4), ("LHBase", 4), ("PPBase", 4), ("DiskFontBase", 4), ("ReqToolsBase", 4),
        ("ReqBase", 4), ("XFDMasterBase", 4), ("WorkBenchBase", 4), ("RexxSysBase", 4),
        ("CommoditiesBase", 4), ("IconBase", 4), ("LocaleBase", 4), ("WinHandle", 4), ("TitlePuffer", 4),
        ("SoundSystemname", 4), ("Songname", 4), ("Reserved2", 4), ("Reserved3", 4), ("PubScreen", 4),
        ("CiaBBase", 4), ("UPS_Structure", 4), ("ModuleInfoTagList", 4), ("Author", 4),
        ("Identifier", 4), ("EagleVersion", 4), ("Speed", 2), ("ARGN", 4), ("ARG1", 4), ("ARG2", 4),
        ("ARG3", 4), ("ARG4", 4), ("ARG5", 4), ("ARG6", 4), ("ARG7", 4), ("ARG8", 4), ("Voices", 2),
        ("Voice1Vol", 2), ("Voice2Vol", 2), ("Voice3Vol", 2), ("Voice4Vol", 2), ("VoiceTable", 4),
        ("VoiceTableEntries", 2), ("Unused1", 2), ("SomePrefs", 4), ("Timeout", 4), ("FirstSnd", 2),
        ("SubSongs", 2), ("MODNr", 4), ("MODS", 4), ("PlayerTagList", 4), ("TextFont", 4), ("Volume", 2),
        ("Balance", 2), ("LeftBalance", 2), ("RightBalance", 2), ("UnUsed6", 2), ("UnUsed7", 2),
        ("UnUsed8", 2), ("UnUsed9", 2), ("DefTimer", 2), ("CurrentPosition", 2), ("WORDReserved3", 2),
        ("WORDReserved4", 2), ("WORDReserved5", 2), ("WORDReserved6", 2), ("WORDReserved7", 2),
        ("WORDReserved8", 2), ("Dirs", 4), ("LoadedFiles", 4), ("AppPort", 4), ("SampleInfoStructure", 4),
        ("MinTimeOut", 4), ("CurrentTime", 4), ("Duration", 4), ("FirstUserStruct", 4),
        ("FirstFileStruct", 4), ("Entries", 4), ("Modulesize", 4), ("Playerlist", 4), ("Enginelist", 4),
        ("Moduleslist", 4), ("AmplifierList", 4), ("ActiveAmplifier", 4), ("AudioStruct", 4),
        ("AmplifierTagList", 4), ("ConfigDirArrayPtr", 4), ("PlayerDirArrayPtr", 4),
        ("EngineDirArrayPtr", 4), ("FirstPlayerStruct", 4), ("ChkSegment", 4),
        ("EagleplayerDirArrayPtr", 4),
    ];

    fn offset(name: &str) -> u32 {
        let mut o = dtg::NOTE_PLAYER;
        for &(n, s) in LAYOUT {
            if n == name {
                return o;
            }
            o += s;
        }
        assert_eq!(name, "SizeOf");
        o
    }

    #[test]
    fn globals_layout() {
        assert_eq!(dtg::NOTE_PLAYER, 0x74);
        assert_eq!(offset("FTPRReserved1"), dtg::NOTE_PLAYER);
        assert_eq!(offset("SaveMem"), epg::SAVE_MEM);
        assert_eq!(offset("ModuleRestore"), epg::SAVE_MEM + 13 * 4);
        assert_eq!(offset("Songname"), epg::SONG_NAME);
        assert_eq!(offset("CiaBBase"), epg::CIAB_BASE);
        assert_eq!(offset("Identifier"), epg::IDENTIFIER);
        assert_eq!(offset("EagleVersion"), epg::EAGLE_VERSION);
        assert_eq!(offset("ARGN"), epg::ARGN);
        assert_eq!(offset("ARG1"), epg::ARG1);
        assert_eq!(offset("Voices"), epg::VOICES);
        assert_eq!(offset("Voice1Vol"), epg::VOICE1_VOL);
        assert_eq!(offset("FirstSnd"), epg::FIRST_SND);
        assert_eq!(offset("SubSongs"), epg::SUB_SONGS);
        assert_eq!(offset("PlayerTagList"), epg::PLAYER_TAG_LIST);
        assert_eq!(offset("Modulesize"), epg::MODULE_SIZE);
        assert_eq!(offset("AmplifierTagList"), epg::AMPLIFIER_TAG_LIST);
        assert_eq!(offset("EagleplayerDirArrayPtr"), epg::EAGLEPLAYER_DIR_ARRAY_PTR);
        assert!(EAGLE_DIR_VOLUME.len() < 0x10);
        assert_eq!(offset("SizeOf"), epg::SIZE);
    }

    #[test]
    fn jump_table_offsets() {
        let at = |n: &str| enpp_offset(ENPP_NAMES.iter().position(|&x| x == n).unwrap() as u16);
        // the offsets JamCracker and FutureComposer1.4 call
        assert_eq!(at("DMAMask"), -0xF6);
        assert_eq!(at("PokeAdr"), -0xFC);
        assert_eq!(at("PokeLen"), -0x102);
        assert_eq!(at("PokePer"), -0x108);
        assert_eq!(at("PokeVol"), -0x10E);
        assert_eq!(at("Amplifier"), -0x11A);
        assert_eq!(at("GetListData"), -66);
        assert_eq!(at("SetListData"), -384);
        assert_eq!(at("GetHardwareType"), -390);
        assert!(JUMP_TABLE >= 6 * ENPP_COUNT as u32 && JUMP_TABLE % 4 == 0);
    }

    #[test]
    fn tag_values() {
        assert_eq!(tag::INIT_AMPLIFIER, 0x8000_4573);
        assert_eq!(tag::CHECK5, 0x8000_456C);
        assert_eq!(tag::EAGLE_BASE, 0x8000_457A);
        assert_eq!(epamt::FLAGS, 0x8000_4150);
        let mut t = HashMap::new();
        t.insert(deli::tag::CHECK2, 0);
        assert!(!has_ep_tags(&t));
        t.insert(tag::INIT_AMPLIFIER, 0);
        assert!(has_ep_tags(&t));
    }

    #[test]
    fn find_tag_follows_control_tags() {
        let mut b = Bus::new();
        let w = |b: &mut Bus, a: u32, v: &[u32]| {
            for (i, &x) in v.iter().enumerate() {
                b.w32(a + 4 * i as u32, x);
            }
        };
        // IGNORE, SKIP 1 (hides SUB_SONGS=9), MORE -> second list
        w(&mut b, 0x1000, &[1, 0, 3, 1, mi::SUB_SONGS, 9, 2, 0x2000]);
        w(&mut b, 0x2000, &[mi::SONG_NAME, 0x1234, mi::SUB_SONGS, 5, 0, 0]);
        assert_eq!(find_tag(&b, 0x1000, mi::SONG_NAME), Some(0x1234));
        assert_eq!(find_tag(&b, 0x1000, mi::SUB_SONGS), Some(5));
        assert_eq!(find_tag(&b, 0x1000, mi::AUTHOR_NAME), None);
    }
}
