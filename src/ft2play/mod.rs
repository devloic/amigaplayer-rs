//! Multi-channel MODs on ft2play, a port of 8bitbubsy's ft2play (Copyright (c) 2020-2024, Olav
//! Sørensen, BSD 3-Clause licence: see LICENSE-ft2play), the C port of FastTracker 2.09's
//! replayer and its SB16 / WAV-writer mixer, as the audacious-uade plugin's `ft2play` backend
//! drives it (`player_ft2play.cc`, SPDX LGPL-2.1-or-later, Copyright (C) 2024-2025 Matti
//! Tiainen, compatible with this crate's GPL-2.0-or-later).
//!
//! The aim is that the samples are the plugin's: the same loader (`loadMusicMOD`, with its
//! memory file's end-of-file rule), the same tick handling and mixer, 8 for the amplification,
//! linear interpolation, no volume ramping (an XM of version 1.04 has it, a MOD does not), the
//! master volume at its maximum, and the wrapper's way of rendering: one tick of
//! `(5 * rate / 2) / BPM` frames per call, a subsong starting at its position as `setPos`
//! leaves it, and the wrapper's song end (a song position and row seen twice, or ft2play's own
//! end of tune). The output is the mixer's 16-bit samples unchanged.
//!
//! Which files: the tags the multichannel replayer of [`crate::replay::multimod`] counts
//! ft2play subsongs for (`2CHN`, `6CHN`, `8CHN` and the even `10CH`..`32CH`); the four-channel
//! tags stay with the built-in ProTracker. What only an XM can use is not ported (see
//! [`replay`]). Selected with the engine setting [`crate::engine::ModEngine::Ft2play`]
//! (`--mod-engine ft2play`); the default is unchanged.
//!
//! Known difference: ft2play `malloc`s a sample's memory and reads what the file has, so
//! a sample the file ends within leaves the rest of its memory as it was; here it reads 0.

mod mixer;
mod module;
mod replay;
mod tables;

use crate::aiff::{AiffPlayer, Stream};
use module::Module;
use replay::Replay;
use std::collections::BTreeSet;
use std::sync::Arc;

/// Whether `d` is a module this engine plays: a multichannel tag ft2play counts subsongs for
/// and a file its loader takes.
pub fn detect(d: &[u8]) -> bool {
    d.len() > 1084 && tag_ok(&d[1080..1084]) && Module::load(d).is_some()
}

fn tag_ok(sig: &[u8]) -> bool {
    match sig {
        b"2CHN" | b"6CHN" | b"8CHN" => true,
        [a @ b'1'..=b'3', c @ b'0'..=b'9', b'C', b'H'] => {
            let n = (a - b'0') as usize * 10 + (c - b'0') as usize;
            (10..=32).contains(&n) && n % 2 == 0
        }
        _ => false,
    }
}

/// The module on the recorded-audio player.
pub fn player(d: &[u8], rate: u32) -> Result<AiffPlayer, String> {
    let p = Ft2Player::new(d, rate).ok_or("ft2play does not load this module")?;
    let title = p.m.name.clone();
    Ok(AiffPlayer::from_stream(Box::new(p), "Fasttracker (ft2play)", title, rate))
}

/// `get_subsongs` of `player_ft2play.cc`: a walk from position 0 that follows Position Jump (B)
/// and Pattern Break (D) only (the last of each in a row, over the channels), and where a jump
/// lands on a position already seen (or a pattern ends) starts a new subsong at the lowest
/// position not seen yet.
fn subsongs(m: &Module) -> Vec<i16> {
    const LEN: i16 = 64;
    let song_len = m.len as i16;
    let ant_ptn = m.song_tab[..128].iter().copied().max().unwrap_or(0) as i32 + 1;
    let patt_nr = |pos: i16| m.song_tab[pos as u8 as usize] as i32;
    let mut subsongs = vec![0i16];
    let mut seen: BTreeSet<i16> = BTreeSet::new();
    let mut notseen: BTreeSet<i16> = (0..song_len).filter(|&i| patt_nr(i) <= ant_ptn).collect();
    let mut prev_jump = (0i16, 0i16);
    let (mut patt_pos, mut song_pos) = (0i16, 0i16);
    let mut jump = false;
    loop {
        if jump && seen.contains(&song_pos) && !notseen.is_empty() {
            song_pos = *notseen.iter().next().unwrap();
            patt_pos = 0;
            if patt_nr(song_pos) > ant_ptn {
                seen.insert(song_pos);
                notseen.remove(&song_pos);
                song_pos += 1;
                if song_pos >= song_len {
                    break;
                }
                continue;
            }
            subsongs.push(song_pos);
        }
        jump = false;
        seen.insert(song_pos);
        notseen.remove(&song_pos);
        if notseen.is_empty() {
            break;
        }
        let (mut eff_b, mut eff_d) = (-1i16, -1i16);
        if let Some(Some(p)) = m.patt.get(patt_nr(song_pos) as usize) {
            let mut c = 0;
            while c < m.chn && (eff_b == -1 || eff_d == -1) {
                let cell = p[patt_pos as usize * m.chn + c];
                if cell.eff_typ == 0xB {
                    eff_b = cell.eff as i16;
                } else if cell.eff_typ == 0xD {
                    eff_d = (cell.eff >> 4) as i16 * 10 + (cell.eff & 0x0F) as i16;
                }
                c += 1;
            }
        }
        let pj = (eff_b, eff_d);
        if eff_b >= 0 || eff_d >= 0 {
            let (old_pos, old_patt) = (song_pos, patt_pos);
            song_pos = if eff_b == -1 { song_pos + 1 } else { eff_b };
            if song_pos >= song_len {
                break;
            } else if pj == prev_jump && eff_b >= 0 {
                seen.insert(song_pos);
                jump = true;
            }
            patt_pos = if eff_d == -1 { 0 } else { eff_d };
            if patt_pos >= LEN {
                patt_pos = 0;
            }
            if old_pos == song_pos && old_patt == patt_pos {
                break;
            } else if old_pos != song_pos || (old_patt != patt_pos && patt_pos == 0) {
                jump = true;
            }
            prev_jump = pj;
        } else {
            patt_pos += 1;
            if patt_pos >= LEN {
                song_pos += 1;
                patt_pos = 0;
                jump = true;
                if song_pos >= song_len {
                    break;
                }
            }
        }
    }
    subsongs
}

