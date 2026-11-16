//! Not libxmp: AdLib (OPL2) instruments of S3M modules, played on the Opal
//! OPL3 emulator (`crate::opl`) from the libxmp port's player state.
//!
//! libxmp 4.7.4 loads an S3M AdLib instrument without a sample, so its notes
//! get no mixer voice and play silent. Here the tracker side stays libxmp's:
//! `read_event_st3` decides when a note starts (`opl_event`), and
//! `play_channel` runs its volume, frequency and pan processing for a channel
//! whose note is an OPL one as for a sample voice (tremolo, tremor, vibrato,
//! arpeggio, slides, global and channel volume), then hands the results to the
//! OPL (`opl_update`) instead of to a voice. The OPL side is OpenMPT's
//! (`OPL.cpp`, and where `Sndmix.cpp` / `Snd_fx.cpp` call it for an S3M):
//!
//! - a note with an AdLib instrument loads the patch into the channel's voice
//!   and keys the old note off, then the tick's frequency write keys it on
//!   again, which retriggers the envelopes (`kOPLNoteOffOnNoteChange`); tone
//!   portamento slides the playing note without a retrigger;
//! - every tick: frequency (while keyed on; an S3M note off stops the writes),
//!   volume (libxmp's final volume, 0..1024, scaled to the OPL's 0..63) and pan;
//! - `^^` (key off) keys the note off, its release then plays at the channel's
//!   volume, which effects still change (libxmp's own release, a cut without an
//!   envelope, is left out for an OPL note); SCx (note cut)
//!   keys it off with volume 0 (OpenMPT's `NoteCut`); Qxy does not retrigger an
//!   OPL note (ST3, OpenMPT RetrigSlide.s3m);
//! - a position jump by the player (`set_position`) resets the OPL.
//!
//! Choices of this port, where the two do not meet: the OPL note replaces a
//! sample voice still playing on its channel and a sample note keys an OPL note
//! off (ST3 has separate PCM and AdLib channels; libxmp would keep the old
//! voice); libxmp's period, which for an AdLib instrument holds the C2SPD and
//! the octave libxmp adds for it, is turned into OpenMPT's frequency with that
//! octave taken back (ST3: C-5 at 8363 Hz is 261.625 Hz); the global volume
//! scales the OPL volume register as libxmp scales a sample's volume (OpenMPT
//! scales the OPL output by it). The output is mixed into the mixer buffer at
//! OpenMPT's level relative to samples (`OPL::Mix`'s 6169 / 65536 times
//! `m_nVSTiVolume / 48`), times the libxmp mixer's level against OpenMPT's
//! for samples, so that an OPL voice is as loud against a sample voice as in
//! OpenMPT.
//!
//! A module without AdLib instruments has `opl` None, and nothing here runs.

use super::player::*;
use crate::opl::{muldivr_unsigned, Opl};

/// The OPL and what each tracker channel plays on it.
pub struct OplState {
    pub opl: Opl,
    /// The channel plays an OPL note (its volume, frequency and pan go to the OPL).
    on: Vec<bool>,
    /// The note is keyed on (its frequency is written every tick).
    keyed: Vec<bool>,
    /// The mixer gain of an Opal sample, in the libxmp mixer's 32-bit buffer.
    gain: i32,
}

/// OPL frequency in mHz from libxmp's mixer period: the sample rate libxmp
/// would play at (428 * 8363 / period, C4_NTSC_RATE) as OpenMPT maps it to pitch
/// (8363 Hz is 261.625 Hz), an octave down for the 12 semitones libxmp's S3M
/// loader adds to an AdLib instrument.
fn period_to_millihertz(period: f64) -> u32 {
    if period <= 0.0 {
        return u32::MAX;
    }
    let mhz = 428.0 * 261625.0 / 2.0 / period;
    if mhz >= u32::MAX as f64 { u32::MAX } else { (mhz + 0.5) as u32 }
}

/// libxmp's mixer is this much louder than OpenMPT 0.7.3's (openmpt123) on the
/// same sample voice: a full-volume 8-bit square panned hard left in an S3M
/// (master volume 48, stereo) comes out 4/3 as loud (+2.5 dB; 2.1 to 2.6 dB at
/// the other pans). The OPL gain is raised as much, so that OPL against samples
/// keeps OpenMPT's balance; an OPL-only module then plays 2.5 dB louder than in
/// openmpt123, like the libxmp port's sample-based S3Ms.
const XMP_OVER_MPT_PCM: f64 = 4.0 / 3.0;

impl OplState {
    fn new(rate: i32, channels: usize, vsti_volume: i32) -> OplState {
        // OPL::Mix: factor = volumeFactorQ16 * 6169 / 65536, volumeFactorQ16 =
        // 65536 * m_nVSTiVolume / 48, added to OpenMPT's 27-bit mix buffer (16-bit
        // output = buffer >> 12); the libxmp buffer is downmixed by >> 11.
        let q16 = (65536 * vsti_volume / 48) as u32;
        let factor = (q16 as u64 * 6169 / 65536) as f64;
        let gain = (factor / 4096.0 * 2048.0 * XMP_OVER_MPT_PCM).round() as i32;
        OplState { opl: Opl::new(rate as u32, channels), on: vec![false; channels], keyed: vec![false; channels], gain }
    }
}

