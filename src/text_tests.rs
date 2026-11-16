use super::*;

fn module(tag: &[u8; 4], title: &str, names: &[&str]) -> Vec<u8> {
    let mut b = vec![0u8; 1084 + 1024];
    b[..title.len()].copy_from_slice(title.as_bytes());
    for (i, n) in names.iter().enumerate() {
        let at = 20 + i * 30;
        b[at..at + n.len()].copy_from_slice(n.as_bytes());
    }
    b[950] = 1;
    b[1080..1084].copy_from_slice(tag);
    b
}

#[test]
fn a_protracker_module_gives_its_title_and_sample_names() {
    let b = module(b"M.K.", "defcon1", &["", "made by f-x", "", "", "greets to all", "\u{0}"]);
    assert_eq!(extract(&b).as_deref(), Some("defcon1\n\nmade by f-x\n\ngreets to all"));
}

#[test]
fn multichannel_and_startrekker_tags_are_modules_too() {
    for tag in [b"6CHN", b"16CH", b"FLT8", b"M!K!"] {
        assert_eq!(extract(&module(tag, "t", &["x"])).as_deref(), Some("t\nx"), "{tag:?}");
    }
}

#[test]
fn a_soundtracker_module_needs_a_plausible_header() {
    let mut b = vec![0u8; 600 + 1024];
    b[..5].copy_from_slice(b"intro");
    b[20..26].copy_from_slice(b"hello!");
    b[470] = 2;
    assert_eq!(extract(&b).as_deref(), Some("intro\nhello!"));
    // Control bytes in a name: not a SoundTracker header.
    b[22] = 0x07;
    assert_eq!(extract(&b), None);
}

