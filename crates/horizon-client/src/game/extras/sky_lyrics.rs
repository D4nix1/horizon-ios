//! Sky lyrics (modded-pc): each sung word pops up and glows, with a now-playing card top left
//! and the line in a pill at the top. Three modes (the client menu's Lyrics tile, right click):
//!
//! - **World**: the word appears where the crosshair points and stays at that spot in the
//!   world, as real geometry in the text displays' pass: blocks in front of it hide it, and it
//!   is placed in front of a wall rather than inside it.
//! - **Scatter**: the word pops at a random free spot on the screen, never over the hotbar,
//!   the bars, the chat corner, the crosshair, the card, the pill or another word.
//! - **Focus**: one big word at a time above the crosshair.
//!
//! What plays, where it is, its lyrics and their word timings come from LyricsPlus's engine
//! (`super::super::lyricsplus::now_playing`): its sources, sync offsets and K menu still apply.
//! The words take the album cover's accent colour.
use super::super::Game;
use super::super::lyricsplus;
use horizon_client::hud::{HudAtlas, HudBatch, pictures::Picture, rgba};
use horizon_client::hud_kit::fit::ellipsize;
use horizon_client::mesher::Vertex;
use lyricsplus_engine::lrc::Lyrics;
use std::cell::Cell;
use std::sync::{Arc, Mutex};

/// How far ahead of the eyes a World word appears, in blocks (nearer when a block is closer).
const DISTANCE: f64 = 6.0;
/// A World word's font pixel, in blocks.
const WORLD_PX: f32 = 0.05;
const PANEL: u32 = rgba(16, 16, 22, 200);
const DIM: u32 = rgba(170, 170, 185, 255);
/// The sung colour before a cover gives one (0xRRGGBB).
const SUNG: u32 = 0xFFD54F;
/// Full sky and block light: World words glow.
const FULL_LIGHT: u32 = 240 | 240 << 8;

/// Where sung words appear.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::game) enum Mode {
    World,
    Scatter,
    Focus,
}

/// The modes in the order the Lyrics tile cycles them, with their names.
pub(in crate::game) const MODES: [(Mode, &str, &str); 3] = [
    (Mode::World, "horizon.client.lyrics_mode.world", "World"),
    (
        Mode::Scatter,
        "horizon.client.lyrics_mode.scatter",
        "Scatter",
    ),
    (Mode::Focus, "horizon.client.lyrics_mode.focus", "Focus"),
];

impl Mode {
    /// How long a word stays, in seconds.
    fn life(self) -> f64 {
        match self {
            Mode::World => 4.0,
            Mode::Scatter => 2.6,
            Mode::Focus => 1.6,
        }
    }
}

/// Where a sung word is.
#[derive(Clone, Copy, Debug)]
enum Place {
    /// A world position.
    World([f64; 3]),
    /// GUI pixels: the word's centre, and its size in GUI pixels per font pixel.
    Screen { x: f32, y: f32, scale: f32 },
}

/// A sung word on show.
pub(in crate::game) struct Floating {
    text: String,
    place: Place,
    /// When it was sung, in song seconds.
    born: f64,
}

/// The cover and its accent, decoded on their own thread.
type Cover = (u64, Arc<Picture>, u32);

#[derive(Default)]
pub(in crate::game) struct SkyLyrics {
    floating: Vec<Floating>,
    /// Words sung up to here (song seconds) are already spawned.
    spawned_until: f64,
    /// This frame's song time, for the world pass.
    now: f64,
    serial: u64,
    cover: Arc<Mutex<Option<Cover>>>,
    cover_tried: u64,
    /// Scatter's random state.
    seed: u64,
    /// Which parts show (the client menu's switches), read once from its settings.
    pub(in crate::game) shows: Cell<Option<Shows>>,
}

/// The parts and mode the client menu switches.
#[derive(Clone, Copy, Debug)]
pub(in crate::game) struct Shows {
    pub words: bool,
    pub card: bool,
    pub pill: bool,
    /// An index into [`MODES`].
    pub mode: usize,
}

