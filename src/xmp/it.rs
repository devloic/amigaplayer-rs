//! The Impulse Tracker parts of libxmp's player: `read_event_it` and its helpers
//! (`read_event.c`), and the MIDI macro parser behind IT's filter effects
//! (`player.c`). A child module of `player`, so that it shares its private helpers.
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use super::*;
use crate::xmp::mixer::VOICE_REVERSE;

/// `struct midi_stream`.
struct MidiStream<'a> {
    data: &'a [u8],
    pos: usize,
    buffer: i32,
    param: i32,
}

impl MidiStream<'_> {
    /// `*in->pos`: the string ends at a NUL (or the end of the macro).
    fn peek(&self) -> u8 {
        self.data.get(self.pos).copied().unwrap_or(0)
    }
}

fn apply_midi_macro_effect(xc: &mut ChannelData, type_: i32, val: i32) {
    match type_ {
        0 => xc.filter.cutoff = val << 1,
        1 => xc.filter.resonance = val << 1,
        _ => {}
    }
}

impl Context {
    fn midi_nibble(&self, xc: &ChannelData, chn: usize, inp: &mut MidiStream) -> i32 {
        if inp.buffer >= 0 {
            let val = inp.buffer;
            inp.buffer = -1;
            return val;
        }
        while inp.peek() != 0 {
            // a (signed) char in libxmp: bytes over 0x7f match nothing
            let val = inp.peek() as i8 as i32;
            inp.pos += 1;
            if (b'0' as i32..=b'9' as i32).contains(&val) {
                return val - b'0' as i32;
            }
            if (b'A' as i32..=b'F' as i32).contains(&val) {
                return val - b'A' as i32 + 10;
            }
            let mut byte = -1;
            match val as u8 {
                b'z' => byte = inp.param,
                b'n' => byte = xc.key & 0x7f,
                b'h' => byte = chn as i32,
                // not clamped, see ZxxSecrets.it
                b'o' => byte = xc.offset.memory,
                b'm' => {
                    let voc = self.virt_mapchannel(chn as i32);
                    byte = if voc >= 0 { (self.p.virt.voice_array[voc as usize].flags & VOICE_REVERSE != 0) as i32 } else { 0 };
                }
                b'v' => {
                    let ins_vol = self.get_instrument(xc.ins).map_or(0x40, |i| i.vol);
                    let v = (self.p.gvol as u32)
                        .wrapping_mul(xc.volume as u32)
                        .wrapping_mul(xc.mastervol as u32)
                        .wrapping_mul(xc.gvl as u32)
                        .wrapping_mul(ins_vol as u32)
                        >> 24;
                    byte = (v as i32).clamp(1, 127);
                }
                b'u' => byte = (xc.macro_.finalvol >> 3).clamp(1, 127),
                b'x' => byte = (xc.macro_.notepan >> 1).clamp(0, 127),
                b'y' => byte = (xc.info_finalpan >> 1).clamp(0, 127),
                b'a' | b'b' | b'p' | b's' => byte = 0,
                b'c' => return 0,
                _ => {}
            }
            if byte >= 0 {
                inp.buffer = byte & 0xf;
                return (byte >> 4) & 0xf;
            }
        }
        -1
    }

    fn midi_byte(&self, xc: &ChannelData, chn: usize, inp: &mut MidiStream) -> i32 {
        let a = self.midi_nibble(xc, chn, inp);
        let b = self.midi_nibble(xc, chn, inp);
        if a >= 0 && b >= 0 { (a << 4) | b } else { -1 }
    }

