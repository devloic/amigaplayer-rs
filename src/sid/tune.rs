// Port of NostalgicPlayer's C# port of libsidplayfp (by Simon White, Antti Lankila and
// Leandro Nini): Source/Ports/LibSidPlayFp/SidTune/PSid.cs, SidTuneBase.cs,
// SidTuneInfoImpl.cs and SidPlayFp/SidTuneInfo.cs, git e183943.
// Licence: GNU General Public License version 2 (LICENSE-LibSidPlayFp); the upstream
// files carry only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more information."
//
// Structure changed for Rust: the loader class hierarchy (SidTuneBase, PSid) and the
// info object are one struct; errors are a `String` instead of exceptions. Only the
// PSID/RSID container is read: the MUS (Compute!'s Sidplayer) and bare PRG loaders are
// not ported, nor the MD5 (song length database) and PETSCII comment code. The header's
// data offset is checked against the file here (upstream reads past it).

use super::c64::C64;

/// Different clock types
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Clock {
    Unknown,
    Pal,
    Ntsc,
    Any,
}

/// Different SID models
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SidModel {
    Unknown,
    Mos6581,
    Mos8580,
    Any,
}

/// Compatibility modes
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Compatibility {
    /// File is C64 compatible
    C64,
    /// File is PSID specific
    Psid,
    /// File is Real C64 only
    R64,
    /// File requires C64 Basic
    Basic,
}

/// Vertical-Blanking-Interrupt
pub const SPEED_VBI: u8 = 0;
/// CIA 1 Timer A
pub const SPEED_CIA_1A: u8 = 60;

/// Also PSID file format limit
const MAX_SONGS: u32 = 256;
/// The Commodore 64 memory size
const MAX_MEMORY: u32 = 65536;
/// Minimum load address for real C64 only tunes
const SIDTUNE_R64_MIN_LOAD_ADDR: u16 = 0x07e8;

const PSID_MAX_STR_LEN: usize = 32;

const PSID_MUS: u16 = 1 << 0;
const PSID_SPECIFIC: u16 = 1 << 1; // These two are mutually exclusive
const PSID_BASIC: u16 = 1 << 1;
const PSID_CLOCK: u16 = 3 << 2;

const PSID_CLOCK_PAL: u16 = 1 << 2;
const PSID_CLOCK_NTSC: u16 = 1 << 3;
const PSID_CLOCK_ANY: u16 = PSID_CLOCK_PAL | PSID_CLOCK_NTSC;

const PSID_SIDMODEL_6581: u16 = 1;
const PSID_SIDMODEL_8580: u16 = 2;
const PSID_SIDMODEL_ANY: u16 = PSID_SIDMODEL_6581 | PSID_SIDMODEL_8580;

const PSID_HEADER_SIZE: usize = 118;
const PSID_V2_HEADER_SIZE: usize = PSID_HEADER_SIZE + 6;

// Magic fields
const PSID_ID: u32 = 0x50534944;
const RSID_ID: u32 = 0x52534944;

/// The file starts as a PSID or RSID file does.
pub fn detect(data: &[u8]) -> bool {
    data.len() >= 4 && matches!(&data[..4], b"PSID" | b"RSID")
}

/// A loaded PSID/RSID tune and what its header says.
pub struct Tune {
    /// "PSID" or "RSID"
    pub format: &'static str,
    /// header version, 1 to 4
    pub version: u16,
    pub load_addr: u16,
    pub init_addr: u16,
    pub play_addr: u16,
    /// number of songs, 1..=256
    pub songs: u32,
    /// the default song, 1..=songs
    pub start_song: u32,
    /// the selected song, 1..=songs
    pub current_song: u32,
    pub compatibility: Compatibility,
    /// the selected song's speed: `SPEED_VBI` or `SPEED_CIA_1A`
    pub song_speed: u8,
    /// the selected song's clock
    pub clock_speed: Clock,
    pub reloc_start_page: u8,
    pub reloc_pages: u8,
    /// [0] is the base SID's ($d400), then the extra ones
    pub sid_chip_addresses: Vec<u16>,
    pub sid_models: Vec<SidModel>,
    pub name: String,
    pub author: String,
    pub released: String,
    /// length of the C64 data
    pub c64_data_len: u32,

    song_speeds: Vec<u8>,
    clock_speeds: Vec<Clock>,
    /// For files with header: offset to real data
    file_offset: usize,
    cache: Vec<u8>,
}

fn be16(d: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([d[o], d[o + 1]])
}

fn be32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// An info string: up to 32 characters of ISO 8859-1 ("may be 32 characters long
/// without trailing zero").
fn info_string(d: &[u8]) -> String {
    let end = d.iter().position(|&b| b == 0).unwrap_or(d.len());
    d[..end].iter().map(|&b| if b < 0x20 || (0x7f..0xa0).contains(&b) { ' ' } else { b as char }).collect::<String>().trim().to_string()
}

