//! Per-program configuration (`m68k-recomp hunk --config prog.toml`).
//!
//! ```toml
//! # Top-level keys first (TOML puts every key after a [table] header into that table).
//! base = 0x20000               # load address of the first section (raw: of the image)
//! bases = [0x20000, 0x30000]   # or: the load address of every section (hunk files)
//! align = 0x100                # section alignment when placing from `base` (default 256)
//! hunk_gap = 8                 # bytes kept free after a section (default 8: the next section's
//!                              # LoadSeg header, size + next pointer, sits just before it)
//! pcrel_seeds = true           # `pea/lea func(pc)` targets that start with a prologue are seeds
//! gap_seeds = true             # `link` prologues in code no seed reached are seeds (unreferenced)
//! overlay_stubs = true         # overlaid files: the overlay entries named by the supervisor's
//!                              # stubs are seeds, the stubs themselves data (they are rewritten)
//! entries = [0x1000]           # raw images: entry points (hunk: the first code section)
//! profile = "manx"             # generic|asm, manx|aztec, sasc|lattice
//! out = "../out/rr_gen"        # relative paths are relative to this file
//! name = "rr_gen"
//! flat = false
//!
//! seeds = ["0:00006", 0x20100] # extra function entries: "<section>:<hex offset>", hex string or integer
//! seeds_files = ["rr_seeds.txt"]   # same line format as --seeds
//! names_files = ["rr_names.txt"]   # same line format as --names
//! ir = true                    # compile through the IR (same as --ir); ir_split = 400
//! names = true                 # readable function names (same as --names; a [names] table
//!                              # below is the other form of this key, see NamesCfg)
//! rr_notes = ["rr_names.txt"]  # Rocket Ranger notes: names, doc comments, role tags
//! keep_interpreted = ["0:1f00"]   # entries left to the interpreter (also: map tags)
//! guarded = ["0:2000"]         # entries that check their code words first (IR only)
//! # [[map]] file = "wings-map.json" / format = "wings"|"dotc"|"sinbad" / program = "Defender"
//! coverage = ["cov.txt"]       # dispatcher coverage files (see coverage.rs)
//!
//! data = [["0:1000", "0:1200"]]     # [start, end) ranges that are data, never compiled
//! exclude = [["0:3000", "0:3400"]]  # [start, end) code ranges left to the interpreter
//!
//! [input]
//! adf = "/path/Disk.1"         # or: file = "prog" (hunk executable, or raw image with kind = "raw")
//! path = "RR"                  # file inside the ADF
//! kind = "hunk"                # "hunk" (default) or "raw"
//!
//! [names]
//! "0:00006" = "anim_open_views"
//!
//! [[hint]]                     # a computed jump through a table
//! at = "0:2468"                # address of the jmp/jsr
//! table = "0:2470"
//! count = 12
//! format = "rel16"             # rel16 (word offsets from `base`, default the table),
//!                              # abs32 (absolute pointers), code (entries are code, `stride` bytes apart)
//! [[hint]]                     # or an explicit target list
//! at = 0x21234
//! targets = [0x21300, "0:1310"]
//! ```
use crate::Image;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// An address: integer, `"<section>:<hex offset>"`, or a hex string (`"1a2b"`, `"0x1a2b"`, `"$1a2b"`).
#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum Addr {
    Int(u64),
    Str(String),
}

impl Addr {
    pub fn resolve(&self, bases: &[u32]) -> Result<u32, String> {
        match self {
            Addr::Int(v) => u32::try_from(*v).map_err(|_| format!("address {:#x} out of range", v)),
            Addr::Str(s) => parse_addr(s, bases),
        }
    }
}

pub fn parse_hex(s: &str) -> Result<u32, String> {
    let t = s.trim();
    let t = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).or_else(|| t.strip_prefix('$')).unwrap_or(t);
    u32::from_str_radix(t, 16).map_err(|_| format!("bad hex number {:?}", s))
}