impl Context {
    /// Creates the OPL for a module with AdLib instruments (`start_player`).
    pub(crate) fn opl_start(&mut self) {
        self.opl = if self.m.opl_patches.iter().any(|p| p.is_some()) {
            let n = self.p.virt.virt_channels.max(0) as usize;
            Some(Box::new(OplState::new(self.s.freq, n, self.m.opl_vsti_volume)))
        } else {
            None
        };
    }

    /// The player jumped (`OPL::Reset` from `CSoundFile::SetCurrentOrder`).
    pub(crate) fn opl_reset(&mut self) {
        if let Some(o) = self.opl.as_mut() {
            o.opl.reset();
            o.on.fill(false);
            o.keyed.fill(false);
        }
    }

    pub(crate) fn opl_chan_on(&self, chn: usize) -> bool {
        self.opl.as_ref().is_some_and(|o| o.on.get(chn).copied().unwrap_or(false))
    }

    fn opl_patch_of(&self, ins: i32) -> Option<crate::opl::OplPatch> {
        if ins < 0 {
            return None;
        }
        self.m.opl_patches.get(ins as usize).copied().flatten()
    }

    /// `read_event_st3`'s note: `note` >= 0 when a note starts (not with tone
    /// portamento), `xc.ins` the instrument it plays.
    pub(crate) fn opl_event(&mut self, chn: usize, key_off: bool, note: i32) {
        if self.opl.is_none() || chn >= self.opl.as_ref().unwrap().on.len() {
            return;
        }
        if key_off {
            let o = self.opl.as_mut().unwrap();
            if o.on[chn] {
                o.opl.note_off(chn);
                o.keyed[chn] = false;
                // OpenMPT: "OPL voices are not cut but enter the release portion
                // of their envelope. In S3M we can still modify the volume after
                // note-off": libxmp's release (fade to 0 without an envelope) is
                // left out for an OPL note
                self.p.xc_data[chn].reset_note(NOTE_RELEASE);
            }
            return;
        }
        if note < 0 {
            return;
        }
        let ins = self.p.xc_data[chn].ins;
        match self.opl_patch_of(ins) {
            Some(patch) => {
                // the OPL note replaces a sample voice on the channel
                self.virt_resetchannel(chn as i32);
                let o = self.opl.as_mut().unwrap();
                o.opl.patch(chn, &patch);
                o.opl.note_off(chn);
                o.on[chn] = true;
                o.keyed[chn] = true;
            }
            None => {
                let o = self.opl.as_mut().unwrap();
                if o.on[chn] {
                    o.opl.note_off(chn);
                    o.on[chn] = false;
                    o.keyed[chn] = false;
                }
            }
        }
    }

    /// SCx cut the channel's note (OpenMPT `NoteCut(nChn, false)`).
    pub(crate) fn opl_cut(&mut self, chn: usize) {
        if let Some(o) = self.opl.as_mut() {
            if o.on.get(chn).copied().unwrap_or(false) {
                o.opl.note_cut(chn, false);
                o.keyed[chn] = false;
            }
        }
    }

    /// After `play_channel`'s processing: the tick's frequency, volume and pan.
    pub(crate) fn opl_update(&mut self, chn: usize) {
        let xc = &self.p.xc_data[chn];
        let (period, final_period, finalvol, finalpan) = (xc.period, xc.final_period, xc.info_finalvol, xc.info_finalpan);
        let muted = chn < self.p.channel_mute.len() && self.p.channel_mute[chn];
        let Some(o) = self.opl.as_mut() else { return };
        if period < 0.25 {
            // libxmp resets the channel (QUIRK_ST3BUGS): the note is cut
            o.opl.note_cut(chn, true);
            o.on[chn] = false;
            o.keyed[chn] = false;
            return;
        }
        if o.keyed[chn] {
            o.opl.frequency(chn, period_to_millihertz(final_period), false);
        }
        let vol = if muted { 0 } else { muldivr_unsigned(finalvol.max(0) as u32, 63, 1024).min(63) };
        o.opl.volume(chn, vol as u8, false);
        o.opl.pan(chn, finalpan);
    }

    /// Adds the OPL's output for the tick to the mixer buffer (`OPL::Mix`).
    pub(crate) fn opl_mix(&mut self, frames: usize) {
        let Some(o) = self.opl.as_mut() else { return };
        if !o.opl.is_active() {
            return;
        }
        let g = o.gain;
        for f in self.s.buf32[..frames * 2].chunks_exact_mut(2) {
            let (l, r) = o.opl.sample();
            f[0] = f[0].wrapping_add(l as i32 * g);
            f[1] = f[1].wrapping_add(r as i32 * g);
        }
    }
}