/// Every word of `lyrics` with when it is sung, in song seconds, for lines `lines`.
fn words_in(
    lyrics: &Lyrics,
    lines: std::ops::RangeInclusive<usize>,
    duration_ms: u64,
) -> Vec<(f64, String)> {
    lines
        .flat_map(|i| lyrics.words(i, duration_ms))
        .map(|(ms, w)| (ms as f64 / 1000.0, w))
        .filter(|(_, w)| !w.is_empty())
        .collect()
}

/// Overshoot then settle: 0 → ~1.25 → 1.
fn elastic(k: f32) -> f32 {
    if k >= 1.0 {
        return 1.0;
    }
    1.0 + 2f32.powf(-10.0 * k) * ((k - 0.075) * std::f32::consts::TAU / 0.3).sin()
}

/// xorshift64: Scatter's spots, no allocation, no dependency.
fn next(seed: &mut u64) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    (*seed >> 40) as f32 / (1u64 << 24) as f32
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

/// The screen's parts a Scatter word keeps off, in a `w` by `h` GUI: the hotbar and the bars
/// above it, the chat corner, the crosshair, the effect icons, the corners the FPS counter
/// and keystrokes use, and the card and pill when they show.
fn busy(w: f32, h: f32, shows: Shows) -> Vec<[f32; 4]> {
    let mut rects = vec![
        [w / 2.0 - 106.0, h - 68.0, 212.0, 68.0],
        [0.0, h - 110.0, 330.0_f32.min(w * 0.5), 110.0],
        [w / 2.0 - 28.0, h / 2.0 - 18.0, 56.0, 36.0],
        [w - 64.0, 0.0, 64.0, 54.0],
        [0.0, 0.0, 96.0, 96.0],
        [w - 96.0, 0.0, 96.0, 96.0],
        [w - 96.0, h - 96.0, 96.0, 96.0],
        // Boss bars.
        [w / 2.0 - 100.0, 0.0, 200.0, 40.0],
    ];
    if shows.card {
        rects.push([0.0, 0.0, 210.0, 58.0]);
    }
    if shows.pill {
        rects.push([w / 2.0 - 160.0, 0.0, 320.0, 28.0]);
    }
    rects
}

/// A free spot for a `size` word in a `w` by `h` GUI: off `busy` and every rectangle in
/// `taken`, 8 pixels inside the edges. None when 40 tries find no room.
fn free_spot(
    seed: &mut u64,
    w: f32,
    h: f32,
    size: [f32; 2],
    busy: &[[f32; 4]],
    taken: &[[f32; 4]],
) -> Option<[f32; 2]> {
    let margin = 8.0;
    let (room_w, room_h) = (w - size[0] - 2.0 * margin, h - size[1] - 2.0 * margin);
    if room_w <= 0.0 || room_h <= 0.0 {
        return None;
    }
    (0..40).find_map(|_| {
        let (x, y) = (margin + next(seed) * room_w, margin + next(seed) * room_h);
        // A little air around it, so neighbours don't touch.
        let rect = [x - 4.0, y - 3.0, size[0] + 8.0, size[1] + 6.0];
        let clear = !busy.iter().chain(taken).any(|b| overlaps(rect, *b));
        clear.then_some([x + size[0] / 2.0, y + size[1] / 2.0])
    })
}

/// A word's rectangle on the screen, for `free_spot`'s `taken`.
fn screen_rect(atlas: &HudAtlas, f: &Floating) -> Option<[f32; 4]> {
    let Place::Screen { x, y, scale } = f.place else {
        return None;
    };
    let (ww, wh) = (HudBatch::text_width(atlas, &f.text) * scale, 9.0 * scale);
    Some([x - ww / 2.0, y - wh / 2.0, ww, wh])
}

