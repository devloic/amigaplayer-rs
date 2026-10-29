//! Farandole Composer extras (`far_extras.c`): the tempo model, the persistent
//! effects and the vibrato the format needs beyond the generic player.
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use super::extras::*;
use super::module::*;
use super::player::*;

pub const FAR_GUS_CHANNELS: i32 = 17;
/// Number of real ticks for each FAR "tick" in old tempo mode (the real
/// replayer uses 8).
pub const FAR_OLD_TEMPO_MULT: i32 = 1 << 2;

/// `struct far_channel_extras`.
#[derive(Clone, Copy, Default, Debug)]
pub struct FarChannelExtras {
    /// Is vibrato persistent?
    pub vib_sustain: i32,
    /// Vibrato rate.
    pub vib_rate: i32,
}

/// `struct far_module_extras`.
#[derive(Clone, Copy, Default, Debug)]
pub struct FarModuleExtras {
    pub coarse_tempo: i32,
    pub fine_tempo: i32,
    pub tempo_mode: i32,
    /// Vibrato depth for all channels.
    pub vib_depth: i32,
}

impl FarModuleExtras {
    /// `libxmp_far_new_module_extras`.
    pub fn new() -> Self {
        FarModuleExtras { vib_depth: 4, ..Default::default() }
    }
}

/// tempo[0] = 256; tempo[i] = floor(128 / i).
const FAR_TEMPOS: [i32; 16] = [256, 128, 64, 42, 32, 25, 21, 18, 16, 14, 12, 11, 10, 9, 9, 8];

/// `libxmp_far_translate_tempo`: Some((speed, bpm)) for a valid tempo; `fine`
/// may be clamped even when the result is None.
pub fn far_translate_tempo(mode: i32, fine_change: i32, coarse: i32, fine: &mut i32) -> Option<(i32, i32)> {
    if !(0..=15).contains(&coarse) || !(0..=1).contains(&mode) {
        return None;
    }

    // Compatibility for FAR's broken fine tempo "clamping".
    if fine_change < 0 && FAR_TEMPOS[coarse as usize] + *fine <= 0 {
        *fine = 0;
    } else if fine_change > 0 && FAR_TEMPOS[coarse as usize] + *fine >= 100 {
        *fine = 100;
    }

    let speed;
    let mut bpm;
    if mode == 1 {
        // "New" FAR tempo
        let tempo = FAR_TEMPOS[coarse as usize] + *fine;
        if tempo == 0 {
            return None;
        }
        // uint32 divisor: a negative tempo converts to a huge unsigned value
        let mut divisor: u32 = (1197255i32 / tempo) as u32;

        let mut s = 0;
        while divisor > 0xffff {
            divisor >>= 1;
            s += 1;
        }
        if s >= 2 {
            s += 1;
        }
        s += 3;
        // Add an extra tick because the FAR replayer checks the tick
        // remaining count before decrementing it but after handling each tick.
        s += 1;
        speed = s;
        bpm = (1197255u32 / divisor) as i32;
    } else {
        // "Old" FAR tempo
        speed = 4 * FAR_OLD_TEMPO_MULT;
        bpm = (FAR_TEMPOS[coarse as usize] + *fine * 2) * FAR_OLD_TEMPO_MULT;
    }

    if bpm < XMP_MIN_BPM {
        bpm = XMP_MIN_BPM;
    }
    Some((speed, bpm))
}

/// `libxmp_far_update_vibrato`.
fn far_update_vibrato(lfo: &mut Lfo, rate: i32, depth: i32) {
    lfo.depth = gus_frequency_steps(depth << 1, FAR_GUS_CHANNELS) as i32;
    lfo.rate = rate * 3;
}

