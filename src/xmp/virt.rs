//! libxmp's virtual channels (`virtual.c`): maps tracks (and, for IT-style new
//! note actions, background channels) onto mixer voices.
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//! The IT parts (duplicate check, `libxmp_virt_setnna`, `libxmp_virt_pastnote`)
//! only act for the loaders that set QUIRK_VIRTUAL or pass a duplicate check.

use super::mixer::*;
use super::module::*;
use super::player::*;

const FREE: i32 = -1;

fn do_virt_resetvoice(vi: &mut MixerVoice) {
    let (l, r, flags) = (vi.sleft, vi.sright, vi.flags & ANTICLICK);
    *vi = MixerVoice::default();
    vi.sleft = l;
    vi.sright = r;
    vi.flags = flags;
    vi.chn = FREE;
    vi.root = FREE;
}

impl Context {
    pub fn virt_getroot(&self, chn: i32) -> i32 {
        let voc = self.p.virt.virt_channel[chn as usize].map;
        if voc < 0 {
            return -1;
        }
        self.p.virt.voice_array[voc as usize].root
    }

    pub fn virt_resetvoice(&mut self, voc: i32, mute: bool) {
        if voc as u32 >= self.p.virt.maxvoc as u32 {
            return;
        }
        if mute {
            self.mixer_setvol(voc as usize, 0);
        }
        let v = &mut self.p.virt;
        v.virt_used -= 1;
        let (root, chn) = (v.voice_array[voc as usize].root, v.voice_array[voc as usize].chn);
        if let Some(c) = v.virt_channel.get_mut(root as usize) {
            c.count -= 1;
        }
        if let Some(c) = v.virt_channel.get_mut(chn as usize) {
            c.map = FREE;
        }
        do_virt_resetvoice(&mut v.voice_array[voc as usize]);
    }

    /// `libxmp_virt_on`.
    pub fn virt_on(&mut self, num: i32) {
        let numvoc = self.s.numvoc;
        let numvoices = |n: i32| if n > numvoc || n < 0 { numvoc } else { n };
        let v = &mut self.p.virt;
        v.num_tracks = num;
        let mut num = numvoices(-1);
        v.virt_channels = v.num_tracks;
        if self.m.quirk & QUIRK_VIRTUAL != 0 {
            v.virt_channels += num;
        } else if num > v.virt_channels {
            num = v.virt_channels;
        }
        v.maxvoc = numvoices(num);
        v.voice_array = vec![MixerVoice { chn: FREE, root: FREE, ..Default::default() }; v.maxvoc as usize];
        v.virt_channel = vec![VirtChannel { count: 0, map: FREE }; v.virt_channels as usize];
        v.virt_used = 0;
    }

    /// `libxmp_virt_reset`.
    pub fn virt_reset(&mut self) {
        let v = &mut self.p.virt;
        if v.virt_channels < 1 {
            return;
        }
        for vi in v.voice_array.iter_mut() {
            do_virt_resetvoice(vi);
        }
        for c in v.virt_channel.iter_mut() {
            c.map = FREE;
            c.count = 0;
        }
        v.virt_used = 0;
    }

    fn free_voice(&mut self) -> i32 {
        let v = &mut self.p.virt;
        let mut num = FREE;
        let mut vol = i32::MAX;
        for (i, vi) in v.voice_array.iter().enumerate() {
            if vi.chn >= v.num_tracks && vi.vol < vol {
                num = i as i32;
                vol = vi.vol;
            }
        }
        if num >= 0 {
            let (chn, root) = (v.voice_array[num as usize].chn, v.voice_array[num as usize].root);
            v.virt_channel[chn as usize].map = FREE;
            v.virt_channel[root as usize].count -= 1;
            v.virt_used -= 1;
        }
        num
    }

    fn alloc_voice(&mut self, chn: i32) -> i32 {
        let mut i = self.p.virt.voice_array.iter().position(|vi| vi.chn == FREE).map_or(-1, |i| i as i32);
        if i < 0 {
            i = self.free_voice();
        }
        if i >= 0 {
            let v = &mut self.p.virt;
            v.virt_channel[chn as usize].count += 1;
            v.virt_used += 1;
            v.voice_array[i as usize].chn = chn;
            v.voice_array[i as usize].root = chn;
            v.virt_channel[chn as usize].map = i;
        }
        i
    }

