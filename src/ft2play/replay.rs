//! The replayer of ft2play (`pmp_main.c`, `dump_*` of `pmp_mix.c`, `setPos` of `pmplay.c`):
//! FT2.09's tick handling, bug for bug, as far as a MOD reaches it. What only an XM can use
//! is not here: the volume column, the envelopes, the auto vibrato, key off and the fade out,
//! the effects G to Z, the linear frequency table and the 16-bit samples. (A MOD sets none of
//! them: its instruments have no envelopes, its notes are 1..96 and its volume column is 0.)

use super::mixer::Mixer;
use super::module::{Cell, Module, Smp};
use super::tables::{AMIGA_PERIODS, ARP_TAB, VIB_TAB};
use std::sync::Arc;

const IS_VOL: u8 = 1;
const IS_PERIOD: u8 = 2;
const IS_NYTON: u8 = 4;
const IS_PAN: u8 = 8;
const IS_QUICKVOL: u8 = 16;

const MAX_FRQ: u16 = 32000;
const MAX_NOTES: u16 = 10 * 12 * 16 + 16;

/// `stmTyp`: a channel of the replayer.
#[derive(Clone, Copy, Default)]
struct Stm {
    status: u8,
    rel_ton_nr: i8,
    fine_tune: i8,
    instr_nr: u8,
    eff_typ: u8,
    eff: u8,
    smp_offset: u8,
    wave_ctrl: u8,
    porta_dir: u8,
    gliss_funk: u8,
    vib_pos: u8,
    trem_pos: u8,
    vib_speed: u8,
    vib_depth: u8,
    trem_speed: u8,
    trem_depth: u8,
    patt_pos: u8,
    loop_cnt: u8,
    vol_slide_speed: u8,
    f_vol_slide_up_speed: u8,
    f_vol_slide_down_speed: u8,
    f_porta_up_speed: u8,
    f_porta_down_speed: u8,
    porta_up_speed: u8,
    porta_down_speed: u8,
    ton_nr: u8,
    real_vol: u8,
    old_vol: u8,
    out_vol: u8,
    old_pan: u8,
    out_pan: u8,
    final_pan: u8,
    env_sustain_active: bool,
    out_period: u16,
    real_period: u16,
    final_period: u16,
    final_vol: u16,
    ton_typ: u16,
    want_period: u16,
    porta_speed: u16,
    fade_out_amp: u16,
    fade_out_speed: u16,
}

/// `songTyp`, the part that moves.
#[derive(Clone, Copy, Default)]
pub(super) struct Song {
    patt_del_time: u8,
    patt_del_time2: u8,
    p_break_pos: u8,
    pub p_break_flag: bool,
    pub pos_jump_flag: bool,
    pub jump_loop_flag: bool,
    pub song_pos: i16,
    patt_nr: i16,
    pub patt_pos: i16,
    patt_len: i16,
    speed: u16,
    pub tempo: u16,
    glob_vol: u16,
    pub timer: u16,
}

pub(super) struct Replay {
    m: Arc<Module>,
    pub s: Song,
    stm: [Stm; 32],
    mixer: Mixer,
    rate: i32,
    speed_val: i32,
    freq_div_factor: u32,
    dump_flag: bool,
    pub out: Vec<i16>,
}

impl Replay {
    /// ft2play after `loadMusicFromData`, `dump_Init(rate, 8, 0)` (amplification 8) and
    /// `setPos(start_pos, 0)`, with the interpolating mixer and no volume ramping, as the
    /// audacious-uade plugin sets it up.
    pub fn new(m: Arc<Module>, rate: u32, start_pos: i32) -> Replay {
        let rate = rate as i32;
        let mut r = Replay {
            m,
            s: Song { tempo: 6, speed: 125, timer: 1, glob_vol: 64, ..Song::default() },
            stm: [Stm::default(); 32],
            mixer: Mixer::new(8, true),
            rate,
            speed_val: 0,
            // bit-exact to FT2
            freq_div_factor: (65536.0 * 1712.0 / rate as f64 * 8363.0).round() as u32,
            dump_flag: false,
            out: Vec::new(),
        };
        r.stop_voices();
        r.set_pos(start_pos);
        r.speed_val = (rate * 5 / 2) / r.s.speed as i32;
        r
    }

