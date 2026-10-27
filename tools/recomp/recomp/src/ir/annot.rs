//! The annotation layer: what is known about a program beyond its code. It is kept apart from the
//! IR (which it only refers to by function key or address) and fed from the per-game mapping
//! drafts (`wings-map.json`, `dotc-map.json`, `sinbad-map.json`).
//!
//! Functions are keyed by (program, section, offset), so that an annotation survives relocation
//! (overlays and modules are loaded at different addresses in different runs or crates).
use super::json::{self, Json};
use std::collections::BTreeMap;

/// Program (executable / module), hunk or overlay section index, offset in the section.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FuncKey {
    pub program: String,
    pub section: u32,
    /// None when the map gives no section bases (resolve with [`AnnotSet::rebase`]).
    pub offset: Option<u32>,
}

/// Calling convention of a function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Abi {
    /// Manx/Aztec C: arguments on the stack (16-bit `int`), result in d0, d4-d7/a2-a5 preserved,
    /// a4 = small-data base.
    ManxStack16,
    /// Arguments in registers (`register asm`, library-style), listed by register.
    RegisterAsm { args: Vec<String>, result: Option<String> },
    /// Green Hills-style C: `link a6` frame, 32-bit `int`, absolute globals.
    GreenHillsA6,
    Unknown,
}

/// The run-time context a function is entered in, and how it must be translated.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tag {
    /// Interrupt server / handler (runs between any two instructions of the main program).
    Interrupt,
    /// Called back by the OS or a library (input handler, hook, copper/blitter callback, ...).
    Callback,
    /// Entry of a separate task or process.
    Task,
    /// Lives in an overlay (node number) or a separately loaded module.
    Overlay(u32),
    /// A jump stub (data-section `jmp`, library vector wrapper).
    Stub,
    /// Self-modifying code or code written at run time: run it in the interpreter.
    KeepInterpreted,
    /// Native code only behind a run-time check (code bytes unchanged, expected entry state).
    Guarded,
}

/// A signature: arguments and result, in the ABI's terms.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Signature {
    pub args: Vec<String>,
    pub result: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuncAnnot {
    pub key: FuncKey,
    /// Absolute address in the reference load layout, when known.
    pub abs: Option<u32>,
    pub name: Option<String>,
    /// Free text: role or note from the map.
    pub note: Option<String>,
    /// "verified", "evidence", "guess", "disassembly", ...
    pub confidence: Option<String>,
    pub abi: Abi,
    pub sig: Option<Signature>,
    pub tags: Vec<Tag>,
}

/// A structure layout: offset -> (type, description).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StructLayout {
    pub name: String,
    pub size: Option<u32>,
    pub fields: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default)]
pub struct AnnotSet {
    pub program: String,
    /// Section bases of the reference layout (empty if the map has none).
    pub bases: Vec<u32>,
    pub funcs: Vec<FuncAnnot>,
    pub structs: Vec<StructLayout>,
}

impl AnnotSet {
    /// Annotations by absolute address.
    pub fn by_abs(&self) -> BTreeMap<u32, &FuncAnnot> {
        self.funcs.iter().filter_map(|f| f.abs.map(|a| (a, f))).collect()
    }
    /// Annotations by key.
    pub fn by_key(&self) -> BTreeMap<&FuncKey, &FuncAnnot> {
        self.funcs.iter().map(|f| (&f.key, f)).collect()
    }
    /// Fills in section offsets (and absolute addresses) from section bases.
    pub fn rebase(&mut self, bases: &[u32]) {
        self.bases = bases.to_vec();
        for f in &mut self.funcs {
            let base = bases.get(f.key.section as usize).copied();
            match (f.abs, f.key.offset, base) {
                (Some(a), None, Some(b)) if a >= b => f.key.offset = Some(a - b),
                (None, Some(o), Some(b)) => f.abs = Some(b + o),
                _ => {}
            }
        }
    }
    /// Entry -> doc line, for the emitters' `names`.
    pub fn names(&self) -> BTreeMap<u32, String> {
        self.funcs
            .iter()
            .filter_map(|f| {
                let a = f.abs?;
                let n = f.name.as_deref()?;
                Some((a, match &f.note {
                    Some(t) => format!("{}: {}", n, t),
                    None => n.to_string(),
                }))
            })
            .collect()
    }
    /// Absolute entries tagged with `t` (needs `abs`; call [`AnnotSet::rebase`] first when the map
    /// has only section offsets).
    pub fn entries_tagged(&self, t: &Tag) -> Vec<u32> {
        self.funcs.iter().filter(|f| f.tags.contains(t)).filter_map(|f| f.abs).collect()
    }
    /// Entry -> name, for `#[doc(alias)]`: only names that are plain identifiers.
    pub fn aliases(&self) -> BTreeMap<u32, String> {
        self.funcs
            .iter()
            .filter_map(|f| {
                let n = f.name.as_deref()?;
                let ok = !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                Some((f.abs?, n.to_string())).filter(|_| ok)
            })
            .collect()
    }
    pub fn named(&self) -> usize {
        self.funcs.iter().filter(|f| f.name.is_some()).count()
    }
    pub fn tagged(&self, t: &Tag) -> usize {
        self.funcs.iter().filter(|f| f.tags.contains(t)).count()
    }
}