/// Decode SID model flags
fn get_sid_model(model_flag: u16) -> SidModel {
    if model_flag & PSID_SIDMODEL_ANY == PSID_SIDMODEL_ANY {
        return SidModel::Any;
    }
    if model_flag & PSID_SIDMODEL_6581 != 0 {
        return SidModel::Mos6581;
    }
    if model_flag & PSID_SIDMODEL_8580 != 0 {
        return SidModel::Mos8580;
    }
    SidModel::Unknown
}

/// Check if extra SID address is valid for PSID specs.
fn validate_address(address: u8) -> bool {
    // Only even values are valid
    if address & 1 != 0 {
        return false;
    }

    // Ranges $00-$41 ($D000-$D410) and $80-$DF ($D800-$DDF0) are invalid.
    // Any invalid value means that no second SID is used, like $00
    if address <= 0x41 || (0x80..=0xdf).contains(&address) {
        return false;
    }

    true
}

impl Tune {
    /// Loads a PSID or RSID file (PSid.Load + SidTuneBase.AcceptSidTune). The header has
    /// been extended for 'RSID' format; the following changes are present:
    ///     Id = 'RSID'
    ///     Version = 2, 3 or 4
    ///     Play, Load and Speed reserved 0
    ///     PSidSpecific flag is called C64BASIC flag
    ///     Init cannot be under ROMS/IO memory area
    ///     Load address cannot be less than 0x07E8
    ///     Info strings may be 32 characters long without trailing zero
    pub fn load(data: &[u8]) -> Result<Tune, String> {
        // File format check
        if data.len() < 4 {
            return Err("not a SID file".into());
        }
        let id = be32(data, 0);
        if id != PSID_ID && id != RSID_ID {
            return Err("not a SID file".into());
        }

        // ---- ReadHeader ----
        // Due to security concerns, input must be at least as long as version 1
        // header plus 16-bit C64 load address. That is the area which will be
        // accessed
        if data.len() < PSID_HEADER_SIZE + 2 {
            return Err("SIDTUNE ERROR: File is most likely truncated".into());
        }
        // Read v1 fields
        let version = be16(data, 4);
        let data_offset = be16(data, 6);
        let load = be16(data, 8);
        let init = be16(data, 10);
        let play = be16(data, 12);
        let songs = be16(data, 14);
        let start = be16(data, 16);
        let mut speed = be32(data, 18);
        let (mut flags, mut h_reloc_start_page, mut h_reloc_pages, mut sid_chip_base2, mut sid_chip_base3) = (0u16, 0u8, 0u8, 0u8, 0u8);
        if version >= 2 {
            if data.len() < PSID_V2_HEADER_SIZE + 2 {
                return Err("SIDTUNE ERROR: File is most likely truncated".into());
            }
            // Read v2/3/4 fields
            flags = be16(data, 118);
            h_reloc_start_page = data[120];
            h_reloc_pages = data[121];
            sid_chip_base2 = data[122];
            sid_chip_base3 = data[123];
        }

        // ---- TryLoad ----
        let mut compatibility = Compatibility::C64;
        let format;

        // Require a valid ID and version number
        if id == PSID_ID {
            match version {
                1 => compatibility = Compatibility::Psid,
                2..=4 => {}
                _ => return Err("Unsupported PSID version".into()),
            }
            format = "PSID";
        } else {
            match version {
                2..=4 => {}
                _ => return Err("Unsupported RSID version".into()),
            }
            format = "RSID";
            compatibility = Compatibility::R64;
        }

        let mut t = Tune {
            format,
            version,
            load_addr: load,
            init_addr: init,
            play_addr: play,
            songs: songs as u32,
            start_song: start as u32,
            current_song: 0,
            compatibility,
            song_speed: SPEED_VBI,
            clock_speed: Clock::Unknown,
            reloc_start_page: 0,
            reloc_pages: 0,
            sid_chip_addresses: vec![0xd400],
            sid_models: vec![SidModel::Unknown],
            name: info_string(&data[22..22 + PSID_MAX_STR_LEN]),
            author: info_string(&data[54..54 + PSID_MAX_STR_LEN]),
            released: info_string(&data[86..86 + PSID_MAX_STR_LEN]),
            c64_data_len: 0,
            song_speeds: vec![SPEED_VBI; MAX_SONGS as usize],
            clock_speeds: vec![Clock::Unknown; MAX_SONGS as usize],
            file_offset: data_offset as usize,
            cache: Vec::new(),
        };

        let mut clock = Clock::Unknown;
        let mut mus_player = false;

        if version >= 2 {
            // Check clock
            if flags & PSID_MUS != 0 {
                // MUS tunes run at any speed
                clock = Clock::Any;
                mus_player = true;
            } else {
                match flags & PSID_CLOCK {
                    PSID_CLOCK_ANY => clock = Clock::Any,
                    PSID_CLOCK_PAL => clock = Clock::Pal,
                    PSID_CLOCK_NTSC => clock = Clock::Ntsc,
                    _ => {}
                }
            }

            // These flags is only available for the appropriate
            // file formats
            match compatibility {
                Compatibility::C64 => {
                    if flags & PSID_SPECIFIC != 0 {
                        t.compatibility = Compatibility::Psid;
                    }
                }
                Compatibility::R64 => {
                    if flags & PSID_BASIC != 0 {
                        t.compatibility = Compatibility::Basic;
                    }
                }
                _ => {}
            }

            t.clock_speed = clock;

            t.sid_models[0] = get_sid_model(flags >> 4);

            t.reloc_start_page = h_reloc_start_page;
            t.reloc_pages = h_reloc_pages;

            if version >= 3 {
                if validate_address(sid_chip_base2) {
                    t.sid_chip_addresses.push(0xd000 | (sid_chip_base2 as u16) << 4);
                    t.sid_models.push(get_sid_model(flags >> 6));
                }

                if version >= 4 && sid_chip_base3 != sid_chip_base2 && validate_address(sid_chip_base3) {
                    t.sid_chip_addresses.push(0xd000 | (sid_chip_base3 as u16) << 4);
                    t.sid_models.push(get_sid_model(flags >> 8));
                }
            }
        }

        // Check reserved fields to force real C64 compliance
        // as required by the RSID specification
        if compatibility == Compatibility::R64 {
            if t.load_addr != 0 || t.play_addr != 0 || speed != 0 {
                return Err("SIDTUNE ERROR: File contains invalid data".into());
            }

            // Real C64 tunes appear as CIA
            speed = !0;
        }

        // Create the speed/clock setting table
        t.convert_old_style_speed_to_tables(speed, clock);

        if mus_player {
            return Err("Compute!'s Sidplayer MUS data is not supported".into());
        }

        t.accept_sid_tune(data)?;
        t.select_song(0);
        Ok(t)
    }