#[test]
fn random_bytes_and_silent_modules_say_nothing() {
    let junk: Vec<u8> = (0..4000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    assert_eq!(extract(&junk), None);
    assert_eq!(extract(&module(b"M.K.", "", &[])), None);
    assert_eq!(extract(b"short"), None);
}

#[test]
fn amiga_latin1_survives_and_control_bytes_do_not() {
    let mut b = module(b"M.K.", "x", &[]);
    b[20..24].copy_from_slice(&[0xe9, b't', 0x01, b'e']);
    assert_eq!(extract(&b).as_deref(), Some("x\nét e"));
}

#[test]
fn hively_reads_the_song_and_instrument_names_its_pointer_names() {
    let mut b = b"HVL\x01\x00\x10\x00\x00\x00\x00\x02\x00\x00\x00\x00\x00".to_vec();
    b.extend_from_slice(b"pantsomime\0JOEY&SPLIF\0shout to: porky\0extra\0");
    assert_eq!(
        extract_for("HivelyTracker", &b).as_deref(),
        Some("pantsomime\nJOEY&SPLIF\nshout to: porky")
    );
}

#[test]
fn a_wanted_team_hunk_gives_title_author_and_credits_not_its_tag() {
    let mut b = vec![0u8; 0x20];
    b.extend_from_slice(&[0x70, 0xff, 0x4e, 0x75]);
    b.extend_from_slice(b"FREDGRAY\0\0\0\xe2\0\0\0\x8c");
    b.extend_from_slice(b"Black Lamp\0Fred Gray\0(c) 1988 by FireBird\n\nRipped by X\0\x01\x0c");
    assert_eq!(
        extract_for("Fred Gray", &b).as_deref(),
        Some("Black Lamp\nFred Gray\n(c) 1988 by FireBird\n\nRipped by X")
    );
    // Without the replayer's opening it is not one of these files.
    b[0x20] = 0;
    assert_eq!(extract_for("Fred Gray", &b), None);
}

#[test]
fn art_of_noise_keeps_a_multi_line_remark() {
    let mut b = b"AON4artofnoise".to_vec();
    for (tag, text) in [(b"NAME", &b"dawn"[..]), (b"AUTH", b"twice/lego"), (b"RMRK", b"line one\nline two")] {
        b.extend_from_slice(tag);
        b.extend_from_slice(&(text.len() as u32).to_be_bytes());
        b.extend_from_slice(text);
    }
    assert_eq!(
        extract_for("Art of Noise 4V", &b).as_deref(),
        Some("dawn\ntwice/lego\nline one\nline two")
    );
}

#[test]
fn sap_reads_its_text_header() {
    let b = b"SAP\r\nAUTHOR \"Lukasz Sychowicz\"\r\nNAME \"Drunk Chessboard\"\r\nDATE \"1996\"\r\n\xff\xff";
    assert_eq!(
        extract_for("Pokeynoise", b).as_deref(),
        Some("Drunk Chessboard\nLukasz Sychowicz\n1996")
    );
}

#[test]
fn a_protracker4_comment_stops_at_the_module_image() {
    let mut b = b"FORM\0\0\0\0MODLVERS\0\0\0\x16\0\0\0\0PT4.00".to_vec();
    b.extend_from_slice(b"INFO\0\0\0\x50alwayz happy!\0");
    b.extend_from_slice(b"CMNT\0\0\x10\0hello world\0PTDT\0\0\x10\0");
    let mut image = module(b"M.K.", "ignored", &["by dr.cloo"]);
    image.truncate(1084);
    b.extend_from_slice(&image);
    assert_eq!(
        extract_for("Protracker4", &b).as_deref(),
        Some("alwayz happy!\nhello world\n\nby dr.cloo")
    );
}

#[test]
fn placeholders_and_binary_name_fields_are_not_shown() {
    let b = module(b"M.K.", "song", &["Instrument 1", "sample 02", "(empty)", "real words"]);
    assert_eq!(extract_for("ProTracker", &b).as_deref(), Some("song\n\nreal words"));
    // A Master SoundTracker file with something else in its name fields.
    let mut junk = module(b"M.K.", "x", &[]);
    junk[20] = 0x03;
    assert_eq!(extract_for("Master SoundTracker", &junk), None);
}

// -- PC trackers and the other readers added with them --------------------

fn put(b: &mut Vec<u8>, at: usize, data: &[u8]) {
    if b.len() < at + data.len() {
        b.resize(at + data.len(), 0);
    }
    b[at..at + data.len()].copy_from_slice(data);
}

fn le(n: usize, bytes: usize) -> Vec<u8> {
    n.to_le_bytes()[..bytes].to_vec()
}

fn be(n: usize, bytes: usize) -> Vec<u8> {
    n.to_be_bytes()[8 - bytes..].to_vec()
}

/// `text`, NUL-padded or cut to `len` bytes.
fn pad(text: &str, len: usize) -> Vec<u8> {
    let mut v = text.as_bytes().to_vec();
    v.resize(len, 0);
    v
}

/// A length byte and the string.
fn sized_str(text: &str) -> Vec<u8> {
    let mut v = vec![text.len() as u8];
    v.extend_from_slice(text.as_bytes());
    v
}

fn chunk_le(id: &[u8], data: &[u8]) -> Vec<u8> {
    let mut v = id.to_vec();
    v.extend(le(data.len(), 4));
    v.extend_from_slice(data);
    v
}

fn chunk_be(id: &[u8], data: &[u8]) -> Vec<u8> {
    let mut v = id.to_vec();
    v.extend(be(data.len(), 4));
    v.extend_from_slice(data);
    v
}

fn s3m_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"My tune");
    put(&mut b, 32, &le(2, 2));
    put(&mut b, 34, &le(2, 2));
    put(&mut b, 44, b"SCRM");
    // two orders, then the instruments' paragraph pointers
    put(&mut b, 98, &le(8, 2));
    put(&mut b, 100, &le(13, 2));
    put(&mut b, 128, &[1]);
    put(&mut b, 128 + 48, b"first sample");
    put(&mut b, 128 + 76, b"SCRS");
    // an empty slot that still has a name typed into it
    put(&mut b, 208 + 48, b"typed in an empty slot");
    put(&mut b, 288, &[0]);
    b
}