    /// `stopVoices`.
    fn stop_voices(&mut self) {
        for ch in self.stm.iter_mut() {
            ch.ton_typ = 0;
            ch.rel_ton_nr = 0;
            ch.instr_nr = 0;
            ch.status = IS_VOL;
            ch.real_vol = 0;
            ch.out_vol = 0;
            ch.old_vol = 0;
            ch.old_pan = 128;
            ch.out_pan = 128;
            ch.final_pan = 128;
            ch.final_vol = 0;
            ch.vib_depth = 0;
        }
    }

    /// `setPos(pos, 0)`.
    fn set_pos(&mut self, pos: i32) {
        self.s.song_pos = pos as i16;
        if self.m.len > 0 && self.s.song_pos >= self.m.len as i16 {
            self.s.song_pos = self.m.len as i16 - 1;
        }
        self.s.patt_nr = self.m.song_tab[self.s.song_pos as u8 as usize] as i16;
        self.s.patt_len = 64;
        self.s.patt_pos = 0;
        if self.s.patt_pos >= self.s.patt_len {
            self.s.patt_pos = self.s.patt_len - 1;
        }
        self.s.timer = 1;
    }

    fn chn(&self) -> usize {
        self.m.chn
    }

    /// `dump_EndOfTune`.
    pub fn end_of_tune(&mut self, end_song_pos: i32) -> bool {
        let s = &self.s;
        let mut r = (self.dump_flag && s.patt_pos == 0 && s.timer == 1) || s.tempo == 0;
        // FT2 bugfix for EEx (pattern delay) on the first row of a pattern
        if s.patt_del_time2 > 0 || s.jump_loop_flag {
            r = false;
        }
        if s.song_pos as i32 == end_song_pos && s.patt_pos == 0 && s.timer == 1 {
            self.dump_flag = true;
        }
        r
    }

    /// `dump_GetFrame`: one tick, `self.out` its stereo samples.
    pub fn frame(&mut self) {
        self.main_player();
        self.update_channel_vol_pan_frq();
        let n = self.speed_val as usize;
        self.mixer.mix(&self.m.instr, self.m.chn, n, &mut self.out);
    }

    /// `getFrequenceValue` for the Amiga frequency table.
    fn frequence_value(&self, period: u16) -> u32 {
        if period == 0 { 0 } else { self.freq_div_factor / period as u32 }
    }

    /// `mix_UpdateChannelVolPanFrq`.
    fn update_channel_vol_pan_frq(&mut self) {
        for i in 0..self.chn() {
            let ch = &mut self.stm[i];
            let status = ch.status;
            ch.status = 0;
            if status == 0 {
                continue;
            }
            let vol = (status & IS_VOL != 0).then_some(ch.final_vol);
            let pan = (status & IS_PAN != 0).then_some(ch.final_pan);
            let period = ch.final_period;
            let frq = (status & IS_PERIOD != 0).then(|| self.frequence_value(period));
            self.mixer.update_channel(i, vol, pan, frq);
        }
    }

    /// `P_SetSpeed`.
    fn set_bpm(&mut self, bpm: u16) {
        let bpm = if bpm == 0 { 125 } else { bpm } as i32;
        self.speed_val = ((self.rate + self.rate) + (self.rate >> 1)) / bpm;
    }

    // ---- ft2play's pmp_main.c ----

    fn retrig_volume(ch: &mut Stm) {
        ch.real_vol = ch.old_vol;
        ch.out_vol = ch.old_vol;
        ch.out_pan = ch.old_pan;
        ch.status |= IS_VOL + IS_PAN + IS_QUICKVOL;
    }

    /// `retrigEnvelopeVibrato`: a MOD's instruments have no envelope, fade out or auto vibrato.
    fn retrig_envelope_vibrato(ch: &mut Stm) {
        if ch.wave_ctrl & 0x04 == 0 {
            ch.vib_pos = 0;
        }
        if ch.wave_ctrl & 0x40 == 0 {
            ch.trem_pos = 0;
        }
        ch.env_sustain_active = true;
        ch.fade_out_speed = 0;
        ch.fade_out_amp = 32768;
    }

