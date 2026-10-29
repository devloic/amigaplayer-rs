//! libxmp's module scan (`scan.c`): finds the sequences (subsongs), their
//! durations, their end points and the per-order replay state the player
//! restarts from.
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//!
//! Not ported yet: the FAR tempo handling (`far_extras`), which comes with the
//! FAR loader.

use super::module::*;
use super::player::*;

const VBLANK_TIME_THRESHOLD: f64 = 480000.0;

impl Context {
    fn scan_module(&mut self, ep: i32, chain: i32) -> f64 {
        let row_limit = if self.m.read_event_type == ReadEvent::Med { 3200 } else { 1024 };
        let len = self.m.module.len as i32;
        if len == 0 {
            return 0.0;
        }
        for i in 0..len as usize {
            let pat = self.m.module.xxo[i] as usize;
            let n = if pat >= self.m.module.pat { 1 } else { self.m.module.xxp[pat].rows.max(1) };
            let c = &mut self.m.scan_cnt[i];
            for v in c.iter_mut().take(n) {
                *v = 0;
            }
        }
        let mut f = FlowControl { loop_: vec![PatternLoop::default(); XMP_MAX_CHANNELS], ..Default::default() };
        f.loop_dest = -1;
        f.loop_param = -1;
        f.loop_start = -1;
        f.jump = -1;

        let tf = self.m.time_factor;
        let mut gvl = self.m.module.gvl;
        let mut bpm = self.m.module.bpm;
        let mut speed = self.m.module.spd;
        let base_time = self.m.rrate as i32;
        let mut st26_speed = 0;
        let has_marker = self.m.has_quirk(QUIRK_MARKER);
        let mut ord = ep - 1;
        let (mut gvol_memory, mut row_count, mut row_count_total, mut frame_count) = (0, 0, 0, 0);
        let (mut orders_since_last_valid, mut any_valid) = (0, false);
        let (mut start_time, mut time) = (0.0f64, 0.0f64);
        let mut inside_loop = false;
        let mut pdelay = 0;
        let mut row;
        let chn_n = self.m.module.chn;

        'end_module: {
            loop {
                if orders_since_last_valid > 512 {
                    break;
                }
                orders_since_last_valid += 1;
                ord += 1;
                if ord as u32 >= len as u32 {
                    let md = &self.m.module;
                    if md.rst > md.len || md.xxo[md.rst] as usize >= md.pat {
                        ord = ep;
                    } else if self.get_sequence(md.rst as i32) == chain {
                        ord = md.rst as i32;
                    } else {
                        ord = ep;
                    }
                    let pat = self.m.module.xxo[ord as usize];
                    if has_marker && pat == XMP_MARK_END {
                        break;
                    }
                }
                let pat = self.m.module.xxo[ord as usize] as usize;
                if ep != 0 && self.p.sequence_control[ord as usize] != NO_SEQUENCE {
                    if pat >= self.m.module.pat {
                        if has_marker && pat as u8 == XMP_MARK_END {
                            ord = len;
                        }
                        continue;
                    }
                    break;
                }
                self.p.sequence_control[ord as usize] = chain as u8;
                if pat >= self.m.module.pat {
                    if has_marker && pat as u8 == XMP_MARK_END {
                        ord = len;
                    }
                    continue;
                }
                let rows = self.m.module.xxp[pat].rows as i32;
                if f.jumpline >= rows {
                    f.jumpline = 0;
                }
                if self.m.has_flow_mode(FLOW_LOOP_PATTERN_RESET) {
                    f.loop_start = -1;
                    f.loop_count = 0;
                    for l in f.loop_.iter_mut().take(chn_n) {
                        *l = PatternLoop::default();
                    }
                }
                if self.m.scan_cnt[ord as usize][f.jumpline as usize] != 0 && !inside_loop {
                    break;
                }
                {
                    let info = &mut self.m.xxo_info[ord as usize];
                    if info.time < 0.0 {
                        info.gvl = gvl;
                        info.bpm = bpm;
                        info.speed = speed;
                        info.time = time + tf * frame_count as f64 * base_time as f64 / bpm as f64;
                        info.st26_speed = st26_speed;
                        info.start_row = f.jumpline;
                    }
                    if info.start_row == 0 && ord != 0 && ord == ep {
                        start_time = time + tf * frame_count as f64 * base_time as f64 / bpm as f64;
                    }
                }

                let mut last_row = rows;
                row = f.jumpline;
                f.jumpline = 0;
                while row < last_row {
                    if bpm < XMP_MIN_BPM {
                        bpm = XMP_MIN_BPM;
                    }
                    if row_count_total > row_limit {
                        break 'end_module;
                    }
                    if f.loop_active_num == 0 && self.m.scan_cnt[ord as usize][row as usize] != 0 {
                        row_count -= 1;
                        break 'end_module;
                    }
                    let c = &mut self.m.scan_cnt[ord as usize][row as usize];
                    *c = c.wrapping_add(1);
                    orders_since_last_valid = 0;
                    any_valid = true;
                    if self.m.scan_cnt[ord as usize][row as usize] == 0 {
                        break 'end_module;
                    }
                    pdelay = 0;

                    for chn in 0..chn_n {
                        let track = &self.m.module.xxt[self.m.module.xxp[pat].index[chn]];
                        if row as usize >= track.rows {
                            continue;
                        }
                        let ev = track.event[row as usize];
                        let (mut f1, p1, mut f2, p2) = (ev.fxt, ev.fxp as i32, ev.f2t, ev.f2p as i32);
                        if f1 == 0 && f2 == 0 {
                            continue;
                        }
                        if f1 == FX_GLOBALVOL || f2 == FX_GLOBALVOL {
                            gvl = if f1 == FX_GLOBALVOL { p1 } else { p2 };
                            gvl = if gvl > self.m.gvolbase { self.m.gvolbase } else { gvl.max(0) };
                        }
                        if f1 == FX_GVOL_SLIDE || f2 == FX_GVOL_SLIDE {
                            let mut parm = if f1 == FX_GVOL_SLIDE { p1 } else { p2 };
                            if parm == 0 {
                                parm = gvol_memory;
                            }
                            if parm != 0 {
                                gvol_memory = parm;
                                let (h, l) = (parm >> 4, parm & 0xf);
                                let vsall = self.m.quirk & QUIRK_VSALL != 0;
                                if self.m.has_quirk(QUIRK_FINEFX) && l == 0xf && h != 0 {
                                    gvl += h;
                                } else if self.m.has_quirk(QUIRK_FINEFX) && h == 0xf && l != 0 {
                                    gvl -= l;
                                } else if vsall {
                                    gvl += (h - l) * speed;
                                } else {
                                    gvl += (h - l) * (speed - 1);
                                }
                            }
                        }
                        for i in 0..2 {
                            let parm = if i != 0 { p1 } else { p2 };
                            if (if i != 0 { f1 } else { f2 }) != FX_SPEED || parm == 0 {
                                continue;
                            }
                            frame_count += row_count * speed;
                            row_count = 0;
                            if self.m.has_quirk(QUIRK_NOBPM) || self.p.flags & XMP_FLAGS_VBLANK != 0 || parm < 0x20 {
                                speed = parm;
                                st26_speed = 0;
                            } else {
                                time += tf * frame_count as f64 * base_time as f64 / bpm as f64;
                                frame_count = 0;
                                bpm = parm;
                            }
                        }
                        if f1 == FX_SPEED_CP {
                            f1 = FX_S3M_SPEED;
                        }
                        if f2 == FX_SPEED_CP {
                            f2 = FX_S3M_SPEED;
                        }
                        if f1 == FX_ICE_SPEED && p1 != 0 {
                            st26_speed = if p1 & 0xf != 0 { ((p1 >> 4) << 8) | (p1 & 0xf) } else { p1 >> 4 };
                        }
                        // FAR tempo: not ported yet (comes with the FAR loader)
                        if f1 == FX_ULT_TEMPO || f2 == FX_ULT_TEMPO {
                            let (mut parm, mut parm2) = (0, 0);
                            for (ff, pp) in [(f2, p2), (f1, p1)] {
                                if ff == FX_ULT_TEMPO {
                                    if pp == 0 {
                                        parm = 6;
                                        parm2 = 125;
                                    } else if pp < 0x30 {
                                        parm = pp;
                                    } else {
                                        parm2 = pp;
                                    }
                                }
                            }
                            frame_count += row_count * speed;
                            row_count = 0;
                            if parm > 0 {
                                speed = parm;
                                st26_speed = 0;
                            }
                            if parm2 > 0 {
                                time += tf * frame_count as f64 * base_time as f64 / bpm as f64;
                                frame_count = 0;
                                bpm = parm2;
                            }
                        }
                        if (f1 == FX_S3M_SPEED && p1 != 0) || (f2 == FX_S3M_SPEED && p2 != 0) {
                            let parm = if f1 == FX_S3M_SPEED { p1 } else { p2 };
                            if parm > 0 {
                                frame_count += row_count * speed;
                                row_count = 0;
                                speed = parm;
                                st26_speed = 0;
                            }
                        }
                        if (f1 == FX_S3M_BPM && p1 != 0) || (f2 == FX_S3M_BPM && p2 != 0) {
                            let parm = if f1 == FX_S3M_BPM { p1 } else { p2 };
                            if parm >= XMP_MIN_BPM {
                                frame_count += row_count * speed;
                                row_count = 0;
                                time += tf * frame_count as f64 * base_time as f64 / bpm as f64;
                                frame_count = 0;
                                bpm = parm;
                            }
                        }
                        if (f1 == FX_IT_BPM && p1 != 0) || (f2 == FX_IT_BPM && p2 != 0) {
                            let parm = if f1 == FX_IT_BPM { p1 } else { p2 };
                            frame_count += row_count * speed;
                            row_count = 0;
                            time += tf * frame_count as f64 * base_time as f64 / bpm as f64;
                            frame_count = 0;
                            if parm >> 4 == 0 || parm >> 4 == 1 {
                                let up = parm >> 4 == 1;
                                time += tf * base_time as f64 / bpm as f64;
                                for _ in 1..speed {
                                    if up {
                                        bpm = (bpm + (parm & 0xf)).min(0xff);
                                    } else {
                                        bpm = (bpm - (parm & 0xf)).max(0x20);
                                    }
                                    time += tf * base_time as f64 / bpm as f64;
                                }
                                time -= tf * speed as f64 * base_time as f64 / bpm as f64;
                            } else {
                                bpm = parm;
                            }
                        }
                        if f1 == FX_IT_ROWDELAY {
                            let c = &mut self.m.scan_cnt[ord as usize][row as usize];
                            *c = (*c as i32 + (p1 & 0x0f)).min(255) as u8;
                            frame_count += (p1 & 0x0f) * speed;
                        }
                        if f1 == FX_IT_BREAK || f2 == FX_IT_BREAK {
                            let parm = if f1 == FX_IT_BREAK { p1 } else { p2 };
                            self.process_pattern_break(&mut f, parm);
                        }
                        if f1 == FX_JUMP || f2 == FX_JUMP {
                            self.process_pattern_jump(&mut f, if f1 == FX_JUMP { p1 } else { p2 });
                            if f.pbreak != 0 {
                                inside_loop = false;
                            }
                        }
                        if f1 == FX_BREAK || f2 == FX_BREAK {
                            let parm = if f1 == FX_BREAK { p1 } else { p2 };
                            self.process_pattern_break(&mut f, 10 * (parm >> 4) + (parm & 0xf));
                        }
                        if f1 == FX_LINE_JUMP || f2 == FX_LINE_JUMP {
                            self.process_line_jump(&mut f, ord, if f1 == FX_LINE_JUMP { p1 } else { p2 });
                        }
                        if f1 == FX_EXTENDED || f2 == FX_EXTENDED {
                            let parm = if f1 == FX_EXTENDED { p1 } else { p2 };
                            if (parm >> 4) as u8 == EX_PATT_DELAY && (self.m.read_event_type != ReadEvent::St3 || pdelay == 0) {
                                pdelay = parm & 0x0f;
                            }
                            if (parm >> 4) as u8 == EX_PATTERN_LOOP {
                                self.process_pattern_loop(&mut f, chn, row, parm & 0xf);
                                if parm & 0xf > 0 && f.loop_dest < 0 {
                                    inside_loop = false;
                                } else if parm & 0xf == 0 {
                                    inside_loop = true;
                                }
                            }
                        }
                        if f1 == FX_PATT_DELAY {
                            pdelay += p1;
                        }
                        if f2 == FX_PATT_DELAY {
                            pdelay += p2;
                        }
                    }

                    if pdelay > 0 {
                        frame_count += pdelay * speed;
                    }
                    f.loop_param = -1;
                    if f.loop_dest >= 0 {
                        row = f.loop_dest - 1;
                        f.loop_dest = -1;
                    }
                    if f.pbreak != 0 {
                        f.pbreak = 0;
                        last_row = 0;
                    }
                    if st26_speed != 0 {
                        frame_count += row_count * speed;
                        row_count = 0;
                        if st26_speed & 0x10000 != 0 {
                            speed = (st26_speed & 0xff00) >> 8;
                        } else {
                            speed = st26_speed & 0xff;
                        }
                        st26_speed ^= 0x10000;
                    }
                    row += 1;
                    row_count += 1;
                    row_count_total += 1;
                }
                // pdelay is that of the last row scanned
                if f.jumpline != 0 && pdelay != 0 {
                    f.jumpline += 1;
                }
                if f.jump >= 0 {
                    ord = f.jump - 1;
                    f.jump = -1;
                }
                frame_count += row_count * speed;
                row_count_total = 0;
                row_count = 0;
            }
            row = f.jumpline;
        }