fn it_file(flags: usize) -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"IMPM");
    put(&mut b, 4, b"it title");
    for (at, n) in [(32, 1), (34, 1), (36, 1), (44, flags), (46, 1)] {
        put(&mut b, at, &le(n, 2));
    }
    let message = b"line one\r\nline two\rthree\0";
    put(&mut b, 54, &le(message.len(), 2));
    put(&mut b, 56, &le(500, 4));
    // one order, so the pointer tables start at 193
    put(&mut b, 193, &le(300, 4));
    put(&mut b, 197, &le(400, 4));
    put(&mut b, 300, b"IMPI");
    put(&mut b, 300 + 32, b"instrument one");
    put(&mut b, 400, b"IMPS");
    put(&mut b, 400 + 20, b"sample one");
    put(&mut b, 500, message);
    b
}

fn xm_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"Extended Module: ");
    put(&mut b, 17, b"xm title");
    put(&mut b, 37, &[0x1a]);
    put(&mut b, 38, b"FastTracker v2.00   ");
    put(&mut b, 58, &le(0x0104, 2));
    put(&mut b, 60, &le(276, 4));
    put(&mut b, 70, &le(1, 2));
    put(&mut b, 72, &le(1, 2));
    // one pattern (a 9-byte header, 4 bytes of data), then one instrument
    put(&mut b, 336, &le(9, 4));
    put(&mut b, 343, &le(4, 2));
    put(&mut b, 349, &le(263, 4));
    put(&mut b, 353, b"xm instrument");
    put(&mut b, 349 + 27, &le(1, 2));
    put(&mut b, 349 + 29, &le(40, 4));
    // its sample: 3 bytes of data after the 40-byte header
    put(&mut b, 612, &le(3, 4));
    put(&mut b, 612 + 18, b"xm sample");
    put(&mut b, 652, &[1, 2, 3]);
    // OpenMPT's song comment
    let comment = b"hello\rworld\0";
    put(&mut b, 655, b"text");
    put(&mut b, 659, &le(comment.len(), 4));
    put(&mut b, 663, comment);
    b
}

fn mtm_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"MTM\x10");
    put(&mut b, 4, b"mtm title");
    put(&mut b, 24, &le(1, 2));
    put(&mut b, 28, &le(80, 2));
    put(&mut b, 30, &[1]);
    put(&mut b, 32, &[64, 1]);
    put(&mut b, 66, b"mtm sample");
    // 66 + 37 + 128 + 192 * 1 track + 64 * 1 pattern
    put(&mut b, 487, b"first line");
    put(&mut b, 527, b"second line");
    put(&mut b, 566, &[0]);
    b
}

fn c669_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"if");
    put(&mut b, 2, b"line one");
    put(&mut b, 2 + 72, b"line three");
    put(&mut b, 111, &[1]);
    put(&mut b, 240, &[0xff]);
    b
}

fn ult_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"MAS_UTrack_V004");
    put(&mut b, 15, b"ult title");
    put(&mut b, 47, &[2]);
    put(&mut b, 48, b"message one");
    put(&mut b, 80, b"message two");
    put(&mut b, 112, &[1]);
    put(&mut b, 113, b"ult instrument");
    put(&mut b, 113 + 65, &[0]);
    b
}

fn far_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"FAR\xfe");
    put(&mut b, 4, b"far title");
    put(&mut b, 44, b"\r\n\x1a");
    put(&mut b, 47, &le(869 + 132, 2));
    put(&mut b, 96, &le(132, 2));
    put(&mut b, 98, b"far text");
    // no patterns; the sample map says sample 0 exists
    put(&mut b, 869 + 132, &[1]);
    put(&mut b, 869 + 132 + 8, b"far sample");
    put(&mut b, 869 + 132 + 8 + 48, &[0]);
    b
}

