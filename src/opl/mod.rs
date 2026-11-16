//! OPL (AdLib FM) synthesis: the Opal OPL3 emulator (`opal`, public domain) and
//! the driver that turns a tracker channel's note, volume, frequency and pan into
//! OPL register writes, ported from libopenmpt 0.8.7's `soundlib/OPL.cpp` /
//! `OPL.h` (BSD-3-Clause, OpenMPT Devs; Schism Tracker contributors bisqwit,
//! JosepMa, Malvineous, code relicensed from GPL to BSD with permission; see
//! LICENSE-openmpt).
//!
//! As in OpenMPT: OPL3 mode (0x105 = 1), no 4-op voices (0x104 = 0), 18 voices
//! allocated to tracker channels on demand (a channel keeps its voice, a cut
//! channel's voice is reused first, then a released one), the tracker volume
//! 0..63 scaling the carrier's total level (both operators' in additive mode),
//! the pan reduced to hard left, centre or hard right. The register logger of
//! OpenMPT (for its VGM export) is left out.

pub mod opal;

use opal::Opal;

/// An AdLib instrument: the 12 bytes of an S3M `SCRI` instrument at offset 16
/// (modulator/carrier characteristics, scale/level, attack/decay,
/// sustain/release, waveforms, feedback/connection, unused), OpenMPT's `OPLPatch`.
pub type OplPatch = [u8; 12];

const OPL_CHANNELS: usize = 18;
const OPL_CHANNEL_CUT: u8 = 0x80;
const OPL_CHANNEL_MASK: u8 = 0x7f;
const OPL_CHANNEL_INVALID: u8 = 0xff;
const OPL_BASERATE: u64 = 49716;
const CHANNEL_INVALID: usize = usize::MAX;

const AM_VIB: u16 = 0x20;
const KSL_LEVEL: u16 = 0x40;
const ATTACK_DECAY: u16 = 0x60;
const SUSTAIN_RELEASE: u16 = 0x80;
const WAVE_SELECT: u16 = 0xe0;
const FNUM_LOW: u16 = 0xa0;
const KEYON_BLOCK: u16 = 0xb0;
const FEEDBACK_CONNECTION: u16 = 0xc0;

const KEYON_BIT: u8 = 0x20;
const KSL_MASK: u8 = 0xc0;
const TOTAL_LEVEL_MASK: u8 = 0x3f;
const CONNECTION_BIT: u8 = 0x01;
const VOICE_TO_LEFT: u8 = 0x10;
const VOICE_TO_RIGHT: u8 = 0x20;
const STEREO_BITS: u8 = VOICE_TO_LEFT | VOICE_TO_RIGHT;

/// `Util::muldivr_unsigned`.
pub fn muldivr_unsigned(a: u32, b: u32, c: u32) -> u32 {
    ((a as u64 * b as u64 + (c / 2) as u64) / c as u64).min(u32::MAX as u64) as u32
}

/// OpenMPT's `OPL` class (without the register logger).
#[derive(Clone)]
pub struct Opl {
    opl: Box<Opal>,
    key_on_block: [u8; OPL_CHANNELS],
    opl_to_chan: [usize; OPL_CHANNELS],
    chan_to_opl: Vec<u8>,
    patches: [OplPatch; OPL_CHANNELS],
    is_active: bool,
}

impl Opl {
    /// An OPL for `channels` tracker channels, rendering at `sample_rate`.
    pub fn new(sample_rate: u32, channels: usize) -> Opl {
        let mut o = Opl {
            opl: Box::new(Opal::new(sample_rate as i32)),
            key_on_block: [0; OPL_CHANNELS],
            opl_to_chan: [CHANNEL_INVALID; OPL_CHANNELS],
            chan_to_opl: vec![OPL_CHANNEL_INVALID; channels],
            patches: [[0; 12]; OPL_CHANNELS],
            is_active: false,
        };
        o.reset();
        o
    }

    /// Whether a note was ever started (`m_isActive`): until then `mix` adds nothing.
    pub fn is_active(&self) -> bool {
        self.is_active
    }

    /// The next stereo output sample of the chip (`Opal::Sample`), for `Mix`.
    pub fn sample(&mut self) -> (i16, i16) {
        self.opl.sample()
    }

    fn channel_to_register(opl_ch: u8) -> u16 {
        if opl_ch < 9 { opl_ch as u16 } else { (opl_ch as u16 - 9) | 0x100 }
    }

    /// A channel's first operator address.
    fn operator_to_register(opl_ch: u8) -> u16 {
        const OPL_CHANNEL_TO_OPERATOR: [u16; 9] = [0, 1, 2, 8, 9, 10, 16, 17, 18];
        if opl_ch < 9 {
            OPL_CHANNEL_TO_OPERATOR[opl_ch as usize]
        } else {
            OPL_CHANNEL_TO_OPERATOR[opl_ch as usize - 9] | 0x100
        }
    }