    /// `execute_midi_macro`: IT's internal F0 F0 / F0 F1 messages of a macro
    /// set the channel's filter; the real-time statuses FA, FC and FF reset it.
    fn execute_midi_macro(&self, xc: &mut ChannelData, chn: usize, midi: &[u8], param: i32) {
        let mut inp = MidiStream { data: midi, pos: 0, buffer: -1, param };
        while inp.peek() != 0 {
            let mut cmd = -1;
            let mut byte = self.midi_byte(xc, chn, &mut inp);
            if byte == 0xf0 {
                byte = self.midi_byte(xc, chn, &mut inp);
                if byte == 0xf0 || byte == 0xf1 {
                    cmd = byte & 0xf;
                }
            }
            if cmd < 0 {
                if byte == 0xfa || byte == 0xfc || byte == 0xff {
                    apply_midi_macro_effect(xc, 0, 127);
                    apply_midi_macro_effect(xc, 1, 0);
                }
                continue;
            }
            cmd = self.midi_byte(xc, chn, &mut inp) | (cmd << 8);
            let val = self.midi_byte(xc, chn, &mut inp);
            if !(0..0x80).contains(&cmd) || !(0..0x80).contains(&val) {
                continue;
            }
            apply_midi_macro_effect(xc, cmd, val);
        }
    }

    /// `update_midi_macro`: before the process_* functions of a frame.
    pub(super) fn update_midi_macro(&mut self, chn: usize) {
        let mut xc = self.p.xc_data[chn];
        if xc.test(MIDI_MACRO) && self.has_quirk(QUIRK_FILTER) {
            if xc.macro_.slide > 0.0 {
                xc.macro_.val += xc.macro_.slide;
                if xc.macro_.val > xc.macro_.target {
                    xc.macro_.val = xc.macro_.target;
                    xc.macro_.slide = 0.0;
                }
            } else if xc.macro_.slide < 0.0 {
                xc.macro_.val += xc.macro_.slide;
                if xc.macro_.val < xc.macro_.target {
                    xc.macro_.val = xc.macro_.target;
                    xc.macro_.slide = 0.0;
                }
            } else if self.p.frame != 0 {
                // non-smooth macros run on frame 0 only
                self.p.xc_data[chn] = xc;
                return;
            }
            let val = xc.macro_.val as i32;
            let midi = self.m.midi.take();
            if val >= 0x80 {
                if let Some(cfg) = &midi {
                    self.execute_midi_macro(&mut xc, chn, &cfg.fixed[(val - 0x80) as usize], val);
                } else if val < 0x90 {
                    // default fixed macro: resonance
                    apply_midi_macro_effect(&mut xc, 1, (val - 0x80) << 3);
                }
            } else if let Some(cfg) = &midi {
                let active = xc.macro_.active as usize;
                self.execute_midi_macro(&mut xc, chn, &cfg.param[active], val);
            } else if xc.macro_.active == 0 {
                // default parameterized macro 0: cutoff
                apply_midi_macro_effect(&mut xc, 0, val);
            }
            self.m.midi = midi;
        }
        self.p.xc_data[chn] = xc;
    }

    // read_event.c, IT

    fn reset_envelope_volume(&mut self, chn: usize) {
        if !self.is_valid_instrument(self.p.xc_data[chn].ins) {
            return;
        }
        let xc = &mut self.p.xc_data[chn];
        xc.reset_note(NOTE_ENV_END);
        xc.v_idx = -1;
    }

    fn reset_envelopes_carry(&mut self, chn: usize) {
        let ins = self.p.xc_data[chn].ins;
        if !self.is_valid_instrument(ins) {
            return;
        }
        let xxi = &self.m.module.xxi[ins as usize];
        let (a, p, f) = (xxi.aei.flg, xxi.pei.flg, xxi.fei.flg);
        let xc = &mut self.p.xc_data[chn];
        xc.reset_note(NOTE_ENV_END);
        if a & XMP_ENVELOPE_CARRY == 0 {
            xc.v_idx = -1;
        }
        if p & XMP_ENVELOPE_CARRY == 0 {
            xc.p_idx = -1;
        }
        if f & XMP_ENVELOPE_CARRY == 0 {
            xc.f_idx = -1;
        }
    }

    /// `copy_channel`: the state of `from` into the background channel `to`.
    fn copy_channel(&mut self, to: i32, from: usize) {
        if to > 0 && to as usize != from {
            self.p.xc_data[to as usize] = self.p.xc_data[from];
        }
    }