fn stm_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"stm title");
    put(&mut b, 20, b"!Scream!");
    put(&mut b, 28, &[0x1a, 2, 2, 21]);
    put(&mut b, 48, b"stm sample");
    put(&mut b, 48 + 31 * 32, &[0]);
    b
}

fn stx_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"stx title");
    put(&mut b, 20, b"!Scream!");
    put(&mut b, 34, &le(5, 2));
    put(&mut b, 50, &le(1, 2));
    put(&mut b, 60, b"SCRM");
    put(&mut b, 80, &le(10, 2));
    put(&mut b, 160, &[1]);
    put(&mut b, 160 + 48, b"stx sample");
    put(&mut b, 160 + 76, b"SCRS");
    b
}

fn psm_file() -> Vec<u8> {
    let mut b = b"PSM \0\0\0\0FILE".to_vec();
    b.extend(chunk_le(b"TITL", b"psm title"));
    b.extend(chunk_le(b"DSMP", &[&[0u8; 13][..], &pad("psm sample", 37)].concat()));
    b
}

fn psm16_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"PSM\xfe");
    put(&mut b, 4, b"psm16 title");
    put(&mut b, 76, &le(1, 2));
    put(&mut b, 94, &le(100, 4));
    put(&mut b, 100 + 13, b"psm16 sample");
    put(&mut b, 100 + 63, &[0]);
    b
}

fn dsm_file() -> Vec<u8> {
    let mut b = b"RIFF\0\0\0\0DSMF".to_vec();
    b.extend(chunk_le(b"SONG", &pad("dsm title", 192)));
    b.extend(chunk_le(b"INST", &[&[0u8; 36][..], &pad("dsm sample", 28)].concat()));
    b
}

fn dsm_dynamic_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"DSm\x1a");
    put(&mut b, 4, &[0x20]);
    put(&mut b, 5, b"dss title");
    put(&mut b, 45, &[1, 1, 1]);
    // one channel, one order (pattern 0), 8 bytes of track name
    put(&mut b, 74, b"dss sample");
    put(&mut b, 74 + 31, &[0]);
    b
}

fn amf_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"AMF\x0b");
    put(&mut b, 4, b"amf title");
    put(&mut b, 36, &[1, 1]);
    put(&mut b, 38, &le(1, 2));
    put(&mut b, 40, &[4]);
    // 41 + 16 (pan table) + 1 order of 4 channels * 2
    put(&mut b, 66, b"amf sample");
    put(&mut b, 66 + 64, &[0]);
    b
}

fn dmf_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"DDMF");
    put(&mut b, 4, &[8]);
    put(&mut b, 13, b"dmf title");
    put(&mut b, 43, b"dmf artist");
    let mut smpi = vec![1, 10];
    smpi.extend_from_slice(b"dmf sample");
    smpi.extend_from_slice(&[0; 30]);
    put(&mut b, 66, &chunk_le(b"SMPI", &smpi));
    b
}

fn mt2_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"MT20");
    put(&mut b, 8, &le(0x0204, 2));
    put(&mut b, 42, b"mt2 title");
    put(&mut b, 112, &le(1, 2));
    put(&mut b, 122, &le(1, 2));
    put(&mut b, 124, &le(1, 2));
    // no drums, no extra chunks, no patterns: 255 instruments, 256 samples
    put(&mut b, 388, b"mt2 instrument");
    put(&mut b, 388 + 255 * 36, b"mt2 sample");
    put(&mut b, 388 + 255 * 36 + 256 * 36 - 1, &[0]);
    b
}

fn ams1_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"Extreme");
    put(&mut b, 8, &[1]);
    put(&mut b, 10, &[1]);
    // one 17-byte sample header at 18, then the strings
    let mut tail = sized_str("ams1 title");
    tail.extend(sized_str("ams1 sample"));
    put(&mut b, 35, &tail);
    b
}

