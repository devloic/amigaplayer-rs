//! Format extras (`extras.c`): the per-format state and hooks (FAR now; MED and
//! HMN later) that the generic player calls at fixed points. The extras of a
//! module live in `ModuleData::extra`, those of a channel in
//! `ChannelData::extra`, as `m->extra` and `xc->extra` in libxmp.
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use super::far_extras::*;
use super::player::*;

/// `m->extra`.
#[derive(Clone, Copy, Default, Debug)]
pub enum ModuleExtra {
    #[default]
    None,
    Far(FarModuleExtras),
}

/// `xc->extra`.
#[derive(Clone, Copy, Default, Debug)]
pub enum ChannelExtra {
    #[default]
    None,
    Far(FarChannelExtras),
}

impl ModuleExtra {
    /// `libxmp_new_channel_extras` / `libxmp_reset_channel_extras`: a channel's
    /// extras in their initial state for this module.
    pub fn new_channel_extras(&self) -> ChannelExtra {
        match self {
            ModuleExtra::Far(_) => ChannelExtra::Far(FarChannelExtras::default()),
            ModuleExtra::None => ChannelExtra::None,
        }
    }
}

impl Context {
    /// `libxmp_play_extras`.
    pub(crate) fn play_extras(&mut self, chn: usize) {
        if let ChannelExtra::Far(_) = self.p.xc_data[chn].extra {
            self.far_play_extras(chn);
        }
    }

    /// `libxmp_extras_get_volume`.
    pub(crate) fn extras_get_volume(&self, chn: usize) -> i32 {
        self.p.xc_data[chn].volume
    }

    /// `libxmp_extras_get_period`.
    pub(crate) fn extras_get_period(&self, _chn: usize) -> i32 {
        0
    }

    /// `libxmp_extras_get_linear_bend`.
    pub(crate) fn extras_get_linear_bend(&self, _chn: usize) -> i32 {
        0
    }

    /// `libxmp_extras_process_fx`.
    pub(crate) fn extras_process_fx(&mut self, xc: &mut ChannelData, note: u8, fxt: u8, fxp: u8) {
        if let ChannelExtra::Far(_) = xc.extra {
            self.far_extras_process_fx(xc, note, fxt, fxp);
        }
    }
}