    fn start_tone(&mut self, ton: u8, eff_typ: u8, eff: u8, ch: &mut Stm, c: usize) {
        let mut ton = ton;
        // if we came from a retrig, we did not check the note yet
        if ton == 0 {
            ton = ch.ton_nr;
            if ton == 0 {
                return;
            }
        }
        ch.ton_nr = ton;
        let idx = if self.m.instr.get(ch.instr_nr as usize).is_some_and(|i| i.is_some()) { ch.instr_nr as usize } else { 0 };
        let m = Arc::clone(&self.m);
        let s: &Smp = m.instr[idx].as_ref().unwrap();
        ch.rel_ton_nr = 0;
        if ton >= 10 * 12 {
            return;
        }
        ch.old_vol = s.vol;
        ch.old_pan = s.pan;
        if eff_typ == 0x0E && (eff & 0xF0) == 0x50 {
            // EFx: set finetune
            ch.fine_tune = (((eff & 0x0F) << 4) as i32 - 128) as i8;
        } else {
            ch.fine_tune = s.fine;
        }
        let tmp_ton = ((((ton - 1) as i32) << 4) + ((ch.fine_tune >> 3) as i32 + 16)) as u16;
        if tmp_ton < MAX_NOTES {
            ch.out_period = AMIGA_PERIODS[tmp_ton as usize];
            ch.real_period = ch.out_period;
        }
        ch.status |= IS_PERIOD + IS_VOL + IS_PAN + IS_NYTON + IS_QUICKVOL;
        let start = if eff_typ == 9 {
            // 9xx: set sample offset
            if eff != 0 {
                ch.smp_offset = ch.eff;
            }
            (ch.smp_offset as i32) << 8
        } else {
            0
        };
        self.mixer.start_tone(c, idx, s, start);
    }

    fn fine_porta_up(ch: &mut Stm, mut param: u8) {
        if param == 0 {
            param = ch.f_porta_up_speed;
        }
        ch.f_porta_up_speed = param;
        ch.real_period = ch.real_period.wrapping_sub((param as u16) << 2);
        if (ch.real_period as i16) < 1 {
            ch.real_period = 1;
        }
        ch.out_period = ch.real_period;
        ch.status |= IS_PERIOD;
    }

    fn fine_porta_down(ch: &mut Stm, mut param: u8) {
        if param == 0 {
            param = ch.f_porta_down_speed;
        }
        ch.f_porta_down_speed = param;
        ch.real_period = ch.real_period.wrapping_add((param as u16) << 2);
        // (FT2 bug: a signed comparison)
        if (ch.real_period as i16) > MAX_FRQ as i16 - 1 {
            ch.real_period = MAX_FRQ - 1;
        }
        ch.out_period = ch.real_period;
        ch.status |= IS_PERIOD;
    }

    fn jump_loop(&mut self, ch: &mut Stm, param: u8) {
        if param == 0 {
            ch.patt_pos = (self.s.patt_pos & 0xFF) as u8;
        } else if ch.loop_cnt == 0 {
            ch.loop_cnt = param;
            self.s.p_break_pos = ch.patt_pos;
            self.s.p_break_flag = true;
            self.s.jump_loop_flag = true;
        } else {
            ch.loop_cnt -= 1;
            if ch.loop_cnt > 0 {
                self.s.p_break_pos = ch.patt_pos;
                self.s.p_break_flag = true;
                self.s.jump_loop_flag = true;
            } else {
                self.s.jump_loop_flag = false;
            }
        }
    }

    fn vol_fine_up(ch: &mut Stm, mut param: u8) {
        if param == 0 {
            param = ch.f_vol_slide_up_speed;
        }
        ch.f_vol_slide_up_speed = param;
        ch.real_vol = ch.real_vol.wrapping_add(param);
        if ch.real_vol > 64 {
            ch.real_vol = 64;
        }
        ch.out_vol = ch.real_vol;
        ch.status |= IS_VOL;
    }

    fn vol_fine_down(ch: &mut Stm, mut param: u8) {
        if param == 0 {
            param = ch.f_vol_slide_down_speed;
        }
        ch.f_vol_slide_down_speed = param;
        ch.real_vol = ch.real_vol.wrapping_sub(param);
        if (ch.real_vol as i8) < 0 {
            ch.real_vol = 0;
        }
        ch.out_vol = ch.real_vol;
        ch.status |= IS_VOL;
    }

