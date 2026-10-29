//! libxmp's effect processing (`effects.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp). The `goto`s of the C code are a
//! small state machine here (`Go`); every effect keeps libxmp's arithmetic,
//! including the 8-bit truncation of the parameter.
//!
//! Not ported yet (the hooks are marked): the format "extras" (`libxmp_extras_process_fx`,
//! used by MED and HMN; FAR is in `far_extras.rs`).

use super::module::*;
use super::player::*;

/// The `goto` targets of `libxmp_process_fx`.
#[derive(Clone, Copy)]
enum Go {
    Fx(u8),
    Arpeggio,
    FPortaUp,
    FPortaDn,
    XfPortaUp,
    XfPortaDn,
    VolSlide,
    FVslideUp,
    FVslideDn,
    SetPan,
    Retrig,
    PattDelay,
    S3mSpeed,
    S3mBpm,
    GvolSlide,
    TrkFvslide,
}

impl Context {
    fn do_toneporta(&self, xc: &mut ChannelData, note: i32) {
        let instrument = &self.m.module.xxi[xc.ins as usize];
        let mut mapped = 0i32;
        if (xc.key as u32 as usize) < XMP_MAX_KEYS {
            mapped = instrument.map[xc.key as usize].ins as i32;
        }
        if mapped >= instrument.nsm {
            mapped = 0;
        }
        let sub_xpo = instrument.sub.get(mapped as usize).map_or(0, |s| s.xpo);
        if ((note - 1) as u32 as usize) < XMP_MAX_KEYS && (xc.ins as u32 as usize) < self.m.module.ins {
            let note = note - 1;
            let mut mapped_xpo = 0;
            if (xc.key_porta as u32 as usize) < XMP_MAX_KEYS {
                mapped_xpo = instrument.map[xc.key_porta as usize].xpo as i32;
            }
            xc.porta.target = note_to_period(&self.m, note + sub_xpo + mapped_xpo, xc.finetune, xc.per_adj);
        }
        xc.porta.dir = if xc.period < xc.porta.target { 1 } else { -1 };
    }

