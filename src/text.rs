//! Text a composer left inside a module.
//!
//! Ported from azuracast-rust crates/ac-amiga/src/modtext.rs (e936add), written for the user,
//! who licenses it GPL-2.0-or-later for amigaplayer-rs (2026-11-16).
//!
//! A tracker module has no comment field, so composers wrote into the fields
//! it does have: the 20-byte song title and the 22-byte sample names, which
//! together read as a few lines of greetings, credits or a message. This
//! module pulls those fields out so a player can show them beside the song.
//!
//! Detection follows ModInfo v2.4 (Sylvain "Asle" Chipaux, 2002-2014): the
//! four bytes at 1080 name a 31-sample module (`M.K.`, `M!K!`, `FLT4`,
//! `6CHN`, `16CH`, ...), and anything else is tried as a 15-sample
//! SoundTracker module, whose 600-byte header has no tag at all. ModInfo
//! accepts any file there ("Soundtracker (or not :)"); here the header must
//! also look like one -- printable names, a song length of 1..=128 -- since a
//! wrong guess would put binary junk on the player.
//!
//! Other formats keep their text elsewhere, and [`extract_for`] reads it by
//! the renderer's name for the format. Each layout below was checked against
//! every gallery file of that format (a sweep of the 1576-file gallery,
//! 2026-11-12); formats whose fields held only default names, a replayer's
//! constant banner or nothing at all are not read.
//!
//! The PC trackers the renderer also plays (S3M, IT, XM, MTM, 669, ULT, FAR,
//! STM/STX, PSM, DSM, AMF, DMF, MT2, AMS, MDL, DBM, GDM, Liquid, Megatracker,
//! Symphonie, ...) name themselves by version and tracker ("OpenMPT 1.30 IT
//! 2.14"), so [`pc_module`] recognises them by their own magic instead. Their
//! readers keep the title, the instrument and sample names and, where the
//! format has one, the song message (IT, XM `text`, MTM, 669, ULT, FAR, GDM,
//! Symphonie). Not read: MO3 (its header and message are inside the LZ-packed
//! music data), Digital Symphony's information text and instrument names
//! (LZW-packed; only its title is), and formats whose instrument names sit
//! past packed or variable-length data this does not step over (IMF, Liquid
//! LIQ: title only).

/// The text a composer left in the module `data`, which plays as `format` (the name
/// [`Song::format`](crate::Song::format) gives): lines joined with `\n`, or `""` when the
/// format has none or the fields hold nothing but a tracker's placeholders.
///
/// A file-level wrapper (gzip, xz) or packer (PowerPacker, Pack-Ice, ...) and a ProWizard
/// packed module are undone first, as [`Song::open`](crate::Song::open) does. Never panics,
/// whatever `data` is; SID, chip-music and sampled-audio formats give `""`.
pub fn module_text(format: &str, data: &[u8]) -> String {
    if matches!(format, "PSID" | "RSID") {
        return String::new();
    }
    #[cfg(feature = "archives")]
    if let Some(d) = crate::decrunch::unwrap_wrapper(data) {
        return text_of(format, d);
    }
    text_of(format, data.to_vec())
}

fn text_of(format: &str, data: Vec<u8>) -> String {
    let data = crate::decrunch::decrunch(data);
    let data = if crate::replay::timetracker::is_timetracker(&data) { data } else { crate::prowizard::convert(data, std::path::Path::new("")) };
    extract_for(format, &data).unwrap_or_default()
}

/// The text in `bytes`, a file identified as `format`. Formats without a reader here are
/// still tried as a ProTracker-family module, which identifies itself by its tag.
fn extract_for(format: &str, bytes: &[u8]) -> Option<String> {
    let lines = match format {
        "HivelyTracker" | "AHX" => hively(bytes),
        "Musicline Editor" => musicline(bytes),
        "Digibooster" => digibooster(bytes),
        "Digital Sound Studio" => names(bytes, 10, 46, 30, 31),
        "Quadra Composer" => quadra(bytes),
        "Face The Music" => face_the_music(bytes),
        "PreTracker" => pretracker(bytes),
        "PolyTracker" => polytracker(bytes),
        "Oktalyzer" => oktalyzer(bytes),
        "Art of Noise 4V" | "Art of Noise 8V" => art_of_noise(bytes),
        "Protracker4" => protracker4(bytes),
        "OctaMED" | "MED" | "OctaMED SoundStudio" => octamed(bytes),
        "IFF-SMUS" => smus(bytes),
        "Pokeynoise" => sap(bytes),
        "YM-2149" => ym(bytes),
        "InStereo! 1.0" => vec![field(slice(bytes, 36, 32))],
        "SoundMon 2.0" | "SoundMon 2.2" => vec![field(slice(bytes, 0, 26))],
        "Digital Mugician 1" => names(bytes, 80, 16, 12, 8),
        "Synthesis" => vec![field(slice(bytes, 36, 28))],
        "Dave Lowe" | "Beathoven Synthesizer" | "Sound Programming Language" | "Fred Gray"
        | "Future Player" | "Riff Raff" | "Howie Davies" | "Benn Daglish SID" => hunk_strings(bytes),
        // (the 15-instrument trackers: no tag, 600-byte header)
        "Ultimate SoundTracker 1.0" | "Ultimate SoundTracker 1.8" | "SoundTracker II" | "SoundTracker VI"
        | "SoundTracker IX" | "SoundTracker 2.x" | "Master SoundTracker" | "SoundTracker" => mod_names(bytes, 15),
        "Multichannel MOD" | "SoundTracker 2.6" | "StarTrekker" | "StarTrekker 8"
        | "FastTracker 4ch" | "Janne Salmijarvi Optimizer" | "IceTracker" => mod_names(bytes, 31),
        "Digital Mugician 2" => names(bytes, 80, 16, 12, 8),
        "SoundFX1.3" => soundfx(bytes),
        "JamCracker" => jamcracker(bytes),
        "Sean Connolly" | "Maniacs of Noise" | "Special FX" => stub_text(bytes),
        "Images Music System" => mod_names(bytes, 31),
        "Coconizer" => coconizer(bytes),
        // The PC trackers' names carry the version and tracker ("Scream
        // Tracker 3.20 S3M", "OpenMPT 1.28 IT 2.00"), so they are told apart
        // by their own magic; a tagged MOD is left to `extract`.
        _ => match pc_module(bytes) {
            Some(lines) => lines,
            None => return extract(bytes),
        },
    };
    finish(lines)
}

/// Placeholders blanked, then [`tidy`].
fn finish(lines: Vec<String>) -> Option<String> {
    tidy(lines.into_iter().map(|l| if is_placeholder(&l) { String::new() } else { l }).collect())
}

/// Title and sample names of a ProTracker-family module, as lines of text.
/// `None` when the bytes are not such a module or say nothing.
fn extract(bytes: &[u8]) -> Option<String> {
    let samples = sample_count(bytes)?;
    let mut lines = vec![field(&bytes[0..20])];
    for i in 0..samples {
        let at = 20 + i * 30;
        lines.push(field(&bytes[at..at + 22]));
    }
    finish(lines)
}

