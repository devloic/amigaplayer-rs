//! The mixer of ft2play (`pmp_mix.c`, `snd_masm.c`, `snd_masm.h`): FT2's SB16 / WAV-writer
//! mixer, bit for bit. Only what an 8-bit forward-looping or one-shot sample needs is here: the
//! 16-bit, ping-pong and volume-ramping routines are used by XMs only.

use super::module::Smp;
use super::tables::PANNING_TAB;

pub(super) const S_FWD: u8 = 1;
pub(super) const S_REV: u8 = 2;
pub(super) const S_OFF: u8 = 8;

/// `CIType`: one voice of the mixer.
#[derive(Clone, Copy, Default)]
pub(super) struct Voice {
    /// the instrument whose first sample plays (0: none)
    sbase: usize,
    stype: u8,
    span: u8,
    svol: u8,
    slvol1: i32,
    srvol1: i32,
    slen: i32,
    srep_l: i32,
    spos: i32,
    sposdec: u32,
    sfrq: u32,
}

pub(super) struct Mixer {
    voices: [Voice; 32 * 2],
    /// `CDA_Amp`: 8 times the amplification level
    amp: u32,
    interp: bool,
    pub buf: Vec<i32>,
}

impl Mixer {
    /// `mix_ClearChannels` with the amplification of `dump_Init`.
    pub fn new(amp_level: u32, interp: bool) -> Mixer {
        let mut m = Mixer { voices: [Voice::default(); 64], amp: 8 * amp_level, interp, buf: Vec::new() };
        m.clear();
        m
    }

    fn clear(&mut self) {
        for v in self.voices.iter_mut() {
            *v = Voice { span: 128, stype: S_OFF, ..Voice::default() };
        }
    }

    /// `updateVolume` (without the ramping, which only an XM of version 1.04 has).
    fn update_volume(&mut self, ch: usize) {
        let v = &mut self.voices[2 * ch];
        let vol = v.svol as u32 * self.amp;
        v.slvol1 = (vol.wrapping_mul(PANNING_TAB[256 - v.span as usize]) >> (32 - 28)) as i32;
        v.srvol1 = (vol.wrapping_mul(PANNING_TAB[v.span as usize]) >> (32 - 28)) as i32;
    }

    /// `mix_UpdateChannel` for `Status_SetVol`, `Status_SetPan` and `Status_SetFrq`.
    pub fn update_channel(&mut self, ch: usize, vol: Option<u16>, pan: Option<u8>, frq: Option<u32>) {
        if let Some(p) = pan {
            self.voices[2 * ch].span = p;
        }
        if let Some(mut vol) = vol {
            if vol > 0 {
                vol -= 1; // 0..256 -> 0..255
            }
            self.voices[2 * ch].svol = vol as u8;
        }
        if vol.is_some() || pan.is_some() {
            self.update_volume(ch);
        }
        if let Some(f) = frq {
            self.voices[2 * ch].sfrq = f;
        }
    }

    /// `P_StartTone` -> `mix_UpdateChannel` with `Status_StartTone + Status_StopTone`: `instr` is
    /// the instrument's number and `s` its sample.
    pub fn start_tone(&mut self, ch: usize, instr: usize, s: &Smp, start_pos: i32) {
        let v = &mut self.voices[2 * ch];
        v.stype = S_OFF;
        let mut typ = s.typ;
        let len;
        if typ & (S_FWD | S_REV) != 0 {
            v.srep_l = s.rep_l;
            len = s.rep_s + s.rep_l;
        } else {
            typ &= !(S_FWD | S_REV);
            len = s.len;
            if len == 0 {
                return;
            }
        }
        // an overflown 9xx cuts the voice
        if start_pos >= len {
            return;
        }
        v.slen = len;
        v.spos = start_pos;
        v.sposdec = 0;
        v.sbase = instr;
        v.stype = typ;
    }

    /// Mixes `num` samples of every voice of the first `chn` channels into `buf`, which it
    /// clears first (`dump_GetFrame`), and gives the 16-bit result.
    pub fn mix(&mut self, samples: &[Option<Smp>], chn: usize, num: usize, out: &mut Vec<i16>) {
        self.buf.clear();
        self.buf.resize(num * 2, 0);
        let interp = self.interp;
        for v in self.voices[..chn * 2].iter_mut() {
            let data: &[i8] = samples.get(v.sbase).and_then(|s| s.as_ref()).map_or(&[], |s| &s.data);
            mix_voice(v, data, num as u32, &mut self.buf, interp);
        }
        out.clear();
        out.extend(self.buf.iter().map(|&x| {
            let o = x >> 8;
            // CLAMP16
            if o as i16 as i32 != o { (0x7FFF ^ (o >> 31)) as i16 } else { o as i16 }
        }));
    }
}