fn ams2_file() -> Vec<u8> {
    let mut b = b"AMShdr\x1a".to_vec();
    b.extend(sized_str("ams2 title"));
    // minor 1, major 2, one instrument; tempo, speed, flags
    b.extend_from_slice(&[1, 2, 1, 0, 0, 0, 0, 0, 0, 0]);
    b.extend(sized_str("ams2 instrument"));
    // one sample, a 120-byte key table, three empty envelopes, 5 bytes
    b.push(1);
    b.extend_from_slice(&[0; 120 + 15 + 5]);
    b.extend(sized_str("ams2 sample"));
    b.extend_from_slice(&[0; 20]);
    b.extend(sized_str("ams2 composer"));
    b
}

fn mdl_file() -> Vec<u8> {
    let mut b = b"DMDL\x11".to_vec();
    b.extend(chunk_le(b"IN", &[pad("mdl title", 32), pad("mdl author", 20)].concat()));
    b.extend(chunk_le(b"II", &[&[1, 0, 1][..], &pad("mdl instrument", 32), &[0; 14]].concat()));
    b.extend(chunk_le(b"IS", &[&[1, 0][..], &pad("mdl sample", 32), &[0; 26]].concat()));
    b
}

fn dbm_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"DBM0");
    put(&mut b, 16, b"dbm title");
    let mut chunks = chunk_be(b"INFO", &[0, 1]);
    chunks.extend(chunk_be(b"SONG", &pad("dbm song", 46)));
    chunks.extend(chunk_be(b"INST", &pad("dbm instrument", 50)));
    put(&mut b, 60, &chunks);
    b
}

fn gdm_file() -> Vec<u8> {
    let mut b = vec![0u8; 400];
    put(&mut b, 0, b"GDM\xfe");
    put(&mut b, 4, b"gdm title");
    put(&mut b, 36, b"gdm author");
    put(&mut b, 0x47, b"GMFS");
    put(&mut b, 127, &le(150, 4));
    let message = b"gdm line one\r\nline two";
    put(&mut b, 136, &le(300, 4));
    put(&mut b, 140, &le(message.len(), 4));
    put(&mut b, 150, b"gdm instrument");
    put(&mut b, 300, message);
    b
}

fn no_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"NO\0\0");
    put(&mut b, 4, &[8]);
    put(&mut b, 5, b"no title");
    put(&mut b, 299, &[9]);
    put(&mut b, 300, b"no sample");
    put(&mut b, 299 + 63 * 46 - 1, &[0]);
    b
}

fn mgt_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"MGT\x10\xbdMCS");
    put(&mut b, 18, &be(1, 2));
    put(&mut b, 26, &be(100, 4));
    put(&mut b, 34, &be(150, 4));
    put(&mut b, 100, b"mgt title");
    put(&mut b, 150, b"mgt instrument");
    put(&mut b, 150 + 79, &[0]);
    b
}

fn symphony_file() -> Vec<u8> {
    let mut b = vec![2, 1, 0x13, 0x13, 0x14, 0x12, 1, 0x0b, 0];
    put(&mut b, 17, &[0x80; 63]);
    put(&mut b, 80, &sized_str("symphony set"));
    b
}

/// A Symphonie file whose packed text claims `extra` bytes more than it holds.
fn symphonie_claiming(extra: usize) -> Vec<u8> {
    let mut b = b"SymM".to_vec();
    b.extend([be(1, 4), be(0xffff_ffff, 4), be(4, 4)].concat());
    // a kind with a 4-byte value, the instruments (-14), the text (-16)
    b.extend([be(0xffff_ffff, 4), be(0, 4)].concat());
    b.extend(be(0xffff_fff2, 4));
    b.extend(chunk_be(b"", &pad("bass drum", 256)));
    let text = b"first line\nsecond line";
    let mut packed = b"PACK\xff\xff".to_vec();
    packed.extend(be(text.len() + extra, 4));
    packed.extend([0, text.len() as u8]);
    packed.extend_from_slice(text);
    packed.push(0xff);
    b.extend(be(0xffff_fff0, 4));
    b.extend(chunk_be(b"", &packed));
    b
}