/// 31 for a tagged module, 15 for a plausible SoundTracker one.
fn sample_count(bytes: &[u8]) -> Option<usize> {
    if bytes.len() >= 1084 && is_31_sample_tag(&bytes[1080..1084]) {
        return Some(31);
    }
    // 15 samples: 20 + 15*30 = 470, then song length, restart byte, 128
    // positions: 600 bytes before the first pattern.
    if bytes.len() < 600 + 1024 {
        return None;
    }
    let song_length = bytes[470];
    if !(1..=128).contains(&song_length) {
        return None;
    }
    let names_ok = std::iter::once(&bytes[0..20])
        .chain((0..15).map(|i| &bytes[20 + i * 30..42 + i * 30]))
        .all(looks_like_text);
    let positions_ok = bytes[472..600].iter().all(|&p| p < 64);
    (names_ok && positions_ok).then_some(15)
}

fn is_31_sample_tag(tag: &[u8]) -> bool {
    matches!(tag, b"M.K." | b"M!K!" | b"M&K!" | b"N.T." | b"FLT4" | b"FLT8" | b"CD81" | b"OKTA")
        || (tag[0].is_ascii_digit() && &tag[1..] == b"CHN")
        || (tag[0].is_ascii_digit() && tag[1].is_ascii_digit() && &tag[2..] == b"CH")
}

/// A tracker's default name ("Instrument 3", "(empty)"), a lone number or
/// symbols: not something a composer wrote. Kept as a blank line, since
/// composers space their messages with such fields.
fn is_placeholder(line: &str) -> bool {
    let t = line.trim().to_ascii_lowercase();
    let stripped = |prefix: &str| {
        t.strip_prefix(prefix)
            .is_some_and(|rest| rest.trim_start().chars().all(|c| c.is_ascii_digit()))
    };
    !t.is_empty()
        && (stripped("instrument")
            || stripped("sample")
            || stripped("st-")
            || matches!(t.as_str(), "(empty)" | "untitled" | "no name" | "noname")
            || t.starts_with("unnamed")
            || t.contains("synthetic")
            || !t.chars().any(|c| c.is_alphanumeric()))
}

/// `len` bytes at `at`, or as many as the file has (none past its end).
fn slice(b: &[u8], at: usize, len: usize) -> &[u8] {
    let start = at.min(b.len());
    &b[start..at.saturating_add(len).min(b.len())]
}

fn be16(b: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?) as usize)
}

fn be32(b: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize)
}

/// `count` fixed name fields of `len` bytes, `stride` apart from `at`.
fn names(b: &[u8], at: usize, stride: usize, len: usize, count: usize) -> Vec<String> {
    (0..count).map(|i| field(slice(b, at + i * stride, len))).collect()
}

/// A MOD-layout header read without the tag check: the renderer has
/// already said what the file is.
fn mod_names(b: &[u8], samples: usize) -> Vec<String> {
    // Still checked: some players' files (Master SoundTracker) keep
    // something other than names there.
    let fields_ok = std::iter::once(slice(b, 0, 20))
        .chain((0..samples).map(|i| slice(b, 20 + i * 30, 22)))
        .all(looks_like_text);
    if !fields_ok {
        return Vec::new();
    }
    let mut lines = vec![field(slice(b, 0, 20))];
    lines.extend(names(b, 20, 30, 22, samples));
    lines
}

/// NUL-terminated strings from `at`, up to `max`.
fn c_strings(b: &[u8], at: usize, max: usize) -> Vec<String> {
    slice(b, at, usize::MAX).split(|&c| c == 0).take(max).map(field).collect()
}

/// Text that may span lines (a remark, an annotation): NULs and CRs as line
/// breaks, other control bytes as spaces.
fn block(raw: &[u8]) -> Vec<String> {
    let text: String = raw
        .iter()
        .map(|&b| match b {
            0 | b'\r' | b'\n' => '\n',
            0x20..=0x7e | 0xa0..=0xff => b as char,
            _ => ' ',
        })
        .collect();
    text.split('\n').map(|l| l.trim_end().to_string()).collect()
}

/// IFF-style chunks (`tag`, big-endian length, data) from `at`.
fn chunks(b: &[u8], at: usize, pad: bool) -> Vec<(&[u8], &[u8])> {
    chunks_by(b, at, 4, false, pad)
}

/// Chunks of an `id_len`-byte tag, a 32-bit length (little-endian when
/// `little`) and that much data, from `at`.
fn chunks_by(b: &[u8], mut at: usize, id_len: usize, little: bool, pad: bool) -> Vec<(&[u8], &[u8])> {
    let mut out = Vec::new();
    while let Some(tag) = b.get(at..at.saturating_add(id_len)) {
        let at_len = at + id_len;
        let len = if little { le32(b, at_len) } else { be32(b, at_len) };
        let Some(len) = len else { break };
        out.push((tag, slice(b, at_len + 4, len)));
        at = at_len.saturating_add(4).saturating_add(len).saturating_add(usize::from(pad && len % 2 == 1));
    }
    out
}

/// HivelyTracker and AHX: a u16 at 4 points at the song name, followed by
/// one NUL-terminated name per instrument (count at 10, or 12 for AHX).
fn hively(b: &[u8]) -> Vec<String> {
    let Some(at) = be16(b, 4) else { return Vec::new() };
    let count = if b.starts_with(b"HVL") { b.get(10) } else { b.get(12) };
    c_strings(b, at, 1 + usize::from(*count.unwrap_or(&0)))
}

/// Musicline: a 32-byte title in the `TUNE` chunk, and the `INFO` chunk's
/// title, author, date and remark strings.
fn musicline(b: &[u8]) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(i) = find(b, b"TUNE", 0) {
        lines.push(field(slice(b, i + 8, 32)));
    }
    if let Some(j) = rfind(b, b"INFO") {
        let len = be32(b, j + 4).unwrap_or(0);
        lines.extend(slice(b, j + 8, len).split(|&c| c == 0).map(field));
    }
    lines
}

fn digibooster(b: &[u8]) -> Vec<String> {
    let mut lines = vec![field(slice(b, 610, 32))];
    lines.extend(names(b, 642, 30, 30, 31));
    lines
}

fn quadra(b: &[u8]) -> Vec<String> {
    if slice(b, 8, 4) != b"EMOD" {
        return Vec::new();
    }
    let count = usize::from(*b.get(63).unwrap_or(&0));
    let mut lines = vec![field(slice(b, 22, 20)), field(slice(b, 42, 20))];
    lines.extend(names(b, 68, 34, 20, count));
    lines
}

fn face_the_music(b: &[u8]) -> Vec<String> {
    let mut lines = vec![field(slice(b, 16, 32)), field(slice(b, 48, 32))];
    for i in 0..200 {
        let rec = slice(b, 80 + 32 * i, 32);
        if rec.len() < 32 || !rec[2..].iter().all(|&c| c == 0 || (0x20..0x7f).contains(&c)) {
            break;
        }
        lines.push(field(&rec[2..]));
    }
    lines
}

fn pretracker(b: &[u8]) -> Vec<String> {
    let mut lines = vec![field(slice(b, 20, 20)), field(slice(b, 40, 20))];
    if let Some(at) = be32(b, 12).filter(|&p| p > 0 && p < b.len()) {
        lines.extend(c_strings(b, at, 40));
    }
    lines
}

fn polytracker(b: &[u8]) -> Vec<String> {
    let count = b.get(34..36).map_or(0, |n| u16::from_le_bytes([n[0], n[1]]) as usize).min(255);
    let mut lines = vec![field(slice(b, 0, 28))];
    lines.extend(names(b, 608 + 48, 80, 28, count));
    lines
}