/// `text` with its glow, centred on (0, 0) in font pixels: the glow's vertices, then the word's.
fn glowing(
    b: &mut HudBatch,
    atlas: &HudAtlas,
    text: &str,
    accent: u32,
    alpha: f32,
    age: f64,
) -> usize {
    let ww = HudBatch::text_width(atlas, text);
    let (x, y) = (-ww / 2.0, -4.5);
    let (cr, cg, cb) = ((accent >> 16) as u8, (accent >> 8) as u8, accent as u8);
    // Soft rings of the word in its colour, pulsing.
    let pulse = 0.6 + 0.4 * ((age * 6.0).sin() as f32 * 0.5 + 0.5);
    for (ring, a) in [(2.0f32, 40.0f32), (1.2, 70.0), (0.6, 110.0)] {
        for n in 0..8 {
            let ang = n as f32 * std::f32::consts::FRAC_PI_4;
            let glow = rgba(cr, cg, cb, (a * alpha * pulse) as u8);
            b.text(
                atlas,
                text,
                x + ring * ang.cos(),
                y + ring * ang.sin(),
                glow,
                false,
            );
        }
    }
    let core = b.vertices.len();
    // Fresh words flash white, then settle into their colour.
    let heat = (1.0 - (age / 0.5) as f32).clamp(0.0, 1.0);
    let mix = |c: u8| (f32::from(c) + (255.0 - f32::from(c)) * (0.55 + 0.45 * heat)) as u8;
    let color = rgba(mix(cr), mix(cg), mix(cb), (255.0 * alpha) as u8);
    b.text(atlas, text, x, y, color, false);
    core
}

/// A word's opacity and pop at `age` seconds of a `life`.
fn fade_pop(age: f64, life: f64) -> (f32, f32) {
    let k = (age / 0.35).min(1.0) as f32;
    let alpha = (((life - age) / 0.6).min(1.0) as f32).max(0.0) * k.sqrt();
    (alpha, elastic(k).max(0.05))
}