    /// The E effects of tick 0.
    fn e_effects_tick_zero(&mut self, ch: &mut Stm, param: u8) {
        let p = param & 0x0F;
        match param >> 4 {
            0x1 => Self::fine_porta_up(ch, p),
            0x2 => Self::fine_porta_down(ch, p),
            0x3 => ch.gliss_funk = p,
            0x4 => ch.wave_ctrl = (ch.wave_ctrl & 0xF0) | p,
            0x6 => self.jump_loop(ch, p),
            0x7 => ch.wave_ctrl = (p << 4) | (ch.wave_ctrl & 0x0F),
            0xA => Self::vol_fine_up(ch, p),
            0xB => Self::vol_fine_down(ch, p),
            0xC => {
                // EC0 only; the other cuts come on the tick
                if p == 0 {
                    ch.real_vol = 0;
                    ch.out_vol = 0;
                    ch.status |= IS_VOL + IS_QUICKVOL;
                }
            }
            0xE => {
                if self.s.patt_del_time2 == 0 {
                    self.s.patt_del_time = p + 1;
                }
            }
            _ => {}
        }
    }

    /// `checkEffects`: the tick-0 effects.
    fn check_effects(&mut self, ch: &mut Stm) {
        let param = ch.eff;
        if ch.eff_typ == 0 && param == 0 {
            return;
        }
        match ch.eff_typ {
            0x8 => {
                ch.out_pan = param;
                ch.status |= IS_PAN;
            }
            0xB => {
                // position jump
                self.s.song_pos = param as i16 - 1;
                self.s.p_break_pos = 0;
                self.s.pos_jump_flag = true;
            }
            0xC => {
                ch.out_vol = param.min(64);
                ch.real_vol = ch.out_vol;
                ch.status |= IS_VOL + IS_QUICKVOL;
            }
            0xD => {
                // pattern break
                self.s.pos_jump_flag = true;
                let p = (param >> 4) * 10 + (param & 0x0F);
                self.s.p_break_pos = if p <= 63 { p } else { 0 };
            }
            0xE => self.e_effects_tick_zero(ch, param),
            0xF => {
                if param >= 32 {
                    self.s.speed = param as u16;
                    self.set_bpm(self.s.speed);
                } else if param > 0 {
                    self.s.tempo = param as u16;
                    self.s.timer = self.s.tempo;
                }
            }
            _ => {}
        }
    }

    fn fix_tone_porta(&mut self, ch: &mut Stm, p: &Cell, inst: u8) {
        if p.ton > 0 {
            let porta_tmp = ((((p.ton as i32 - 1) + ch.rel_ton_nr as i32) << 4) + ((ch.fine_tune >> 3) as i32 + 16)) as u16;
            if porta_tmp < MAX_NOTES {
                ch.want_period = AMIGA_PERIODS[porta_tmp as usize];
                ch.porta_dir = if ch.want_period == ch.real_period {
                    0
                } else if ch.want_period > ch.real_period {
                    1
                } else {
                    2
                };
            }
        }
        if inst > 0 {
            Self::retrig_volume(ch);
            Self::retrig_envelope_vibrato(ch);
        }
    }

    fn get_new_note(&mut self, ch: &mut Stm, p: &Cell, c: usize) {
        if ch.eff_typ == 0 {
            if ch.eff != 0 {
                // an arpeggio (0xy) was running: set the period back
                ch.out_period = ch.real_period;
                ch.status |= IS_PERIOD;
            }
        } else if (ch.eff_typ == 4 || ch.eff_typ == 6) && (p.eff_typ != 4 && p.eff_typ != 6) {
            // a vibrato on the previous row ends at this one: set the period back
            ch.out_period = ch.real_period;
            ch.status |= IS_PERIOD;
        }
        ch.eff_typ = p.eff_typ;
        ch.eff = p.eff;
        ch.ton_typ = ((p.instr as u16) << 8) | p.ton as u16;
        let mut inst = p.instr;
        if inst > 0 {
            if inst <= 128 {
                ch.instr_nr = inst;
            } else {
                inst = 0;
            }
        }
        let mut check_efx = true;
        if p.eff_typ == 0x0E {
            if (0xD1..=0xDF).contains(&p.eff) {
                // ED1..EDF: note delay
                return;
            } else if p.eff == 0x90 {
                // E90: retrigger note
                check_efx = false;
            }
        }
        if check_efx {
            if p.eff_typ == 3 || p.eff_typ == 5 {
                // portamento (3xx / 5xx)
                if p.eff_typ != 5 && p.eff != 0 {
                    ch.porta_speed = (p.eff as u16) << 2;
                }
                self.fix_tone_porta(ch, p, inst);
                self.check_effects(ch);
                return;
            }
            if p.ton == 0 {
                if inst > 0 {
                    Self::retrig_volume(ch);
                    Self::retrig_envelope_vibrato(ch);
                }
                self.check_effects(ch);
                return;
            }
        }
        self.start_tone(p.ton, p.eff_typ, p.eff, ch, c);
        if inst > 0 {
            Self::retrig_volume(ch);
            Self::retrig_envelope_vibrato(ch);
        }
        self.check_effects(ch);
    }