fn symphonie_file() -> Vec<u8> {
    symphonie_claiming(0)
}

fn funk_file() -> Vec<u8> {
    let mut b = vec![0u8; 1500];
    put(&mut b, 0, b"Funk");
    put(&mut b, 5, &[0x30, 0x11]);
    put(&mut b, 8, &le(1500, 4));
    put(&mut b, 401, b"funk instrument");
    b
}

fn asylum_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"ASYLUM Music Format V1.0");
    put(&mut b, 34, &[1]);
    put(&mut b, 294, b"asylum sample");
    put(&mut b, 294 + 36, &[0]);
    b
}

fn chip_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"chip title");
    put(&mut b, 22, b"chip sample");
    put(&mut b, 952, b"KRIS");
    b
}

fn coconizer_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, &[0x84]);
    put(&mut b, 1, b"coco title\r");
    put(&mut b, 21, &[1]);
    put(&mut b, 52, b"coco sample");
    put(&mut b, 99, &[0]);
    b
}

fn ims_file() -> Vec<u8> {
    let mut b = Vec::new();
    put(&mut b, 0, b"ims title");
    put(&mut b, 20, b"ims sample");
    put(&mut b, 20 + 31 * 30, &[0]);
    b
}

fn stubs_file(opcode: [u8; 2], text: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    // three stubs; the first jumps to the code at 48
    put(&mut b, 0, &[opcode[0], opcode[1], 0, 46]);
    put(&mut b, 4, &[opcode[0], opcode[1], 1, 0]);
    put(&mut b, 8, &[opcode[0], opcode[1], 1, 0]);
    put(&mut b, 12, text);
    put(&mut b, 48, &[0x4b, 0xfa, 6, 0x8a]);
    b
}