/// `"<section>:<hex offset>"` or a hex absolute address.
pub fn parse_addr(s: &str, bases: &[u32]) -> Result<u32, String> {
    match s.split_once(':') {
        Some((sec, o)) => {
            let i: usize = sec.trim().parse().map_err(|_| format!("bad section in {:?}", s))?;
            let b = bases.get(i).ok_or_else(|| format!("no section {} in {:?}", i, s))?;
            Ok(b.wrapping_add(parse_hex(o)?))
        }
        None => parse_hex(s),
    }
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub file: Option<String>,
    pub adf: Option<String>,
    pub path: Option<String>,
    pub kind: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct HintCfg {
    pub at: Addr,
    pub table: Option<Addr>,
    pub count: Option<u32>,
    pub format: Option<String>,
    pub base: Option<Addr>,
    pub stride: Option<u32>,
    #[serde(default)]
    pub targets: Vec<Addr>,
}

impl HintCfg {
    /// The hinted jump/call address and its targets.
    pub fn resolve(&self, img: &Image, bases: &[u32]) -> Result<(u32, Vec<u32>), String> {
        let at = self.at.resolve(bases)?;
        let mut out = Vec::new();
        for t in &self.targets {
            out.push(t.resolve(bases)?);
        }
        if let Some(tbl) = &self.table {
            let tbl = tbl.resolve(bases)?;
            let count = self.count.ok_or_else(|| format!("hint at ${:x}: table needs count", at))?;
            let fmt = self.format.as_deref().unwrap_or("rel16");
            for i in 0..count {
                let t = match fmt {
                    "rel16" => {
                        let base = match &self.base {
                            Some(b) => b.resolve(bases)?,
                            None => tbl,
                        };
                        let w = img.word(tbl + 2 * i).ok_or_else(|| format!("hint at ${:x}: table entry {} outside the image", at, i))?;
                        base.wrapping_add(w as i16 as i32 as u32)
                    }
                    "abs32" => img.long(tbl + 4 * i).ok_or_else(|| format!("hint at ${:x}: table entry {} outside the image", at, i))?,
                    "code" => tbl + self.stride.unwrap_or(4) * i,
                    f => return Err(format!("hint at ${:x}: unknown table format {:?} (rel16, abs32, code)", at, f)),
                };
                out.push(t);
            }
        }
        if out.is_empty() {
            return Err(format!("hint at ${:x}: needs `targets` or `table` + `count`", at));
        }
        for &t in &out {
            if t & 1 != 0 || img.word(t).is_none() {
                return Err(format!("hint at ${:x}: target ${:x} is odd or outside the image", at, t));
            }
        }
        Ok((at, out))
    }
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub input: Input,
    pub base: Option<Addr>,
    #[serde(default)]
    pub bases: Vec<Addr>,
    pub align: Option<u32>,
    pub hunk_gap: Option<u32>,
    pub pcrel_seeds: Option<bool>,
    pub gap_seeds: Option<bool>,
    pub overlay_stubs: Option<bool>,
    #[serde(default)]
    pub entries: Vec<Addr>,
    pub profile: Option<String>,
    pub out: Option<String>,
    pub name: Option<String>,
    pub flat: Option<bool>,
    pub no_flag_elim: Option<bool>,
    /// Count 68000 cycles in the generated code (`--cycles`).
    pub cycles: Option<bool>,
    #[serde(default)]
    pub seeds: Vec<Addr>,
    #[serde(default)]
    pub seeds_files: Vec<String>,
    /// `names = true`: readable function names (`--names`); or a `[names]` table of
    /// address -> description (see [`NamesCfg`]).
    #[serde(default)]
    pub names: NamesCfg,
    #[serde(default)]
    pub names_files: Vec<String>,
    #[serde(default)]
    pub coverage: Vec<String>,
    /// Compile through the IR (`--ir`).
    pub ir: Option<bool>,
    /// Largest function emitted in one piece (`--ir-split`).
    pub ir_split: Option<usize>,
    /// Rocket Ranger notes files (`<addr> <name> [note]`): names, doc comments, tags.
    #[serde(default)]
    pub rr_notes: Vec<String>,
    /// Game maps (see [`MapCfg`]).
    #[serde(default, rename = "map")]
    pub maps: Vec<MapCfg>,
    /// Entries (section:offset or address) that stay on the interpreter path, besides the map tags.
    #[serde(default)]
    pub keep_interpreted: Vec<Addr>,
    /// Entries that check their code words at entry (IR only), besides the map tags.
    #[serde(default)]
    pub guarded: Vec<Addr>,
    #[serde(default)]
    pub data: Vec<[Addr; 2]>,
    #[serde(default)]
    pub exclude: Vec<[Addr; 2]>,
    #[serde(default, rename = "hint")]
    pub hints: Vec<HintCfg>,
}

/// The `names` key: `names = true` turns on readable function names (`--names`), a `[names]`
/// table gives descriptions by address. TOML has one value per key, so a config with a table
/// enables the readable names with `--names` on the command line.
#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum NamesCfg {
    Flag(bool),
    Table(BTreeMap<String, String>),
}