/// `PMPMix32Proc`.
fn mix_voice(v: &mut Voice, base: &[i8], num: u32, buf: &mut [i32], interp: bool) {
    if v.stype & S_OFF != 0 {
        return; // voice is not active
    }
    if v.slvol1 | v.srvol1 == 0 {
        // silence mix: only the position moves
        let to_mix = v.sfrq as u64 * num as u64; // 16.16fp
        let samples = (to_mix >> 16) as i32;
        let frac = (to_mix & 0xFFFF) as i32 + (v.sposdec >> 16) as i32;
        let mut real_pos = v.spos.wrapping_add(samples).wrapping_add(frac >> 16);
        let pos_frac = frac & 0xFFFF;
        if real_pos >= v.slen {
            let mut stype = v.stype;
            if stype & (S_FWD | S_REV) != 0 {
                loop {
                    stype ^= 4;
                    real_pos -= v.srep_l;
                    if real_pos < v.slen || v.srep_l <= 0 {
                        break;
                    }
                }
                v.stype = stype;
            } else {
                v.stype = S_OFF;
                return;
            }
        }
        v.sposdec = (pos_frac as u32) << 16;
        v.spos = real_pos;
        return;
    }
    mix8(v, base, num, buf, interp, v.slvol1 == v.srvol1);
}

/// `mix8b`, `mix8bIntrp`, `mix8bCenter` and `mix8bIntrpCenter`.
fn mix8(v: &mut Voice, base: &[i8], num: u32, buf: &mut [i32], interp: bool, center: bool) {
    let (l_vol, r_vol) = (v.slvol1 as i64, v.srvol1 as i64);
    let at = |i: isize| -> i32 { if i < 0 { 0 } else { base.get(i as usize).copied().unwrap_or(0) as i32 } };
    let mut out = 0usize;
    let mut real_pos = v.spos;
    let mut pos = v.sposdec;
    // address of FT2's mix buffer minus a mix sample's size (used for its quirky LERP)
    let mut mix_buff_pos: u16 = (32768 + 96) - 8;
    let mut left = num as i32;
    while left > 0 {
        // LIMIT_MIX_NUM
        let sfrq = v.sfrq as i32;
        let mut i = (v.slen - 1).wrapping_sub(real_pos);
        if i > 0xFFFF {
            i = 0xFFFF;
        }
        let mut to_mix = if sfrq != 0 {
            let tmp = ((i as u32) << 16) | (0xFFFF_0000u32.wrapping_sub(pos) >> 16);
            (tmp / sfrq as u32).wrapping_add(1) as i32
        } else {
            65535
        };
        if to_mix > left || to_mix < 0 {
            to_mix = left;
        }
        left -= to_mix;

        // HANDLE_POS_START (forward only: `S_REV` is never set by a MOD)
        let mut ptr = real_pos as isize;
        pos = pos.wrapping_add(mix_buff_pos as u32);
        let ip_h = (sfrq >> 16) as isize;
        let ip_l = (((sfrq & 0xFFFF) as u32) << 16).wrapping_add(8); // 8 = the mixer buffer's increase
        for _ in 0..to_mix {
            let sample = if interp {
                let s1 = at(ptr) << 8;
                let mut s2 = at(ptr + 1) << 8;
                // LERP
                s2 = s2.wrapping_sub(s1);
                pos >>= 1;
                s2 = ((s2 as i64 * (pos as i32) as i64) >> 32) as i32;
                pos = pos.wrapping_add(pos);
                s2 = s2.wrapping_add(s2);
                s2 = s2.wrapping_add(s1);
                s2 << (28 - 16)
            } else {
                at(ptr) << (28 - 8)
            };
            if center {
                let m = ((sample as i64 * l_vol) >> 32) as i32;
                buf[out] = buf[out].wrapping_add(m);
                buf[out + 1] = buf[out + 1].wrapping_add(m);
            } else {
                buf[out] = buf[out].wrapping_add(((sample as i64 * l_vol) >> 32) as i32);
                buf[out + 1] = buf[out + 1].wrapping_add(((sample as i64 * r_vol) >> 32) as i32);
            }
            out += 2;
            // INC_POS
            ptr += ip_h;
            ptr += (ip_l > !pos) as isize;
            pos = pos.wrapping_add(ip_l);
        }

        // HANDLE_POS_END
        real_pos = ptr as i32;
        mix_buff_pos = (pos & 0xFFFF) as u16;
        pos &= 0xFFFF_0000;
        if real_pos >= v.slen {
            let mut stype = v.stype;
            if stype & (S_FWD | S_REV) != 0 {
                loop {
                    real_pos -= v.srep_l;
                    stype ^= 4;
                    if real_pos < v.slen || v.srep_l <= 0 {
                        break;
                    }
                }
                v.stype = stype;
            } else {
                v.stype = S_OFF;
                return;
            }
        }
    }
    // SET_BACK_MIXER_POS
    v.sposdec = pos & 0xFFFF_0000;
    v.spos = real_pos;
}
