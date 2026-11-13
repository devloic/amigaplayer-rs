//! SDL3 front end: transport controls, module info, oscilloscopes and a
//! module list (files can be dropped onto the window or picked in a dialog).

use amigaplayer_rs::engine::Engine;
use amigaplayer_rs::SAMPLE_RATE;
use anyhow::{anyhow, Result};
use sdl3::audio::{AudioCallback, AudioFormat, AudioSpec, AudioStream};
use sdl3::event::Event;
use sdl3::keyboard::Keycode;
use sdl3::mouse::MouseButton;
use sdl3::pixels::Color;
use sdl3::render::{Canvas, FPoint, FRect};
use sdl3::video::Window;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const BG: Color = Color::RGB(0x2a, 0x2e, 0x38);
const PANEL: Color = Color::RGB(0x38, 0x3d, 0x4a);
const LIGHT: Color = Color::RGB(0x70, 0x78, 0x8c);
const DARK: Color = Color::RGB(0x18, 0x1a, 0x20);
const TEXT: Color = Color::RGB(0xe8, 0xe8, 0xe0);
const DIM: Color = Color::RGB(0x98, 0x9c, 0xa8);
const ACCENT: Color = Color::RGB(0xf0, 0xa0, 0x30);
const SCOPE: Color = Color::RGB(0x60, 0xd0, 0x90);
const ERROR: Color = Color::RGB(0xff, 0x70, 0x60);

const ROW_H: f32 = 12.0;

struct Audio {
    engine: Arc<Mutex<Engine>>,
    buf: Vec<f32>,
}

impl AudioCallback<f32> for Audio {
    fn callback(&mut self, stream: &mut AudioStream, requested: i32) {
        let n = (requested.max(0) as usize) & !1;
        self.buf.resize(n, 0.0);
        if let Ok(mut e) = self.engine.lock() {
            e.render(&mut self.buf);
        }
        let _ = stream.put_data_f32(&self.buf);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Action {
    PrevFile,
    PrevSub,
    PlayPause,
    Stop,
    NextSub,
    NextFile,
    PrevPatt,
    NextPatt,
    AddFiles,
    AddDir,
    Clear,
    Random,
    Advance,
    A500,
    Led,
    Mono,
    SepDown,
    SepUp,
}

struct Button {
    rect: FRect,
    label: &'static str,
    action: Action,
}

struct App {
    engine: Arc<Mutex<Engine>>,
    list: Vec<PathBuf>,
    sel: usize,
    playing_idx: Option<usize>,
    scroll: usize,
    random: bool,
    advance: bool,
    status: String,
    status_is_error: bool,
    rng: u64,
    dialog_tx: Sender<Vec<PathBuf>>,
    dialog_rx: Receiver<Vec<PathBuf>>,
    dragging_volume: bool,
}

fn module_files(p: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if p.is_dir() {
        if depth == 0 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(p) else { return };
        let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        v.sort();
        for e in v {
            module_files(&e, out, depth - 1);
        }
    } else if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("info")) {
        out.push(p.to_path_buf());
    }
}

fn fmt_time(s: f64) -> String {
    let s = s as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn fit(s: &str, width: f32) -> String {
    let n = (width / 8.0).max(0.0) as usize;
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "~"
    }
}

impl App {
    fn add_paths(&mut self, paths: &[PathBuf]) {
        let before = self.list.len();
        for p in paths {
            module_files(p, &mut self.list, 6);
        }
        let added = self.list.len() - before;
        self.set_status(format!("{added} file(s) added"), false);
    }

    fn set_status(&mut self, s: String, err: bool) {
        self.status = s;
        self.status_is_error = err;
    }

