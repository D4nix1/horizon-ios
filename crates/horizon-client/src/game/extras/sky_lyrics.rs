//! Sky lyrics (modded-pc branch): word-synced lyrics drawn in the sky, after WorldLyrics.
//!
//! Drop an enhanced LRC file at `sky-lyrics.lrc` in the game folder (or point
//! `HORIZON_SKY_LYRICS` at one). The song clock starts when the file appears or changes,
//! so save/touch it as you press play. Supported:
//! `[mm:ss.xx] <mm:ss.xx>word <mm:ss.xx>word` (word sync), plain `[mm:ss.xx] line`
//! (words spread over the line), `[offset:+/-ms]`, `[font:minecraft:alt]` (any pack font),
//! `[color:#RRGGBB]` (the sung colour).
use super::super::Game;
use horizon_client::hud::{HudAtlas, HudBatch, pictures::Picture, rgba};
use horizon_client::hud_kit::fit::ellipsize;
use horizon_client::text::{Style, Text};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub at: f64,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub at: f64,
    pub words: Vec<Word>,
}
#[derive(Debug, Default, PartialEq)]
pub struct Song {
    pub lines: Vec<Line>,
    pub font: Option<String>,
    pub sung: Option<u32>,
}

fn stamp(s: &str) -> Option<f64> {
    let (m, s) = s.split_once(':')?;
    Some(m.trim().parse::<f64>().ok()? * 60.0 + s.trim().parse::<f64>().ok()?)
}

/// Enhanced LRC. Lines without word stamps get their words spread evenly up to the next line.
pub fn parse(src: &str) -> Song {
    let mut song = Song::default();
    let mut offset = 0.0;
    for raw in src.lines() {
        let mut rest = raw.trim();
        let mut starts = Vec::new();
        while let Some(body) = rest.strip_prefix('[') {
            let Some((tag, after)) = body.split_once(']') else {
                break;
            };
            rest = after;
            match tag.split_once(':') {
                Some(("offset", v)) => offset = v.trim().parse::<f64>().unwrap_or(0.0) / 1000.0,
                Some(("font", v)) => song.font = Some(v.trim().to_string()),
                Some(("color", v)) => {
                    song.sung = u32::from_str_radix(v.trim().trim_start_matches('#'), 16).ok()
                }
                _ => starts.extend(stamp(tag)),
            }
        }
        for at in starts {
            let mut words = Vec::new();
            let mut cur: Option<f64> = None;
            let mut text = rest;
            while !text.is_empty() {
                if let Some(body) = text.strip_prefix('<')
                    && let Some((t, after)) = body.split_once('>')
                    && let Some(t) = stamp(t)
                {
                    cur = Some(t);
                    text = after;
                    continue;
                }
                let end = text[1..].find(['<', ' ']).map_or(text.len(), |i| i + 1);
                let w = text[..end].trim();
                if !w.is_empty() {
                    words.push(Word {
                        at: cur.unwrap_or(f64::NAN),
                        text: w.to_string(),
                    });
                }
                text = &text[end..];
            }
            song.lines.push(Line { at, words });
        }
    }
    song.lines.retain(|l| !l.words.is_empty());
    song.lines.sort_by(|a, b| a.at.total_cmp(&b.at));
    let ends: Vec<f64> = song.lines.iter().skip(1).map(|l| l.at).collect();
    for (i, line) in song.lines.iter_mut().enumerate() {
        let end = ends
            .get(i)
            .copied()
            .unwrap_or(line.at + 4.0)
            .min(line.at + 6.0);
        let n = line.words.len() as f64;
        for (j, w) in line.words.iter_mut().enumerate() {
            if w.at.is_nan() {
                w.at = line.at + (end - line.at) * 0.8 * j as f64 / n;
            }
        }
        line.at += offset;
        for w in &mut line.words {
            w.at += offset;
        }
    }
    song
}

#[derive(Default)]
pub(in crate::game) struct SkyLyrics {
    song: Option<Arc<Song>>,
    started: Option<Instant>,
    modified: Option<SystemTime>,
    checked: Option<Instant>,
    /// What the system's media player plays (`playerctl`), read on a thread of its own.
    playing: Option<Arc<Mutex<NowPlaying>>>,
    track: String,
    floating: Vec<Floating>,
    spawned_until: f64,
}