fn oktalyzer(b: &[u8]) -> Vec<String> {
    if !b.starts_with(b"OKTASONG") {
        return Vec::new();
    }
    chunks(b, 8, false)
        .into_iter()
        .find(|(tag, _)| *tag == b"SAMP")
        .map(|(_, data)| data.chunks(32).map(|r| field(slice(r, 0, 20))).collect())
        .unwrap_or_default()
}

/// Art of Noise: `NAME`, `AUTH`, `DATE` and the free-text `RMRK` chunks.
fn art_of_noise(b: &[u8]) -> Vec<String> {
    let mut lines = Vec::new();
    for tag in [b"NAME", b"AUTH", b"DATE", b"RMRK"] {
        if let Some(i) = find(slice(b, 0, 400), tag, 4) {
            let len = be32(b, i + 4).unwrap_or(0);
            lines.extend(block(slice(b, i + 8, len)));
        }
    }
    lines
}

/// ProTracker 4's IFF: the `INFO` title, the `CMNT` comment, and the classic
/// module image in `PTDT`, whose sample names carry the credits. Found by
/// tag rather than by walking the chunks: the `VERS` chunk's length is
/// wrong in the files seen (22 declared, 10 present), and `CMNT`'s overruns.
fn protracker4(b: &[u8]) -> Vec<String> {
    let mut lines = Vec::new();
    let chunk = |tag: &[u8]| {
        let at = find(slice(b, 0, 4096), tag, 12)?;
        Some(slice(b, at + 8, be32(b, at + 4)?))
    };
    if let Some(info) = chunk(b"INFO") {
        lines.push(field(slice(info, 0, 32)));
    }
    if let Some(comment) = chunk(b"CMNT") {
        // Its length overruns into the module image too.
        let end = find(comment, b"PTDT", 0).unwrap_or(comment.len());
        lines.extend(block(&comment[..end]));
    }
    if let Some(at) = find(b, b"PTDT", 12) {
        lines.extend(mod_names(slice(b, at + 8, usize::MAX), 31).into_iter().skip(1));
    }
    lines
}

/// OctaMED MMD0-3: the ExpData block (pointer at 32) points at the song name
/// and the annotation text. The older `MED\x04` files have neither.
fn octamed(b: &[u8]) -> Vec<String> {
    let mut lines = Vec::new();
    if !b.starts_with(b"MMD") {
        return lines;
    }
    let Some(exp) = be32(b, 32).filter(|&e| e > 0 && e + 60 <= b.len()) else { return lines };
    let pointed = |at: usize, max: usize| {
        let (ptr, len) = (be32(b, at)?, be32(b, at + 4)?);
        (ptr > 0 && len > 0 && len < max && ptr + len <= b.len()).then(|| &b[ptr..ptr + len])
    };
    if let Some(name) = pointed(exp + 44, 1000) {
        lines.push(field(name));
    }
    if let Some(text) = pointed(exp + 12, 20_000) {
        lines.extend(block(text));
    }
    lines
}

/// IFF SMUS: `NAME`, `AUTH`, `ANNO`, `(c) ` and the instrument names.
fn smus(b: &[u8]) -> Vec<String> {
    let mut lines = Vec::new();
    for (tag, data) in chunks(b, 12, true) {
        match tag {
            b"NAME" | b"AUTH" | b"(c) " => lines.push(field(data)),
            b"ANNO" => lines.extend(block(data)),
            b"INS1" => lines.push(field(slice(data, 4, usize::MAX))),
            _ => {}
        }
    }
    lines
}

/// SAP: a text header, `NAME "..."`, `AUTHOR "..."`, `DATE "..."`.
fn sap(b: &[u8]) -> Vec<String> {
    let head: String = slice(b, 0, 600).iter().map(|&c| c as char).collect();
    ["NAME", "AUTHOR", "DATE"]
        .iter()
        .filter_map(|key| {
            head.lines()
                .find_map(|l| l.trim_end().strip_prefix(key)?.trim().strip_prefix('"')?.strip_suffix('"').map(str::to_string))
        })
        .collect()
}

/// YM: past the 8-byte `0x80 0x00` digidrum entries from 4, the title,
/// author, converter and replay strings, NUL-terminated.
fn ym(b: &[u8]) -> Vec<String> {
    let mut at = 4;
    while b.get(at) == Some(&0x80) && b.get(at + 1) == Some(&0) {
        at += 8;
    }
    let Some(start) = (at..at + 64).find(|&i| slice(b, i, 3).len() == 3 && slice(b, i, 3).iter().all(|c| (0x20..0x7f).contains(c))) else {
        return Vec::new();
    };
    printable_strings(b, start, 5)
}

/// Wanted Team-style replayer hunks: after the hunk header, `moveq #-1,d0;
/// rts` (`70 FF 4E 75`) and an 8-byte replayer tag (`UNCLEART`, `FREDGRAY`)
/// at 0x24, a pointer table, then title, author and copyright (with the
/// ripper's credit) as consecutive NUL-terminated strings.
fn hunk_strings(b: &[u8]) -> Vec<String> {
    if slice(b, 0x20, 4) != [0x70, 0xff, 0x4e, 0x75] {
        return Vec::new();
    }
    let printable = |c: &u8| (0x20..0x7f).contains(c) || *c == b'\n';
    let Some(start) = (0x2c..0x300.min(b.len())).find(|&i| {
        !printable(&b[i - 1]) && b[i..].iter().take_while(|c| printable(c)).count() >= 4
    }) else {
        return Vec::new();
    };
    printable_strings(b, start, 8)
}

/// Up to `max` consecutive non-empty, wholly printable NUL-terminated strings.
fn printable_strings(b: &[u8], mut at: usize, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    while out.len() < max {
        let Some(len) = b.get(at..).and_then(|r| r.iter().position(|&c| c == 0)) else { break };
        let s = &b[at..at + len];
        if s.is_empty() || len > 400 || !s.iter().all(|&c| (0x20..0x7f).contains(&c) || c == b'\n') {
            break;
        }
        out.extend(block(s));
        at += len + 1;
    }
    out
}

// ---------------------------------------------------------------------------
// PC trackers (xmp / OpenMPT loaders of the renderer)

fn le16(b: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?) as usize)
}

fn le32(b: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?) as usize)
}