    fn map_virt_channel(&self, chn: i32) -> i32 {
        let v = &self.p.virt;
        if chn as u32 >= v.virt_channels as u32 {
            return -1;
        }
        let voc = v.virt_channel[chn as usize].map;
        if voc as u32 >= v.maxvoc as u32 {
            return -1;
        }
        voc
    }

    pub fn virt_mapchannel(&self, chn: i32) -> i32 {
        self.map_virt_channel(chn)
    }

    pub fn virt_resetchannel(&mut self, chn: i32) {
        let voc = self.map_virt_channel(chn);
        if voc >= 0 {
            self.virt_resetvoice(voc, true);
        }
    }

    pub fn virt_setvol(&mut self, chn: i32, mut vol: i32) {
        let voc = self.map_virt_channel(chn);
        if voc < 0 {
            return;
        }
        let root = self.p.virt.voice_array[voc as usize].root;
        if (root as usize) < XMP_MAX_CHANNELS && self.p.channel_mute[root as usize] {
            vol = 0;
        }
        self.mixer_setvol(voc as usize, vol);
        if vol == 0 && chn >= self.p.virt.num_tracks {
            self.virt_resetvoice(voc, true);
        }
    }

    pub fn virt_release(&mut self, chn: i32, rel: bool) {
        let voc = self.map_virt_channel(chn);
        if voc >= 0 {
            self.mixer_release(voc as usize, rel);
        }
    }

    pub fn virt_reverse(&mut self, chn: i32, rev: i32) {
        let voc = self.map_virt_channel(chn);
        if voc >= 0 {
            self.mixer_reverse(voc as usize, rev != 0);
        }
    }

    pub fn virt_setpan(&mut self, chn: i32, pan: i32) {
        let voc = self.map_virt_channel(chn);
        if voc >= 0 {
            self.p.virt.voice_array[voc as usize].pan = pan;
        }
    }

    /// `libxmp_virt_seteffect`.
    pub fn virt_seteffect(&mut self, chn: i32, type_: i32, val: i32) {
        let voc = self.map_virt_channel(chn);
        if voc >= 0 {
            self.mixer_seteffect(voc as usize, type_, val);
        }
    }

    pub fn virt_getvoicepos(&self, chn: i32) -> f64 {
        let voc = self.map_virt_channel(chn);
        if voc < 0 {
            return -1.0;
        }
        self.p.virt.voice_array[voc as usize].pos
    }

    pub fn virt_setnote(&mut self, chn: i32, note: i32) {
        let voc = self.map_virt_channel(chn);
        if voc >= 0 {
            self.mixer_setnote(voc as usize, note);
        }
    }

    /// `libxmp_virt_setnna`: the new note action of the voice playing `chn`.
    pub fn virt_setnna(&mut self, chn: i32, nna: i32) {
        if self.m.quirk & QUIRK_VIRTUAL == 0 {
            return;
        }
        let voc = self.map_virt_channel(chn);
        if voc >= 0 {
            self.p.virt.voice_array[voc as usize].act = nna;
        }
    }

    /// `check_dct`: IT's duplicate check of voice `i` against a new note on `chn`.
    #[allow(clippy::too_many_arguments)]
    fn check_dct(&mut self, i: usize, chn: i32, ins: i32, smp: i32, key: i32, nna: i32, dct: i32, dca: i32) {
        let voc = self.p.virt.virt_channel[chn as usize].map;
        let vi = &mut self.p.virt.voice_array[i];
        if vi.root == chn && vi.ins == ins {
            if nna == XMP_INST_NNA_CUT {
                self.virt_resetvoice(i as i32, true);
                return;
            }
            vi.act = nna;
            if dct == XMP_INST_DCT_INST || (dct == XMP_INST_DCT_SMP && vi.smp == smp) || (dct == XMP_INST_DCT_NOTE && vi.key == key) {
                if nna == XMP_INST_NNA_OFF && dca == XMP_INST_DCA_FADE {
                    vi.act = VIRT_ACTION_OFF;
                } else if dca != 0 {
                    if i as i32 != voc || vi.act != 0 {
                        vi.act = dca;
                    }
                } else {
                    self.virt_resetvoice(i as i32, true);
                }
            }
        }
    }