    /// `fixaEnvelopeVibrato` for an instrument without envelopes or auto vibrato.
    fn fixa_envelope_vibrato(&mut self, ch: &mut Stm) {
        // fade out (a channel that never started a note has no sustain: its amplitude is 0)
        if !ch.env_sustain_active {
            ch.status |= IS_VOL;
            if ch.fade_out_amp >= ch.fade_out_speed {
                ch.fade_out_amp -= ch.fade_out_speed;
            } else {
                ch.fade_out_amp = 0;
                ch.fade_out_speed = 0;
            }
        }
        let mut vol = (((ch.out_vol as u32) << 4) * ch.fade_out_amp as u32) >> 16;
        vol = (vol * self.s.glob_vol as u32) >> 7;
        ch.final_vol = vol as u16; // 0..256
        ch.final_pan = ch.out_pan;
        ch.final_period = ch.out_period;
    }

    /// `relocateTon`: the period to a note number, for arpeggio and portamento (in semitone-slide mode).
    fn relocate_ton(period: u16, arp_note: u8, ch: &Stm) -> u16 {
        let fine_tune = (ch.fine_tune >> 3) as i32 + 16;
        // (FT2 bug: should have been 10*12*16; notes above B-7 have issues)
        let mut hi_period = 8 * 12 * 16;
        let mut lo_period = 0;
        for _ in 0..8 {
            let tmp = (((lo_period + hi_period) >> 1) & !15) + fine_tune;
            let mut look_up = tmp - 8;
            if look_up < 0 {
                look_up = 0;
            }
            if period >= AMIGA_PERIODS[look_up as usize] {
                hi_period = (tmp - fine_tune) & !15;
            } else {
                lo_period = (tmp - fine_tune) & !15;
            }
        }
        let mut tmp = lo_period + fine_tune + ((arp_note as i32) << 4);
        // (FT2 bug: should have been 10*12*16+16, and notice the +2 difference)
        if tmp >= (8 * 12 * 16 + 15) - 1 {
            tmp = (8 * 12 * 16 + 16) - 1;
        }
        AMIGA_PERIODS[tmp as usize]
    }

    fn vibrato2(ch: &mut Stm) {
        let mut tmp_vib = (ch.vib_pos >> 2) & 0x1F;
        match ch.wave_ctrl & 3 {
            0 => tmp_vib = VIB_TAB[tmp_vib as usize],
            1 => {
                tmp_vib <<= 3;
                if (ch.vib_pos as i8) < 0 {
                    tmp_vib = !tmp_vib;
                }
            }
            _ => tmp_vib = 255,
        }
        tmp_vib = ((tmp_vib as u32 * ch.vib_depth as u32) >> 5) as u8;
        if (ch.vib_pos as i8) < 0 {
            ch.out_period = ch.real_period.wrapping_sub(tmp_vib as u16);
        } else {
            ch.out_period = ch.real_period.wrapping_add(tmp_vib as u16);
        }
        ch.status |= IS_PERIOD;
        ch.vib_pos = ch.vib_pos.wrapping_add(ch.vib_speed);
    }

    fn arp(&self, ch: &mut Stm, param: u8) {
        // (the table's overflown bytes are what FT2 reads past its 16 ticks)
        let tick = ARP_TAB[(self.s.timer & 0xFF) as usize];
        if tick == 0 {
            ch.out_period = ch.real_period;
        } else {
            let note = if tick == 1 { param >> 4 } else { param & 0x0F };
            ch.out_period = Self::relocate_ton(ch.real_period, note, ch);
        }
        ch.status |= IS_PERIOD;
    }