    fn check_fadeout(&self, chn: usize, ins: i32) -> bool {
        let Some(xxi) = self.get_instrument(ins) else { return true };
        let xc = &self.p.xc_data[chn];
        xxi.aei.flg & XMP_ENVELOPE_ON == 0 || xxi.aei.flg & XMP_ENVELOPE_CARRY == 0 || xc.ins_fade == 0 || xc.fadeout <= xc.ins_fade
    }

    fn check_invalid_sample(&self, ins: i32, key: i32) -> bool {
        let md = &self.m.module;
        if (ins as usize) < md.ins {
            let smp = md.xxi[ins as usize].map[key as usize].ins as usize;
            if smp == 0xff || smp >= md.smp {
                return true;
            }
        }
        false
    }

    fn fix_period(&mut self, chn: usize, sub: &SubInstrument) {
        if sub.nna == XMP_INST_NNA_CONT {
            let xc = &self.p.xc_data[chn];
            let xpo = self.get_instrument(xc.ins).map_or(0, |i| i.map[xc.key_porta as usize].xpo as i32);
            let per = note_to_period(&self.m, xc.key + sub.xpo + xpo, xc.finetune, xc.per_adj);
            self.p.xc_data[chn].period = per;
        }
    }

    fn is_same_sid(&self, chn: usize, ins: i32, key: i32) -> bool {
        let xc = &self.p.xc_data[chn];
        let s1 = self.get_subinstrument(ins, key);
        let s2 = self.get_subinstrument(xc.ins, xc.key);
        matches!((s1, s2), (Some(a), Some(b)) if a.sid == b.sid)
    }