/// The module in `b` by its own magic, for the formats whose name the
/// renderer builds from the file's version and tracker. `None` when `b` is
/// none of them; `Some` (possibly empty) when it is one, however damaged.
fn pc_module(b: &[u8]) -> Option<Vec<String>> {
    // A tagged MOD is a ProTracker-family module whatever else its bytes say.
    if b.len() >= 1084 && is_31_sample_tag(&b[1080..1084]) {
        return None;
    }
    let lines = if b.starts_with(b"Extended Module: ") {
        xm(b)
    } else if b.starts_with(b"IMPM") {
        it(b)
    } else if slice(b, 44, 4) == b"SCRM" {
        s3m(b)
    } else if slice(b, 60, 4) == b"SCRM" && matches!(slice(b, 20, 8), b"!Scream!" | b"BMOD2STM") {
        stx(b)
    } else if b.len() >= 4 && &b[..3] == b"MTM" && b[3] == 0x10 {
        mtm(b)
    } else if b.starts_with(b"MAS_UTrack_V00") {
        ult(b)
    } else if b.starts_with(b"FAR\xfe") {
        far(b)
    } else if b.starts_with(b"PSM\xfe") {
        psm16(b)
    } else if b.starts_with(b"PSM ") {
        psm(b)
    } else if (b.starts_with(b"RIFF") && slice(b, 8, 4) == b"DSMF") || b.starts_with(b"DSMF") {
        dsm(b)
    } else if b.starts_with(b"DSm\x1a") {
        dsm_dynamic(b)
    } else if b.len() >= 4 && &b[..3] == b"AMF" {
        amf(b)
    } else if b.starts_with(b"DDMF") {
        dmf(b)
    } else if b.starts_with(b"MT20") {
        mt2(b)
    } else if b.starts_with(b"AMShdr\x1a") {
        ams2(b)
    } else if b.starts_with(b"Extreme") && b.get(8) == Some(&1) {
        ams1(b)
    } else if b.starts_with(b"DMDL") {
        mdl(b)
    } else if b.starts_with(b"DBM0") {
        dbm(b)
    } else if b.starts_with(b"GDM\xfe") && slice(b, 0x47, 4) == b"GMFS" {
        gdm(b)
    } else if slice(b, 60, 4) == b"IM10" {
        Some(vec![field(slice(b, 0, 32))])
    } else if b.starts_with(b"Liquid Module:") {
        Some(vec![field(slice(b, 14, 30)), field(slice(b, 44, 20))])
    } else if b.starts_with(b"NO\0\0") && b.get(4).is_some_and(|&n| n <= 29) {
        liquid_no(b)
    } else if b.starts_with(b"MGT") && slice(b, 4, 4) == [0xbd, b'M', b'C', b'S'] {
        mgt(b)
    } else if b.starts_with(&[0x02, 0x01, 0x13, 0x13, 0x14, 0x12, 0x01, 0x0b]) {
        digital_symphony(b)
    } else if b.starts_with(b"SymM") {
        symphonie(b)
    } else if is_funk(b) {
        Some(names(b, 401, 32, 19, 64))
    } else if b.starts_with(b"ASYLUM Music Format V1.0") {
        Some(names(b, 294, 37, 22, usize::from(*b.get(34).unwrap_or(&0)).min(64)))
    } else if slice(b, 952, 4) == b"KRIS" {
        // Chiptracker: a MOD's layout with two more bytes after the title
        let mut lines = vec![field(slice(b, 0, 20))];
        lines.extend(names(b, 22, 30, 22, 31));
        Some(lines)
    } else if is_669(b) {
        Some(slice(b, 2, 108).chunks(36).map(field).collect())
    } else if is_stm(b) {
        stm(b)
    } else {
        return None;
    };
    Some(lines.unwrap_or_default())
}

/// Funktracker: the loader's test, which has the file's own size at 8.
fn is_funk(b: &[u8]) -> bool {
    b.len() >= 12
        && b.starts_with(b"Funk")
        && b[5] >> 1 >= 10
        && b[6] >> 4 <= 7
        && b[6] & 15 <= 9
        && le32(b, 8).is_some_and(|size| size >= 1024 && size == b.len())
}

/// Composer 669 (`if`) and UNIS 669 (`JN`): a 108-byte message of three
/// 36-character lines, then counts and a 0xff marker the loader checks.
fn is_669(b: &[u8]) -> bool {
    b.len() > 240 && matches!(&b[..2], b"if" | b"JN") && b[110] <= 64 && b[111] <= 128 && b[240] == 0xff
}

/// Scream Tracker 2: the loader's test, and a known version.
fn is_stm(b: &[u8]) -> bool {
    b.len() >= 64
        && (b[28] == 0x1a || b[28] == 0x02)
        && b[29] <= 2
        && b[20..28].iter().all(|&c| c <= 0x7f && (c == 0 || c >= 32 || c == 8 || c == 14))
        && matches!(100 * usize::from(b[30]) + usize::from(b[31]), 110 | 200 | 210 | 220 | 221)
        && &b[60..64] != b"SCRM"
}

/// `rows` of `width` bytes each, one line per row.
fn rows(raw: &[u8], width: usize) -> Vec<String> {
    raw.chunks(width).map(field).collect()
}

/// A song message in one run: it ends at the first NUL, and CR, LF or CRLF
/// break its lines (Impulse Tracker and OpenMPT write CR).
fn message(raw: &[u8]) -> Vec<String> {
    let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    let mut text = Vec::with_capacity(end);
    for (i, &c) in raw[..end].iter().enumerate() {
        // CRLF: the LF breaks the line, the CR before it does not.
        if c == b'\r' && raw.get(i + 1) == Some(&b'\n') {
            continue;
        }
        text.push(c);
    }
    block(&text)
}

/// An instrument or sample header of Scream Tracker 3 (also STX's) at
/// paragraph `para`: its 28-byte name, when the slot holds one.
fn s3m_name(b: &[u8], para: usize) -> Option<String> {
    let rec = slice(b, para.checked_mul(16)?, 80);
    if rec.len() < 80 {
        return None;
    }
    let ok = match rec[0] {
        1 => &rec[76..80] == b"SCRS",
        2..=7 => &rec[76..80] == b"SCRI",
        // An empty slot: ST3 still saves what was typed in its name.
        0 => para != 0 && looks_like_text(&rec[48..76]),
        _ => false,
    };
    ok.then(|| field(&rec[48..76]))
}

/// Scream Tracker 3: the title and the 28-byte instrument names.
fn s3m(b: &[u8]) -> Option<Vec<String>> {
    let (orders, ins) = (le16(b, 32)?, le16(b, 34)?);
    if ins > 255 || orders > 255 {
        return None;
    }
    let mut lines = vec![field(slice(b, 0, 28))];
    for i in 0..ins {
        if let Some(name) = le16(b, 96 + orders + 2 * i).and_then(|para| s3m_name(b, para)) {
            lines.push(name);
        }
    }
    Some(lines)
}

/// STM2STX and BMOD2STM: the title, then the S3M-style instrument headers
/// at the paragraph pointers the header's instrument table holds.
fn stx(b: &[u8]) -> Option<Vec<String>> {
    let (table, ins) = (le16(b, 34)?, le16(b, 50)?);
    if ins > 255 {
        return None;
    }
    let mut lines = vec![field(slice(b, 0, 20))];
    for i in 0..ins {
        if let Some(name) = le16(b, table * 16 + 2 * i).and_then(|para| s3m_name(b, para)) {
            lines.push(name);
        }
    }
    Some(lines)
}

/// Scream Tracker 2: the title and 31 twelve-character instrument names.
fn stm(b: &[u8]) -> Option<Vec<String>> {
    if 100 * usize::from(b[30]) + usize::from(b[31]) < 200 {
        // 1.10 songs keep their instrument table elsewhere.
        return Some(vec![field(slice(b, 0, 20))]);
    }
    let mut lines = vec![field(slice(b, 0, 20))];
    lines.extend(names(b, 48, 32, 12, 31));
    Some(lines)
}