impl Game<'_> {
    /// The sky lyrics' parts and mode; all on, World, until the client menu changes them.
    pub(in crate::game) fn sky_shows(&self) -> Shows {
        if let Some(shows) = self.sky_lyrics.shows.get() {
            return shows;
        }
        let id = super::client_menu::ID;
        let read = |key| {
            self.native_setting(id, key)
                .is_none_or(|v| v.trim() != "false")
        };
        let mode = self
            .native_setting(id, "lyrics_mode")
            .and_then(|v| v.trim().parse().ok())
            .filter(|&m: &usize| m < MODES.len())
            .unwrap_or(0);
        let shows = Shows {
            words: read("lyric_words"),
            card: read("song_card"),
            pill: read("lyrics_pill"),
            mode,
        };
        self.sky_lyrics.shows.set(Some(shows));
        shows
    }

    /// The cover's accent (0xRRGGBB) once it is decoded, else the default gold.
    fn sky_accent(&self) -> u32 {
        let cover = self
            .sky_lyrics
            .cover
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        cover
            .as_ref()
            .filter(|(serial, ..)| *serial == self.sky_lyrics.serial)
            .map_or(SUNG, |(.., accent)| *accent)
    }

    /// Where a World word goes: `DISTANCE` along the look, or just in front of the block the
    /// look meets first, so it is never inside a wall.
    fn world_spot(&self, index: usize) -> Option<[f64; 3]> {
        let (origin, dir) = self.aim()?;
        let reach = self
            .clip_ray(origin, dir, DISTANCE)
            .map(|hit| {
                let d: [f64; 3] = std::array::from_fn(|i| hit.point[i] - origin[i]);
                (d.iter().map(|v| v * v).sum::<f64>().sqrt() - 0.6).max(1.2)
            })
            .unwrap_or(DISTANCE);
        // A little scatter so a fast line doesn't stack in one spot.
        let k = index as f64;
        let (side, lift) = (
            (k * 2.39).sin() * 0.1 * reach,
            (k * 1.71).cos() * 0.07 * reach,
        );
        // Right of the look, level with the ground.
        let right = [-dir[2], 0.0, dir[0]];
        let flat = (right[0] * right[0] + right[2] * right[2]).sqrt().max(1e-6);
        Some([
            origin[0] + dir[0] * reach + right[0] / flat * side,
            origin[1] + dir[1] * reach + lift,
            origin[2] + dir[2] * reach + right[2] / flat * side,
        ])
    }

    pub(in crate::game) fn draw_sky_lyrics(&mut self, batch: &mut HudBatch) {
        let shows = self.sky_shows();
        let mode = MODES[shows.mode].0;
        let Some((snap, pos_ms)) = lyricsplus::now_playing(self) else {
            self.sky_lyrics.floating.clear();
            return;
        };
        let Some(track) = snap.track.clone() else {
            return;
        };
        let s = &mut self.sky_lyrics;
        if track.serial != s.serial {
            s.serial = track.serial;
            s.floating.clear();
            s.spawned_until = 0.0;
        }
        // A new cover: decode it and take its accent, off the frame.
        if let Some(art) = snap
            .art
            .as_ref()
            .filter(|a| a.serial == track.serial && s.cover_tried != a.serial)
        {
            s.cover_tried = art.serial;
            let (slot, bytes, serial) = (s.cover.clone(), art.bytes.clone(), art.serial);
            let _ = std::thread::Builder::new()
                .name("sky-lyrics-cover".into())
                .spawn(move || {
                    if let Ok((w, h, pixels)) = horizon_client::hud::decode_image_bytes(&bytes) {
                        let accent = lyricsplus_engine::art::palette(w, h, &pixels).accent;
                        let picture = Picture::from_rgba(w, h, pixels);
                        *slot.lock().unwrap_or_else(|p| p.into_inner()) =
                            Some((serial, picture, accent & 0xFF_FFFF));
                    }
                });
        }
        let cover = s
            .cover
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .filter(|(serial, ..)| *serial == track.serial)
            .map(|(_, p, _)| p.clone());
        let accent = self.sky_accent();
        let atlas = &self.gui.atlas;
        if shows.card && !self.debug {
            let progress =
                (track.duration_ms > 0).then(|| snap.position_ms as f32 / track.duration_ms as f32);
            let card = Card {
                title: &track.title,
                artist: &track.artist,
                cover: cover.as_ref(),
                progress,
                playing: snap.playing,
                accent,
            };
            draw_card(batch, atlas, &card);
        }
        let lyrics = &snap.lyrics;
        let t = pos_ms as f64 / 1000.0;
        self.sky_lyrics.now = t;

        // The pill: the line being sung, or notes through a long instrumental gap.
        if shows.pill {
            if let Some((left, total)) = lyrics.gap_remaining(pos_ms) {
                draw_gap_pill(
                    batch,
                    atlas,
                    t,
                    1.0 - left as f32 / total.max(1) as f32,
                    accent,
                );
            } else if let Some(i) = lyrics.index_at(pos_ms) {
                let line = words_in(lyrics, i..=i, track.duration_ms);
                let start = lyrics.lines[i].time_ms as f64 / 1000.0;
                let end = lyrics.end_of(i, track.duration_ms) as f64 / 1000.0;
                let alpha = ((t - start).min(end - t) / 0.3).clamp(0.0, 1.0) as f32;
                if !line.is_empty() && alpha > 0.0 {
                    draw_pill(batch, atlas, &line, t, alpha, accent);
                }
            }
        }

        // Spawn each word as it is sung; a seek, a mode change or words off starts over.
        let s = &mut self.sky_lyrics;
        if t < s.spawned_until - 0.5 || t > s.spawned_until + 2.0 {
            s.spawned_until = t;
            s.floating.clear();
        }
        if !shows.words {
            s.floating.clear();
            return;
        }
        let stale = |f: &Floating| match f.place {
            Place::World(_) => mode != Mode::World,
            Place::Screen { .. } => mode == Mode::World,
        };
        s.floating.retain(|f| !stale(f));
        let Some(i) = lyrics.index_at(pos_ms) else {
            return;
        };
        let (w, h) = (batch.width, batch.height);
        let fresh: Vec<(f64, String)> =
            words_in(lyrics, i.saturating_sub(1)..=i, track.duration_ms)
                .into_iter()
                .filter(|(at, _)| *at > self.sky_lyrics.spawned_until && *at <= t)
                .collect();
        for (at, text) in fresh {
            let place = match mode {
                Mode::World => match self.world_spot(self.sky_lyrics.floating.len()) {
                    Some(p) => Place::World(p),
                    None => continue,
                },
                Mode::Focus => {
                    // One word at a time: the last one makes way.
                    self.sky_lyrics.floating.clear();
                    Place::Screen {
                        x: w / 2.0,
                        y: (h * 0.32).floor(),
                        scale: 3.5,
                    }
                }
                Mode::Scatter => {
                    let s = &mut self.sky_lyrics;
                    if s.seed == 0 {
                        s.seed = 0x9E37_79B9_7F4A_7C15
                            ^ track.serial.wrapping_mul(0x2545_F491_4F6C_DD1D);
                    }
                    let scale = 1.8 + next(&mut s.seed) * 1.0;
                    let size = [HudBatch::text_width(atlas, &text) * scale, 9.0 * scale];
                    let taken: Vec<[f32; 4]> = s
                        .floating
                        .iter()
                        .filter_map(|f| screen_rect(atlas, f))
                        .collect();
                    match free_spot(&mut s.seed, w, h, size, &busy(w, h, shows), &taken) {
                        Some([x, y]) => Place::Screen { x, y, scale },
                        // No room this time: the word passes unseen rather than cover the HUD.
                        None => continue,
                    }
                }
            };
            self.sky_lyrics.floating.push(Floating {
                text,
                place,
                born: at,
            });
        }
        let s = &mut self.sky_lyrics;
        s.spawned_until = s.spawned_until.max(t);
        let life = mode.life();
        s.floating.retain(|f| t - f.born < life);

        // Screen words; World words are drawn in the world pass (`sky_lyrics_world`).
        // Over an open screen they would sit on its buttons: hide them there.
        if self.screen != super::super::Screen::Playing {
            return;
        }
        for f in &self.sky_lyrics.floating {
            let Place::Screen { x, y, scale } = f.place else {
                continue;
            };
            let age = t - f.born;
            let (alpha, pop) = fade_pop(age, life);
            // Focus words rise a little as they go.
            let rise = if mode == Mode::Focus {
                -(age as f32) * 6.0
            } else {
                0.0
            };
            let start = batch.vertices.len();
            glowing(batch, atlas, &f.text, accent, alpha, age);
            for v in &mut batch.vertices[start..] {
                v.pos = [
                    x + v.pos[0] * scale * pop,
                    y + rise + v.pos[1] * scale * pop,
                ];
            }
        }
    }

    /// World words, camera-relative, into the text displays' vertices: they face the camera,
    /// drift up as they live, and blocks in front of them hide them.
    pub(in crate::game) fn sky_lyrics_world(&mut self, camera: glam::DVec3, out: &mut Vec<Vertex>) {
        let shows = self.sky_shows();
        if !shows.words || MODES[shows.mode].0 != Mode::World || self.sky_lyrics.floating.is_empty()
        {
            return;
        }
        let Some((_, yaw, pitch, _)) = self.locator_camera else {
            return;
        };
        let (sy, cy) = yaw.to_radians().sin_cos();
        let (sp, cp) = pitch.to_radians().sin_cos();
        // The camera's right and up, as the locator bar builds them.
        let right = glam::Vec3::new(-cy, 0.0, -sy);
        let up = glam::Vec3::new(-sy * sp, cp, cy * sp);
        let toward = right.cross(up);
        let (t, accent, life) = (self.sky_lyrics.now, self.sky_accent(), Mode::World.life());
        let mut b = HudBatch::new([1, 1]);
        for f in &self.sky_lyrics.floating {
            let Place::World(at) = f.place else {
                continue;
            };
            let age = t - f.born;
            let (alpha, pop) = fade_pop(age, life);
            let centre = glam::Vec3::new(
                (at[0] - camera.x) as f32,
                (at[1] + age * 0.25 - camera.y) as f32,
                (at[2] - camera.z) as f32,
            );
            b.vertices.clear();
            let core = glowing(&mut b, &self.gui.atlas, &f.text, accent, alpha, age);
            let px = WORLD_PX * pop;
            for (q, quad) in b.vertices.chunks_exact(6).enumerate() {
                // The word a hair in front of its glow, so they don't fight over pixels.
                let lift = if q * 6 >= core { 0.004 } else { 0.0 };
                let corner = |i: usize| {
                    let v = quad[i];
                    let p = centre + right * (v.pos[0] * px) - up * (v.pos[1] * px) - toward * lift;
                    Vertex {
                        pos: p.to_array(),
                        uv: v.uv,
                        layer: horizon_client::gfx::TEXT_POLYGON_OFFSET_OVERLAY,
                        color: v.color,
                        light: FULL_LIGHT,
                        normal: [0; 4],
                        glint_uv: [0.0, 0.0],
                    }
                };
                let corners = horizon_client::text_display::CORNERS.map(corner);
                // Both windings: the layer culls one face, and which one depends on the view.
                out.extend(corners);
                out.extend(corners.into_iter().rev());
            }
        }
        // Glyphs the font had to rasterize go up with the HUD's.
        self.map_patches.append(&mut b.patches);
    }
}