/// Convoluted algorithm for delay times for retrigger and note offset effects.
fn far_retrigger_delay(me: &FarModuleExtras, param: i32) -> i32 {
    if !(0..=15).contains(&me.coarse_tempo) || param < 1 {
        return -1;
    }
    let mut delay = (FAR_TEMPOS[me.coarse_tempo as usize] + me.fine_tempo) / param;

    if me.tempo_mode != 0 {
        // Effects divide by 4, timer increments by 2 (round up).
        ((delay >> 2) + 1) >> 1
    } else {
        // Effects divide by 2, timer increments by 2 (round up). Old tempo mode
        // handles every 8th tick. Delay values >4 result in no retrigger.
        delay = (((delay >> 1) + 1) >> 1) * FAR_OLD_TEMPO_MULT;
        if delay >= 4 * FAR_OLD_TEMPO_MULT {
            return -1;
        }
        if delay < FAR_OLD_TEMPO_MULT {
            return FAR_OLD_TEMPO_MULT;
        }
        delay
    }
}

impl Context {
    /// `libxmp_far_update_tempo`.
    fn far_update_tempo(&mut self, me: &mut FarModuleExtras, fine_change: i32) {
        if let Some((speed, bpm)) = far_translate_tempo(me.tempo_mode, fine_change, me.coarse_tempo, &mut me.fine_tempo) {
            self.p.speed = speed;
            self.p.bpm = bpm;
        }
    }

    /// `libxmp_far_play_extras`.
    pub(crate) fn far_play_extras(&mut self, chn: usize) {
        let depth = match self.m.extra {
            ModuleExtra::Far(me) => me.vib_depth,
            _ => return,
        };
        let xc = &mut self.p.xc_data[chn];
        if let ChannelExtra::Far(ce) = xc.extra {
            // FAR vibrato depth is global, even though rate isn't. This might
            // have been changed by a different channel, so make sure it's applied.
            if xc.flags & VIBRATO != 0 || xc.per_flags & VIBRATO != 0 {
                far_update_vibrato(&mut xc.vibrato.lfo, ce.vib_rate, depth);
            }
        }
    }