    /// `read_event_it`: Impulse Tracker's note, instrument and new note action
    /// handling (the OpenMPT and libxmp test cases the C code names).
    pub(super) fn read_event_it(&mut self, e: &Event, chn: usize) {
        let mut ev = *e;

        // Impulse Tracker's "always read instrument" bug
        if ev.ins != 0 {
            self.p.xc_data[chn].delayed_ins = 0;
        } else if ev.note != 0 && self.p.xc_data[chn].delayed_ins != 0 {
            ev.ins = self.p.xc_data[chn].delayed_ins as u8;
            self.p.xc_data[chn].delayed_ins = 0;
        }

        self.p.xc_data[chn].flags = 0;
        let mut note = -1;
        let mut key = ev.note as i32;
        let mut not_same_ins = false;
        let mut not_same_smp = false;
        let mut new_invalid_ins = false;
        let mut is_toneporta = false;
        let mut is_release = false;
        let mut reset_env = false;
        let mut reset_susloop = false;
        let mut use_ins_vol = false;
        let mut candidate_ins = self.p.xc_data[chn].ins;
        let sample_mode = !self.has_quirk(QUIRK_VIRTUAL);
        let mut toneporta_offset = false;
        let mut retrig_ins = false;
        let nins = self.m.module.ins;

        // keyoff + instrument retrigs the current instrument in old fx mode
        if self.has_quirk(QUIRK_ITOLDFX) && ev.note == XMP_KEY_OFF && self.is_valid_instrument(ev.ins as i32 - 1) {
            retrig_ins = true;
        }

        // notes with unmapped instruments are ignored
        if ev.ins != 0 {
            if (ev.ins as usize) <= nins && Self::is_valid_note(ev.note as i32 - 1) {
                let ins = ev.ins as i32 - 1;
                if self.check_invalid_sample(ins, ev.note as i32 - 1) {
                    candidate_ins = ins;
                    ev = Event::default();
                }
            }
        } else if Self::is_valid_note(ev.note as i32 - 1) {
            let ins = self.p.xc_data[chn].old_ins - 1;
            if !self.is_valid_instrument(ins) {
                new_invalid_ins = true;
            } else if self.check_invalid_sample(ins, ev.note as i32 - 1) {
                ev = Event::default();
            }
        }

        if Self::is_toneporta_fx(ev.fxt) || Self::is_toneporta_fx(ev.f2t) {
            is_toneporta = true;
        }
        if self.p.xc_data[chn].test_note(NOTE_ENV_RELEASE | NOTE_FADEOUT) {
            is_release = true;
        }
        if self.p.xc_data[chn].period <= 0.0 || self.p.xc_data[chn].test_note(NOTE_END) {
            is_toneporta = false;
        }

        // Off-Porta.it
        if is_toneporta && ev.fxt == FX_OFFSET {
            toneporta_offset = true;
            if !self.has_quirk(QUIRK_PRENV) {
                self.p.xc_data[chn].reset_note(NOTE_ENV_END);
            }
        }

        // instrument
        if ev.ins != 0 {
            let ins = ev.ins as i32 - 1;
            let mut set_new_ins = true;

            // portamento_after_keyoff.it
            if is_release && key == 0 {
                if is_toneporta {
                    if self.has_quirk(QUIRK_PRENV) || self.p.xc_data[chn].test_note(NOTE_SET) {
                        is_toneporta = false;
                        self.reset_envelopes_carry(chn);
                    }
                } else {
                    // OpenMPT wnoteoff.it
                    self.reset_envelopes_carry(chn);
                }
            }

            if is_toneporta && self.p.xc_data[chn].ins == ins && !self.has_quirk(QUIRK_PRENV) {
                if self.is_same_sid(chn, ins, key - 1) {
                    set_new_ins = !is_release;
                } else {
                    not_same_ins = true;
                    not_same_smp = true;
                }
            }

            if set_new_ins {
                self.p.xc_data[chn].set(NEW_INS);
                reset_env = true;
            }
            // the sample's default volume is always used for a valid sample
            // (Atomic Playboy, default_volume.it)
            use_ins_vol = true;
            self.p.xc_data[chn].per_flags = 0;

            if self.is_valid_instrument(ins) {
                // OpenMPT StoppedInstrSwap.it for the cut case
                if key == 0 && !self.p.xc_data[chn].test_note(NOTE_KEY_CUT) {
                    // retrig the new instrument in sample mode
                    if sample_mode && self.p.xc_data[chn].test_note(NOTE_END) {
                        self.virt_voicepos(chn as i32, 0.0);
                    }
                    // IT: reset the note for every new != ins
                    if self.p.xc_data[chn].ins == ins {
                        self.p.xc_data[chn].set(NEW_INS);
                        use_ins_vol = true;
                    } else {
                        key = self.p.xc_data[chn].key + 1;
                    }
                    self.p.xc_data[chn].reset_note(NOTE_SET);
                }

                if self.p.xc_data[chn].ins != ins && (!is_toneporta || !self.has_quirk(QUIRK_PRENV)) {
                    candidate_ins = ins;
                    if !self.is_same_sid(chn, ins, key - 1) {
                        not_same_ins = true;
                        if is_toneporta {
                            // the new instrument's volume
                            if let Some(sub) = self.get_subinstrument(ins, key) {
                                self.p.xc_data[chn].volume = sub.vol;
                                use_ins_vol = false;
                            }
                        }
                    }
                }
            } else {
                // in sample mode invalid instruments cut the note
                // (OpenMPT SampleNumberChange.it)
                if sample_mode {
                    self.p.xc_data[chn].volume = 0;
                }
                // invalid instruments are ignored
                new_invalid_ins = true;
                self.p.xc_data[chn].flags = 0;
                use_ins_vol = false;
            }
        }

        // note
        if key != 0 {
            self.p.xc_data[chn].set(NEW_NOTE);
            self.p.xc_data[chn].set_note(NOTE_SET);

            if key == XMP_KEY_FADE as i32 {
                self.p.xc_data[chn].set_note(NOTE_FADEOUT);
                reset_env = false;
                reset_susloop = false;
                use_ins_vol = false;
            } else if key == XMP_KEY_CUT as i32 {
                let xc = &mut self.p.xc_data[chn];
                xc.set_note(NOTE_END | NOTE_CUT | NOTE_KEY_CUT);
                xc.period = 0.0;
                self.virt_resetchannel(chn as i32);
            } else if key == XMP_KEY_OFF as i32 {
                let ins = self.p.xc_data[chn].ins;
                let sus = self.is_valid_instrument(ins)
                    && Self::sustain_check(&self.m.module.xxi[ins as usize].aei, self.p.xc_data[chn].v_idx);
                let xc = &mut self.p.xc_data[chn];
                if sus {
                    xc.set_note(NOTE_SUSEXIT);
                } else {
                    xc.set_note(NOTE_RELEASE);
                }
                xc.set(KEY_OFF);
                // the instrument volume if an instrument is on this row
                // (OpenMPT NoteOffInstr.it row 4), never an envelope reset
                // (OpenMPT wnoteoff.it)
                reset_env = false;
                reset_susloop = false;
                if ev.ins == 0 {
                    use_ins_vol = false;
                }
            } else if !new_invalid_ins {
                // the sample sustain release carries for tone portamento
                // (Atomic Playboy, portamento_sustain.it)
                if !is_toneporta {
                    reset_env = true;
                    reset_susloop = true;
                }
                if is_toneporta {
                    let xc = &mut self.p.xc_data[chn];
                    if not_same_ins || xc.test_note(NOTE_END) {
                        xc.set(NEW_INS);
                        xc.reset_note(NOTE_ENV_RELEASE | NOTE_SUSEXIT | NOTE_FADEOUT);
                    } else {
                        if Self::is_valid_note(key - 1) {
                            xc.key_porta = key - 1;
                        }
                        key = 0;
                    }
                }
            }
        }

        if Self::is_valid_note(key - 1) && !new_invalid_ins {
            if self.p.xc_data[chn].test_note(NOTE_CUT) {
                // OpenMPT NoteOffInstr.it
                use_ins_vol = true;
            }
            key -= 1;
            self.p.xc_data[chn].key = key;
            self.p.xc_data[chn].reset_note(NOTE_END);

            if let Some(sub) = self.get_subinstrument(candidate_ins, key) {
                let transp = self.m.module.xxi[candidate_ins as usize].map[key as usize].xpo as i32;
                // clear the note delay before duplicating channels (it_note_delay_nna.it)
                self.p.xc_data[chn].delay = 0;
                note = key + sub.xpo + transp;
                let mut smp = sub.sid;
                if !self.is_valid_sample(smp) {
                    smp = -1;
                }
                let mut dct = sub.dct;
                if not_same_smp {
                    self.fix_period(chn, &sub);
                    // toneporta, even when not executed, disables NNA and DCAs for
                    // the current note (portamento_nna_sample.it, gxsmp2.it)
                    self.virt_setnna(chn as i32, XMP_INST_NNA_CUT);
                    dct = XMP_INST_DCT_OFF;
                }
                let to = self.virt_setpatch(chn as i32, candidate_ins, smp, note, key, sub.nna, dct, sub.dca);

                // random volume swing
                let rvv = sub.rvv & 0xff;
                self.p.xc_data[chn].rvv = if rvv != 0 { self.get_random(rvv.clamp(0, 100) as u32 + 1) as i32 } else { 0 };
                // random pan swing
                let rvv = (sub.rvv & 0xff00) >> 8;
                self.p.xc_data[chn].rpv = if rvv != 0 {
                    let r = rvv.clamp(0, 64);
                    self.get_random(r as u32 + 1) as i32 - r / 2
                } else {
                    0
                };

                if to < 0 {
                    return;
                }
                if to != chn as i32 {
                    self.copy_channel(to, chn);
                    self.p.xc_data[to as usize].flags = 0;
                }
                if smp >= 0 {
                    self.p.xc_data[chn].smp = smp;
                }
            } else {
                self.p.xc_data[chn].flags = 0;
                use_ins_vol = false;
            }
        }

        // after the virtual channel copy
        if (is_toneporta || retrig_ins) && self.has_quirk(QUIRK_PRENV) && ev.ins != 0 {
            self.reset_envelopes_carry(chn);
        }

        if self.is_valid_instrument(candidate_ins) {
            if self.p.xc_data[chn].ins != candidate_ins {
                // reset the envelopes when the instrument changes
                self.reset_envelopes(chn);
            }
            let rls = self.m.module.xxi[candidate_ins as usize].rls;
            let xc = &mut self.p.xc_data[chn];
            xc.ins = candidate_ins;
            xc.ins_fade = rls;
        }

        // a new instrument after the previous envelope finished (OpenMPT
        // EnvReset.it), after the channel copies for NNA; in carry mode check
        // the fadeout; the volume envelope only (it_fade_env_reset_carry.it)
        if ev.ins != 0 && self.p.xc_data[chn].test_note(NOTE_ENV_END) {
            if self.check_fadeout(chn, candidate_ins) {
                self.reset_envelope_volume(chn);
            } else {
                reset_env = false;
            }
        }

        if reset_env {
            let xc = &mut self.p.xc_data[chn];
            if ev.note != 0 {
                xc.reset_note(NOTE_ENV_RELEASE | NOTE_SUSEXIT | NOTE_FADEOUT);
            }
            // after copying to the new virtual channel (ambio.it)
            xc.fadeout = 0x10000;
        }
        if reset_susloop && ev.note != 0 {
            self.p.xc_data[chn].reset_note(NOTE_SAMPLE_RELEASE);
        }

        // OpenMPT wnoteoff.it vs noteoff3.it
        if retrig_ins && not_same_ins {
            self.p.xc_data[chn].set(NEW_INS);
            self.virt_voicepos(chn as i32, 0.0);
            let xc = &mut self.p.xc_data[chn];
            xc.fadeout = 0x10000;
            xc.reset_note(NOTE_RELEASE | NOTE_SUSEXIT | NOTE_FADEOUT);
        }

        let (ins, k) = (self.p.xc_data[chn].ins, self.p.xc_data[chn].key);
        let sub = self.get_subinstrument(ins, k);

        self.set_effect_defaults(note, sub, chn, is_toneporta);
        if let Some(s) = sub {
            if note >= 0 {
                // reset the pan (OpenMPT PanReset.it)
                Self::set_channel_pan(&mut self.p.xc_data[chn], s.pan);
                if self.p.xc_data[chn].test_note(NOTE_CUT) {
                    self.reset_envelopes(chn);
                } else if !toneporta_offset || self.has_quirk(QUIRK_PRENV) {
                    self.reset_envelopes_carry(chn);
                }
                self.p.xc_data[chn].reset_note(NOTE_CUT);
            }
        }

        // the new volume, even for a key off (OpenMPT NoteOffInstr.it row 4)
        if ev.vol != 0 && (!self.p.xc_data[chn].test_note(NOTE_CUT) || ev.ins != 0) {
            Self::set_channel_volume(&mut self.p.xc_data[chn], ev.vol as i32 - 1);
        }

        // IT: always reset the sample offset
        self.p.xc_data[chn].offset.val &= !0xffff;

        // the volume column effects after the standard ones (Storlek test 25)
        self.process_fx(chn, &ev, 0);
        self.process_fx(chn, &ev, 1);
        self.set_period(note, sub, chn, is_toneporta);

        let Some(sub) = sub else { return };

        if note >= 0 {
            self.p.xc_data[chn].note = note;
        }
        if note >= 0 || toneporta_offset {
            // an offset past the length starts at 0 (it_high_offset_memory.it),
            // or at the sample end for old FX (it_high_offset_memory_oldfx.it)
            let xc = &self.p.xc_data[chn];
            let mut off = 0;
            if xc.test(OFFSET)
                && (self.has_quirk(QUIRK_ITOLDFX)
                    || (self.is_valid_sample(xc.smp) && xc.offset.val < self.m.module.xxs[xc.smp as usize].len))
            {
                off = xc.offset.val;
            }
            self.virt_voicepos(chn as i32, off as f64);
        }

        let xc = &mut self.p.xc_data[chn];
        if use_ins_vol && !xc.test(NEW_VOL) {
            xc.volume = sub.vol;
        }
    }
}