    fn porta_up(ch: &mut Stm, mut param: u8) {
        if param == 0 {
            param = ch.porta_up_speed;
        }
        ch.porta_up_speed = param;
        ch.real_period = ch.real_period.wrapping_sub((param as u16) << 2);
        if (ch.real_period as i16) < 1 {
            ch.real_period = 1;
        }
        ch.out_period = ch.real_period;
        ch.status |= IS_PERIOD;
    }

    fn porta_down(ch: &mut Stm, mut param: u8) {
        if param == 0 {
            param = ch.porta_down_speed;
        }
        ch.porta_down_speed = param;
        ch.real_period = ch.real_period.wrapping_add((param as u16) << 2);
        if (ch.real_period as i16) > MAX_FRQ as i16 - 1 {
            ch.real_period = MAX_FRQ - 1;
        }
        ch.out_period = ch.real_period;
        ch.status |= IS_PERIOD;
    }

    fn tone_porta(ch: &mut Stm) {
        if ch.porta_dir == 0 {
            return;
        }
        if ch.porta_dir > 1 {
            ch.real_period = ch.real_period.wrapping_sub(ch.porta_speed);
            if (ch.real_period as i16) <= (ch.want_period as i16) {
                ch.porta_dir = 1;
                ch.real_period = ch.want_period;
            }
        } else {
            ch.real_period = ch.real_period.wrapping_add(ch.porta_speed);
            if ch.real_period >= ch.want_period {
                ch.porta_dir = 1;
                ch.real_period = ch.want_period;
            }
        }
        if ch.gliss_funk != 0 {
            // semitone-slide
            ch.out_period = Self::relocate_ton(ch.real_period, 0, ch);
        } else {
            ch.out_period = ch.real_period;
        }
        ch.status |= IS_PERIOD;
    }

    fn vibrato(ch: &mut Stm, param: u8) {
        if ch.eff > 0 {
            let d = param & 0x0F;
            if d > 0 {
                ch.vib_depth = d;
            }
            let s = (param & 0xF0) >> 2;
            if s > 0 {
                ch.vib_speed = s;
            }
        }
        Self::vibrato2(ch);
    }

    fn tremolo(ch: &mut Stm, param: u8) {
        if param > 0 {
            let d = param & 0x0F;
            if d > 0 {
                ch.trem_depth = d;
            }
            let s = (param & 0xF0) >> 2;
            if s > 0 {
                ch.trem_speed = s;
            }
        }
        let mut tmp_trem = (ch.trem_pos >> 2) & 0x1F;
        match (ch.wave_ctrl >> 4) & 3 {
            0 => tmp_trem = VIB_TAB[tmp_trem as usize],
            1 => {
                tmp_trem <<= 3;
                // (FT2 bug: should have been tremPos)
                if (ch.vib_pos as i8) < 0 {
                    tmp_trem = !tmp_trem;
                }
            }
            _ => tmp_trem = 255,
        }
        tmp_trem = ((tmp_trem as u32 * ch.trem_depth as u32) >> 6) as u8;
        let trem_vol = if (ch.trem_pos as i8) < 0 {
            (ch.real_vol as i16 - tmp_trem as i16).max(0)
        } else {
            (ch.real_vol as i16 + tmp_trem as i16).min(64)
        };
        ch.out_vol = trem_vol as u8;
        ch.status |= IS_VOL;
        ch.trem_pos = ch.trem_pos.wrapping_add(ch.trem_speed);
    }

    /// `volume`: the volume slide.
    fn volume(ch: &mut Stm, mut param: u8) {
        if param == 0 {
            param = ch.vol_slide_speed;
        }
        ch.vol_slide_speed = param;
        let mut new_vol = ch.real_vol;
        if param & 0xF0 == 0 {
            new_vol = new_vol.wrapping_sub(param);
            if (new_vol as i8) < 0 {
                new_vol = 0;
            }
        } else {
            param >>= 4;
            new_vol = new_vol.wrapping_add(param);
            if new_vol > 64 {
                new_vol = 64;
            }
        }
        ch.out_vol = new_vol;
        ch.real_vol = new_vol;
        ch.status |= IS_VOL;
    }