/// What the card shows.
struct Card<'a> {
    title: &'a str,
    artist: &'a str,
    cover: Option<&'a Arc<Picture>>,
    progress: Option<f32>,
    playing: bool,
    accent: u32,
}

/// Top left: the cover, title, artist and a progress bar in the cover's colour.
fn draw_card(batch: &mut HudBatch, atlas: &HudAtlas, c: &Card<'_>) {
    let (x, y, side) = (6.0, 6.0, 36.0);
    let text_x = x + 6.0 + side + 7.0;
    let title = ellipsize(atlas, c.title, 130.0);
    let artist = ellipsize(atlas, c.artist, 130.0);
    let text_w = HudBatch::text_width(atlas, &title)
        .max(HudBatch::text_width(atlas, &artist))
        .max(90.0);
    batch.fill_rounded(
        atlas,
        [x, y, text_x - x + text_w + 8.0, side + 12.0],
        8.0,
        PANEL,
    );
    match c.cover {
        Some(p) => batch.picture_owned(p, x + 6.0, y + 6.0, side, side, u32::MAX),
        None => batch.fill_rounded(
            atlas,
            [x + 6.0, y + 6.0, side, side],
            5.0,
            rgba(60, 60, 75, 255),
        ),
    }
    batch.text(atlas, &title, text_x, y + 7.0, u32::MAX, false);
    batch.text(atlas, &artist, text_x, y + 18.0, DIM, false);
    if let Some(done) = c.progress {
        let by = y + 33.0;
        batch.fill_rounded(
            atlas,
            [text_x, by, text_w, 3.0],
            1.5,
            rgba(255, 255, 255, 50),
        );
        let fill = (text_w * done.clamp(0.0, 1.0)).max(3.0);
        let a = c.accent;
        let bar = rgba((a >> 16) as u8, (a >> 8) as u8, a as u8, 255);
        batch.fill_rounded(atlas, [text_x, by, fill, 3.0], 1.5, bar);
    }
    if !c.playing {
        batch.text(
            atlas,
            "II",
            x + side - 4.0,
            y + side - 4.0,
            rgba(255, 255, 255, 230),
            true,
        );
    }
}