    /// `libxmp_process_fx`: effect `fnum` (0 = first column, 1 = second) of `e`.
    pub fn process_fx(&mut self, chn: usize, e: &Event, fnum: i32) {
        let st3bugs = self.m.quirk & QUIRK_ST3BUGS != 0;
        let q = |x: i32| self.m.quirk & x != 0;
        let mut xc = self.p.xc_data[chn];
        // deferred work that touches other channels or the flow control
        let mut pair_volume: Option<(usize, i32)> = None;
        let mut flow_op: Option<(u8, i32)> = None;
        let mut reverse: Option<i32> = None;
        // FX_IT_INSTFUNC 0-6: past note actions and NNA changes, which only
        // touch the voices and the background channels
        let mut instfunc: Option<i32> = None;

        if self.m.read_event_type != ReadEvent::It {
            xc.key_porta = xc.key;
        }
        let mut note = e.note as i32;
        let (fxt, mut fxp) = if fnum == 0 { (e.fxt, e.fxp as i32) } else { (e.f2t, e.f2p as i32) };

        // EFFECT_MEMORY and friends; `fxp` is an uint8 in libxmp.
        fn mem(st3bugs: bool, p: &mut i32, m: &mut i32, volmem: &mut i32) {
            let m = if st3bugs { volmem } else { m };
            if *p == 0 {
                *p = *m & 0xff;
            } else {
                *m = *p;
            }
        }
        macro_rules! effect_memory {
            ($m:ident . $($f:ident).+) => {{
                if st3bugs {
                    if fxp == 0 { fxp = xc.vol.memory & 0xff; } else { xc.vol.memory = fxp; }
                } else if fxp == 0 {
                    fxp = $m.$($f).+ & 0xff;
                } else {
                    $m.$($f).+ = fxp;
                }
            }};
        }
        macro_rules! effect_memory_get {
            ($m:expr) => {
                if st3bugs { xc.vol.memory } else { $m }
            };
        }
        macro_rules! effect_memory_setonly {
            ($m:ident . $($f:ident).+) => {{
                if fxp == 0 { fxp = $m.$($f).+ & 0xff; } else { $m.$($f).+ = fxp; }
                if st3bugs && fxp != 0 {
                    xc.vol.memory = fxp;
                }
            }};
        }
        macro_rules! effect_memory_s3m {
            () => {
                if st3bugs {
                    if fxp == 0 { fxp = xc.vol.memory & 0xff; } else { xc.vol.memory = fxp; }
                }
            };
        }
        let _ = mem;
        let msn_ = |x: i32| (x & 0xff) >> 4;
        let lsn_ = |x: i32| x & 0x0f;

        let mut go = Go::Fx(fxt);
        loop {
            match go {
                Go::Arpeggio => {
                    if !q(QUIRK_ARPMEM) || fxp != 0 {
                        xc.arpeggio.val[0] = 0;
                        xc.arpeggio.val[1] = msn_(fxp) as i8;
                        xc.arpeggio.val[2] = lsn_(fxp) as i8;
                        xc.arpeggio.size = 3;
                    }
                }
                Go::FPortaUp => {
                    if fxp != 0 {
                        xc.flags |= FINE_BEND;
                        xc.freq.fslide = -(fxp as f64);
                    }
                }
                Go::FPortaDn => {
                    if fxp != 0 {
                        xc.flags |= FINE_BEND;
                        xc.freq.fslide = fxp as f64;
                    }
                }
                Go::XfPortaUp => {
                    xc.flags |= FINE_BEND;
                    xc.freq.fslide = -0.25 * fxp as f64;
                }
                Go::XfPortaDn => {
                    xc.flags |= FINE_BEND;
                    xc.freq.fslide = 0.25 * fxp as f64;
                }
                Go::VolSlide => {
                    if q(QUIRK_FINEFX) {
                        let (h, l) = (msn_(fxp), lsn_(fxp));
                        if l == 0xf && h != 0 {
                            xc.vol.memory = fxp;
                            fxp >>= 4;
                            go = Go::FVslideUp;
                            continue;
                        } else if h == 0xf && l != 0 {
                            xc.vol.memory = fxp;
                            fxp &= 0x0f;
                            go = Go::FVslideDn;
                            continue;
                        }
                    }
                    if fxp == 0 {
                        fxp = xc.vol.memory & 0xff;
                        if fxp != 0 {
                            continue;
                        }
                    }
                    if fxp != 0 {
                        xc.flags |= VOL_SLIDE;
                        xc.vol.memory = fxp;
                        let (h, l) = (msn_(fxp), lsn_(fxp));
                        if self.m.flow_mode == FLOW_MODE_ORPHEUS {
                            xc.vol.slide = h - l;
                        } else if q(QUIRK_VOLPDN) {
                            xc.vol.slide = if l != 0 { -l } else { h };
                        } else {
                            xc.vol.slide = if h != 0 { h } else { -l };
                        }
                    }
                    if q(QUIRK_FINEFX) && (msn_(xc.vol.memory) == 0xf || lsn_(xc.vol.memory) == 0xf) {
                        xc.flags |= FINE_VOLS;
                        xc.vol.fslide = xc.vol.slide;
                    }
                }
                Go::FVslideUp => {
                    xc.flags |= FINE_VOLS;
                    xc.vol.fslide = fxp;
                }
                Go::FVslideDn => {
                    xc.flags |= FINE_VOLS;
                    xc.vol.fslide = -fxp;
                }
                Go::SetPan => {
                    xc.pan.val = fxp;
                    xc.pan.surround = 0;
                    xc.rpv = 0;
                }
                Go::Retrig => {
                    xc.flags |= RETRIG;
                    xc.retrig.val = fxp;
                    xc.retrig.count = fxp + 1;
                    xc.retrig.type_ = 0;
                    xc.retrig.limit = if q(QUIRK_RTONCE) { 1 } else { 0 };
                }
                Go::PattDelay => {
                    if self.m.read_event_type != ReadEvent::St3 || self.p.flow.delay == 0 {
                        self.p.flow.delay = fxp;
                    }
                }
                Go::S3mSpeed => {
                    if fxp != 0 {
                        self.p.speed = fxp;
                        self.p.st26_speed = 0;
                    }
                }
                Go::S3mBpm => {
                    let min_bpm = (0.5 + self.m.time_factor * XMP_MIN_BPM as f64 / 10.0) as i32;
                    if fxp < min_bpm {
                        fxp = min_bpm;
                    }
                    self.p.bpm = fxp;
                }
                Go::GvolSlide => {
                    if fxp != 0 {
                        xc.flags |= GVOL_SLIDE;
                        xc.gvol.memory = fxp;
                        let (h, l) = (msn_(fxp), lsn_(fxp));
                        if q(QUIRK_FINEFX) && l == 0xf && h != 0 {
                            xc.gvol.slide = 0;
                            xc.gvol.fslide = h;
                        } else if q(QUIRK_FINEFX) && h == 0xf && l != 0 {
                            xc.gvol.slide = 0;
                            xc.gvol.fslide = -l;
                        } else {
                            xc.gvol.slide = if h != 0 { h } else { -l };
                            xc.gvol.fslide = 0;
                        }
                    } else {
                        fxp = xc.gvol.memory & 0xff;
                        if fxp != 0 {
                            continue;
                        }
                    }
                }
                Go::TrkFvslide => {
                    xc.flags |= TRK_FVSLIDE;
                    if fxp != 0 {
                        xc.trackvol.fslide = msn_(fxp) - lsn_(fxp);
                    }
                }
                Go::Fx(fx) => match fx {
                    FX_ARPEGGIO => {
                        go = Go::Arpeggio;
                        continue;
                    }
                    FX_S3M_ARPEGGIO => {
                        effect_memory!(xc.arpeggio.memory);
                        go = Go::Arpeggio;
                        continue;
                    }
                    FX_OKT_ARP3 => {
                        if fxp != 0 {
                            xc.arpeggio.val[0] = -msn_(fxp) as i8;
                            xc.arpeggio.val[1] = 0;
                            xc.arpeggio.val[2] = lsn_(fxp) as i8;
                            xc.arpeggio.size = 3;
                        }
                    }
                    FX_OKT_ARP4 => {
                        if fxp != 0 {
                            xc.arpeggio.val[0] = 0;
                            xc.arpeggio.val[1] = lsn_(fxp) as i8;
                            xc.arpeggio.val[2] = 0;
                            xc.arpeggio.val[3] = -msn_(fxp) as i8;
                            xc.arpeggio.size = 4;
                        }
                    }
                    FX_OKT_ARP5 => {
                        if fxp != 0 {
                            xc.arpeggio.val[0] = lsn_(fxp) as i8;
                            xc.arpeggio.val[1] = lsn_(fxp) as i8;
                            xc.arpeggio.val[2] = 0;
                            xc.arpeggio.size = 3;
                        }
                    }
                    FX_PORTA_UP | FX_PORTA_DN => {
                        let up = fx == FX_PORTA_UP;
                        if !up && q(QUIRK_FT2BUGS) {
                            effect_memory!(xc.freq.down_memory);
                        } else {
                            effect_memory!(xc.freq.memory);
                        }
                        if q(QUIRK_FINEFX) && (fnum == 0 || !q(QUIRK_ITVPOR)) {
                            match msn_(fxp) {
                                0xf => {
                                    fxp &= 0x0f;
                                    go = if up { Go::FPortaUp } else { Go::FPortaDn };
                                    continue;
                                }
                                0xe => {
                                    fxp &= 0x0f;
                                    go = if up { Go::XfPortaUp } else { Go::XfPortaDn };
                                    continue;
                                }
                                _ => {}
                            }
                        }
                        if fxp != 0 {
                            xc.flags |= PITCHBEND;
                            xc.freq.slide = if up { -fxp } else { fxp };
                            if q(QUIRK_UNISLD) {
                                xc.porta.memory = fxp;
                            }
                        }
                    }
                    FX_TONEPORTA => {
                        effect_memory_setonly!(xc.porta.memory);
                        if fxp != 0 {
                            if q(QUIRK_UNISLD) {
                                xc.freq.memory = fxp;
                            }
                            xc.porta.slide += fxp;
                        }
                        if q(QUIRK_IGSTPOR) && note == 0 && xc.porta.dir == 0 {
                            break;
                        }
                        if !self.is_valid_instrument(xc.ins) {
                            break;
                        }
                        self.do_toneporta(&mut xc, note);
                        xc.flags |= TONEPORTA;
                    }
                    FX_VIBRATO | FX_FINE_VIBRATO => {
                        effect_memory_setonly!(xc.vibrato.memory);
                        xc.flags |= VIBRATO;
                        let depth = if fx == FX_VIBRATO { lsn_(fxp) << 2 } else { lsn_(fxp) };
                        set_lfo_notzero(&mut xc.vibrato.lfo, depth, msn_(fxp));
                    }
                    FX_TONE_VSLIDE => {
                        let l = effect_memory_get!(xc.porta.memory);
                        xc.porta.slide += l;
                        if !self.is_valid_instrument(xc.ins) {
                            break;
                        }
                        self.do_toneporta(&mut xc, note);
                        xc.flags |= TONEPORTA;
                        go = Go::VolSlide;
                        continue;
                    }
                    FX_VIBRA_VSLIDE => {
                        xc.flags |= VIBRATO;
                        go = Go::VolSlide;
                        continue;
                    }
                    FX_TREMOLO => {
                        effect_memory!(xc.tremolo.memory);
                        xc.flags |= TREMOLO;
                        set_lfo_notzero(&mut xc.tremolo.lfo, lsn_(fxp), msn_(fxp));
                    }
                    FX_SETPAN => {
                        if q(QUIRK_PROTRACK) {
                            break;
                        }
                        go = Go::SetPan;
                        continue;
                    }
                    FX_OFFSET => {
                        if q(QUIRK_FT2BUGS) {
                            if fxp == 0 {
                                fxp = xc.offset.memory & 0xff;
                            }
                        } else {
                            effect_memory!(xc.offset.memory);
                        }
                        xc.flags |= OFFSET;
                        if note != 0 {
                            xc.offset.val &= !0xffff;
                            xc.offset.val |= fxp << 8;
                            xc.offset.val2 = fxp << 8;
                        }
                        if e.ins != 0 {
                            xc.offset.val2 = fxp << 8;
                        }
                    }
                    FX_VOLSLIDE => {
                        go = Go::VolSlide;
                        continue;
                    }
                    FX_VOLSLIDE_2 => {
                        if fxp != 0 {
                            xc.flags |= VOL_SLIDE_2;
                            let (h, l) = (msn_(fxp), lsn_(fxp));
                            xc.vol.slide2 = if h != 0 { h } else { -l };
                        }
                    }
                    FX_JUMP => flow_op = Some((FX_JUMP, fxp)),
                    FX_VOLSET => {
                        xc.flags |= NEW_VOL;
                        xc.volume = fxp;
                        if xc.split != 0 {
                            pair_volume = Some((xc.pair as usize, xc.volume));
                        }
                    }
                    FX_BREAK => flow_op = Some((FX_BREAK, 10 * msn_(fxp) + lsn_(fxp))),
                    FX_EXTENDED => {
                        effect_memory_s3m!();
                        let ext = (fxp >> 4) as u8;
                        fxp &= 0x0f;
                        match ext {
                            EX_FILTER => {
                                if self.m.read_event_type == ReadEvent::Mod && self.m.period_type == PeriodType::ModRng {
                                    self.p.filter = (fxp & 1 == 0) as i32;
                                }
                            }
                            EX_F_PORTA_UP => {
                                effect_memory!(xc.fine_porta.up_memory);
                                go = Go::FPortaUp;
                                continue;
                            }
                            EX_F_PORTA_DN => {
                                effect_memory!(xc.fine_porta.down_memory);
                                go = Go::FPortaDn;
                                continue;
                            }
                            EX_GLISS => {
                                if fxp != 0 {
                                    xc.set_note(NOTE_GLISSANDO);
                                } else {
                                    xc.reset_note(NOTE_GLISSANDO);
                                }
                            }
                            EX_VIBRATO_WF => xc.vibrato.lfo.type_ = fxp & 3,
                            EX_FINETUNE => {
                                if !q(QUIRK_FT2BUGS) || note > 0 {
                                    xc.finetune = ((fxp << 4) as u8 as i8) as i32;
                                }
                            }
                            EX_PATTERN_LOOP => flow_op = Some((EX_PATTERN_LOOP | 0x80, fxp)),
                            EX_TREMOLO_WF => xc.tremolo.lfo.type_ = fxp & 3,
                            EX_SETPAN => {
                                fxp = (fxp << 4) & 0xff;
                                go = Go::SetPan;
                                continue;
                            }
                            EX_RETRIG => {
                                go = Go::Retrig;
                                continue;
                            }
                            EX_F_VSLIDE_UP => {
                                effect_memory!(xc.fine_vol.up_memory);
                                go = Go::FVslideUp;
                                continue;
                            }
                            EX_F_VSLIDE_DN => {
                                effect_memory!(xc.fine_vol.down_memory);
                                go = Go::FVslideDn;
                                continue;
                            }
                            EX_CUT => {
                                xc.flags |= RETRIG;
                                xc.set_note(NOTE_CUT);
                                xc.retrig.val = fxp + 1;
                                xc.retrig.count = xc.retrig.val;
                                xc.retrig.type_ = 0x10;
                            }
                            EX_DELAY => {}
                            EX_PATT_DELAY => {
                                go = Go::PattDelay;
                                continue;
                            }
                            EX_INVLOOP => xc.invloop.speed = fxp,
                            _ => {}
                        }
                    }
                    FX_SPEED => {
                        if q(QUIRK_NOBPM) || self.p.flags & XMP_FLAGS_VBLANK != 0 || fxp < 0x20 {
                            go = Go::S3mSpeed;
                        } else {
                            go = Go::S3mBpm;
                        }
                        continue;
                    }
                    FX_FINETUNE => xc.finetune = fxp - 0x80,
                    FX_F_VSLIDE_UP => {
                        effect_memory!(xc.fine_vol.up_memory);
                        go = Go::FVslideUp;
                        continue;
                    }
                    FX_F_VSLIDE_DN => {
                        effect_memory!(xc.fine_vol.up_memory);
                        go = Go::FVslideDn;
                        continue;
                    }
                    FX_F_PORTA_UP => {
                        go = Go::FPortaUp;
                        continue;
                    }
                    FX_F_PORTA_DN => {
                        go = Go::FPortaDn;
                        continue;
                    }
                    FX_PATT_DELAY => {
                        go = Go::PattDelay;
                        continue;
                    }
                    FX_S3M_SPEED => {
                        effect_memory_s3m!();
                        go = Go::S3mSpeed;
                        continue;
                    }
                    FX_S3M_BPM => {
                        go = Go::S3mBpm;
                        continue;
                    }
                    FX_IT_BPM => {
                        if msn_(fxp) == 0 {
                            xc.flags |= TEMPO_SLIDE;
                            if lsn_(fxp) != 0 {
                                xc.tempo_slide = -lsn_(fxp);
                            }
                        } else if msn_(fxp) == 1 {
                            xc.flags |= TEMPO_SLIDE;
                            xc.tempo_slide = lsn_(fxp);
                        } else {
                            if fxp < XMP_MIN_BPM {
                                fxp = XMP_MIN_BPM;
                            }
                            self.p.bpm = fxp;
                        }
                    }
                    FX_IT_ROWDELAY => {
                        let f = &mut self.p.flow;
                        if f.rowdelay_set == 0 {
                            f.rowdelay = fxp;
                            f.rowdelay_set = ROWDELAY_ON | ROWDELAY_FIRST_FRAME;
                        }
                    }
                    FX_VSLIDE_UP_2 | FX_VSLIDE_DN_2 => {
                        effect_memory!(xc.vol.memory2);
                        xc.flags |= VOL_SLIDE_2;
                        xc.vol.slide2 = if fx == FX_VSLIDE_UP_2 { fxp } else { -fxp };
                    }
                    FX_F_VSLIDE_UP_2 | FX_F_VSLIDE_DN_2 => {
                        effect_memory!(xc.vol.memory2);
                        xc.flags |= FINE_VOLS_2;
                        xc.vol.fslide2 = if fx == FX_F_VSLIDE_UP_2 { fxp } else { -fxp };
                    }
                    FX_IT_BREAK => flow_op = Some((FX_BREAK | 0x80, fxp)),
                    FX_GLOBALVOL => {
                        self.p.gvol = if fxp > self.m.gvolbase { self.m.gvolbase } else { fxp };
                    }
                    FX_GVOL_SLIDE => {
                        go = Go::GvolSlide;
                        continue;
                    }
                    FX_KEYOFF => xc.keyoff = fxp + 1,
                    FX_ENVPOS => {
                        if q(QUIRK_FT2BUGS) {
                            if let Some(i) = self.get_instrument(xc.ins) {
                                if i.aei.flg & XMP_ENVELOPE_SUS != 0 {
                                    xc.p_idx = fxp;
                                }
                            }
                        } else {
                            xc.p_idx = fxp;
                        }
                        xc.v_idx = fxp;
                        xc.f_idx = fxp;
                    }
                    FX_PANSLIDE => {
                        effect_memory!(xc.pan.memory);
                        xc.flags |= PAN_SLIDE;
                        xc.pan.slide = lsn_(fxp) - msn_(fxp);
                    }
                    FX_PANSL_NOMEM => {
                        xc.flags |= PAN_SLIDE;
                        xc.pan.slide = lsn_(fxp) - msn_(fxp);
                    }
                    FX_IT_PANSLIDE => {
                        xc.flags |= PAN_SLIDE;
                        if fxp != 0 {
                            if msn_(fxp) == 0xf {
                                xc.pan.slide = 0;
                                xc.pan.fslide = lsn_(fxp);
                            } else if lsn_(fxp) == 0xf {
                                xc.pan.slide = 0;
                                xc.pan.fslide = -msn_(fxp);
                            } else {
                                xc.pan.slide = lsn_(fxp) - msn_(fxp);
                                xc.pan.fslide = 0;
                            }
                        }
                    }
                    FX_MULTI_RETRIG => {
                        effect_memory_s3m!();
                        if fxp != 0 {
                            xc.retrig.val = lsn_(fxp);
                            xc.retrig.type_ = msn_(fxp);
                        }
                        if note != 0 {
                            xc.retrig.count = xc.retrig.val + 1;
                        }
                        xc.retrig.limit = 0;
                        xc.flags |= RETRIG;
                    }
                    FX_TREMOR => {
                        let mut tm = xc.tremor.memory as i32;
                        if st3bugs {
                            effect_memory!(xc.vol.memory);
                        } else {
                            if fxp == 0 {
                                fxp = tm;
                            } else {
                                tm = fxp;
                            }
                            xc.tremor.memory = tm as u8;
                        }
                        xc.tremor.up = msn_(fxp) as u8;
                        xc.tremor.down = lsn_(fxp) as u8;
                        if self.m.read_event_type != ReadEvent::Ft2 {
                            if xc.tremor.up == 0 {
                                xc.tremor.up += 1;
                            }
                            if xc.tremor.down == 0 {
                                xc.tremor.down += 1;
                            }
                        }
                        xc.flags |= TREMOR;
                    }
                    FX_XF_PORTA => {
                        let h = msn_(fxp) as u8;
                        fxp &= 0x0f;
                        match h {
                            XX_XF_PORTA_UP => {
                                effect_memory!(xc.fine_porta.xf_up_memory);
                                go = Go::XfPortaUp;
                                continue;
                            }
                            XX_XF_PORTA_DN => {
                                effect_memory!(xc.fine_porta.xf_down_memory);
                                go = Go::XfPortaDn;
                                continue;
                            }
                            _ => {}
                        }
                    }
                    FX_SURROUND => xc.pan.surround = fxp,
                    FX_REVERSE => reverse = Some(fxp),
                    FX_TRK_VOL => {
                        if fxp <= self.m.volbase {
                            xc.mastervol = fxp;
                        }
                    }
                    FX_TRK_VSLIDE => {
                        if fxp == 0 {
                            fxp = xc.trackvol.memory & 0xff;
                            if fxp == 0 {
                                break;
                            }
                        }
                        if q(QUIRK_FINEFX) {
                            let (h, l) = (msn_(fxp), lsn_(fxp));
                            if h == 0xf && l != 0 {
                                xc.trackvol.memory = fxp;
                                fxp &= 0x0f;
                                go = Go::TrkFvslide;
                                continue;
                            } else if l == 0xf && h != 0 {
                                xc.trackvol.memory = fxp;
                                fxp &= 0xf0;
                                go = Go::TrkFvslide;
                                continue;
                            }
                        }
                        xc.flags |= TRK_VSLIDE;
                        if fxp != 0 {
                            let (h, l) = (msn_(fxp), lsn_(fxp));
                            xc.trackvol.memory = fxp;
                            xc.trackvol.slide = if q(QUIRK_VOLPDN) {
                                if l != 0 { -l } else { h }
                            } else if h != 0 {
                                h
                            } else {
                                -l
                            };
                        }
                    }
                    FX_TRK_FVSLIDE => {
                        go = Go::TrkFvslide;
                        continue;
                    }
                    FX_IT_INSTFUNC => match fxp {
                        0..=6 => instfunc = Some(fxp),
                        7 => xc.per_flags |= VENV_PAUSE,
                        8 => xc.per_flags &= !VENV_PAUSE,
                        9 => xc.per_flags |= PENV_PAUSE,
                        0xa => xc.per_flags &= !PENV_PAUSE,
                        0xb => xc.per_flags |= FENV_PAUSE,
                        0xc => xc.per_flags &= !FENV_PAUSE,
                        _ => {}
                    },
                    FX_FLT_CUTOFF => xc.filter.cutoff = fxp,
                    FX_FLT_RESN => xc.filter.resonance = fxp,
                    FX_MACRO_SET => xc.macro_.active = lsn_(fxp),
                    FX_MACRO => {
                        xc.flags |= MIDI_MACRO;
                        xc.macro_.val = fxp as f32;
                        xc.macro_.slide = 0.0;
                    }
                    FX_MACROSMOOTH => {
                        if self.p.speed != 0 && xc.macro_.val < 128.0 {
                            xc.flags |= MIDI_MACRO;
                            xc.macro_.target = fxp as f32;
                            xc.macro_.slide = (fxp as f32 - xc.macro_.val) / self.p.speed as f32;
                        }
                    }
                    FX_PANBRELLO => {
                        xc.flags |= PANBRELLO;
                        set_lfo_notzero(&mut xc.panbrello.lfo, lsn_(fxp) << 4, msn_(fxp));
                    }
                    FX_PANBRELLO_WF => xc.panbrello.lfo.type_ = fxp & 3,
                    FX_HIOFFSET => {
                        xc.offset.val &= 0xffff;
                        xc.offset.val |= fxp << 16;
                    }
                    FX_VOL_ADD | FX_VOL_SUB => {
                        if !self.is_valid_instrument(xc.ins) {
                            break;
                        }
                        xc.flags |= NEW_VOL;
                        let v = self.m.module.xxi[xc.ins as usize].sub[0].vol;
                        if fx == FX_VOL_ADD {
                            xc.volume = (v + fxp).min(self.m.volbase);
                        } else {
                            xc.volume = (v - fxp).max(0);
                        }
                    }
                    FX_PITCH_ADD | FX_PITCH_SUB => {
                        xc.per_flags |= TONEPORTA;
                        let base = note_to_period(&self.m, note - 1, xc.finetune, 0.0);
                        if fx == FX_PITCH_ADD {
                            xc.porta.target = base + fxp as f64;
                            xc.porta.dir = 1;
                        } else {
                            xc.porta.target = base - fxp as f64;
                            xc.porta.dir = -1;
                        }
                        xc.porta.slide = 2;
                    }
                    FX_ICE_SPEED => {
                        if fxp != 0 {
                            self.p.st26_speed = if lsn_(fxp) != 0 { (msn_(fxp) << 8) | lsn_(fxp) } else { msn_(fxp) };
                        }
                    }
                    FX_VOLSLIDE_UP | FX_VOLSLIDE_DN => {
                        let up = fx == FX_VOLSLIDE_UP;
                        if q(QUIRK_FINEFX) && msn_(fxp) == 0xf && lsn_(fxp) != 0 {
                            fxp &= 0x0f;
                            go = if up { Go::FVslideUp } else { Go::FVslideDn };
                            continue;
                        }
                        if fxp != 0 {
                            xc.vol.slide = if up { fxp } else { -fxp };
                        }
                        xc.flags |= VOL_SLIDE;
                    }
                    FX_F_VSLIDE => {
                        xc.flags |= FINE_VOLS;
                        if fxp != 0 {
                            let (h, l) = (msn_(fxp), lsn_(fxp));
                            xc.vol.fslide = if h != 0 { h } else { -l };
                        }
                    }
                    FX_IMF_FPORTA_DN | FX_IMF_FPORTA_UP => {
                        effect_memory!(xc.freq.memory);
                        if fxp != 0 {
                            xc.flags |= FINE_BEND;
                            xc.freq.fslide = if fx == FX_IMF_FPORTA_DN { 0.0625 * fxp as f64 } else { -0.0625 * fxp as f64 };
                        }
                    }
                    FX_NSLIDE_DN | FX_NSLIDE_UP | FX_NSLIDE_R_DN | FX_NSLIDE_R_UP => {
                        let r = fx == FX_NSLIDE_R_DN || fx == FX_NSLIDE_R_UP;
                        if fxp != 0 {
                            if r {
                                xc.retrig.val = msn_(fxp);
                                xc.retrig.count = msn_(fxp) + 1;
                                xc.retrig.type_ = 0;
                                xc.retrig.limit = 0;
                            }
                            xc.noteslide.slide = if fx == FX_NSLIDE_UP || fx == FX_NSLIDE_R_UP { lsn_(fxp) } else { -lsn_(fxp) };
                            xc.noteslide.speed = msn_(fxp);
                            xc.noteslide.count = msn_(fxp);
                        }
                        if r {
                            xc.flags |= RETRIG;
                        }
                        xc.flags |= NOTE_SLIDE;
                    }
                    FX_NSLIDE2_DN | FX_NSLIDE2_UP => {
                        xc.flags |= NOTE_SLIDE;
                        xc.noteslide.slide = if fx == FX_NSLIDE2_UP { fxp } else { -fxp };
                        xc.noteslide.speed = 1;
                        xc.noteslide.count = 1;
                    }
                    FX_F_NSLIDE_DN => {
                        xc.flags |= FINE_NSLIDE;
                        xc.noteslide.fslide = -fxp;
                    }
                    FX_F_NSLIDE_UP => {
                        xc.flags |= FINE_NSLIDE;
                        xc.noteslide.fslide = fxp;
                    }
                    FX_PER_VIBRATO => {
                        if lsn_(fxp) != 0 {
                            xc.per_flags |= VIBRATO;
                        } else {
                            xc.per_flags &= !VIBRATO;
                        }
                        set_lfo_notzero(&mut xc.vibrato.lfo, lsn_(fxp) << 2, msn_(fxp));
                    }
                    FX_PER_PORTA_UP | FX_PER_PORTA_DN => {
                        xc.per_flags |= PITCHBEND;
                        xc.freq.slide = if fx == FX_PER_PORTA_UP { -fxp } else { fxp };
                        xc.freq.memory = fxp;
                        if fxp == 0 {
                            xc.per_flags &= !PITCHBEND;
                        }
                    }
                    FX_PER_TPORTA => {
                        if !self.is_valid_instrument(xc.ins) {
                            break;
                        }
                        xc.per_flags |= TONEPORTA;
                        self.do_toneporta(&mut xc, note);
                        xc.porta.slide = fxp;
                        if fxp == 0 {
                            xc.per_flags &= !TONEPORTA;
                        }
                    }
                    FX_PER_VSLD_UP | FX_PER_VSLD_DN => {
                        xc.per_flags |= VOL_SLIDE;
                        xc.vol.slide = if fx == FX_PER_VSLD_UP { fxp } else { -fxp };
                        if fxp == 0 {
                            xc.per_flags &= !VOL_SLIDE;
                        }
                    }
                    FX_VIBRATO2 => {
                        xc.flags |= VIBRATO;
                        set_lfo_notzero(&mut xc.vibrato.lfo, lsn_(fxp) << 3, msn_(fxp));
                    }
                    FX_MED_RETRIG => {
                        xc.flags |= RETRIG;
                        xc.retrig.val = lsn_(fxp);
                        xc.retrig.count = lsn_(fxp) + 1;
                        xc.retrig.type_ = 0;
                        xc.retrig.limit = 0;
                    }
                    FX_SPEED_CP | FX_PER_CANCEL => {
                        if fx == FX_SPEED_CP && fxp != 0 {
                            self.p.speed = fxp;
                            self.p.st26_speed = 0;
                        }
                        xc.per_flags = 0;
                    }
                    FX_669_PORTA_UP | FX_669_PORTA_DN => {
                        xc.per_flags |= PITCHBEND;
                        xc.freq.slide = if fx == FX_669_PORTA_UP { 80 * fxp } else { -80 * fxp };
                        xc.freq.memory = fxp;
                        if fxp == 0 {
                            xc.per_flags &= !PITCHBEND;
                        }
                    }
                    FX_669_TPORTA => {
                        if !self.is_valid_instrument(xc.ins) {
                            break;
                        }
                        xc.per_flags |= TONEPORTA;
                        self.do_toneporta(&mut xc, note);
                        xc.porta.slide = 40 * fxp;
                        if fxp == 0 {
                            xc.per_flags &= !TONEPORTA;
                        }
                    }
                    FX_669_FINETUNE => xc.finetune = 80 * (fxp as u8 as i8) as i32,
                    FX_669_VIBRATO => {
                        if lsn_(fxp) != 0 {
                            xc.vibrato.lfo.type_ = 669;
                            xc.per_flags |= VIBRATO;
                        } else {
                            xc.per_flags &= !VIBRATO;
                        }
                        set_lfo_notzero(&mut xc.vibrato.lfo, 669, 1);
                    }
                    FX_ULT_TEMPO => {
                        if fxp == 0 {
                            self.p.speed = 6;
                            self.p.st26_speed = 0;
                            fxp = 125;
                        } else if fxp < 0x30 {
                            go = Go::S3mSpeed;
                            continue;
                        }
                        go = Go::S3mBpm;
                        continue;
                    }
                    FX_ULT_TPORTA => {
                        if !self.is_valid_instrument(xc.ins) {
                            break;
                        }
                        xc.per_flags |= TONEPORTA;
                        effect_memory!(xc.porta.memory);
                        // EFFECT_MEMORY(note, xc->porta.note_memory)
                        {
                            let m = if st3bugs { &mut xc.vol.memory } else { &mut xc.porta.note_memory };
                            if note == 0 {
                                note = *m & 0xff;
                            } else {
                                *m = note;
                            }
                        }
                        self.do_toneporta(&mut xc, note);
                        xc.porta.slide = fxp;
                        if fxp == 0 {
                            xc.per_flags &= !TONEPORTA;
                        }
                    }
                    FX_LINE_JUMP => flow_op = Some((FX_LINE_JUMP, fxp)),
                    FX_RETRIG => {
                        go = Go::Retrig;
                        continue;
                    }
                    _ => {
                        self.extras_process_fx(&mut xc, note as u8, fxt, fxp as u8);
                    }
                },
            }
            break;
        }

        self.p.xc_data[chn] = xc;
        if let Some((pair, v)) = pair_volume {
            self.p.xc_data[pair].volume = v;
        }
        if let Some((op, val)) = flow_op {
            let mut f = std::mem::take(&mut self.p.flow);
            match op {
                FX_JUMP => self.process_pattern_jump(&mut f, val),
                FX_LINE_JUMP => self.process_line_jump(&mut f, self.p.ord, val),
                x if x == EX_PATTERN_LOOP | 0x80 => self.process_pattern_loop(&mut f, chn, self.p.row, val),
                _ => self.process_pattern_break(&mut f, val),
            }
            self.p.flow = f;
        }
        if let Some(r) = reverse {
            self.virt_reverse(chn as i32, r);
        }
        match instfunc {
            Some(0) => self.virt_pastnote(chn as i32, VIRT_ACTION_CUT),
            Some(1) => self.virt_pastnote(chn as i32, VIRT_ACTION_OFF),
            Some(2) => self.virt_pastnote(chn as i32, VIRT_ACTION_FADE),
            Some(3) => self.virt_setnna(chn as i32, XMP_INST_NNA_CUT),
            Some(4) => self.virt_setnna(chn as i32, XMP_INST_NNA_CONT),
            Some(5) => self.virt_setnna(chn as i32, XMP_INST_NNA_OFF),
            Some(6) => self.virt_setnna(chn as i32, XMP_INST_NNA_FADE),
            _ => {}
        }
    }
}

fn set_lfo_notzero(lfo: &mut Lfo, depth: i32, rate: i32) {
    if depth != 0 {
        lfo.depth = depth;
    }
    if rate != 0 {
        lfo.rate = rate;
    }
}