fn hexnum(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches('$').trim_start_matches("0x");
    u32::from_str_radix(s, 16).ok()
}

/// "S:OOOOO" -> (section, offset).
fn sec_off(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once(':')?;
    Some((a.parse().ok()?, hexnum(b)?))
}

/// Context tags guessed from a note: interrupt / callback / task words. Conservative: a tag is
/// set only on explicit wording.
fn text_tags(t: &str, tags: &mut Vec<Tag>) {
    let l = t.to_ascii_lowercase();
    if l.contains("interrupt") || l.contains("vertb") || l.contains("level 3") {
        tags.push(Tag::Interrupt);
    }
    if l.contains("callback") || l.contains("input handler") || l.contains("hook") {
        tags.push(Tag::Callback);
    }
    if l.contains("task entry") || l.contains("process entry") || l.contains("createproc") || l.contains("createtask") {
        tags.push(Tag::Task);
    }
    if l.contains("self-modif") || l.contains("smc") {
        tags.push(Tag::KeepInterpreted);
    }
}

fn structs_of(j: Option<&Json>) -> Vec<StructLayout> {
    let mut out = Vec::new();
    for (name, v) in j.and_then(|j| j.obj()).into_iter().flatten() {
        let mut s = StructLayout { name: name.clone(), ..Default::default() };
        match v {
            Json::Str(t) => s.fields.push((String::new(), t.clone())),
            Json::Obj(m) => {
                s.size = m.get("size").and_then(|n| n.num()).map(|n| n as u32);
                // Either {"fields": {off: desc}} or the offsets directly ("+8": ...).
                let fields = m.get("fields").and_then(|f| f.obj()).unwrap_or(m);
                for (k, d) in fields {
                    if k.starts_with('+') {
                        let d = match d {
                            Json::Str(t) => t.clone(),
                            other => format!("{:?}", other),
                        };
                        s.fields.push((k.clone(), d));
                    }
                }
                if s.fields.is_empty() {
                    if let Some(l) = m.get("layout").and_then(|l| l.str()) {
                        s.fields.push((String::new(), l.to_string()));
                    }
                }
            }
            _ => {}
        }
        out.push(s);
    }
    out
}

fn read(path: &std::path::Path) -> Result<Json, String> {
    let s = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    json::parse(&s)
}

/// Wings of Fury (`wings-map.json`): Manx C, one program with a root and overlays.
pub fn load_wings(path: &std::path::Path) -> Result<AnnotSet, String> {
    let j = read(path)?;
    let program = j.get("program").and_then(|p| p.str()).unwrap_or("wings").to_string();
    let mut bases: Vec<(u32, u32)> = j.get("bases").and_then(|b| b.obj()).into_iter().flatten()
        .filter_map(|(k, v)| Some((k.parse().ok()?, hexnum(v.str()?)?))).collect();
    bases.sort();
    let mut set = AnnotSet { program: program.clone(), bases: bases.iter().map(|b| b.1).collect(), ..Default::default() };
    for f in j.get("functions").map(|f| f.arr()).unwrap_or(&[]) {
        let Some((sec, off)) = f.get("addr").and_then(|a| a.str()).and_then(sec_off) else { continue };
        let mut tags = Vec::new();
        if let Some(k) = f.get("kind").and_then(|k| k.str()).and_then(|k| k.strip_prefix("overlay")) {
            tags.push(Tag::Overlay(k.parse().unwrap_or(0)));
        }
        if f.get("wrapper_of").is_some() {
            tags.push(Tag::Stub);
        }
        let note = f.get("note").and_then(|n| n.str()).map(str::to_string);
        if let Some(n) = &note {
            text_tags(n, &mut tags);
        }
        let confidence = note.as_deref().and_then(|n| {
            let l = n.to_ascii_lowercase();
            if l.contains("(verified)") { Some("verified") } else if l.contains("guess") { Some("guess") } else { None }
        }).map(str::to_string);
        set.funcs.push(FuncAnnot {
            key: FuncKey { program: program.clone(), section: sec, offset: Some(off) },
            abs: f.get("abs").and_then(|a| a.str()).and_then(hexnum),
            name: f.get("name").and_then(|n| n.str()).map(str::to_string),
            note,
            confidence,
            abi: Abi::ManxStack16,
            sig: None,
            tags,
        });
    }
    set.structs = structs_of(j.get("structures"));
    Ok(set)
}