/// Top centre: the line being sung, its sung words lit.
fn draw_pill(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    words: &[(f64, String)],
    t: f64,
    alpha: f32,
    accent: u32,
) {
    let space = HudBatch::text_width(atlas, " ");
    // A long line is cut to the screen: the words around the one being sung.
    let max = batch.width - 40.0;
    let widths: Vec<f32> = words
        .iter()
        .map(|(_, w)| HudBatch::text_width(atlas, w))
        .collect();
    let current = words.iter().rposition(|(at, _)| *at <= t).unwrap_or(0);
    let width =
        |lo: usize, hi: usize| widths[lo..hi].iter().sum::<f32>() + space * (hi - lo - 1) as f32;
    // Grow the window around the current word, a word each side in turn, while it fits.
    let (mut lo, mut hi) = (current, current + 1);
    loop {
        let grow_right = hi < words.len() && (lo == 0 || hi - current <= current - lo);
        let (l, h) = if grow_right {
            (lo, hi + 1)
        } else if lo > 0 {
            (lo - 1, hi)
        } else {
            break;
        };
        if width(l, h) > max {
            break;
        }
        (lo, hi) = (l, h);
    }
    let tw = width(lo, hi);
    let (h, pad) = (16.0, 10.0);
    let w = tw + pad * 2.0;
    let x = (batch.width - w) * 0.5;
    let start = batch.vertices.len();
    batch.fill_rounded(atlas, [x, 5.0, w, h], h * 0.5, PANEL);
    let sung = rgba((accent >> 16) as u8, (accent >> 8) as u8, accent as u8, 255);
    let mut wx = x + pad;
    for (at, word) in &words[lo..hi] {
        let color = if t >= *at {
            sung
        } else {
            rgba(255, 255, 255, 140)
        };
        wx += batch.text(atlas, word, wx, 9.0, color, false) + space;
    }
    batch.alpha_since(start, alpha);
}