    /// Cache the data of a single-file sid tune
    fn accept_sid_tune(&mut self, buf: &[u8]) -> Result<(), String> {
        // Fix bad sid tune setup
        if self.songs > MAX_SONGS {
            self.songs = MAX_SONGS;
        } else if self.songs == 0 {
            self.songs = 1;
        }

        if self.start_song == 0 || self.start_song > self.songs {
            self.start_song = 1;
        }

        // (not upstream's: there the offset is taken as it is)
        if self.file_offset > buf.len() {
            return Err("SIDTUNE ERROR: File is most likely truncated".into());
        }
        self.c64_data_len = (buf.len() - self.file_offset) as u32;

        // Calculate any remaining addresses and then
        // confirm all the file details are correct
        self.resolve_addrs(&buf[self.file_offset..])?;

        if !self.check_reloc_info() {
            return Err("SIDTUNE ERROR: Bad reloc data".into());
        }

        if !self.check_compatibility() {
            return Err("SIDTUNE ERROR: Bad address data".into());
        }

        // Check the size of the data
        if self.c64_data_len > MAX_MEMORY {
            return Err("SIDTUNE ERROR: Size of music data exceeds C64 memory".into());
        }

        if self.c64_data_len == 0 {
            return Err("SIDTUNE ERROR: No data to load".into());
        }

        self.cache = buf.to_vec();
        Ok(())
    }

    /// Convert 32-bit PSID-style speed word to internal tables
    fn convert_old_style_speed_to_tables(&mut self, mut speed: u32, clock: Clock) {
        // Create the speed/clock setting tables
        //
        // This routine implements the PSIDv2NG compliant speed conversion. All tunes
        // above 32 use the same song speed as tune 32
        let to_do = self.songs.min(MAX_SONGS);

        for s in 0..to_do as usize {
            self.clock_speeds[s] = clock;
            self.song_speeds[s] = if speed & 1 != 0 { SPEED_CIA_1A } else { SPEED_VBI };

            if s < 31 {
                speed >>= 1;
            }
        }
    }

    /// Select sub-song (0 = default starting song) and return active
    /// song number out of [1,2,..,SIDTUNE_MAX_SONGS]
    pub fn select_song(&mut self, song_num: u32) -> u32 {
        // Check whether selected song is valid, use start song if not
        let song = if song_num == 0 || song_num > self.songs { self.start_song } else { song_num };
        let song = song.clamp(1, MAX_SONGS);

        // Copy any song-specific variable information
        // such a speed/clock setting to the info structure
        self.current_song = song;

        // Retrieve song speed definition
        self.song_speed = match self.compatibility {
            Compatibility::R64 => SPEED_CIA_1A,
            // This does not take into account the PlaySID bug upon evalutating the
            // SPEED field. It would most likely break compatibility to lots of
            // sidtunes, which have been converted from .SID format and vice versa.
            // The .SID format does the bit-wise/song-wise evaluation of the SPEED
            // value correctly, like it is described in the PlaySID documentation
            Compatibility::Psid => self.song_speeds[(song as usize - 1) & 31],
            _ => self.song_speeds[song as usize - 1],
        };

        self.clock_speed = self.clock_speeds[song as usize - 1];

        self.current_song
    }

