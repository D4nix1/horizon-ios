//! Sky lyrics (modded-pc): each sung word pops at the crosshair and hangs there in the world,
//! glowing, with a now-playing card top left and the line in a pill at the top.
//!
//! What plays, where it is, its lyrics and their word timings all come from LyricsPlus's engine
//! (`super::super::lyricsplus::now_playing`): its sources, sync offsets and K menu still apply.
//! LyricsPlus's own HUD and world drawing are off; this draws instead.
use super::super::Game;
use super::super::lyricsplus;
use horizon_client::hud::{HudAtlas, HudBatch, pictures::Picture, rgba};
use horizon_client::hud_kit::fit::ellipsize;
use lyricsplus_engine::lrc::Lyrics;
use std::sync::{Arc, Mutex};

/// Words live this long in the world, in seconds.
const LIFE: f64 = 4.0;
/// How far ahead of the eyes a word appears, in blocks.
const DISTANCE: f64 = 6.0;
/// The words' size at `DISTANCE`, in GUI pixels per font pixel.
const SCALE: f32 = 2.5;
const PANEL: u32 = rgba(16, 16, 22, 200);
const DIM: u32 = rgba(170, 170, 185, 255);
const SUNG: u32 = 0xFFD54F;

/// A sung word standing in the world.
pub(in crate::game) struct Floating {
    text: String,
    at: [f64; 3],
    /// When it was sung, in song seconds.
    born: f64,
}