impl Default for NamesCfg {
    fn default() -> Self {
        NamesCfg::Table(BTreeMap::new())
    }
}

impl NamesCfg {
    /// True for `names = true`.
    pub fn enabled(&self) -> bool {
        matches!(self, NamesCfg::Flag(true))
    }
    /// The `[names]` table (empty for the flag).
    pub fn table(&self) -> BTreeMap<String, String> {
        match self {
            NamesCfg::Table(t) => t.clone(),
            NamesCfg::Flag(_) => BTreeMap::new(),
        }
    }
}

/// `[[map]]`: a game map for names, roles and tags: `format` is `wings`, `dotc` or `sinbad`;
/// `program` selects the executable in the multi-program maps (dotc, sinbad).
#[derive(Deserialize, Clone, Debug)]
pub struct MapCfg {
    pub file: String,
    pub format: String,
    pub program: Option<String>,
}

impl Config {
    /// Parses config text (paths are kept as written).
    pub fn parse(text: &str) -> Result<Config, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }
    /// Reads a config; relative paths in it are made relative to the file's directory.
    pub fn load(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        let mut c = Config::parse(&text).map_err(|e| format!("{}: {}", path.display(), e))?;
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let fix = |s: &mut String| {
            let p = PathBuf::from(&*s);
            if p.is_relative() {
                *s = dir.join(p).to_string_lossy().into_owned();
            }
        };
        for s in [&mut c.input.file, &mut c.input.adf, &mut c.out].into_iter().flatten() {
            fix(s);
        }
        for v in [&mut c.seeds_files, &mut c.names_files, &mut c.coverage, &mut c.rr_notes] {
            v.iter_mut().for_each(|s| fix(s));
        }
        c.maps.iter_mut().for_each(|m| fix(&mut m.file));
        Ok(c)
    }
}

/// A line without its `#` comment (whole-line or trailing).
fn strip_comment(l: &str) -> &str {
    match l.find('#') {
        Some(i) => &l[..i],
        None => l,
    }
}

/// Seed file lines: `<section>:<hex offset>` or `<hex absolute>`; `#` comments (whole-line or
/// trailing).
pub fn read_seeds(path: &str, bases: &[u32]) -> Result<Vec<u32>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path, e))?;
    let mut v = Vec::new();
    for l in text.lines() {
        let l = strip_comment(l).trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        v.push(parse_addr(l, bases).map_err(|e| format!("{}: {}", path, e))?);
    }
    Ok(v)
}

/// Names file lines: `<addr> <description>`; lines starting with `#` and trailing ` # comments`
/// are ignored.
pub fn read_names(path: &str, bases: &[u32], into: &mut BTreeMap<u32, String>) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path, e))?;
    for l in text.lines() {
        if l.trim_start().starts_with('#') {
            continue;
        }
        let l = match l.find(" #") {
            Some(i) => l[..i].trim_end(),
            None => l,
        };
        let mut it = l.splitn(2, ' ');
        if let (Some(a), Some(n)) = (it.next(), it.next()) {
            into.insert(parse_addr(a, bases).map_err(|e| format!("{}: {}", path, e))?, n.to_string());
        }
    }
    Ok(())
}