    /// Copy sid tune into C64 memory (64 KB)
    pub fn place_sid_tune_in_c64_mem(&self, mem: &mut C64) {
        // The Basic ROM sets these values on loading a file
        // Program end address
        let start = self.load_addr;
        let end = start.wrapping_add(self.c64_data_len as u16);

        mem.write_mem_word(0x2d, end); // Variables start
        mem.write_mem_word(0x2f, end); // Arrays start
        mem.write_mem_word(0x31, end); // Strings start
        mem.write_mem_word(0xac, start);
        mem.write_mem_word(0xae, end);

        // Copy data from cache to the correct destination
        let from = self.file_offset.min(self.cache.len());
        let to = (from + self.c64_data_len as usize).min(self.cache.len());
        mem.fill_ram(self.load_addr, &self.cache[from..to]);
    }

    /// The address of SID chip `i` (0 = the base SID), 0 when the tune has none there.
    pub fn sid_chip_base(&self, i: usize) -> u16 {
        self.sid_chip_addresses.get(i).copied().unwrap_or(0)
    }

    /// The model the tune wants for SID chip `i`.
    pub fn sid_model(&self, i: usize) -> SidModel {
        self.sid_models.get(i).copied().unwrap_or(SidModel::Unknown)
    }

    /// Check for valid relocation information
    fn check_reloc_info(&mut self) -> bool {
        // Fix relocation information
        if self.reloc_start_page == 0xff {
            self.reloc_pages = 0;
            return true;
        }

        if self.reloc_pages == 0 {
            self.reloc_start_page = 0;
            return true;
        }

        // Calculate start/end page
        let start_p = self.reloc_start_page;
        let end_p = start_p.wrapping_add(self.reloc_pages).wrapping_sub(1);
        if end_p < start_p {
            return false;
        }

        {
            // Check against load range
            let start_lp = (self.load_addr >> 8) as u8;
            let end_lp = start_lp.wrapping_add((self.c64_data_len.wrapping_sub(1) >> 8) as u8);

            if (start_p <= start_lp && end_p >= start_lp) || (start_p <= end_lp && end_p >= end_lp) {
                return false;
            }
        }

        // Check that the relocation information does not use the following
        // memory areas: 0x0000-0x03FF, 0xA000-0xBFFF and 0xD000-0xFFFF
        if start_p < 0x04 || (0xa0..=0xbf).contains(&start_p) || start_p >= 0xd0 || (0xa0..=0xbf).contains(&end_p) || end_p >= 0xd0 {
            return false;
        }

        true
    }

    /// Common address resolution procedure
    fn resolve_addrs(&mut self, c64_data: &[u8]) -> Result<(), String> {
        // Originally used as a first attempt at an RSID
        // style format. Now reserved for future use
        if self.play_addr == 0xffff {
            self.play_addr = 0;
        }

        // loadAddr = 0 means, the address is stored in front of the C64 data
        if self.load_addr == 0 {
            if self.c64_data_len < 2 || c64_data.len() < 2 {
                return Err("SIDTUNE ERROR: File is incomplete or corrupt".into());
            }

            self.load_addr = u16::from_le_bytes([c64_data[0], c64_data[1]]);
            self.file_offset += 2;
            self.c64_data_len -= 2;
        }

        if self.compatibility == Compatibility::Basic {
            if self.init_addr != 0 {
                return Err("SIDTUNE ERROR: Bad address data".into());
            }
        } else if self.init_addr == 0 {
            self.init_addr = self.load_addr;
        }
        Ok(())
    }

    /// Check if compatibility constraints are fulfilled
    fn check_compatibility(&self) -> bool {
        if self.compatibility == Compatibility::R64 {
            // Check valid init address
            match self.init_addr >> 12 {
                0x0a | 0x0b | 0x0d | 0x0e | 0x0f => return false,
                _ => {
                    if self.init_addr < self.load_addr
                        || self.init_addr as u32 > (self.load_addr as u32 + self.c64_data_len).wrapping_sub(1)
                    {
                        return false;
                    }
                }
            }

            // Check tune is loadable on a real C64
            if self.load_addr < SIDTUNE_R64_MIN_LOAD_ADDR {
                return false;
            }
        }

        true
    }
}