    /// `libxmp_far_extras_process_fx`.
    pub(crate) fn far_extras_process_fx(&mut self, xc: &mut ChannelData, note: u8, fxt: u8, fxp: u8) {
        let (mut me, mut ce) = match (self.m.extra, xc.extra) {
            (ModuleExtra::Far(me), ChannelExtra::Far(ce)) => (me, ce),
            _ => return,
        };
        let mut fxp = fxp as i32;
        let note = note as i32;
        let mut update_tempo = false;
        let mut update_vibrato = false;
        let mut fine_change = 0;

        // Tempo effects and vibrato are multiplexed to reduce the effects count.
        match fxt {
            FX_FAR_PORTA_UP => {
                xc.flags |= FINE_BEND;
                xc.per_flags &= !TONEPORTA;
                xc.freq.fslide = gus_frequency_steps(fxp << 2, FAR_GUS_CHANNELS);
            }
            FX_FAR_PORTA_DN => {
                xc.flags |= FINE_BEND;
                xc.per_flags &= !TONEPORTA;
                xc.freq.fslide = -gus_frequency_steps(fxp << 2, FAR_GUS_CHANNELS);
            }
            // This effect scales with tempo and only corresponds to (param)
            // rows at tempo 4.
            FX_FAR_TPORTA => {
                if self.is_valid_instrument(xc.ins) {
                    let mut tempo = FAR_TEMPOS[me.coarse_tempo as usize] + me.fine_tempo;

                    xc.per_flags |= TONEPORTA;
                    if Self::is_valid_note(note - 1) {
                        xc.porta.target = note_to_period(&self.m, note - 1, xc.finetune, xc.per_adj);
                    }
                    xc.porta.dir = if xc.period < xc.porta.target { 1 } else { -1 };

                    // Parameter of 0 is equivalent to 1.
                    if fxp < 1 {
                        fxp = 1;
                    }
                    // Tempos <=0 cause crashes and other weird behavior here in
                    // Farandole Composer, don't emulate that.
                    if tempo < 1 {
                        tempo = 1;
                    }

                    let diff = (xc.porta.target - xc.period) as i32;
                    let step = diff.abs() * 8 / (tempo * fxp);

                    xc.porta.slide = if step > 0 { step } else { 1 };
                }
            }
            // This effect scales with tempo and only corresponds to (param/2)
            // rows at tempo 4.
            FX_FAR_SLIDEVOL => {
                let mut tempo = FAR_TEMPOS[me.coarse_tempo as usize] + me.fine_tempo;
                let target = (fxp >> 4) << 4;
                fxp &= 0x0f;

                if fxp < 1 {
                    fxp = 1;
                }
                if tempo < 1 {
                    tempo = 1;
                }

                let diff = target - xc.volume;
                let mut step = diff * 16 / (tempo * fxp);
                if step == 0 {
                    step = if diff > 0 { 1 } else { -1 };
                }

                xc.per_flags |= VOL_SLIDE;
                xc.vol.slide = step;
                xc.vol.target = target + 1;
            }
            FX_FAR_VIBDEPTH => {
                me.vib_depth = fxp & 0x0f;
                update_vibrato = true;
            }
            FX_FAR_VIBRATO => {
                if ce.vib_sustain == 0 {
                    // With sustain, regular vibrato only sets the rate.
                    ce.vib_sustain = fxp >> 4;
                    if ce.vib_sustain == 0 {
                        xc.flags |= VIBRATO;
                    }
                }
                ce.vib_rate = fxp & 0x0f;
                update_vibrato = true;
            }
            // Retrigger note param times at intervals that roughly evenly divide
            // the row. A param of 0 crashes Farandole Composer.
            FX_FAR_RETRIG => {
                let delay = far_retrigger_delay(&me, fxp);
                if note != 0 && fxp > 1 && delay >= 0 && delay <= self.p.speed {
                    xc.flags |= RETRIG;
                    xc.retrig.val = if delay != 0 { delay } else { 1 };
                    xc.retrig.count = delay + 1;
                    xc.retrig.type_ = 0;
                    xc.retrig.limit = fxp - 1;
                }
            }
            // A better effect name would probably be "retrigger once".
            FX_FAR_DELAY => {
                if note != 0 {
                    let delay = if me.tempo_mode != 0 { fxp } else { fxp * FAR_OLD_TEMPO_MULT };
                    xc.flags |= RETRIG;
                    xc.retrig.val = if delay != 0 { delay } else { 1 };
                    xc.retrig.count = delay + 1;
                    xc.retrig.type_ = 0;
                    xc.retrig.limit = if fxp != 0 { 1 } else { 0 };
                }
            }
            FX_FAR_TEMPO => {
                if fxp >> 4 != 0 {
                    me.tempo_mode = (fxp >> 4) - 1;
                } else {
                    me.coarse_tempo = fxp & 0x0f;
                }
                update_tempo = true;
            }
            FX_FAR_F_TEMPO => {
                if fxp >> 4 != 0 {
                    me.fine_tempo += fxp >> 4;
                    fine_change = fxp >> 4;
                } else if fxp & 0x0f != 0 {
                    me.fine_tempo -= fxp & 0x0f;
                    fine_change = -(fxp & 0x0f);
                } else {
                    me.fine_tempo = 0;
                }
                update_tempo = true;
            }
            _ => {}
        }

        if update_vibrato {
            if ce.vib_rate != 0 {
                if ce.vib_sustain != 0 {
                    xc.per_flags |= VIBRATO;
                }
            } else {
                xc.per_flags &= !VIBRATO;
                ce.vib_sustain = 0;
            }
            far_update_vibrato(&mut xc.vibrato.lfo, ce.vib_rate, me.vib_depth);
        }

        if update_tempo {
            self.far_update_tempo(&mut me, fine_change);
        }

        xc.extra = ChannelExtra::Far(ce);
        self.m.extra = ModuleExtra::Far(me);
    }
}