/// Impulse Tracker: the title, the song message (CR line breaks) and the
/// instrument names, or the sample names of a module without instruments.
fn it(b: &[u8]) -> Option<Vec<String>> {
    let (orders, ins, smp) = (le16(b, 32)?, le16(b, 34)?, le16(b, 36)?);
    let (flags, special) = (le16(b, 44)?, le16(b, 46)?);
    if ins > 255 || smp > 255 {
        return None;
    }
    let mut lines = vec![field(slice(b, 4, 26))];
    if special & 1 != 0 {
        let (len, at) = (le16(b, 54)?, le32(b, 56)?);
        if len > 0 && at > 0 {
            lines.extend(message(slice(b, at, len)));
        }
    }
    let table = 192 + orders;
    let header = |tag: &[u8], index: usize, name_at: usize| {
        let at = le32(b, table + 4 * index)?;
        (slice(b, at, 4) == tag).then(|| field(slice(b, at + name_at, 26)))
    };
    let instruments: Vec<String> = (0..ins).filter_map(|i| header(b"IMPI", i, 32)).collect();
    let samples: Vec<String> = (0..smp)
        .filter_map(|i| {
            let at = le32(b, table + 4 * (ins + i))?;
            (slice(b, at, 4) == b"IMPS").then(|| field(slice(b, at + 20, 26)))
        })
        .collect();
    let has_instruments = flags & 4 != 0 && !instruments.is_empty();
    if has_instruments {
        // Samples usually repeat their instrument's name.
        let extra: Vec<String> = samples.iter().filter(|s| !instruments.contains(s)).cloned().collect();
        lines.extend(instruments);
        lines.extend(extra);
    } else {
        lines.extend(samples);
    }
    Some(lines)
}

/// FastTracker II and its clones: the module name, the instrument and
/// sample names, and the song comment OpenMPT appends (`text`).
fn xm(b: &[u8]) -> Option<Vec<String>> {
    let version = le16(b, 58)?;
    let (patterns, instruments) = (le16(b, 70)?, le16(b, 72)?);
    let mut pos = le32(b, 60)?.saturating_add(60);
    let mut lines = vec![field(slice(b, 17, 20))];
    // XM 1.04 puts the patterns first, and each sample's data after its
    // headers; older files hold the instruments straight after the header.
    let modern = version >= 0x0104;
    if modern {
        for _ in 0..patterns {
            let (Some(header), Some(data)) = (le32(b, pos), le16(b, pos.saturating_add(7))) else { return Some(lines) };
            pos = pos.saturating_add(header).saturating_add(data);
        }
    }
    for _ in 0..instruments {
        let start = pos;
        let Some(size) = le32(b, start) else { return Some(lines) };
        if size < 29 || start >= b.len() {
            return Some(lines);
        }
        let name = field(slice(b, start + 4, 22));
        let count = le16(b, start + 27).unwrap_or(0);
        lines.push(name.clone());
        pos = start.saturating_add(size);
        if count == 0 {
            continue;
        }
        let header = le32(b, start + 29).unwrap_or(0);
        if !(40..=0x100).contains(&header) || count > 16 {
            return Some(lines);
        }
        let mut data = 0usize;
        for s in 0..count {
            let at = pos.saturating_add(s * header);
            let sample = field(slice(b, at + 18, 22));
            if sample != name {
                lines.push(sample);
            }
            data = data.saturating_add(le32(b, at).unwrap_or(0));
        }
        pos = pos.saturating_add(count * header);
        if modern {
            pos = pos.saturating_add(data);
        }
    }
    // OpenMPT's extension chunks after the last instrument.
    if !modern {
        return Some(lines);
    }
    while let (Some(tag), Some(len)) = (b.get(pos..pos.saturating_add(4)), le32(b, pos.saturating_add(4))) {
        if tag == b"text" {
            lines.extend(message(slice(b, pos + 8, len)));
            break;
        }
        if tag == b"XTPM" {
            break;
        }
        pos = pos.saturating_add(8).saturating_add(len);
    }
    Some(lines)
}

/// MultiTracker: the title, 22-byte sample names and the comment, lines of
/// 40 bytes (39 characters) after the track and pattern tables.
fn mtm(b: &[u8]) -> Option<Vec<String>> {
    let (tracks, patterns, extra) = (le16(b, 24)?, usize::from(*b.get(26)?), le16(b, 28)?);
    let samples = usize::from(*b.get(30)?);
    if samples > 63 {
        return None;
    }
    let mut lines = vec![field(slice(b, 4, 20))];
    lines.extend(names(b, 66, 37, 22, samples));
    let at = 66 + 37 * samples + 128 + 192 * tracks + 64 * (patterns + 1);
    lines.extend(slice(b, at, extra).chunks(40).map(|l| field(&l[..l.len().min(39)])));
    Some(lines)
}

/// Ultra Tracker: the title, the message (32-byte lines) and the 32-byte
/// instrument names.
fn ult(b: &[u8]) -> Option<Vec<String>> {
    let version = usize::from(b.get(14)?.checked_sub(b'0')?);
    if !(1..=4).contains(&version) {
        return None;
    }
    let message = usize::from(*b.get(47)?);
    let mut lines = vec![field(slice(b, 15, 32))];
    lines.extend(rows(slice(b, 48, message * 32), 32));
    let at = 48 + message * 32;
    let count = usize::from(*b.get(at)?);
    lines.extend(names(b, at + 1, if version >= 4 { 66 } else { 64 }, 32, count));
    Some(lines)
}

/// Farandole Composer: the title, the text (lines of 132 characters, NULs
/// as spaces) and the sample names, which sit after the patterns.
fn far(b: &[u8]) -> Option<Vec<String>> {
    let text = le16(b, 96)?.min(44 * 132);
    let mut lines = vec![field(slice(b, 4, 40))];
    lines.extend(slice(b, 98, text).chunks(132).map(|l| {
        let spaced: Vec<u8> = l.iter().map(|&c| if c == 0 { b' ' } else { c }).collect();
        field(&spaced)
    }));
    let sizes = 98 + text + 256 + 3;
    let header = le16(b, 47)?;
    let mut pos = if header > 869 + text { header } else { 869 + text };
    for i in 0..256 {
        let Some(size) = le16(b, sizes + 2 * i) else { return Some(lines) };
        pos = pos.saturating_add(size);
    }
    let map = slice(b, pos, 8);
    pos = pos.saturating_add(8);
    if map.len() < 8 {
        return Some(lines);
    }
    for i in 0..64 {
        if map[i / 8] & (1 << (i % 8)) == 0 {
            continue;
        }
        let Some(length) = le32(b, pos.saturating_add(32)) else { break };
        lines.push(field(slice(b, pos, 32)));
        pos = pos.saturating_add(48).saturating_add(length);
    }
    Some(lines)
}

/// Epic MegaGames MASI (PSM) and Sinaria: chunks of id, little-endian
/// length and data. The title is in `TITL`, each sample's name in `DSMP`.
fn psm(b: &[u8]) -> Option<Vec<String>> {
    if slice(b, 8, 4) != b"FILE" {
        return None;
    }
    let all = chunks_by(b, 12, 4, true, false);
    // Sinaria files have 8-character ids; the loader tells them by a
    // pattern chunk's header.
    let sinaria = all.iter().any(|(id, data)| *id == b"PBOD" && data.len() >= 14 && data[9] != 0 && data[13] == 0);
    let mut lines = Vec::new();
    for (id, data) in &all {
        match *id {
            b"TITL" => lines.push(field(data)),
            b"DSMP" => lines.push(field(slice(data, if sinaria { 17 } else { 13 }, 31))),
            _ => {}
        }
    }
    Some(lines)
}