/// Every synthetic file: the format name it is read under, its bytes, a
/// length short of any text, and what it says.
fn samples() -> Vec<(&'static str, Vec<u8>, usize, &'static str)> {
    let mut jam = Vec::new();
    put(&mut jam, 0, b"BeEp");
    put(&mut jam, 4, &be(1, 2));
    put(&mut jam, 6, b"jam instrument");
    put(&mut jam, 45, &[0]);
    let mut fx = Vec::new();
    put(&mut fx, 60, b"SONG");
    put(&mut fx, 80, b"fx sample");
    put(&mut fx, 80 + 30 * 14, &[0]);
    let mut mug = Vec::new();
    put(&mut mug, 80, b"mug2 song");
    put(&mut mug, 80 + 16 * 7, &[0]);
    vec![
        ("Scream Tracker 3.20 S3M", s3m_file(), 44, "My tune\nfirst sample\ntyped in an empty slot"),
        ("Impulse Tracker 2.14 IT 2.14", it_file(4), 4, "it title\nline one\nline two\nthree\ninstrument one\nsample one"),
        ("OpenMPT 1.30 XM 1.04", xm_file(), 17, "xm title\nxm instrument\nxm sample\nhello\nworld"),
        ("MultiTracker 1.00 MTM", mtm_file(), 4, "mtm title\nmtm sample\nfirst line\nsecond line"),
        ("Composer 669", c669_file(), 100, "line one\n\nline three"),
        ("Ultra Tracker 1.6 ULT V004", ult_file(), 15, "ult title\nmessage one\nmessage two\nult instrument"),
        ("Farandole Composer 1.0", far_file(), 4, "far title\nfar text\nfar sample"),
        ("Scream Tracker 2.21", stm_file(), 40, "stm title\nstm sample"),
        ("STM2STX 1.1", stx_file(), 40, "stx title\nstx sample"),
        ("Epic MegaGames MASI PSM", psm_file(), 12, "psm title\npsm sample"),
        ("Epic MegaGames MASI 16 PSM 1.00", psm16_file(), 4, "psm16 title\npsm16 sample"),
        ("DSIK RIFF DSM", dsm_file(), 12, "dsm title\ndsm sample"),
        ("Dynamic Studio", dsm_dynamic_file(), 4, "dss title\ndss sample"),
        ("DSMI 1.1 AMF", amf_file(), 4, "amf title\namf sample"),
        ("Delusion Digital Music Format v8 (X-Tracker)", dmf_file(), 4, "dmf title\ndmf artist\ndmf sample"),
        ("MadTracker 2.04", mt2_file(), 4, "mt2 title\nmt2 instrument\nmt2 sample"),
        ("Extreme's Tracker AMS", ams1_file(), 9, "ams1 title\nams1 sample"),
        ("Velvet Studio AMS", ams2_file(), 8, "ams2 title\nams2 composer\nams2 instrument\nams2 sample"),
        ("Digitrakker MDL 1.1", mdl_file(), 5, "mdl title\nmdl author\nmdl instrument\nmdl sample"),
        ("DigiBooster Pro 3.10 DBM0", dbm_file(), 4, "dbm title\ndbm song\ndbm instrument"),
        ("GDM 1.00 (2GDM 1.00)", gdm_file(), 0x47, "gdm title\ngdm author\ngdm line one\nline two\ngdm instrument"),
        ("Imago Orpheus 1.0 IMF", [&[0u8; 60][..], b"IM10"].concat(), 60, ""),
        ("Liquid Tracker", no_file(), 4, "no title\nno sample"),
        ("Megatracker MGT v1.0", mgt_file(), 8, "mgt title\nmgt instrument"),
        ("Digital Symphony", symphony_file(), 8, "symphony set"),
        ("Symphonie", symphonie_file(), 16, "first line\nsecond line\nbass drum"),
        ("Funktracker", funk_file(), 12, "funk instrument"),
        ("Asylum Music Format v1.0", asylum_file(), 24, "asylum sample"),
        ("Chiptracker", chip_file(), 900, "chip title\nchip sample"),
        ("Coconizer", coconizer_file(), 0, "coco title\ncoco sample"),
        ("Images Music System", ims_file(), 0, "ims title\nims sample"),
        ("Digital Mugician 2", mug, 0, "mug2 song"),
        ("SoundFX1.3", fx, 0, "fx sample"),
        ("JamCracker", jam, 0, "jam instrument"),
        ("Maniacs of Noise", stubs_file([0x4e, 0xfa], b"  Music by A.Person 01-02-90\0"), 12, "Music by A.Person 01-02-90"),
        ("Sean Connolly", stubs_file([0x60, 0x00], b"Player by Sean. Merry xmas!\0"), 12, "Player by Sean. Merry xmas!"),
        ("Special FX", stubs_file([0x60, 0x00], b"'Wild' music by J. Dunn, ripped\xff\xff"), 12, "'Wild' music by J. Dunn, ripped"),
    ]
}

#[test]
fn every_pc_tracker_and_added_reader_gives_what_its_header_holds() {
    for (format, bytes, _, expected) in samples() {
        let got = extract_for(format, &bytes);
        assert_eq!(got.as_deref().unwrap_or(""), expected, "{format}");
    }
}

#[test]
fn a_truncated_file_gives_nothing_or_a_prefix_and_never_panics() {
    for (format, bytes, cut, _) in samples() {
        assert_eq!(extract_for(format, &[]), None, "{format}: empty");
        if cut > 0 {
            assert_eq!(extract_for(format, &bytes[..cut]), None, "{format}: cut at {cut}");
        }
        for n in (0..bytes.len()).step_by(if bytes.len() > 4000 { 7 } else { 1 }) {
            let _ = extract_for(format, &bytes[..n]);
        }
    }
}