/// The module playing one subsong, one tick at a time.
pub struct Ft2Player {
    m: Arc<Module>,
    rate: u32,
    starts: Vec<i16>,
    sub: usize,
    r: Replay,
    /// the wrapper's loop detection: positions and rows passed
    seen: BTreeSet<(i16, i16)>,
    /// the tick being handed out, in 16-bit stereo, and how far
    buf: Vec<i16>,
    at: usize,
    ended: bool,
    produced: bool,
}

impl Ft2Player {
    pub fn new(d: &[u8], rate: u32) -> Option<Ft2Player> {
        let m = Arc::new(Module::load(d)?);
        let starts = subsongs(&m);
        let r = Replay::new(Arc::clone(&m), rate, starts[0] as i32);
        Some(Ft2Player { m, rate, starts, sub: 0, r, seen: BTreeSet::new(), buf: Vec::new(), at: 0, ended: false, produced: false })
    }

    /// `render` of the wrapper: one tick, and whether the song has ended with it. The tick that
    /// ends the song is not part of the output (unless it is the first).
    fn next_tick(&mut self) {
        let jump_loop = |r: &Replay| r.s.p_break_flag || r.s.pos_jump_flag || r.s.jump_loop_flag;
        let prev_pos = (self.r.s.song_pos, self.r.s.patt_pos);
        let prev_jump = jump_loop(&self.r);
        self.r.frame();
        let pos = (self.r.s.song_pos, self.r.s.patt_pos);
        let jump = jump_loop(&self.r);
        let len = self.m.len as i16;
        let end_of_tune = self.r.end_of_tune(len as i32 - 1);
        let mut songend = (end_of_tune || self.r.s.song_pos >= len) && !self.seen.is_empty();
        if prev_jump && !jump && prev_pos.0 >= pos.0 && prev_pos.1 >= pos.1 && len > 1 {
            for i in pos.1..=prev_pos.1 {
                self.seen.remove(&(pos.0, i));
            }
        }
        if !songend && pos != prev_pos && !prev_jump && !jump {
            songend |= !self.seen.insert(pos);
        }
        self.buf.clear();
        self.at = 0;
        if songend {
            self.ended = true;
        }
        if !songend || !self.produced {
            std::mem::swap(&mut self.buf, &mut self.r.out);
        }
        self.produced = true;
    }

    /// The stereo frames of the tick being handed out that are left.
    fn left(&self) -> usize {
        (self.buf.len() - self.at) / 2
    }
}

impl Stream for Ft2Player {
    fn render(&mut self, out: &mut [f32]) -> usize {
        let want = out.len() / 2;
        let mut done = 0;
        while done < want {
            if self.left() == 0 {
                if self.ended {
                    break;
                }
                self.next_tick();
                if self.left() == 0 {
                    break;
                }
            }
            let n = (want - done).min(self.left());
            for (o, s) in out[done * 2..(done + n) * 2].iter_mut().zip(&self.buf[self.at..self.at + n * 2]) {
                *o = *s as f32 / 32768.0;
            }
            self.at += n * 2;
            done += n;
        }
        done
    }

    fn ended(&self) -> bool {
        self.ended && self.left() == 0
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.starts.len() as u16 - 1, self.sub as u16)
    }

    /// Starts the subsong as a fresh `play` of the plugin does: the module loaded again, `setPos`
    /// to the subsong's position.
    fn set_subsong(&mut self, n: u16) {
        let Some(&at) = self.starts.get(n as usize) else { return };
        self.sub = n as usize;
        self.r = Replay::new(Arc::clone(&self.m), self.rate, at as i32);
        self.seen.clear();
        self.buf.clear();
        self.at = 0;
        self.ended = false;
        self.produced = false;
    }

    fn duration_secs(&self) -> Option<f64> {
        None
    }
}

#[cfg(test)]
mod tests;