    fn get_voice(&self, c: usize) -> u8 {
        let v = self.chan_to_opl.get(c).copied().unwrap_or(OPL_CHANNEL_INVALID);
        if v & OPL_CHANNEL_CUT != 0 || v == OPL_CHANNEL_INVALID {
            return OPL_CHANNEL_INVALID;
        }
        v & OPL_CHANNEL_MASK
    }

    fn allocate_voice(&mut self, c: usize) -> u8 {
        if c >= self.chan_to_opl.len() {
            return OPL_CHANNEL_INVALID;
        }
        // Can we re-use a previous channel?
        let opl_ch = self.chan_to_opl[c];
        if opl_ch != OPL_CHANNEL_INVALID {
            if opl_ch & OPL_CHANNEL_CUT == 0 {
                return opl_ch;
            }
            // Check re-use hint
            let opl_ch = opl_ch & OPL_CHANNEL_MASK;
            let owner = self.opl_to_chan[opl_ch as usize];
            if owner == CHANNEL_INVALID || owner == c {
                self.opl_to_chan[opl_ch as usize] = c;
                self.chan_to_opl[c] = opl_ch;
                return opl_ch;
            }
        }
        // Search for unused channel or channel with released note
        let (mut released, mut released_cut) = (OPL_CHANNEL_INVALID, OPL_CHANNEL_INVALID);
        for opl_ch in 0..OPL_CHANNELS as u8 {
            let owner = self.opl_to_chan[opl_ch as usize];
            if owner == CHANNEL_INVALID {
                self.opl_to_chan[opl_ch as usize] = c;
                self.chan_to_opl[c] = opl_ch;
                return opl_ch;
            } else if self.key_on_block[opl_ch as usize] & KEYON_BIT == 0 {
                released = opl_ch;
                if self.chan_to_opl[owner] & OPL_CHANNEL_CUT != 0 {
                    released_cut = opl_ch;
                }
            }
        }
        if released != OPL_CHANNEL_INVALID {
            // Prefer channel that has been marked as cut over channel that has just been released
            if released_cut != OPL_CHANNEL_INVALID {
                released = released_cut;
            }
            let owner = self.opl_to_chan[released as usize];
            self.chan_to_opl[owner] = OPL_CHANNEL_INVALID;
            self.opl_to_chan[released as usize] = c;
            self.chan_to_opl[c] = released;
        }
        self.get_voice(c)
    }

    pub fn note_off(&mut self, c: usize) {
        let opl_ch = self.get_voice(c);
        if opl_ch == OPL_CHANNEL_INVALID || self.key_on_block[opl_ch as usize] & KEYON_BIT == 0 {
            return;
        }
        self.key_on_block[opl_ch as usize] &= !KEYON_BIT;
        self.port(KEYON_BLOCK | Self::channel_to_register(opl_ch), self.key_on_block[opl_ch as usize]);
    }

    /// Note off and volume 0 (not complete silence: the release is still heard
    /// at -48 dB); with `unassign` the voice is free for another channel.
    pub fn note_cut(&mut self, c: usize, unassign: bool) {
        let opl_ch = self.get_voice(c);
        if opl_ch == OPL_CHANNEL_INVALID {
            return;
        }
        self.note_off(c);
        self.volume(c, 0, false);
        if unassign {
            self.opl_to_chan[opl_ch as usize] = CHANNEL_INVALID;
            self.chan_to_opl[c] |= OPL_CHANNEL_CUT;
        }
    }

    /// Sets the voice's frequency (and key on, unless `key_off`).
    pub fn frequency(&mut self, c: usize, milli_hertz: u32, key_off: bool) {
        let opl_ch = self.get_voice(c);
        if opl_ch == OPL_CHANNEL_INVALID {
            return;
        }
        let mut fnum: u16 = 1023;
        let mut block: u8 = 7;
        if milli_hertz <= 6208431 {
            block = match milli_hertz {
                m if m > 3104215 => 7,
                m if m > 1552107 => 6,
                m if m > 776053 => 5,
                m if m > 388026 => 4,
                m if m > 194013 => 3,
                m if m > 97006 => 2,
                m if m > 48503 => 1,
                _ => 0,
            };
            fnum = muldivr_unsigned(milli_hertz, 1 << (20 - block), (OPL_BASERATE * 1000) as u32) as u16;
        }
        // (OpenMPT's Composer 670 "beating oscillators" hack is for CDFM only)
        let fnum = fnum | ((block as u16) << 10);
        let channel = Self::channel_to_register(opl_ch);
        self.key_on_block[opl_ch as usize] = (if key_off { 0 } else { KEYON_BIT }) | (fnum >> 8) as u8;
        self.port(FNUM_LOW | channel, (fnum & 0xff) as u8);
        self.port(KEYON_BLOCK | channel, self.key_on_block[opl_ch as usize]);
        self.is_active = true;
    }