/// The playing track, its lyrics and its position when last read.
#[derive(Default)]
struct NowPlaying {
    key: String,
    song: Option<Arc<Song>>,
    position: f64,
    read: Option<Instant>,
    running: bool,
    title: String,
    artist: String,
    length: f64,
    cover: Option<Arc<Picture>>,
}

struct Track {
    artist: String,
    title: String,
    position: f64,
    running: bool,
    length: f64,
    art: String,
}

/// `playerctl` speaks MPRIS: Spotify, browsers, VLC, mpv... on Linux.
// shortcut: Linux only (MPRIS); Windows needs GlobalSystemMediaTransportControls, add it when someone plays there.
fn now_playing() -> Option<Track> {
    let out = std::process::Command::new("playerctl")
        .args([
            "metadata",
            "--format",
            "{{artist}}\t{{title}}\t{{position}}\t{{status}}\t{{mpris:length}}\t{{mpris:artUrl}}",
        ])
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let mut f = text.trim_end().split('\t');
    let (artist, title) = (f.next()?.to_string(), f.next()?.to_string());
    let position = f.next()?.parse::<f64>().ok()? / 1e6;
    let running = f.next() == Some("Playing");
    let length = f.next().and_then(|l| l.parse::<f64>().ok()).unwrap_or(0.0) / 1e6;
    let art = f.next().unwrap_or_default().to_string();
    (!title.is_empty()).then_some(Track {
        artist,
        title,
        position,
        running,
        length,
        art,
    })
}

/// Timed lyrics from LRCLIB (free, no key). Word-stamped (`<mm:ss>`) lyrics are word-synced;
/// line-timed ones get their words spread over the line.
fn fetch_lyrics(artist: &str, title: &str) -> Option<Song> {
    let body: serde_json::Value = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("Horizon sky lyrics")
        .build()
        .get("https://lrclib.net/api/search")
        .query("artist_name", artist)
        .query("track_name", title)
        .call()
        .ok()?
        .into_string()
        .ok()
        .and_then(|b| serde_json::from_str(&b).ok())?;
    let synced = body
        .as_array()?
        .iter()
        .find_map(|r| r.get("syncedLyrics")?.as_str())?;
    Some(parse(synced))
}

/// The album cover: Spotify gives an https URL, local players a file:// one.
fn fetch_cover(url: &str) -> Option<Arc<Picture>> {
    let bytes = if let Some(path) = url.strip_prefix("file://") {
        std::fs::read(path).ok()?
    } else if url.starts_with("http") {
        let mut body = Vec::new();
        std::io::Read::read_to_end(
            &mut std::io::Read::take(ureq::get(url).call().ok()?.into_reader(), 8 << 20),
            &mut body,
        )
        .ok()?;
        body
    } else {
        return None;
    };
    let (w, h, pixels) = horizon_client::hud::decode_image_bytes(&bytes).ok()?;
    Some(Picture::from_rgba(w, h, pixels))
}