    fn rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn play_index(&mut self, i: usize) {
        if i >= self.list.len() {
            return;
        }
        let path = self.list[i].clone();
        // an archive: extracted, and replaced in the list by its entries that a player takes
        match amigaplayer_rs::playlist::items(&path, None) {
            Ok(items) if items.first().is_some_and(|x| x.entry.is_some()) => {
                let mut e = self.engine.lock().unwrap();
                let ok: Vec<PathBuf> = items.into_iter().filter(|x| e.play_file(&x.path).is_ok()).map(|x| x.path).collect();
                e.stop();
                drop(e);
                let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                if ok.is_empty() {
                    self.playing_idx = Some(i);
                    self.sel = i;
                    self.set_status(format!("{name}: no entry of the archive plays"), true);
                    return;
                }
                self.list.splice(i..=i, ok);
                return self.play_index(i);
            }
            Ok(_) => {}
            Err(err) => {
                self.engine.lock().unwrap().stop();
                self.playing_idx = Some(i);
                self.sel = i;
                self.set_status(err, true);
                return;
            }
        }
        let r = self.engine.lock().unwrap().play_file(&path);
        self.playing_idx = Some(i);
        self.sel = i;
        match r {
            Ok(()) => {
                let e = self.engine.lock().unwrap();
                let p = e.playing.as_ref().unwrap();
                let msg = format!("playing with {}", p.format);
                drop(e);
                self.set_status(msg, false);
            }
            Err(err) => {
                let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                self.set_status(format!("{name}: {err}"), true);
            }
        }
    }

    /// Moves to another file. `skip_failed` keeps going past files that
    /// cannot be played (used for automatic advancing).
    fn next_file(&mut self, dir: i32, skip_failed: bool) {
        if self.list.is_empty() {
            return;
        }
        let n = self.list.len();
        for _ in 0..n {
            let cur = self.playing_idx.unwrap_or(self.sel);
            let next = if self.random && dir > 0 {
                (self.rand() % n as u64) as usize
            } else {
                (cur as i64 + dir as i64).rem_euclid(n as i64) as usize
            };
            self.play_index(next);
            if !skip_failed || self.engine.lock().unwrap().playing.is_some() {
                break;
            }
        }
    }

    fn act(&mut self, a: Action, window: &Window) {
        match a {
            Action::PrevFile => self.next_file(-1, false),
            Action::NextFile => self.next_file(1, false),
            Action::PlayPause => {
                let mut e = self.engine.lock().unwrap();
                if e.playing.is_some() {
                    e.paused = !e.paused;
                } else {
                    drop(e);
                    let i = self.sel;
                    self.play_index(i);
                }
            }
            Action::Stop => {
                self.engine.lock().unwrap().stop();
                self.playing_idx = None;
                self.set_status("stopped".into(), false);
            }
            Action::PrevSub | Action::NextSub => {
                let d = if a == Action::NextSub { 1 } else { -1 };
                let r = self.engine.lock().unwrap().change_subsong(d);
                if let Err(e) = r {
                    self.set_status(e.to_string(), true);
                }
            }
            Action::PrevPatt => self.engine.lock().unwrap().pattern_step(-1),
            Action::NextPatt => self.engine.lock().unwrap().pattern_step(1),
            Action::AddFiles => {
                let tx = self.dialog_tx.clone();
                let r = sdl3::dialog::show_open_file_dialog(
                    &[],
                    None::<&str>,
                    true,
                    window,
                    Box::new(move |res, _| {
                        if let Ok(v) = res {
                            let _ = tx.send(v);
                        }
                    }),
                );
                if r.is_err() {
                    self.set_status("file dialog unavailable - drop files onto the window".into(), true);
                }
            }
            Action::AddDir => {
                let tx = self.dialog_tx.clone();
                sdl3::dialog::show_open_folder_dialog(
                    None::<&str>,
                    false,
                    window,
                    Box::new(move |res, _| {
                        if let Ok(v) = res {
                            let _ = tx.send(v);
                        }
                    }),
                );
            }
            Action::Clear => {
                self.engine.lock().unwrap().stop();
                self.list.clear();
                self.sel = 0;
                self.scroll = 0;
                self.playing_idx = None;
            }
            Action::Random => self.random = !self.random,
            Action::Advance => self.advance = !self.advance,
            Action::A500 => {
                let mut e = self.engine.lock().unwrap();
                e.settings.a500_filter = !e.settings.a500_filter;
            }
            Action::Led => {
                let mut e = self.engine.lock().unwrap();
                e.settings.led_filter = !e.settings.led_filter;
            }
            Action::Mono => {
                let mut e = self.engine.lock().unwrap();
                e.settings.mono = !e.settings.mono;
            }
            Action::SepDown | Action::SepUp => {
                let mut e = self.engine.lock().unwrap();
                let d = if a == Action::SepUp { 0.1 } else { -0.1 };
                e.settings.separation = (e.settings.separation + d).clamp(0.0, 1.0);
            }
        }
    }