/// Epic MegaGames MASI 16 (Protracker Studio): a 59-byte title and a table
/// of 64-byte instrument records, the name 24 bytes at 13.
fn psm16(b: &[u8]) -> Option<Vec<String>> {
    let (count, at) = (le16(b, 76)?, le32(b, 94)?);
    if count > 255 || at == 0 {
        return None;
    }
    let mut lines = vec![field(slice(b, 4, 59))];
    lines.extend(names(b, at + 13, 64, 24, count));
    Some(lines)
}

/// Digisound Interface Kit: the `SONG` chunk's title and every `INST`
/// chunk's 28-byte name, chunks being id, little-endian length and data.
fn dsm(b: &[u8]) -> Option<Vec<String>> {
    let at = if b.starts_with(b"RIFF") { 12 } else { 16 };
    if slice(b, at, 4) != b"SONG" {
        return None;
    }
    let mut lines = Vec::new();
    for (id, data) in chunks_by(b, at, 4, true, false) {
        match id {
            b"SONG" => lines.push(field(slice(data, 0, 28))),
            b"INST" => lines.push(field(slice(data, 36, 28))),
            _ => {}
        }
    }
    Some(lines)
}

/// Dynamic Studio (`DSm\x1a`): a 20-byte title, then 32-byte sample records
/// after the channel table, the orders and 8 bytes of name per track.
fn dsm_dynamic(b: &[u8]) -> Option<Vec<String>> {
    let (channels, samples, orders) = (usize::from(*b.get(45)?), usize::from(*b.get(46)?), usize::from(*b.get(47)?));
    if b[4] != 0x20 || !(1..=16).contains(&channels) || samples == 0 || orders == 0 {
        return None;
    }
    let patterns = slice(b, 64 + channels, orders).iter().copied().max().map_or(0, usize::from) + 1;
    let mut lines = vec![field(slice(b, 5, 20))];
    lines.extend(names(b, 64 + channels + orders + patterns * channels * 8, 32, 22, samples));
    Some(lines)
}

/// DSMI AMF: the 32-byte title and the 32-byte instrument names, after a
/// header and pattern table whose size depends on the version.
fn amf(b: &[u8]) -> Option<Vec<String>> {
    let version = usize::from(*b.get(3)?);
    if version != 1 && !(8..=0x0e).contains(&version) {
        return None;
    }
    let mut lines = vec![field(slice(b, 4, 32))];
    let (ins, orders) = (usize::from(*b.get(36)?), usize::from(*b.get(37)?));
    let channels = if version >= 9 { usize::from(*b.get(40)?) } else { 4 };
    let mut at = if version >= 9 { 41 } else { 40 };
    if version == 9 || version == 10 {
        at += 16;
    }
    if version >= 0x0b {
        at += if version >= 0x0c { 32 } else { 16 };
    }
    if version >= 0x0d {
        at += 2;
    }
    at += orders * (if version >= 0x0e { 2 } else { 0 } + channels * 2);
    // Instrument records: 65 bytes with 32-bit lengths and loops, 59 with
    // 16-bit ones. Version 1.0 files are of either kind; the loader tells
    // them by whether the 65-byte reading makes sense.
    let size = match version {
        0x0a if amf_wide(b, at, ins) => 65,
        0x0a => 59,
        v if v >= 0x0b => 65,
        _ => 59,
    };
    lines.extend(names(b, at + 1, size, 32, ins));
    Some(lines)
}

/// The loader's probe for AMF 1.0: do `ins` records of 65 bytes from `at`
/// hold sane types, sample numbers, lengths, rates, volumes and loops?
fn amf_wide(b: &[u8], at: usize, ins: usize) -> bool {
    (0..ins).all(|i| {
        let p = at + i * 65;
        let sane = || {
            let len = le32(b, p + 50)?;
            Some(
                matches!(b.get(p)?, 0 | 1)
                    && le32(b, p + 46)? <= ins
                    && len <= 0x10_0000
                    && le16(b, p + 54)? != 0
                    && *b.get(p + 56)? <= 0x40
                    && le32(b, p + 57)? <= len
                    && le32(b, p + 61)? <= len,
            )
        };
        sane() == Some(true)
    })
}

/// X-Tracker DMF: the 30-byte title, the 20-byte composer and the sample
/// names, length-prefixed in the `SMPI` chunk.
fn dmf(b: &[u8]) -> Option<Vec<String>> {
    let version = usize::from(*b.get(4)?);
    if !(1..=10).contains(&version) {
        return None;
    }
    let mut lines = vec![field(slice(b, 13, 30)), field(slice(b, 43, 20))];
    let mut at: usize = 66;
    while let (Some(id), Some(mut len)) = (b.get(at..at.saturating_add(4)), le32(b, at.saturating_add(4))) {
        let skip = match (version, id) {
            (3, b"SEQU") => 2,
            (4, b"SEQU") => 4,
            _ => 0,
        };
        if version < 8 && id == b"SMPD" {
            len = b.len().saturating_sub(at + 8);
        }
        if id == b"SMPI" {
            let data = slice(b, at + 8, len);
            let mut p = 1;
            for _ in 0..usize::from(data.first().copied().unwrap_or(0)) {
                let Some(n) = (if version < 2 { Some(30) } else { data.get(p).map(|&n| usize::from(n)) }) else { break };
                p += usize::from(version >= 2);
                lines.push(field(slice(data, p, n)));
                p += n + 16 + if version >= 8 { 8 } else { 0 } + if version > 1 { 6 } else { 2 };
            }
        }
        at = at.saturating_add(8).saturating_add(len).saturating_add(skip);
    }
    Some(lines)
}

/// MadTracker 2: the 64-byte title, then the 32-byte instrument and sample
/// names, which sit after the patterns (and the automation, which is
/// stepped over).
fn mt2(b: &[u8]) -> Option<Vec<String>> {
    let version = le16(b, 8)?;
    if !(0x200..0x300).contains(&version) {
        return None;
    }
    let mut lines = vec![field(slice(b, 42, 64))];
    let (patterns, channels, flags) = (le16(b, 110)?, le16(b, 112)?, le32(b, 118)?);
    let (instruments, samples) = (le16(b, 122)?, le16(b, 124)?);
    let mut pos = 126 + 256;
    let Some(drums) = le16(b, pos).map(|d| d != 0) else { return Some(lines) };
    pos += 2 + if drums { 274 } else { 0 };
    let Some(extra) = le32(b, pos) else { return Some(lines) };
    // The VST plug-in count, which sizes each pattern's automation.
    let mut vst = 0;
    let mut e = pos + 4;
    while let (Some(id), Some(len)) = (b.get(e..e.saturating_add(4)), le32(b, e.saturating_add(4))) {
        if id == b"VST2" {
            vst = le32(b, e + 8).unwrap_or(0);
        }
        e = e.saturating_add(8).saturating_add(len);
        if e > pos + 4 + extra {
            break;
        }
    }
    pos = pos.saturating_add(4).saturating_add(extra);
    for _ in 0..patterns {
        let Some(len) = le32(b, pos.saturating_add(2)) else { return Some(lines) };
        pos = pos.saturating_add(6).saturating_add(len.saturating_add(1) & !1);
    }
    if flags & 2 != 0 {
        let total = if flags & 8 != 0 { channels + if drums { 8 } else { 0 } } else { channels };
        let each = total + if version >= 0x250 { vst } else { 0 } + usize::from(flags & 0x10 != 0);
        for _ in 0..patterns.saturating_mul(each) {
            let wide = version >= 0x203;
            let Some(mut bits) = (if wide { le32(b, pos) } else { le16(b, pos) }) else { return Some(lines) };
            pos += if wide { 8 } else { 4 };
            while bits != 0 {
                if bits & 1 != 0 {
                    pos = pos.saturating_add(260);
                }
                bits >>= 1;
            }
            if pos > b.len() {
                return Some(lines);
            }
        }
    }
    // 255 instrument records, then 256 sample records: a name, a length and
    // that many bytes of header. A sample often repeats its instrument's name.
    let mut instrument_names = Vec::new();
    for (slots, shown, instrument) in [(255, instruments, true), (256, samples, false)] {
        for i in 0..slots {
            let Some(mut len) = le32(b, pos.saturating_add(32)) else { break };
            if instrument && len == 32 {
                len += 108 + 72 * 4;
            }
            if instrument && version > 0x0201 && len != 0 {
                len += 4;
            }
            if i < shown {
                let name = field(slice(b, pos, 32));
                if instrument {
                    instrument_names.push(name.clone());
                }
                if instrument || !instrument_names.contains(&name) {
                    lines.push(name);
                }
            }
            pos = pos.saturating_add(36).saturating_add(len);
        }
    }
    Some(lines)
}