    /// `libxmp_virt_pastnote`: a past note action on the background voices of `chn`.
    pub fn virt_pastnote(&mut self, chn: i32, act: i32) {
        for c in self.p.virt.num_tracks..self.p.virt.virt_channels {
            let voc = self.map_virt_channel(c);
            if voc < 0 {
                continue;
            }
            if self.p.virt.voice_array[voc as usize].root == chn {
                match act {
                    VIRT_ACTION_CUT => self.virt_resetvoice(voc, true),
                    VIRT_ACTION_OFF => self.p.xc_data[c as usize].set_note(NOTE_RELEASE),
                    VIRT_ACTION_FADE => self.p.xc_data[c as usize].set_note(NOTE_FADEOUT),
                    _ => {}
                }
            }
        }
    }

    /// `libxmp_virt_setpatch`.
    #[allow(clippy::too_many_arguments)]
    pub fn virt_setpatch(&mut self, mut chn: i32, ins: i32, mut smp: i32, note: i32, key: i32, nna: i32, dct: i32, dca: i32) -> i32 {
        if chn as u32 >= self.p.virt.virt_channels as u32 {
            return -1;
        }
        if ins < 0 {
            smp = -1;
        }
        if dct != 0 {
            for i in 0..self.p.virt.maxvoc as usize {
                self.check_dct(i, chn, ins, smp, key, nna, dct, dca);
            }
        }
        let mut voc = self.p.virt.virt_channel[chn as usize].map;
        if voc > FREE {
            if self.p.virt.voice_array[voc as usize].act != 0 {
                let vfree = self.alloc_voice(chn);
                if vfree < 0 {
                    return -1;
                }
                let v = &mut self.p.virt;
                chn = v.num_tracks;
                loop {
                    if chn >= v.virt_channels {
                        break;
                    }
                    let mapped = v.virt_channel[chn as usize].map > FREE;
                    chn += 1;
                    if !mapped {
                        break;
                    }
                }
                chn -= 1;
                v.voice_array[voc as usize].chn = chn;
                v.virt_channel[chn as usize].map = voc;
                voc = vfree;
            }
        } else {
            voc = self.alloc_voice(chn);
            if voc < 0 {
                return -1;
            }
        }
        if smp < 0 {
            self.virt_resetvoice(voc, true);
            return chn;
        }
        self.mixer_setpatch(voc as usize, smp, true);
        self.mixer_setnote(voc as usize, note);
        let vi = &mut self.p.virt.voice_array[voc as usize];
        vi.ins = ins;
        vi.act = nna;
        vi.key = key;
        chn
    }

    /// `libxmp_virt_queuepatch` (ProTracker instrument swap).
    pub fn virt_queuepatch(&mut self, chn: i32, ins: i32, mut smp: i32, note: i32) -> i32 {
        if chn as u32 >= self.p.virt.virt_channels as u32 {
            return -1;
        }
        if ins < 0 {
            smp = -1;
        }
        let voc = self.p.virt.virt_channel[chn as usize].map;
        if voc > FREE {
            self.mixer_queuepatch(voc as usize, smp);
            if ins >= 0 {
                self.p.virt.voice_array[voc as usize].ins = ins;
            }
            return chn;
        }
        if smp < 0 {
            return -1;
        }
        self.virt_setpatch(chn, ins, smp, note, 0, 0, 0, 0)
    }

    pub fn virt_setperiod(&mut self, chn: i32, period: f64) {
        let voc = self.map_virt_channel(chn);
        if voc >= 0 {
            self.p.virt.voice_array[voc as usize].period = period;
        }
    }

    pub fn virt_voicepos(&mut self, chn: i32, pos: f64) {
        let voc = self.map_virt_channel(chn);
        if voc >= 0 {
            self.mixer_voicepos(voc as usize, pos, true);
        }
    }

    pub fn virt_cstat(&self, chn: i32) -> i32 {
        let voc = self.map_virt_channel(chn);
        if voc < 0 {
            return VIRT_INVALID;
        }
        if chn < self.p.virt.num_tracks {
            return VIRT_ACTIVE;
        }
        self.p.virt.voice_array[voc as usize].act
    }
}