    fn toggled(&self, a: Action) -> Option<bool> {
        let e = self.engine.lock().unwrap();
        match a {
            Action::Random => Some(self.random),
            Action::Advance => Some(self.advance),
            Action::A500 => Some(e.settings.a500_filter),
            Action::Led => Some(e.settings.led_filter),
            Action::Mono => Some(e.settings.mono),
            Action::PlayPause => Some(e.playing.is_some() && !e.paused),
            _ => None,
        }
    }
}

struct Layout {
    buttons: Vec<Button>,
    volume: FRect,
    scopes: FRect,
    list: FRect,
}

fn layout(w: f32, h: f32) -> Layout {
    let mut buttons = Vec::new();
    let mut x = 8.0;
    let row1: [(&str, Action, f32); 8] = [
        ("|<", Action::PrevFile, 32.0),
        ("<<", Action::PrevSub, 32.0),
        ("Play", Action::PlayPause, 48.0),
        ("Stop", Action::Stop, 48.0),
        (">>", Action::NextSub, 32.0),
        (">|", Action::NextFile, 32.0),
        ("-Pat", Action::PrevPatt, 44.0),
        ("+Pat", Action::NextPatt, 44.0),
    ];
    for (label, action, bw) in row1 {
        buttons.push(Button { rect: FRect::new(x, 76.0, bw, 20.0), label, action });
        x += bw + 4.0;
    }
    let volume = FRect::new(x + 8.0, 76.0, (w - x - 16.0).max(40.0), 20.0);
    let mut x = 8.0;
    let row2: [(&str, Action, f32); 10] = [
        ("Add", Action::AddFiles, 40.0),
        ("Dir", Action::AddDir, 40.0),
        ("Clear", Action::Clear, 52.0),
        ("Random", Action::Random, 60.0),
        ("Next@End", Action::Advance, 76.0),
        ("A500", Action::A500, 44.0),
        ("LED", Action::Led, 36.0),
        ("Mono", Action::Mono, 44.0),
        ("Sep-", Action::SepDown, 40.0),
        ("Sep+", Action::SepUp, 40.0),
    ];
    for (label, action, bw) in row2 {
        buttons.push(Button { rect: FRect::new(x, 100.0, bw, 20.0), label, action });
        x += bw + 4.0;
    }
    let scopes = FRect::new(8.0, 128.0, w - 16.0, 72.0);
    let list = FRect::new(8.0, 208.0, w - 16.0, (h - 208.0 - 22.0).max(ROW_H * 2.0));
    Layout { buttons, volume, scopes, list }
}

fn bevel(c: &mut Canvas<Window>, r: FRect, pressed: bool) {
    c.set_draw_color(PANEL);
    let _ = c.fill_rect(r);
    let (tl, br) = if pressed { (DARK, LIGHT) } else { (LIGHT, DARK) };
    c.set_draw_color(tl);
    let _ = c.draw_line(FPoint::new(r.x, r.y), FPoint::new(r.x + r.w - 1.0, r.y));
    let _ = c.draw_line(FPoint::new(r.x, r.y), FPoint::new(r.x, r.y + r.h - 1.0));
    c.set_draw_color(br);
    let _ = c.draw_line(FPoint::new(r.x, r.y + r.h - 1.0), FPoint::new(r.x + r.w - 1.0, r.y + r.h - 1.0));
    let _ = c.draw_line(FPoint::new(r.x + r.w - 1.0, r.y), FPoint::new(r.x + r.w - 1.0, r.y + r.h - 1.0));
}

fn text(c: &mut Canvas<Window>, s: &str, x: f32, y: f32, col: Color) {
    c.set_draw_color(col);
    let _ = c.draw_debug_text(s, FPoint::new(x, y));
}

fn draw(app: &mut App, c: &mut Canvas<Window>, lay: &Layout) {
    let (w, _h) = c.output_size().unwrap_or((720, 520));
    let w = w as f32;
    c.set_draw_color(BG);
    c.clear();

    // --- info panel
    let info = FRect::new(8.0, 8.0, w - 16.0, 62.0);
    bevel(c, info, true);
    let (name, fmt, sub, time, extra, err, vol) = {
        let e = app.engine.lock().unwrap();
        let vol = e.settings.volume;
        let err = e.last_error.clone();
        match &e.playing {
            Some(p) => {
                let sub = if p.sub_max > p.sub_min {
                    format!("song {}/{}", p.sub - p.sub_min + 1, p.sub_max - p.sub_min + 1)
                } else {
                    "song 1/1".into()
                };
                let extra = match e.native_position() {
                    Some((pos, len, row, bpm)) => format!("pos {pos:03}/{len:03} row {row:02} bpm {bpm}"),
                    None => {
                        let n = e.scope_data().len();
                        format!("{n} voices")
                    }
                };
                let paused = if e.paused { " (paused)" } else { "" };
                let format = if p.author.is_empty() { p.format.clone() } else { format!("{}  by {}", p.format, p.author) };
                (
                    p.module_name.clone(),
                    format,
                    sub,
                    format!("{}{paused}", fmt_time(e.elapsed_secs())),
                    extra,
                    err,
                    vol,
                )
            }
            None => ("- no module -".into(), String::new(), String::new(), String::new(), String::new(), err, vol),
        }
    };
    c.set_scale(2.0, 2.0).ok();
    text(c, &fit(&name, (w - 32.0) / 2.0), 8.0, 7.0, ACCENT);
    c.set_scale(1.0, 1.0).ok();
    text(c, &fit(&format!("Format: {fmt}"), w - 200.0), 16.0, 38.0, TEXT);
    text(c, &sub, w - 180.0, 38.0, TEXT);
    text(c, &fit(&extra, w - 200.0), 16.0, 52.0, DIM);
    text(c, &time, w - 180.0, 52.0, TEXT);

    // --- buttons
    for b in &lay.buttons {
        let on = app.toggled(b.action);
        bevel(c, b.rect, on == Some(true));
        let label = if b.action == Action::PlayPause {
            if on == Some(true) { "Pause" } else { "Play" }
        } else {
            b.label
        };
        let tx = b.rect.x + (b.rect.w - label.len() as f32 * 8.0) / 2.0;
        text(c, label, tx, b.rect.y + 6.0, if on == Some(true) { ACCENT } else { TEXT });
    }
    // volume slider
    let v = lay.volume;
    bevel(c, v, true);
    c.set_draw_color(ACCENT);
    let _ = c.fill_rect(FRect::new(v.x + 2.0, v.y + 2.0, (v.w - 4.0) * vol, v.h - 4.0));
    let vt = format!("Volume {:3}%", (vol * 100.0).round());
    text(c, &vt, v.x + (v.w - vt.len() as f32 * 8.0) / 2.0, v.y + 6.0, DARK);
    let sep = app.engine.lock().unwrap().settings.separation;
    if let Some(last) = lay.buttons.last() {
        let x = last.rect.x + last.rect.w + 8.0;
        let st = if app.engine.lock().unwrap().settings.mono { "Mono".to_string() } else { format!("Stereo {:3}%", (sep * 100.0).round()) };
        text(c, &st, x, last.rect.y + 6.0, DIM);
    }

    // --- scopes
    let sc = lay.scopes;
    bevel(c, sc, true);
    let data = app.engine.lock().unwrap().scope_data();
    if !data.is_empty() {
        let n = data.len().min(16);
        let cw = (sc.w - 8.0) / n as f32;
        for (k, d) in data.iter().take(n).enumerate() {
            let x0 = sc.x + 4.0 + k as f32 * cw;
            let mid = sc.y + sc.h / 2.0;
            c.set_draw_color(DARK);
            let _ = c.fill_rect(FRect::new(x0 + 1.0, sc.y + 4.0, cw - 2.0, sc.h - 8.0));
            let pts: Vec<FPoint> = (0..(cw - 4.0) as usize)
                .map(|i| {
                    let s = d[(i * d.len()) / (cw - 4.0).max(1.0) as usize % d.len()];
                    FPoint::new(x0 + 2.0 + i as f32, mid - s.clamp(-1.0, 1.0) * (sc.h / 2.0 - 6.0))
                })
                .collect();
            c.set_draw_color(SCOPE);
            let _ = c.draw_lines(&pts[..]);
        }
    }

    // --- module list
    let lr = lay.list;
    bevel(c, lr, true);
    let rows = ((lr.h - 4.0) / ROW_H) as usize;
    if app.sel < app.scroll {
        app.scroll = app.sel;
    }
    if rows > 0 && app.sel >= app.scroll + rows {
        app.scroll = app.sel + 1 - rows;
    }
    if app.list.is_empty() {
        text(c, "Drop modules or folders here, or use Add / Dir.", lr.x + 8.0, lr.y + 8.0, DIM);
        text(
            c,
            "Keys: space play/pause, enter play, arrows, n/p, +/-",
            lr.x + 8.0,
            lr.y + 8.0 + ROW_H * 2.0,
            DIM,
        );
    }
    for r in 0..rows {
        let i = app.scroll + r;
        if i >= app.list.len() {
            break;
        }
        let y = lr.y + 2.0 + r as f32 * ROW_H;
        if i == app.sel {
            c.set_draw_color(PANEL);
            let _ = c.fill_rect(FRect::new(lr.x + 2.0, y, lr.w - 4.0, ROW_H));
        }
        let mark = if Some(i) == app.playing_idx { ">" } else { " " };
        let name = app.list[i].file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let col = if Some(i) == app.playing_idx { ACCENT } else { TEXT };
        text(c, &fit(&format!("{mark}{:4} {name}", i + 1), lr.w - 8.0), lr.x + 4.0, y + 2.0, col);
    }

    // --- status line
    let (msg, is_err) = match err {
        Some(e) => (e, true),
        None => (app.status.clone(), app.status_is_error),
    };
    text(c, &fit(&msg, w - 16.0), 8.0, lr.y + lr.h + 7.0, if is_err { ERROR } else { DIM });
    c.present();
}

/// The 68k player side of the command line (a build with `host-68k`).
#[cfg(feature = "host-68k")]
pub struct Players68k {
    pub dirs: amigaplayer_rs::engine::PlayerDirs,
    pub libs: Option<PathBuf>,
    pub use_genies: Vec<String>,
    pub only_player: Option<String>,
    /// The open replayers play what they know (the default); false: the 68k players do.
    pub open_replayers: bool,
}

pub fn run(
    #[cfg(feature = "host-68k")] p68k: Players68k,
    files: Vec<PathBuf>,
    settings: crate::engine::Settings,
) -> Result<()> {
    // Our own application name, so the sound server keeps a volume/mute
    // setting for us instead of sharing the generic "SDL Application" one.
    sdl3::hint::set("SDL_APP_NAME", "DeliPlayer");
    sdl3::hint::set("SDL_AUDIO_DEVICE_STREAM_NAME", "DeliPlayer");
    // SDL's native PipeWire backend can come up with a muted stream; the
    // PulseAudio backend (served by pipewire-pulse) does not. An explicit
    // SDL_AUDIO_DRIVER in the environment still wins.
    if std::env::var_os("SDL_AUDIO_DRIVER").is_none() {
        sdl3::hint::set("SDL_AUDIO_DRIVER", "pulseaudio,pipewire,alsa");
    }
    let sdl = sdl3::init().map_err(|e| anyhow!("SDL init: {e}"))?;
    let video = sdl.video().map_err(|e| anyhow!("SDL video: {e}"))?;
    let audio = sdl.audio().map_err(|e| anyhow!("SDL audio: {e}"))?;

    let mut engine = Engine::new(SAMPLE_RATE as f64);
    engine.settings = settings;
    // The formats the open replayers know are played with them (the default).
    engine.open_replayers = true;
    #[cfg(feature = "host-68k")]
    {
        engine.m.os.libs_dir = p68k.libs;
        engine.only_player = p68k.only_player;
        engine.open_replayers = p68k.open_replayers;
        // the player directories are read when a module needs a 68k player (a genie chosen by
        // name must be loaded to be found: that is done now)
        engine.set_player_dirs(p68k.dirs);
        if !p68k.use_genies.is_empty() {
            engine.load_pending_players();
        }
        for g in &p68k.use_genies {
            engine.use_genie(g)?;
        }
    }
    let engine = Arc::new(Mutex::new(engine));

    let spec = AudioSpec { freq: Some(SAMPLE_RATE as i32), channels: Some(2), format: Some(AudioFormat::f32_sys()) };
    let stream = audio
        .open_playback_stream(&spec, Audio { engine: engine.clone(), buf: Vec::new() })
        .map_err(|e| anyhow!("audio: {e}"))?;
    stream.resume().map_err(|e| anyhow!("audio: {e}"))?;

    let window = video
        .window("DeliPlayer", 720, 520)
        .resizable()
        .position_centered()
        .build()
        .map_err(|e| anyhow!("window: {e}"))?;
    let mut canvas = window.into_canvas();
    let mut events = sdl.event_pump().map_err(|e| anyhow!("events: {e}"))?;

    let (tx, rx) = channel();
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
    let mut app = App {
        engine,
        list: Vec::new(),
        sel: 0,
        playing_idx: None,
        scroll: 0,
        random: false,
        advance: true,
        status: String::new(),
        status_is_error: false,
        rng: seed | 1,
        dialog_tx: tx,
        dialog_rx: rx,
        dragging_volume: false,
    };
    if !files.is_empty() {
        app.add_paths(&files);
        app.play_index(0);
    }

    'main: loop {
        let (w, h) = canvas.output_size().unwrap_or((720, 520));
        let lay = layout(w as f32, h as f32);
        for ev in events.poll_iter() {
            match ev {
                Event::Quit { .. } => break 'main,
                Event::DropFile { filename, .. } => app.add_paths(&[PathBuf::from(filename)]),
                Event::KeyDown { keycode: Some(k), .. } => match k {
                    Keycode::Escape | Keycode::Q => break 'main,
                    Keycode::Space => app.act(Action::PlayPause, canvas.window()),
                    Keycode::Return | Keycode::KpEnter => {
                        let i = app.sel;
                        app.play_index(i);
                    }
                    Keycode::Up => app.sel = app.sel.saturating_sub(1),
                    Keycode::Down => app.sel = (app.sel + 1).min(app.list.len().saturating_sub(1)),
                    Keycode::PageUp => app.sel = app.sel.saturating_sub(20),
                    Keycode::PageDown => app.sel = (app.sel + 20).min(app.list.len().saturating_sub(1)),
                    Keycode::Left => app.act(Action::PrevSub, canvas.window()),
                    Keycode::Right => app.act(Action::NextSub, canvas.window()),
                    Keycode::N => app.act(Action::NextFile, canvas.window()),
                    Keycode::P => app.act(Action::PrevFile, canvas.window()),
                    Keycode::S => app.act(Action::Stop, canvas.window()),
                    Keycode::O => app.act(Action::AddFiles, canvas.window()),
                    Keycode::R => app.act(Action::Random, canvas.window()),
                    Keycode::F => app.act(Action::A500, canvas.window()),
                    Keycode::M => app.act(Action::Mono, canvas.window()),
                    Keycode::Plus | Keycode::Equals | Keycode::KpPlus => {
                        let mut e = app.engine.lock().unwrap();
                        e.settings.volume = (e.settings.volume + 0.05).min(1.0);
                    }
                    Keycode::Minus | Keycode::KpMinus => {
                        let mut e = app.engine.lock().unwrap();
                        e.settings.volume = (e.settings.volume - 0.05).max(0.0);
                    }
                    Keycode::Delete => {
                        if app.sel < app.list.len() {
                            app.list.remove(app.sel);
                            match app.playing_idx {
                                Some(p) if p == app.sel => app.playing_idx = None,
                                Some(p) if p > app.sel => app.playing_idx = Some(p - 1),
                                _ => {}
                            }
                            app.sel = app.sel.min(app.list.len().saturating_sub(1));
                        }
                    }
                    _ => {}
                },
                Event::MouseButtonDown { mouse_btn: MouseButton::Left, clicks, x, y, .. } => {
                    let inside = |r: &FRect| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
                    if let Some(b) = lay.buttons.iter().find(|b| inside(&b.rect)) {
                        let a = b.action;
                        app.act(a, canvas.window());
                    } else if inside(&lay.volume) {
                        app.dragging_volume = true;
                        let v = ((x - lay.volume.x) / lay.volume.w).clamp(0.0, 1.0);
                        app.engine.lock().unwrap().settings.volume = v;
                    } else if inside(&lay.list) {
                        let i = app.scroll + ((y - lay.list.y - 2.0) / ROW_H) as usize;
                        if i < app.list.len() {
                            app.sel = i;
                            if clicks >= 2 {
                                app.play_index(i);
                            }
                        }
                    }
                }
                Event::MouseButtonUp { mouse_btn: MouseButton::Left, .. } => app.dragging_volume = false,
                Event::MouseMotion { x, .. } if app.dragging_volume => {
                    let v = ((x - lay.volume.x) / lay.volume.w).clamp(0.0, 1.0);
                    app.engine.lock().unwrap().settings.volume = v;
                }
                Event::MouseWheel { y, .. } => {
                    let rows = ((lay.list.h - 4.0) / ROW_H) as usize;
                    let max = app.list.len().saturating_sub(rows);
                    let s = app.scroll as i64 - (y * 3.0) as i64;
                    app.scroll = s.clamp(0, max as i64) as usize;
                    // keep the selection visible without fighting the scroll
                    if app.sel < app.scroll || app.sel >= app.scroll + rows {
                        app.sel = app.scroll.min(app.list.len().saturating_sub(1));
                    }
                }
                _ => {}
            }
        }
        while let Ok(v) = app.dialog_rx.try_recv() {
            app.add_paths(&v);
        }
        // song end: advance to the next module
        let (ended, crashed) = {
            let e = app.engine.lock().unwrap();
            (e.songend(), e.playing.is_none() && e.last_error.is_some() && app.playing_idx.is_some())
        };
        if app.advance && (ended || crashed) && !app.list.is_empty() {
            log::info!("advancing: songend={ended} crashed={crashed}");
            app.next_file(1, true);
        } else if ended {
            // stay on this module; clear the flag so it can end again
            let mut e = app.engine.lock().unwrap();
            e.m.deli.songend = false;
            if let Some(amigaplayer_rs::engine::Playing { backend: amigaplayer_rs::engine::Backend::Native(pt), .. }) = &mut e.playing {
                pt.songend = false;
            }
        }
        draw(&mut app, &mut canvas, &lay);
        std::thread::sleep(Duration::from_millis(16));
    }
    drop(stream);
    Ok(())
}
