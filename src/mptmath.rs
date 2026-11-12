//! Format-independent arithmetic of OpenMPT's player, shared by the ports of its loaders that
//! play on players of their own (`crate::symmod`, `crate::dmf`): the linear slide tables, the
//! linear frequency slide (`CSoundFile::DoFreqSlide` for linear slides with frequencies as
//! periods) and `GetPeriodFromNote` for frequencies. Ported from libopenmpt 0.8.7
//! `soundlib/Tables.cpp`, `soundlib/Snd_fx.cpp` and `common/misc_util.h` (BSD-3-Clause,
//! Copyright (c) 2004-2026 OpenMPT Project Developers and Contributors; see `LICENSE-openmpt`).

/// The linear slide tables: 2^(n/192) and 2^(n/768), 16.16 fixed point (`LinearSlideUpTable`,
/// `LinearSlideDownTable`, `FineLinearSlideUpTable`, `FineLinearSlideDownTable`).
pub struct Tables {
    pub up: [u32; 256],
    pub down: [u32; 256],
    pub fine_up: [u32; 16],
    pub fine_down: [u32; 16],
}

impl Tables {
    pub fn new() -> Tables {
        let mut t = Tables { up: [0; 256], down: [0; 256], fine_up: [0; 16], fine_down: [0; 16] };
        for n in 0..256 {
            t.up[n] = (65536.0 * 2f64.powf(n as f64 / 192.0)).round() as u32;
            t.down[n] = (65536.0 * 2f64.powf(-(n as f64) / 192.0)).round() as u32;
        }
        for n in 0..16 {
            t.fine_up[n] = (65536.0 * 2f64.powf(n as f64 / 768.0)).round() as u32;
            t.fine_down[n] = (65536.0 * 2f64.powf(-(n as f64) / 768.0)).round() as u32;
        }
        t
    }
}

impl Default for Tables {
    fn default() -> Tables {
        Tables::new()
    }
}

/// `Util::muldivr`: (a * b + c / 2) / c, truncating.
pub fn muldivr(a: i64, b: i64, c: i64) -> i64 {
    (a * b + c / 2) / c
}

/// `DoFreqSlide` with linear slides on a frequency: positive amounts raise it.
pub fn freq_slide(t: &Tables, period: &mut i64, amount: i32) {
    if *period == 0 || amount == 0 {
        return;
    }
    let old = *period;
    let abs = amount.unsigned_abs();
    if abs < 16 {
        let f = if amount > 0 { t.fine_up[abs as usize] } else { t.fine_down[abs as usize] };
        *period = muldivr(*period, f as i64, 65536);
    } else {
        let mut n = abs / 4;
        while n > 0 {
            let k = n.min(255);
            let f = if amount > 0 { t.up[k as usize] } else { t.down[k as usize] };
            *period = muldivr(*period, f as i64, 65536);
            n -= k;
        }
    }
    if *period == old {
        if amount > 0 && *period < i32::MAX as i64 {
            *period += 1;
        } else if amount < 0 && *period > 1 {
            *period -= 1;
        }
    }
    *period = (*period).clamp(1, i32::MAX as i64);
}

/// `GetPeriodFromNote` for frequencies: c5 * 2^((note - 61) / 12) (note 1..=128; c5 0 = 8363).
pub fn note_freq(t: &Tables, note: u8, c5: u32) -> i64 {
    let n = note.max(1) as u32 - 1;
    let c5 = if c5 == 0 { 8363 } else { c5 };
    let f = (c5 as u64 * ((t.up[((n % 12) * 16) as usize] as u64) << (n / 12))) / (65536u64 << 5);
    f.min(i32::MAX as u64) as i64
}