    fn calc_volume(tracker_vol: u8, ksl_volume: u8) -> u8 {
        if tracker_vol >= 63 {
            return ksl_volume;
        }
        let tv = if tracker_vol > 0 { tracker_vol as u32 + 1 } else { 0 };
        ((ksl_volume & KSL_MASK) as u32 | (63 - ((63 - (ksl_volume & TOTAL_LEVEL_MASK) as u32) * tv) / 64)) as u8
    }

    /// Sets the voice's volume (0..63).
    pub fn volume(&mut self, c: usize, vol: u8, apply_to_modulator: bool) {
        let opl_ch = self.get_voice(c);
        if opl_ch == OPL_CHANNEL_INVALID {
            return;
        }
        let patch = self.patches[opl_ch as usize];
        let modulator = Self::operator_to_register(opl_ch);
        let carrier = modulator + 3;
        if patch[10] & CONNECTION_BIT != 0 || apply_to_modulator {
            // Set volume of both operators in additive mode
            self.port(KSL_LEVEL + modulator, Self::calc_volume(vol, patch[2]));
        }
        if !apply_to_modulator {
            self.port(KSL_LEVEL + carrier, Self::calc_volume(vol, patch[3]));
        }
    }

    /// Sets the voice's pan from `pan` 0..256: 0..84 left, 85..170 centre,
    /// 171..256 right. Returns -1, 0 or 1 (left, centre, right).
    pub fn pan(&mut self, c: usize, pan: i32) -> i8 {
        let opl_ch = self.get_voice(c);
        if opl_ch == OPL_CHANNEL_INVALID {
            return 0;
        }
        let patch = self.patches[opl_ch as usize];
        let mut fb_conn = patch[10] & !STEREO_BITS;
        if pan <= 170 {
            fb_conn |= VOICE_TO_LEFT;
        }
        if pan >= 85 {
            fb_conn |= VOICE_TO_RIGHT;
        }
        self.port(FEEDBACK_CONNECTION | Self::channel_to_register(opl_ch), fb_conn);
        (if fb_conn & VOICE_TO_LEFT != 0 { -1 } else { 0 }) + (if fb_conn & VOICE_TO_RIGHT != 0 { 1 } else { 0 })
    }

    /// Allocates a voice for channel `c` and loads `patch` into it.
    pub fn patch(&mut self, c: usize, patch: &OplPatch) {
        let opl_ch = self.allocate_voice(c);
        if opl_ch == OPL_CHANNEL_INVALID {
            return;
        }
        self.patches[opl_ch as usize] = *patch;
        let modulator = Self::operator_to_register(opl_ch);
        let carrier = modulator + 3;
        for op in 0..2 {
            let op_reg = if op != 0 { carrier } else { modulator };
            self.port(AM_VIB | op_reg, patch[op]);
            self.port(KSL_LEVEL | op_reg, patch[2 + op]);
            self.port(ATTACK_DECAY | op_reg, patch[4 + op]);
            self.port(SUSTAIN_RELEASE | op_reg, patch[6 + op]);
            self.port(WAVE_SELECT | op_reg, patch[8 + op]);
        }
        self.port(FEEDBACK_CONNECTION | Self::channel_to_register(opl_ch), patch[10]);
    }

    /// Whether channel `c` has a voice.
    pub fn has_voice(&self, c: usize) -> bool {
        self.get_voice(c) != OPL_CHANNEL_INVALID
    }

    /// Cuts every note and frees every voice (`OPL::Reset`).
    pub fn reset(&mut self) {
        if self.is_active {
            for c in 0..self.chan_to_opl.len() {
                self.note_cut(c, true);
            }
            self.is_active = false;
        }
        self.key_on_block = [0; OPL_CHANNELS];
        self.opl_to_chan = [CHANNEL_INVALID; OPL_CHANNELS];
        self.chan_to_opl.fill(OPL_CHANNEL_INVALID);
        self.port(0x105, 1); // Enable OPL3
        self.port(0x104, 0); // No 4-op voices
    }

    fn port(&mut self, reg: u16, value: u8) {
        self.opl.port(reg, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voices_follow_channels() {
        let mut o = Opl::new(48000, 20);
        let p: OplPatch = [0x21, 0x21, 0x8f, 0x06, 0xf2, 0xf4, 0x55, 0x37, 0x01, 0x00, 0x0a, 0];
        for c in 0..18 {
            o.patch(c, &p);
            o.frequency(c, 440_000, false);
        }
        assert!(!o.is_active() || o.has_voice(17));
        // all 18 voices keyed on: no voice for a 19th channel
        o.patch(18, &p);
        assert!(!o.has_voice(18));
        // a cut voice goes to the next channel that asks
        o.note_cut(3, true);
        o.patch(19, &p);
        assert!(o.has_voice(19) && !o.has_voice(3));
        assert_eq!(Opl::calc_volume(63, 0x8f), 0x8f);
        assert_eq!(Opl::calc_volume(0, 0x8f), 0xbf);
        assert_eq!(Opl::calc_volume(32, 0x06), 34); // 63 - 57 * 33 / 64
    }
}