/// Top centre through an instrumental gap: three notes lighting in turn, and how far through.
fn draw_gap_pill(batch: &mut HudBatch, atlas: &HudAtlas, t: f64, done: f32, accent: u32) {
    let (w, h) = (70.0, 16.0);
    let x = ((batch.width - w) * 0.5).floor();
    batch.fill_rounded(atlas, [x, 5.0, w, h], h * 0.5, PANEL);
    let lit = rgba((accent >> 16) as u8, (accent >> 8) as u8, accent as u8, 255);
    for n in 0..3 {
        let phase = ((t * 2.0 - n as f64 * 0.33).rem_euclid(1.0) * std::f64::consts::TAU).sin();
        let color = if phase > 0.3 {
            lit
        } else {
            rgba(255, 255, 255, 120)
        };
        batch.text(atlas, "♪", x + 18.0 + n as f32 * 14.0, 9.0, color, false);
    }
    batch.fill_rounded(
        atlas,
        [
            x + 8.0,
            5.0 + h - 3.0,
            (w - 16.0) * done.clamp(0.0, 1.0),
            1.5,
        ],
        0.75,
        lit,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyricsplus_engine::lrc::parse;

    #[test]
    fn words_come_timed_from_the_engine() {
        let lyrics = Lyrics {
            lines: parse("[00:01.00]<00:01.00>one <00:01.50>two\n[00:04.00]three four\n"),
            synced: true,
            ..Default::default()
        };
        let w = words_in(&lyrics, 0..=1, 60_000);
        assert_eq!(
            w.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>(),
            ["one", "two", "three", "four"]
        );
        assert_eq!((w[0].0, w[1].0, w[2].0), (1.0, 1.5, 4.0));
        assert!(w[3].0 > 4.0);
    }

    #[test]
    fn scatter_spots_keep_off_the_hud_and_each_other() {
        let shows = Shows {
            words: true,
            card: true,
            pill: true,
            mode: 1,
        };
        for (w, h) in [(480.0, 270.0), (640.0, 360.0), (960.0, 540.0)] {
            let busy = busy(w, h, shows);
            let mut taken: Vec<[f32; 4]> = Vec::new();
            let mut seed = 0x1234_5678_9ABC_DEF0;
            let size = [60.0, 20.0];
            for _ in 0..6 {
                let Some([x, y]) = free_spot(&mut seed, w, h, size, &busy, &taken) else {
                    continue;
                };
                let rect = [x - size[0] / 2.0, y - size[1] / 2.0, size[0], size[1]];
                assert!(
                    rect[0] >= 8.0 && rect[0] + rect[2] <= w - 8.0,
                    "{rect:?} in {w}x{h}"
                );
                assert!(
                    rect[1] >= 8.0 && rect[1] + rect[3] <= h - 8.0,
                    "{rect:?} in {w}x{h}"
                );
                assert!(
                    !busy.iter().chain(&taken).any(|b| overlaps(rect, *b)),
                    "{rect:?}"
                );
                taken.push(rect);
            }
            assert!(!taken.is_empty(), "no room at all in {w}x{h}");
        }
    }

    #[test]
    fn no_spot_when_the_screen_is_full() {
        let mut seed = 7;
        assert_eq!(
            free_spot(
                &mut seed,
                100.0,
                60.0,
                [60.0, 20.0],
                &[[0.0, 0.0, 100.0, 60.0]],
                &[]
            ),
            None
        );
    }
}