/// A string with a length byte at `pos`, which moves past it.
fn sized(b: &[u8], pos: &mut usize) -> Option<String> {
    let n = usize::from(*b.get(*pos)?);
    let text = field(slice(b, *pos + 1, n));
    *pos += 1 + n;
    Some(text)
}

/// Extreme's Tracker (AMS 1): the title and the sample names, strings with a
/// length byte, after the sample headers.
fn ams1(b: &[u8]) -> Option<Vec<String>> {
    let samples = usize::from(*b.get(10)?);
    let mut pos = 18usize.checked_add(le16(b, 16)?)?.checked_add(17 * samples)?;
    let mut lines = Vec::new();
    for _ in 0..=samples {
        lines.push(sized(b, &mut pos)?);
    }
    Some(lines)
}

/// Velvet Studio (AMS 2): the title, the composer and, per instrument, its
/// name and its samples' names, which sit after a key table and three
/// envelopes.
fn ams2(b: &[u8]) -> Option<Vec<String>> {
    let mut pos = 7;
    let mut lines = vec![sized(b, &mut pos)?];
    let (minor, instruments) = (*b.get(pos)?, usize::from(*b.get(pos + 2)?));
    pos += 7 + if minor >= 2 { 8 } else { 3 };
    let mut names = Vec::new();
    // A damaged file keeps the names read before the damage, and has no
    // composer to be found after them.
    if ams2_instruments(b, &mut pos, minor, instruments, &mut names).is_some() {
        lines.extend(sized(b, &mut pos));
    }
    lines.extend(names);
    Some(lines)
}

/// `ams2`'s instruments from `pos`, their names pushed to `names`.
fn ams2_instruments(b: &[u8], pos: &mut usize, minor: u8, count: usize, names: &mut Vec<String>) -> Option<()> {
    for _ in 0..count {
        let name = sized(b, pos)?;
        let samples = *b.get(*pos)?;
        *pos += 1;
        names.push(name);
        if samples == 0 {
            continue;
        }
        *pos += if minor > 0 { 120 } else { 96.min(b.len().saturating_sub(*pos)) };
        for _ in 0..3 {
            *pos += 5 + 3 * usize::from(*b.get(*pos + 4)?);
        }
        *pos += 5;
        for _ in 0..samples {
            let name = sized(b, pos)?;
            if !names.contains(&name) {
                names.push(name);
            }
            *pos += 20;
        }
    }
    Some(())
}

/// Digitrakker: chunks of a 2-byte id, a little-endian length and data after
/// the version byte. `IN` has the title and composer, `II` the instruments
/// (with their sub-instrument tables) and `IS` the samples.
fn mdl(b: &[u8]) -> Option<Vec<String>> {
    let version = *b.get(4)?;
    let (mut lines, mut instruments, mut samples) = (Vec::new(), Vec::new(), Vec::new());
    for (id, data) in chunks_by(b, 5, 2, true, false) {
        match id {
            b"IN" => {
                lines.push(field(slice(data, 0, 32)));
                lines.push(field(slice(data, 32, 20)));
            }
            b"II" if version >> 4 != 0 => {
                let mut at = 1;
                for _ in 0..usize::from(*data.first()?) {
                    let subs = usize::from(*data.get(at + 1)?);
                    instruments.push(field(slice(data, at + 2, 32)));
                    at += 34 + 14 * subs;
                }
            }
            b"IS" => {
                // 1.x: 59-byte records with a 32-bit rate; 0.x: 57 bytes, and
                // the sample is the instrument.
                let size = if version >> 4 != 0 { 59 } else { 57 };
                let target = if version >> 4 != 0 { &mut samples } else { &mut instruments };
                target.extend(names(data, 2, size, 32, usize::from(*data.first()?)));
            }
            _ => {}
        }
    }
    samples.retain(|s| !instruments.contains(s));
    lines.extend(instruments);
    lines.extend(samples);
    Some(lines)
}

/// DigiBooster Pro: the 44-byte title at 16, then big-endian chunks; `INFO`
/// counts the instruments and `INST` has 50-byte records with a 30-byte name.
fn dbm(b: &[u8]) -> Option<Vec<String>> {
    let mut lines = vec![field(slice(b, 16, 44))];
    let all = chunks_by(b, 60, 4, false, false);
    let count = all.iter().find(|(id, _)| *id == b"INFO").and_then(|(_, d)| be16(d, 0)).unwrap_or(0);
    for (id, data) in &all {
        match *id {
            b"SONG" => lines.push(field(slice(data, 0, 44))),
            b"INST" => lines.extend(names(data, 0, 50, 30, count.min(255))),
            _ => {}
        }
    }
    Some(lines)
}

/// General Digital Music: the 32-byte title and author, the instrument names
/// (32 bytes in 62-byte records) and the message the header points at.
fn gdm(b: &[u8]) -> Option<Vec<String>> {
    let (ins_at, count) = (le32(b, 127)?, usize::from(*b.get(135)?) + 1);
    let mut lines = vec![field(slice(b, 4, 32)), field(slice(b, 36, 32))];
    let (at, len) = (le32(b, 136)?, le32(b, 140)?);
    // Not every file has one, and the fields are not in the loader's
    // reading: only wholly printable text counts.
    if at > 0 && len > 0 && len < 8192 {
        let raw = slice(b, at, len);
        if raw.len() == len && raw.iter().all(|&c| c == 0 || c == b'\r' || c == b'\n' || (0x20..0x7f).contains(&c)) {
            lines.extend(message(raw));
        }
    }
    lines.extend(names(b, ins_at, 62, 32, count));
    Some(lines)
}

/// Liquid Tracker (old, `NO`): a length-prefixed 29-byte title and 63
/// instruments of 46 bytes, the name a length byte and up to 30 characters.
fn liquid_no(b: &[u8]) -> Option<Vec<String>> {
    let n = usize::from(*b.get(4)?).min(29);
    let mut lines = vec![field(slice(b, 5, n))];
    for i in 0..63 {
        let at = 299 + i * 46;
        let n = usize::from(*b.get(at)?).min(30);
        lines.push(field(slice(b, at + 1, n)));
    }
    Some(lines)
}

