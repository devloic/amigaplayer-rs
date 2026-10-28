//! ProWizard: packed and protected ProTracker modules (The Player, NoisePacker,
//! ProRunner, Promizer, ...) converted back to ProTracker modules, which the
//! built-in ProTracker then plays. A port of NostalgicPlayer's ProWizard converter
//! (Copyright (c) Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer), itself
//! after ProWizard for Amiga (Gryzor), ProWizard for PC (Sylvain "Asle" Chipaux)
//! and libxmp.
//!
//! `convert` runs after decrunching. Each format's check is tried in
//! NostalgicPlayer's order, and the first one that recognises the module converts
//! it; when that fails the module stays as it is. Plain ProTracker modules go
//! through the checks too, as in NostalgicPlayer: some formats (Devils Replay,
//! HCD Protector) keep the "M.K." mark, and no check takes a plain module (none
//! of the NostalgicPlayer test files of other formats, plain MODs included, nor
//! of the archive lists, is taken). An executable is left as it is.

pub mod core;
pub mod ac1d;
pub mod benreplay;
pub mod binarypacker;
pub mod channelplayer;
pub mod chiptracker;
pub mod cryptoburners;
pub mod darkdemon;
pub mod devilsreplay;
pub mod digitalillusions;
pub mod eureka;
pub mod fcm;
pub mod fuchs;
pub mod fuzzac;
pub mod gnoi;
pub mod gnuplayer;
pub mod gnuprotected;
pub mod hcd;
pub mod ims;
pub mod kefrens;
pub mod mfp;
pub mod mosh;
pub mod mpc;
pub mod newtron;
pub mod noisepacker;
pub mod noiserunner;
pub mod novotrade;
pub mod ntc;
pub mod perfectsong;
pub mod pha;
pub mod powermusic;
pub mod promizer;
pub mod propacker;
pub mod prorunner;
pub mod pygmy;
pub mod skyt;
pub mod slamtilt;
pub mod stp;
pub mod theplayer23;
pub mod theplayer4;
pub mod theplayer56;
pub mod titanics;
pub mod tmk;
pub mod trackerpacker;
pub mod unic;
pub mod wanton;
pub mod xann;
pub mod zen;

use std::path::Path;

/// Every format, in NostalgicPlayer's order (its agent number).
pub fn formats() -> Vec<Box<dyn core::Format>> {
    let mut v: Vec<(u32, Box<dyn core::Format>)> = Vec::new();
    v.extend(ac1d::formats());
    v.extend(benreplay::formats());
    v.extend(binarypacker::formats());
    v.extend(channelplayer::formats());
    v.extend(chiptracker::formats());
    v.extend(cryptoburners::formats());
    v.extend(darkdemon::formats());
    v.extend(devilsreplay::formats());
    v.extend(digitalillusions::formats());
    v.extend(eureka::formats());
    v.extend(fcm::formats());
    v.extend(fuchs::formats());
    v.extend(fuzzac::formats());
    v.extend(gnoi::formats());
    v.extend(gnuplayer::formats());
    v.extend(gnuprotected::formats());
    v.extend(hcd::formats());
    v.extend(ims::formats());
    v.extend(kefrens::formats());
    v.extend(mfp::formats());
    v.extend(mosh::formats());
    v.extend(mpc::formats());
    v.extend(newtron::formats());
    v.extend(noisepacker::formats());
    v.extend(noiserunner::formats());
    v.extend(novotrade::formats());
    v.extend(ntc::formats());
    v.extend(perfectsong::formats());
    v.extend(pha::formats());
    v.extend(powermusic::formats());
    v.extend(promizer::formats());
    v.extend(propacker::formats());
    v.extend(prorunner::formats());
    v.extend(pygmy::formats());
    v.extend(skyt::formats());
    v.extend(slamtilt::formats());
    v.extend(stp::formats());
    v.extend(theplayer23::formats());
    v.extend(theplayer4::formats());
    v.extend(theplayer56::formats());
    v.extend(titanics::formats());
    v.extend(tmk::formats());
    v.extend(trackerpacker::formats());
    v.extend(unic::formats());
    v.extend(wanton::formats());
    v.extend(xann::formats());
    v.extend(zen::formats());
    v.sort_by_key(|f| f.0);
    v.into_iter().map(|f| f.1).collect()
}

/// The converted module and the name of the format it was packed with.
pub fn identify_convert(data: &[u8], files: &dyn core::Loader) -> Option<(&'static str, Vec<u8>)> {
    if data.len() < 64 || data[..4] == [0, 0, 3, 0xf3] {
        return None;
    }
    for mut f in formats() {
        // a format reading out of its tables fails as the C# one throws
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut s = core::Stream::new(data);
            f.check(&mut s).then(|| core::convert(f.as_mut(), &mut s, files))
        }));
        match r {
            Ok(None) => continue,
            // as in NostalgicPlayer, the first format that recognises the module
            // converts it, or the module stays as it is
            Ok(Some(m)) => return m.map(|m| (f.name(), m)),
            Err(_) => {
                log::debug!("ProWizard: {} panicked", f.name());
                return None;
            }
        }
    }
    None
}

/// The module as a ProTracker module if it is one of the packed formats, else as
/// it is. `path` is where companion files are looked for.
pub fn convert(data: Vec<u8>, path: &Path) -> Vec<u8> {
    let files = core::FsLoader { path: path.to_path_buf() };
    match identify_convert(&data, &files) {
        Some((name, m)) => {
            log::info!("ProWizard: {name}, {} -> {} bytes", data.len(), m.len());
            m
        }
        None => data,
    }
}

#[cfg(test)]
pub mod tests;

#[cfg(test)]
mod plain {
    use super::core::NoFiles;
    use super::identify_convert;
    use super::tests::Song;

    #[test]
    fn plain_modules_are_left_alone() {
        let song = Song::new(2);
        let m = song.module(0x7f);
        assert!(identify_convert(&m, &NoFiles).is_none());
        // and so is a 15-sample SoundTracker module
        let mut st = m[..20 + 15 * 30].to_vec();
        st.extend(&m[950..1080]);
        st.extend(&m[1084..]);
        assert!(identify_convert(&st, &NoFiles).is_none());
    }

    #[test]
    fn every_format_is_listed_once() {
        let f = super::formats();
        let mut names: Vec<_> = f.iter().map(|f| f.name()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), f.len());
        assert_eq!(f.len(), 71);
    }
}