#[test]
fn garbage_after_a_header_never_panics_or_shows_control_bytes() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    for (format, bytes, cut, _) in samples() {
        let header = cut.max(8).min(bytes.len());
        for round in 0..4 {
            let mut junk = bytes.clone();
            for (i, byte) in junk.iter_mut().enumerate().skip(header) {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                // sparse noise on top of the zeroes, wild on later rounds
                if round > 1 || i % 3 == 0 {
                    *byte = (seed >> 33) as u8;
                }
            }
            if let Some(text) = extract_for(format, &junk) {
                assert!(!text.chars().any(|c| c.is_control() && c != '\n'), "{format}: {text:?}");
            }
        }
    }
}

#[test]
fn it_without_instruments_shows_its_sample_names() {
    let got = extract_for("Impulse Tracker 2.14 IT 2.14", &it_file(0));
    assert_eq!(got.as_deref(), Some("it title\nline one\nline two\nthree\nsample one"));
}

#[test]
fn an_xm_sample_named_like_its_instrument_is_not_shown_twice() {
    let mut b = xm_file();
    put(&mut b, 612 + 18, &pad("xm instrument", 22));
    assert_eq!(
        extract_for("FastTracker v2.00 XM 1.04", &b).as_deref(),
        Some("xm title\nxm instrument\nhello\nworld")
    );
}

#[test]
fn a_tagged_mod_is_never_read_as_a_pc_module() {
    let b = module(b"M.K.", "IMPM", &["x"]);
    assert_eq!(extract_for("ProTracker", &b).as_deref(), Some("IMPM\nx"));
}

#[test]
fn a_symphonie_text_that_does_not_unpack_to_its_size_is_dropped() {
    let got = extract_for("Symphonie", &symphonie_claiming(5));
    assert_eq!(got.as_deref(), Some("bass drum"));
}

#[test]
fn stub_credits_stop_where_the_code_starts() {
    // Text running into the code that the first stub jumps to is cut there.
    let mut b = stubs_file([0x4e, 0xfa], b"  Credit by Somebody Else  ");
    put(&mut b, 40, b"K\xfa\x06\x8a\x08\xad");
    assert_eq!(extract_for("Maniacs of Noise", &b).as_deref(), Some("Credit by Somebody Else"));
    // Code that merely follows the stubs is not text.
    let mut code = stubs_file([0x4e, 0xfa], b"");
    put(&mut code, 12, &[0x4b, 0xfa, 0x06, 0x8a, 0x08, 0xad, 0x00, 0x00, 0x01, 0x9a, 0x4a, 0x00, 0x67, 0x08]);
    assert_eq!(extract_for("Maniacs of Noise", &code), None);
}

#[test]
fn module_text_is_the_title_and_sample_names_without_placeholders() {
    let m = crate::testdata::minimal_mod_titled("greetings");
    assert_eq!(module_text("ProTracker", &m), "greetings\nsquare");
    assert_eq!(module_text("ProTracker", &[]), "");
    assert_eq!(module_text("PSID", b"PSID\0\x02 hello"), "");
}

#[test]
fn module_text_unwraps_what_the_song_does() {
    let m = crate::testdata::minimal_mod_titled("greetings");
    #[cfg(feature = "archives")]
    {
        use std::io::Write;
        let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&m).expect("in memory");
        assert_eq!(module_text("ProTracker", &z.finish().expect("in memory")), "greetings\nsquare");
    }
    let song = crate::Song::open("a.mod", &m, 44100).expect("a valid module");
    assert_eq!(song.text(), "greetings\nsquare");
}

#[test]
fn module_text_never_panics_on_any_prefix_under_any_format() {
    let m = crate::testdata::minimal_mod_titled("greetings");
    for format in ["ProTracker", "Oktalyzer", "HivelyTracker", "Symphonie", "OpenMPT 1.30 IT 2.14", "Digital Mugician 2", "Maniacs of Noise", "Dave Lowe", "Musicline Editor"] {
        for n in (0..m.len()).step_by(7) {
            let _ = module_text(format, &m[..n]);
        }
    }
}