/// Defender of the Crown (`dotc-map.json`): one set per executable with functions
/// (`Defender` is Manx C, the modules `MUD_D.C` and `dc*` the a6-frame compiler); modules marked
/// `same_code_as` share the functions of the named one.
pub fn load_dotc(path: &std::path::Path) -> Result<Vec<AnnotSet>, String> {
    let j = read(path)?;
    let structs = structs_of(j.get("structures"));
    let mut out = Vec::new();
    for (name, e) in j.get("executables").and_then(|e| e.obj()).into_iter().flatten() {
        let fns = e.get("functions").map(|f| f.arr()).unwrap_or(&[]);
        if fns.is_empty() {
            continue;
        }
        let bases: Vec<u32> = e.get("sections").map(|s| s.arr()).unwrap_or(&[]).iter()
            .filter_map(|s| s.get("base").and_then(|b| b.num()).map(|b| b as u32)).collect();
        let abi = if name == "Defender" { Abi::ManxStack16 } else { Abi::GreenHillsA6 };
        let mut set = AnnotSet { program: name.clone(), bases, ..Default::default() };
        for f in fns {
            let Some((sec, off)) = f.get("where").and_then(|a| a.str()).and_then(sec_off) else { continue };
            let note = f.get("note").and_then(|n| n.str()).map(str::to_string);
            let mut tags = Vec::new();
            if let Some(n) = &note {
                text_tags(n, &mut tags);
            }
            if !f.get("smc_suspect").map(|s| s.arr()).unwrap_or(&[]).is_empty() && !tags.contains(&Tag::KeepInterpreted) {
                tags.push(Tag::KeepInterpreted);
            }
            set.funcs.push(FuncAnnot {
                key: FuncKey { program: name.clone(), section: sec, offset: Some(off) },
                abs: f.get("addr").and_then(|a| a.str()).and_then(hexnum),
                name: f.get("name").and_then(|n| n.str()).map(str::to_string),
                note,
                confidence: f.get("confidence").and_then(|c| c.str()).map(str::to_string),
                abi: abi.clone(),
                sig: None,
                tags,
            });
        }
        if name == "Defender" {
            set.structs = structs.clone();
        }
        out.push(set);
    }
    Ok(out)
}

/// Sinbad (`sinbad-map.json`): one set per program; the map gives a section index and absolute
/// address but no section bases, so offsets are filled in by [`AnnotSet::rebase`] when the
/// caller knows the load layout (`bases`: program -> section bases).
pub fn load_sinbad(path: &std::path::Path, bases: Option<&BTreeMap<String, Vec<u32>>>) -> Result<Vec<AnnotSet>, String> {
    let j = read(path)?;
    let mut out = Vec::new();
    for (name, p) in j.obj().into_iter().flatten() {
        let mut set = AnnotSet { program: name.clone(), ..Default::default() };
        for f in p.get("functions").map(|f| f.arr()).unwrap_or(&[]) {
            let Some(abs) = f.get("addr").and_then(|a| a.str()).and_then(hexnum) else { continue };
            let sec = f.get("sec").and_then(|s| s.num()).unwrap_or(0.0) as u32;
            let note = f.get("role").and_then(|n| n.str()).map(str::to_string);
            let mut tags = Vec::new();
            if f.get("stub").and_then(|s| s.str()).is_some() {
                tags.push(Tag::Stub);
            }
            if let Some(n) = &note {
                text_tags(n, &mut tags);
            }
            set.funcs.push(FuncAnnot {
                key: FuncKey { program: name.clone(), section: sec, offset: None },
                abs: Some(abs),
                name: None,
                note,
                confidence: f.get("role_source").and_then(|c| c.str()).map(str::to_string),
                abi: Abi::Unknown,
                sig: None,
                tags,
            });
        }
        if let Some(b) = bases.and_then(|b| b.get(name)) {
            set.rebase(b);
        }
        out.push(set);
    }
    Ok(out)
}

/// Rocket Ranger notes (`rr_names.txt`): `<section>:<hex offset> <name> [(<section>:<hex offset>)] [note]`,
/// `#` lines and trailing ` # comments` ignored. A trailing parenthesised address (the notes
/// repeat the address) is dropped; the rest of the line after the name is the note. Manx C.
/// `bases`: the section load addresses, so each function gets its absolute address.
pub fn load_rr_notes(path: &std::path::Path, program: &str, bases: &[u32]) -> Result<AnnotSet, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    let mut set = AnnotSet { program: program.to_string(), bases: bases.to_vec(), ..Default::default() };
    for l in text.lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let l = match l.find(" #") {
            Some(i) => l[..i].trim_end(),
            None => l,
        };
        let mut it = l.splitn(3, char::is_whitespace);
        let (Some(a), Some(name)) = (it.next(), it.next()) else { continue };
        let Some((sec, off)) = sec_off(a) else { continue };
        let note = it.next().map(str::trim).filter(|t| !t.is_empty());
        let note = note.filter(|t| !(t.starts_with('(') && t.ends_with(')') && t[1..t.len() - 1].contains(':') && !t.contains(' '))).map(str::to_string);
        let mut tags = Vec::new();
        if let Some(n) = &note {
            text_tags(n, &mut tags);
        }
        let mut f = FuncAnnot {
            key: FuncKey { program: program.to_string(), section: sec, offset: Some(off) },
            abs: bases.get(sec as usize).map(|b| b + off),
            name: Some(name.to_string()),
            note,
            confidence: None,
            abi: Abi::ManxStack16,
            sig: None,
            tags,
        };
        if let Some(n) = &mut f.note {
            *n = n.trim().to_string();
        }
        set.funcs.push(f);
    }
    Ok(set)
}