fn watch(shared: Arc<Mutex<NowPlaying>>) {
    loop {
        let read = now_playing();
        let key = read
            .as_ref()
            .map(|t| format!("{}\t{}", t.artist, t.title))
            .unwrap_or_default();
        let changed = shared.lock().is_ok_and(|n| n.key != key);
        let (song, cover) = match &read {
            Some(t) if changed => (
                fetch_lyrics(&t.artist, &t.title).map(Arc::new),
                fetch_cover(&t.art),
            ),
            _ => (None, None),
        };
        if let Ok(mut n) = shared.lock() {
            if changed {
                n.key = key;
                n.song = song;
                n.cover = cover;
            }
            match read {
                Some(t) => {
                    (n.position, n.running, n.read) = (t.position, t.running, Some(Instant::now()));
                    (n.title, n.artist, n.length) = (t.title, t.artist, t.length);
                }
                None => n.read = None,
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

fn path() -> PathBuf {
    std::env::var_os("HORIZON_SKY_LYRICS").map_or_else(|| "sky-lyrics.lrc".into(), PathBuf::from)
}

const SCALE: f32 = 2.5;

impl Game<'_> {
    pub(in crate::game) fn draw_sky_lyrics(&mut self, batch: &mut HudBatch) {
        let now = Instant::now();
        let s = &mut self.sky_lyrics;
        if s.checked
            .is_none_or(|c| now.duration_since(c).as_secs_f32() > 0.5)
        {
            s.checked = Some(now);
            let path = path();
            let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            if modified != s.modified {
                s.modified = modified;
                s.song = std::fs::read_to_string(&path)
                    .ok()
                    .map(|t| Arc::new(parse(&t)));
                s.started = Some(now);
                s.floating.clear();
            }
        }
        // A file wins; otherwise what the media player plays.
        let (song, t) = if let (Some(song), Some(started)) = (&s.song, s.started) {
            (song.clone(), now.duration_since(started).as_secs_f64())
        } else {
            let shared = s.playing.get_or_insert_with(|| {
                let shared = Arc::new(Mutex::new(NowPlaying::default()));
                let thread = shared.clone();
                let _ = std::thread::Builder::new()
                    .name("sky lyrics".into())
                    .spawn(move || watch(thread));
                shared
            });
            let Ok(n) = shared.lock() else { return };
            let Some(read) = n.read else { return };
            if n.key != s.track {
                s.track = n.key.clone();
                s.floating.clear();
            }
            let since = if n.running {
                now.duration_since(read).as_secs_f64()
            } else {
                0.0
            };
            let t = n.position + since;
            if !self.debug {
                draw_card(batch, &self.gui.atlas, &n, t);
            }
            let Some(song) = n.song.clone() else { return };
            (song, t)
        };
        let Some((camera, yaw, pitch, fov)) = self.locator_camera else {
            return;
        };
        let (eye_yaw, eye_pitch) = (self.yaw, self.pitch);
        // Spawn each word as it is sung, where the crosshair points; a seek starts over.
        if t < s.spawned_until - 0.5 || t > s.spawned_until + 2.0 {
            s.spawned_until = t;
            s.floating.clear();
        }
        let sung = song.sung.unwrap_or(0xFFD54F);
        for word in song.lines.iter().flat_map(|l| &l.words) {
            if word.at > s.spawned_until && word.at <= t {
                let (sy, cy) = f64::from(eye_yaw.to_radians()).sin_cos();
                let (sp, cp) = f64::from(eye_pitch.to_radians()).sin_cos();
                // A little scatter so a fast line doesn't stack in one spot.
                let k = s.floating.len() as f64;
                let jitter = [(k * 2.39).sin() * 0.6, (k * 1.71).cos() * 0.4];
                let at = [
                    camera[0] - sy * cp * DISTANCE - cy * jitter[0],
                    camera[1] - sp * DISTANCE + jitter[1],
                    camera[2] + cy * cp * DISTANCE - sy * jitter[0],
                ];
                s.floating.push(Floating {
                    text: word.text.clone(),
                    at,
                    born: word.at,
                    color: sung,
                });
            }
        }
        s.spawned_until = s.spawned_until.max(t);
        s.floating.retain(|f| t - f.born < LIFE);

        let atlas = &self.gui.atlas;
        if let Some(i) = song.lines.iter().rposition(|l| l.at <= t) {
            let line = &song.lines[i];
            let last = line.words.last().map_or(line.at, |w| w.at);
            let until = song.lines.get(i + 1).map_or(last + 3.0, |n| n.at);
            let (fade_in, fade_out) = (t - line.at, until - t);
            if fade_out > 0.0 {
                let alpha = (fade_in.min(fade_out) / 0.3).clamp(0.0, 1.0) as f32;
                draw_pill(batch, atlas, line, t, alpha);
            }
        }

        // The camera's basis, as the locator bar builds it.
        let (sy, cy) = f64::from(yaw.to_radians()).sin_cos();
        let (sp, cp) = f64::from(pitch.to_radians()).sin_cos();
        let (f, r, u) = (
            [-sy * cp, -sp, cy * cp],
            [-cy, 0.0, -sy],
            [-sy * sp, cp, cy * sp],
        );
        let tan = (f64::from(fov).to_radians() * 0.5).tan();
        let (w, h) = (f64::from(batch.width), f64::from(batch.height));
        let style = Style {
            font: song.font.clone(),
            ..Style::default()
        };
        // Far words first, so near ones draw over them.
        let mut order: Vec<(f64, &Floating)> = s
            .floating
            .iter()
            .map(|fl| {
                let age = t - fl.born;
                // Drift upward as the word lives.
                let d = [
                    fl.at[0] - camera[0],
                    fl.at[1] - camera[1] + age * 0.25,
                    fl.at[2] - camera[2],
                ];
                (d[0] * f[0] + d[1] * f[1] + d[2] * f[2], fl)
            })
            .collect();
        order.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (z, fl) in order {
            if z < 0.3 {
                continue;
            }
            let age = t - fl.born;
            let d = [
                fl.at[0] - camera[0],
                fl.at[1] - camera[1] + age * 0.25,
                fl.at[2] - camera[2],
            ];
            let dot = |a: [f64; 3]| a[0] * d[0] + a[1] * d[1] + a[2] * d[2];
            let sx = (w * 0.5 * (1.0 + dot(r) / z / (tan * w / h))) as f32;
            let sy_ = (h * 0.5 * (1.0 - dot(u) / z / tan)) as f32;
            // Size follows distance, like text standing in the world.
            let size = (SCALE as f64 * DISTANCE / z / (tan / 0.6)) as f32;
            // Elastic pop in, fade out at the end of its life.
            let k = (age / 0.35).min(1.0) as f32;
            let pop = elastic(k);
            let alpha = (((LIFE - age) / 0.8).min(1.0) as f32).max(0.0) * k.min(1.0).sqrt();
            let scale = size * pop.max(0.05);
            let ww = batch_width(batch, atlas, &fl.text, &style);
            let (x, y) = (sx - ww * 0.5, sy_ - 4.0);
            let text = Text::styled(fl.text.clone(), style.clone());
            let (cr, cg, cb) = (
                (fl.color >> 16) as u8,
                (fl.color >> 8) as u8,
                fl.color as u8,
            );
            let start = batch.vertices.len();
            // Glow: soft rings of the word in its colour behind a white-hot core.
            let pulse = 0.6 + 0.4 * ((age * 6.0).sin() as f32 * 0.5 + 0.5);
            for (ring, a) in [(2.0f32, 40.0f32), (1.2, 70.0), (0.6, 110.0)] {
                for n in 0..8 {
                    let ang = n as f32 * std::f32::consts::FRAC_PI_4;
                    let glow = rgba(cr, cg, cb, (a * alpha * pulse) as u8);
                    batch.text_styled(
                        atlas,
                        &text,
                        x + ring * ang.cos(),
                        y + ring * ang.sin(),
                        glow,
                        false,
                    );
                }
            }
            // Fresh words flash white, then settle into their colour.
            let heat = (1.0 - (age / 0.5) as f32).clamp(0.0, 1.0);
            let mix = |c: u8| (f32::from(c) + (255.0 - f32::from(c)) * (0.55 + 0.45 * heat)) as u8;
            batch.text_styled(
                atlas,
                &text,
                x,
                y,
                rgba(mix(cr), mix(cg), mix(cb), (255.0 * alpha) as u8),
                false,
            );
            batch.scale_since(start, sx, sy_, scale, scale);
        }
    }
}

/// Words live this long in the world, in seconds.
const LIFE: f64 = 4.0;
/// How far ahead of the eyes a word appears, in blocks.
const DISTANCE: f64 = 6.0;

/// A sung word standing in the world.
pub(in crate::game) struct Floating {
    text: String,
    at: [f64; 3],
    born: f64,
    color: u32,
}

/// Overshoot then settle: 0 → ~1.25 → 1.
fn elastic(k: f32) -> f32 {
    if k >= 1.0 {
        return 1.0;
    }
    1.0 + 2f32.powf(-10.0 * k) * ((k - 0.075) * std::f32::consts::TAU / 0.3).sin()
}

const PANEL: u32 = rgba(16, 16, 22, 200);
const DIM: u32 = rgba(170, 170, 185, 255);
const ACCENT: u32 = rgba(255, 213, 79, 255);

/// A filled rectangle with round corners, as one fan from its centre.
fn rounded(batch: &mut HudBatch, atlas: &HudAtlas, [x, y, w, h]: [f32; 4], r: f32, color: u32) {
    let r = r.min(w * 0.5).min(h * 0.5);
    let corners = [
        (x + w - r, y + r, -90.0f32),
        (x + w - r, y + h - r, 0.0),
        (x + r, y + h - r, 90.0),
        (x + r, y + r, 180.0),
    ];
    let mut rim = Vec::with_capacity(36);
    for (cx, cy, from) in corners {
        for k in 0..=8 {
            let a = (from + 90.0 * k as f32 / 8.0).to_radians();
            rim.push([cx + r * a.cos(), cy + r * a.sin()]);
        }
    }
    rim.push(rim[0]);
    batch.fill_fan(atlas, [x + w * 0.5, y + h * 0.5], &rim, color);
}

/// Top left: the cover, title, artist and a progress bar.
fn draw_card(batch: &mut HudBatch, atlas: &HudAtlas, n: &NowPlaying, t: f64) {
    let (x, y, cover) = (6.0, 6.0, 36.0);
    let text_x = x + 6.0 + cover + 7.0;
    let title = ellipsize(atlas, &n.title, 130.0);
    let artist = ellipsize(atlas, &n.artist, 130.0);
    let text_w = HudBatch::text_width(atlas, &title)
        .max(HudBatch::text_width(atlas, &artist))
        .max(90.0);
    let (w, h) = (text_x - x + text_w + 8.0, cover + 12.0);
    rounded(batch, atlas, [x, y, w, h], 8.0, PANEL);
    match &n.cover {
        Some(p) => batch.picture_owned(p, x + 6.0, y + 6.0, cover, cover, rgba(255, 255, 255, 255)),
        None => rounded(
            batch,
            atlas,
            [x + 6.0, y + 6.0, cover, cover],
            5.0,
            rgba(60, 60, 75, 255),
        ),
    }
    batch.text(
        atlas,
        &title,
        text_x,
        y + 7.0,
        rgba(255, 255, 255, 255),
        false,
    );
    batch.text(atlas, &artist, text_x, y + 18.0, DIM, false);
    if n.length > 0.0 {
        let (by, bw) = (y + 33.0, text_w);
        let done = ((t / n.length) as f32).clamp(0.0, 1.0);
        rounded(
            batch,
            atlas,
            [text_x, by, bw, 3.0],
            1.5,
            rgba(255, 255, 255, 50),
        );
        rounded(
            batch,
            atlas,
            [text_x, by, (bw * done).max(3.0), 3.0],
            1.5,
            ACCENT,
        );
    }
    if !n.running {
        batch.text(
            atlas,
            "II",
            x + 6.0 + cover - 10.0,
            y + 6.0 + cover - 10.0,
            rgba(255, 255, 255, 230),
            true,
        );
    }
}

/// Top centre: the line being sung, its sung words lit.
fn draw_pill(batch: &mut HudBatch, atlas: &HudAtlas, line: &Line, t: f64, alpha: f32) {
    let text: Vec<&str> = line.words.iter().map(|w| w.text.as_str()).collect();
    let full = text.join(" ");
    let tw = HudBatch::text_width(atlas, &full);
    let (h, pad) = (16.0, 10.0);
    let w = tw + pad * 2.0;
    let x = (batch.width - w) * 0.5;
    let start = batch.vertices.len();
    rounded(batch, atlas, [x, 5.0, w, h], h * 0.5, PANEL);
    let mut wx = x + pad;
    for word in &line.words {
        let color = if t >= word.at {
            ACCENT
        } else {
            rgba(255, 255, 255, 140)
        };
        wx += batch.text(atlas, &word.text, wx, 9.0, color, false);
        wx += HudBatch::text_width(atlas, " ");
    }
    batch.alpha_since(start, alpha);
}

fn batch_width(
    batch: &mut HudBatch,
    atlas: &horizon_client::hud::HudAtlas,
    s: &str,
    style: &Style,
) -> f32 {
    // Measure with the word's own font: draw it and undo.
    let start = batch.vertices.len();
    let w = batch.text_styled(
        atlas,
        &Text::styled(s.to_string(), style.clone()),
        0.0,
        0.0,
        0,
        false,
    );
    batch.vertices.truncate(start);
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_word_and_line_sync() {
        let s = parse(
            "[offset:500]\n[font:minecraft:alt]\n[00:01.00]<00:01.00>Hello <00:01.50>sky\n[00:04.00]plain two\n",
        );
        assert_eq!(s.font.as_deref(), Some("minecraft:alt"));
        assert_eq!(s.lines.len(), 2);
        assert_eq!(
            s.lines[0].words[1],
            Word {
                at: 2.0,
                text: "sky".into()
            }
        );
        // Plain line: words spread over 0.8 of its 4 s fallback span, plus the offset.
        assert_eq!(s.lines[1].words[0].at, 4.5);
        assert_eq!(s.lines[1].words[1].at, 4.0 + 1.6 + 0.5);
    }
}