    /// The E effects of the ticks after the first.
    fn e_effects_tick_nonzero(&mut self, ch: &mut Stm, param: u8, c: usize) {
        let p = param & 0x0F;
        let left = self.s.tempo.wrapping_sub(self.s.timer);
        match param >> 4 {
            0x9 => {
                // retrigger note
                if p == 0 {
                    return;
                }
                if (self.s.tempo as i32 - self.s.timer as i32) % p as i32 == 0 {
                    self.start_tone(0, 0, 0, ch, c);
                    Self::retrig_envelope_vibrato(ch);
                }
            }
            0xC => {
                if left as u8 == p {
                    ch.real_vol = 0;
                    ch.out_vol = 0;
                    ch.status |= IS_VOL + IS_QUICKVOL;
                }
            }
            0xD => {
                if left as u8 == p {
                    self.start_tone((ch.ton_typ & 0xFF) as u8, 0, 0, ch, c);
                    if ch.ton_typ & 0xFF00 > 0 {
                        Self::retrig_volume(ch);
                    }
                    Self::retrig_envelope_vibrato(ch);
                }
            }
            _ => {}
        }
    }

    /// `doEffects`: the effects of the ticks after the first.
    fn do_effects(&mut self, ch: &mut Stm, c: usize) {
        if ch.eff == 0 && ch.eff_typ == 0 {
            return;
        }
        let param = ch.eff;
        match ch.eff_typ {
            0x0 => self.arp(ch, param),
            0x1 => Self::porta_up(ch, param),
            0x2 => Self::porta_down(ch, param),
            0x3 => Self::tone_porta(ch),
            0x4 => Self::vibrato(ch, param),
            0x5 => {
                Self::tone_porta(ch);
                Self::volume(ch, param);
            }
            0x6 => {
                Self::vibrato2(ch);
                Self::volume(ch, param);
            }
            0x7 => Self::tremolo(ch, param),
            0xA => Self::volume(ch, param),
            0xE => self.e_effects_tick_nonzero(ch, param, c),
            _ => {}
        }
    }

    fn get_next_pos(&mut self) {
        let s = &mut self.s;
        s.patt_pos += 1;
        if s.patt_del_time > 0 {
            s.patt_del_time2 = s.patt_del_time;
            s.patt_del_time = 0;
        }
        if s.patt_del_time2 > 0 {
            s.patt_del_time2 -= 1;
            if s.patt_del_time2 > 0 {
                s.patt_pos -= 1;
            }
        }
        if s.p_break_flag {
            s.p_break_flag = false;
            s.patt_pos = s.p_break_pos as i16;
        }
        if s.patt_pos >= s.patt_len || s.pos_jump_flag {
            s.patt_pos = s.p_break_pos as i16;
            s.p_break_pos = 0;
            s.pos_jump_flag = false;
            s.song_pos += 1;
            if s.song_pos >= self.m.len as i16 {
                s.song_pos = self.m.rep_s as i16;
            }
            s.patt_nr = self.m.song_tab[s.song_pos as u8 as usize] as i16;
            s.patt_len = 64;
            if s.patt_pos >= s.patt_len {
                s.patt_pos = 0;
            }
        }
    }

    /// `mainPlayer`: one tick.
    fn main_player(&mut self) {
        let mut tick_zero = false;
        self.s.timer = self.s.timer.wrapping_sub(1);
        if self.s.timer == 0 {
            self.s.timer = self.s.tempo;
            tick_zero = true;
        }
        let read_new_note = tick_zero && self.s.patt_del_time2 == 0;
        let chn = self.chn();
        if read_new_note {
            let m = Arc::clone(&self.m);
            let row = m.patt.get(self.s.patt_nr as usize).and_then(|p| p.as_ref());
            for c in 0..chn {
                let cell = row.and_then(|p| p.get(self.s.patt_pos as usize * chn + c)).copied().unwrap_or_default();
                let mut ch = self.stm[c];
                self.get_new_note(&mut ch, &cell, c);
                self.fixa_envelope_vibrato(&mut ch);
                self.stm[c] = ch;
            }
        } else {
            for c in 0..chn {
                let mut ch = self.stm[c];
                self.do_effects(&mut ch, c);
                self.fixa_envelope_vibrato(&mut ch);
                self.stm[c] = ch;
            }
        }
        if self.s.timer == 1 {
            self.get_next_pos();
        }
    }
}