/// Megatracker: the song name where the header points, and 32-byte
/// instrument names in 80-byte records.
fn mgt(b: &[u8]) -> Option<Vec<String>> {
    let (song, ins_at, ins) = (be32(b, 26)?, be32(b, 34)?, be16(b, 18)?);
    if ins > 64 {
        return None;
    }
    let mut lines = vec![field(slice(b, song, 32))];
    lines.extend(names(b, ins_at, 80, 32, ins));
    Some(lines)
}

/// Digital Symphony: the track name, after one length byte (and a 24-bit
/// size when it has a sample) per sample. The information text and the
/// instrument names come after LZW-packed blocks.
fn digital_symphony(b: &[u8]) -> Option<Vec<String>> {
    let mut pos = 17;
    for _ in 0..63 {
        let n = *b.get(pos)?;
        pos += if n & 0x80 == 0 { 4 } else { 1 };
    }
    let n = usize::from(*b.get(pos)?);
    Some(vec![field(slice(b, pos + 1, n.min(32)))])
}

/// Symphonie: a stream of 32-bit kinds. The instruments' names (`-14`, 256
/// bytes each, the name in the first 128) and the song text (`-16`) are
/// length-prefixed chunks that are RLE-packed when they start with `PACK`.
fn symphonie(b: &[u8]) -> Option<Vec<String>> {
    if be32(b, 4)? != 1 || be32(b, 8)? != 0xffff_ffff {
        return None;
    }
    let (mut instruments, mut text) = (Vec::new(), Vec::new());
    let mut at = 16;
    while let Some(kind) = be32(b, at) {
        at += 4;
        match kind as u32 as i32 {
            -1 | -2 | -3 | -4 | -5 | -6 | -7 | 10 | 11 | 12 => at = at.saturating_add(4),
            -12 => {}
            -10 | -11 | -13 | -14 | -15 | -16 | -17 | -18 | -19 | -20 | -21 => {
                let Some(len) = be32(b, at) else { break };
                let chunk = slice(b, at + 4, len);
                at = at.saturating_add(4).saturating_add(len);
                match kind as u32 as i32 {
                    -14 if instruments.is_empty() => {
                        instruments = symphonie_unpack(chunk)
                            .chunks_exact(256)
                            .filter(|r| &r[..4] != b"ViRT")
                            .map(|r| field(&r[..128]))
                            .collect();
                    }
                    -16 if text.is_empty() => text = message(&symphonie_unpack(chunk)),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    text.extend(instruments);
    Some(text)
}

/// A Symphonie chunk's data: as stored, or unpacked from the `PACK\xff\xff`
/// run-length format (literal bytes, a repeated dword, a dword twice, zeros).
fn symphonie_unpack(chunk: &[u8]) -> Vec<u8> {
    if chunk.len() < 10 || !chunk.starts_with(b"PACK\xff\xff") {
        return chunk.to_vec();
    }
    let total = be32(chunk, 6).unwrap_or(0).min((chunk.len() - 10).saturating_mul(170)).min(1 << 22);
    let mut out = Vec::with_capacity(total);
    let mut i = 10;
    while i < chunk.len() && out.len() < total {
        let kind = chunk[i] as i8;
        i += 1;
        let Some(&arg) = chunk.get(i) else { break };
        match kind {
            0 => {
                let Some(data) = chunk.get(i + 1..i + 1 + usize::from(arg)) else { break };
                out.extend_from_slice(data);
                i += 1 + usize::from(arg);
            }
            1 => {
                let Some(dword) = chunk.get(i + 1..i + 5) else { break };
                for _ in 0..arg {
                    out.extend_from_slice(dword);
                }
                i += 5;
            }
            2 => {
                let Some(dword) = chunk.get(i..i + 4) else { break };
                out.extend_from_slice(dword);
                out.extend_from_slice(dword);
                i += 4;
            }
            3 => {
                out.resize(out.len() + usize::from(arg), 0);
                i += 1;
            }
            _ => break,
        }
    }
    // The loader refuses a short result rather than pass on part of it.
    if out.len() == total { out } else { Vec::new() }
}

/// Coconizer: a CR-terminated 20-byte title at 1 and 32-byte instrument
/// records after a 32-byte header, the name 11 bytes at 20.
fn coconizer(b: &[u8]) -> Vec<String> {
    let mut lines = vec![field(slice(b, 1, 20))];
    lines.extend(names(b, 32 + 20, 32, 11, usize::from(*b.get(21).unwrap_or(&0)).min(100)));
    lines
}

/// SoundFX 1.3: 15 sample records of 30 bytes from 80 (22-byte names), after
/// the lengths and the `SONG` tag.
fn soundfx(b: &[u8]) -> Vec<String> {
    if slice(b, 60, 4) != b"SONG" {
        return Vec::new();
    }
    names(b, 80, 30, 22, 15)
}

/// JamCracker: `BeEp`, a count, and 40-byte instrument records with a
/// 31-byte name.
fn jamcracker(b: &[u8]) -> Vec<String> {
    if !b.starts_with(b"BeEp") {
        return Vec::new();
    }
    names(b, 6, 40, 31, be16(b, 4).unwrap_or(0).min(255))
}

/// Players that start with jump or branch stubs (`jmp`/`bra` to the entry
/// points) and then carry a line of credit (`Music by Maniacs of
/// Noise/...`, `... player by Sean Connolly`) before their code, which the
/// first stub jumps to. Wholly printable or nothing.
fn stub_text(b: &[u8]) -> Vec<String> {
    let mut at = 0;
    let mut code = usize::MAX;
    while at < 64 && matches!(slice(b, at, 2), [0x60, 0x00] | [0x4e, 0xfa]) {
        let Some(disp) = b.get(at + 2..at + 4).map(|d| i16::from_be_bytes([d[0], d[1]])) else { return Vec::new() };
        code = code.min((at + 2).saturating_add_signed(isize::from(disp)));
        at += 4;
    }
    if at == 0 || code <= at {
        return Vec::new();
    }
    let raw = slice(b, at, (code - at).min(800));
    let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    let mut text = &raw[..end];
    // Padding after the text.
    while let [rest @ .., 0xff] = text {
        text = rest;
    }
    let printable = text.iter().all(|&c| (0x20..0x7f).contains(&c) || c >= 0xa0 || c == b'\n' || c == b'\r');
    if text.len() < 12 || !printable || !text.contains(&b' ') || !text.iter().any(u8::is_ascii_alphabetic) {
        return Vec::new();
    }
    block(text).into_iter().map(|l| l.trim().to_string()).collect()
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// A name field: NUL-padded, and nothing in it below space except padding.
fn looks_like_text(field: &[u8]) -> bool {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    field[..end].iter().all(|&b| b >= 0x20 && b != 0x7f)
}

/// One fixed-length field as text: cut at the first NUL, Latin-1 (the
/// Amiga's character set), control bytes as spaces, trailing spaces dropped.
fn field(raw: &[u8]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    let text: String = raw[..end]
        .iter()
        .map(|&b| match b {
            0x20..=0x7e | 0xa0..=0xff => b as char,
            _ => ' ',
        })
        .collect();
    text.trim_end().to_string()
}

/// Lines as the player shows them: blank lines at either end dropped, a run
/// of blank lines kept as one (composers space paragraphs with an empty
/// sample), and `None` when nothing is left.
fn tidy(lines: Vec<String>) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for line in lines {
        if line.trim().is_empty() && out.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        out.push(if line.trim().is_empty() { String::new() } else { line });
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    (!out.is_empty()).then(|| out.join("\n"))
}


#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