#[derive(Default)]
pub(in crate::game) struct SkyLyrics {
    floating: Vec<Floating>,
    /// Words sung up to here (song seconds) are already spawned.
    spawned_until: f64,
    serial: u64,
    /// The cover, decoded on its own thread: (track serial, picture).
    cover: Arc<Mutex<Option<(u64, Arc<Picture>)>>>,
    cover_tried: u64,
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

impl Game<'_> {
    pub(in crate::game) fn draw_sky_lyrics(&mut self, batch: &mut HudBatch) {
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
        // A new cover: decode it off the frame.
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
                        *slot.lock().unwrap_or_else(|p| p.into_inner()) =
                            Some((serial, Picture::from_rgba(w, h, pixels)));
                    }
                });
        }
        let cover = s
            .cover
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .filter(|(serial, _)| *serial == track.serial)
            .map(|(_, p)| p.clone());
        let atlas = &self.gui.atlas;
        if !self.debug {
            let progress =
                (track.duration_ms > 0).then(|| snap.position_ms as f32 / track.duration_ms as f32);
            draw_card(
                batch,
                atlas,
                &track.title,
                &track.artist,
                cover.as_ref(),
                progress,
                snap.playing,
            );
        }
        let lyrics = &snap.lyrics;
        let t = pos_ms as f64 / 1000.0;
        let Some(i) = lyrics.index_at(pos_ms) else {
            return;
        };

        // The line in the pill, fading in and out with it.
        let line_words = words_in(lyrics, i..=i, track.duration_ms);
        let start = lyrics.lines[i].time_ms as f64 / 1000.0;
        let end = lyrics.end_of(i, track.duration_ms) as f64 / 1000.0;
        let alpha = ((t - start).min(end - t) / 0.3).clamp(0.0, 1.0) as f32;
        if !line_words.is_empty() && alpha > 0.0 {
            draw_pill(batch, atlas, &line_words, t, alpha);
        }

        // Spawn each word as it is sung, where the crosshair points; a seek starts over.
        let s = &mut self.sky_lyrics;
        if t < s.spawned_until - 0.5 || t > s.spawned_until + 2.0 {
            s.spawned_until = t;
            s.floating.clear();
        }
        let Some((camera, yaw, pitch, fov)) = self.locator_camera else {
            return;
        };
        let (eye_yaw, eye_pitch) = (self.yaw, self.pitch);
        for (at, text) in words_in(lyrics, i.saturating_sub(1)..=i, track.duration_ms) {
            if at > s.spawned_until && at <= t {
                let (sy, cy) = f64::from(eye_yaw.to_radians()).sin_cos();
                let (sp, cp) = f64::from(eye_pitch.to_radians()).sin_cos();
                // A little scatter so a fast line doesn't stack in one spot.
                let k = s.floating.len() as f64;
                let jitter = [(k * 2.39).sin() * 0.6, (k * 1.71).cos() * 0.4];
                let pos = [
                    camera[0] - sy * cp * DISTANCE - cy * jitter[0],
                    camera[1] - sp * DISTANCE + jitter[1],
                    camera[2] + cy * cp * DISTANCE - sy * jitter[0],
                ];
                s.floating.push(Floating {
                    text,
                    at: pos,
                    born: at,
                });
            }
        }
        s.spawned_until = s.spawned_until.max(t);
        s.floating.retain(|f| t - f.born < LIFE);

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
        // Each word's offset from the camera, drifting up as it lives; far words first.
        let mut order: Vec<([f64; 3], &Floating)> = s
            .floating
            .iter()
            .map(|fl| {
                let age = t - fl.born;
                (
                    [
                        fl.at[0] - camera[0],
                        fl.at[1] - camera[1] + age * 0.25,
                        fl.at[2] - camera[2],
                    ],
                    fl,
                )
            })
            .collect();
        let depth = |d: &[f64; 3]| d[0] * f[0] + d[1] * f[1] + d[2] * f[2];
        order.sort_by(|a, b| depth(&b.0).total_cmp(&depth(&a.0)));
        for (d, fl) in order {
            let z = depth(&d);
            if z < 0.3 {
                continue;
            }
            let dot = |a: [f64; 3]| a[0] * d[0] + a[1] * d[1] + a[2] * d[2];
            let sx = (w * 0.5 * (1.0 + dot(r) / z / (tan * w / h))) as f32;
            let sy_ = (h * 0.5 * (1.0 - dot(u) / z / tan)) as f32;
            let age = t - fl.born;
            // Size follows distance, like text standing in the world; an elastic pop in.
            let k = (age / 0.35).min(1.0) as f32;
            let scale = (SCALE as f64 * DISTANCE / z / (tan / 0.6)) as f32 * elastic(k).max(0.05);
            let alpha = (((LIFE - age) / 0.8).min(1.0) as f32).max(0.0) * k.sqrt();
            let ww = HudBatch::text_width(atlas, &fl.text);
            let (x, y) = (sx - ww * 0.5, sy_ - 4.0);
            let (cr, cg, cb) = ((SUNG >> 16) as u8, (SUNG >> 8) as u8, SUNG as u8);
            let start = batch.vertices.len();
            // Glow: soft rings of the word in its colour behind a white-hot core.
            let pulse = 0.6 + 0.4 * ((age * 6.0).sin() as f32 * 0.5 + 0.5);
            for (ring, a) in [(2.0f32, 40.0f32), (1.2, 70.0), (0.6, 110.0)] {
                for n in 0..8 {
                    let ang = n as f32 * std::f32::consts::FRAC_PI_4;
                    let glow = rgba(cr, cg, cb, (a * alpha * pulse) as u8);
                    batch.text(
                        atlas,
                        &fl.text,
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
            batch.text(
                atlas,
                &fl.text,
                x,
                y,
                rgba(mix(cr), mix(cg), mix(cb), (255.0 * alpha) as u8),
                false,
            );
            batch.scale_since(start, sx, sy_, scale, scale);
        }
    }
}

/// Overshoot then settle: 0 → ~1.25 → 1.
fn elastic(k: f32) -> f32 {
    if k >= 1.0 {
        return 1.0;
    }
    1.0 + 2f32.powf(-10.0 * k) * ((k - 0.075) * std::f32::consts::TAU / 0.3).sin()
}

/// Top left: the cover, title, artist and a progress bar.
fn draw_card(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    title: &str,
    artist: &str,
    cover: Option<&Arc<Picture>>,
    progress: Option<f32>,
    playing: bool,
) {
    let (x, y, side) = (6.0, 6.0, 36.0);
    let text_x = x + 6.0 + side + 7.0;
    let title = ellipsize(atlas, title, 130.0);
    let artist = ellipsize(atlas, artist, 130.0);
    let text_w = HudBatch::text_width(atlas, &title)
        .max(HudBatch::text_width(atlas, &artist))
        .max(90.0);
    batch.fill_rounded(
        atlas,
        [x, y, text_x - x + text_w + 8.0, side + 12.0],
        8.0,
        PANEL,
    );
    match cover {
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
    if let Some(done) = progress {
        let by = y + 33.0;
        batch.fill_rounded(
            atlas,
            [text_x, by, text_w, 3.0],
            1.5,
            rgba(255, 255, 255, 50),
        );
        let fill = (text_w * done.clamp(0.0, 1.0)).max(3.0);
        batch.fill_rounded(atlas, [text_x, by, fill, 3.0], 1.5, SUNG | 0xFF00_0000);
    }
    if !playing {
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
fn draw_pill(batch: &mut HudBatch, atlas: &HudAtlas, words: &[(f64, String)], t: f64, alpha: f32) {
    let space = HudBatch::text_width(atlas, " ");
    let tw: f32 = words
        .iter()
        .map(|(_, w)| HudBatch::text_width(atlas, w))
        .sum::<f32>()
        + space * (words.len() as f32 - 1.0);
    let (h, pad) = (16.0, 10.0);
    let w = tw + pad * 2.0;
    let x = (batch.width - w) * 0.5;
    let start = batch.vertices.len();
    batch.fill_rounded(atlas, [x, 5.0, w, h], h * 0.5, PANEL);
    let mut wx = x + pad;
    for (at, word) in words {
        let sung = rgba((SUNG >> 16) as u8, (SUNG >> 8) as u8, SUNG as u8, 255);
        let color = if t >= *at {
            sung
        } else {
            rgba(255, 255, 255, 140)
        };
        wx += batch.text(atlas, word, wx, 9.0, color, false) + space;
    }
    batch.alpha_since(start, alpha);
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
}