        if !any_valid {
            return -1.0;
        }
        let pat = self.m.module.xxo[ord as usize] as usize;
        if pat >= self.m.module.pat || row >= self.m.module.xxp[pat].rows as i32 {
            row = 0;
        }
        let num = self.m.scan_cnt[ord as usize].get(row as usize).copied().unwrap_or(0) as i32;
        let sc = &mut self.p.scan[chain as usize];
        sc.num = num;
        sc.row = row;
        sc.ord = ord;
        time -= start_time;
        frame_count += row_count * speed;
        time + tf * frame_count as f64 * base_time as f64 / bpm as f64
    }

    fn reset_scan_data(&mut self) {
        for o in self.m.xxo_info.iter_mut() {
            o.time = -1.0;
        }
        self.p.sequence_control = [NO_SEQUENCE; XMP_MAX_MOD_LENGTH];
        self.p.scan[0] = ScanData::default();
    }

    fn compare_vblank_scan(&mut self) {
        let scan_backup = self.p.scan[0];
        let info_backup = self.m.xxo_info;
        let ctrl_backup = self.p.sequence_control;
        self.reset_scan_data();
        self.m.quirk ^= QUIRK_NOBPM;
        self.p.scan[0].time = self.scan_module(0, 0);
        if self.p.scan[0].time >= scan_backup.time {
            self.m.quirk ^= QUIRK_NOBPM;
            self.p.scan[0] = scan_backup;
            self.m.xxo_info = info_backup;
            self.p.sequence_control = ctrl_backup;
        }
    }

    /// `libxmp_scan_sequences`.
    pub fn scan_sequences(&mut self) {
        let len = self.m.module.len;
        self.p.scan = vec![ScanData::default(); len.max(1)];
        self.reset_scan_data();
        let mut temp_ep = [0usize; XMP_MAX_MOD_LENGTH];
        self.p.scan[0].time = self.scan_module(0, 0);
        let mut seq = 1usize;
        if self.m.compare_vblank && self.p.flags & XMP_FLAGS_VBLANK == 0 && self.p.scan[0].time >= VBLANK_TIME_THRESHOLD {
            self.compare_vblank_scan();
        }
        loop {
            let i = (0..len).find(|&i| self.p.sequence_control[i] == NO_SEQUENCE).unwrap_or(len);
            if i != len && seq < MAX_SEQUENCES {
                temp_ep[seq] = i;
                self.p.scan[seq].time = self.scan_module(i as i32, seq as i32);
                if self.p.scan[seq].time > 0.0 {
                    seq += 1;
                }
            } else {
                break;
            }
        }
        if seq < len {
            self.p.scan.truncate(seq);
        }
        self.m.num_sequences = seq;
        self.p.current_time *= self.m.time_factor / self.p.scan_time_factor;
        self.p.scan_time_factor = self.m.time_factor;
        for i in 0..MAX_SEQUENCES {
            if i < seq {
                let time = self.p.scan[i].time.clamp(0.0, i32::MAX as f64);
                self.m.seq_data[i].entry_point = temp_ep[i];
                self.m.seq_data[i].duration = time as i32;
            } else {
                self.m.seq_data[i].entry_point = 0;
                self.m.seq_data[i].duration = 0;
            }
        }
        for i in 0..len {
            if self.p.sequence_control[i] as usize >= seq {
                self.p.sequence_control[i] = if i > 0 { self.p.sequence_control[i - 1] } else { 0 };
            }
        }
    }
}
