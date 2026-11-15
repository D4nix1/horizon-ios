//! Sky lyrics (modded-pc branch): word-synced lyrics drawn in the sky, after WorldLyrics.
//!
//! Drop an enhanced LRC file at `sky-lyrics.lrc` in the game folder (or point
//! `HORIZON_SKY_LYRICS` at one). The song clock starts when the file appears or changes,
//! so save/touch it as you press play. Supported:
//! `[mm:ss.xx] <mm:ss.xx>word <mm:ss.xx>word` (word sync), plain `[mm:ss.xx] line`
//! (words spread over the line), `[offset:+/-ms]`, `[font:minecraft:alt]` (any pack font),
//! `[color:#RRGGBB]` (the sung colour).
use super::super::Game;
use horizon_client::hud::{HudBatch, rgba};
use horizon_client::text::{Style, Text};
use std::path::PathBuf;
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
    song: Option<Song>,
    started: Option<Instant>,
    modified: Option<SystemTime>,
    checked: Option<Instant>,
    /// The sky direction the lyrics hang at: yaw when the song started.
    anchor_yaw: f32,
}

fn path() -> PathBuf {
    std::env::var_os("HORIZON_SKY_LYRICS").map_or_else(|| "sky-lyrics.lrc".into(), PathBuf::from)
}

const ELEVATION: f64 = 30.0;
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
                s.song = std::fs::read_to_string(&path).ok().map(|t| parse(&t));
                s.started = Some(now);
                s.anchor_yaw = self.yaw;
            }
        }
        let (Some(song), Some(started), Some((_, yaw, pitch, fov))) =
            (&s.song, s.started, self.locator_camera)
        else {
            return;
        };
        let t = now.duration_since(started).as_secs_f64();
        let Some(i) = song.lines.iter().rposition(|l| l.at <= t) else {
            return;
        };
        let line = &song.lines[i];
        let last = line.words.last().map_or(line.at, |w| w.at);
        let until = song.lines.get(i + 1).map_or(last + 3.0, |n| n.at);
        let (fade_in, fade_out) = (t - line.at, until - t);
        if fade_out <= 0.0 {
            return;
        }
        let alpha = (fade_in.min(fade_out) / 0.3).clamp(0.0, 1.0) as f32;

        // Project the anchor's sky direction into GUI space, like the locator bar's camera.
        let (sy, cy) = f64::from(yaw.to_radians()).sin_cos();
        let (sp, cp) = f64::from(pitch.to_radians()).sin_cos();
        let (f, r, u) = (
            [-sy * cp, -sp, cy * cp],
            [-cy, 0.0, -sy],
            [-sy * sp, cp, cy * sp],
        );
        let (sa, ca) = f64::from(s.anchor_yaw.to_radians()).sin_cos();
        let (se, ce) = ELEVATION.to_radians().sin_cos();
        let d = [-sa * ce, se, ca * ce];
        let dot = |a: [f64; 3]| a[0] * d[0] + a[1] * d[1] + a[2] * d[2];
        let z = dot(f);
        if z < 0.1 {
            return;
        }
        let tan = (f64::from(fov).to_radians() * 0.5).tan();
        let (w, h) = (f64::from(batch.width), f64::from(batch.height));
        let cx = (w * 0.5 * (1.0 + dot(r) / z / (tan * w / h))) as f32;
        // A slow float, so the words drift like they hang in the air.
        let bob = (t * 1.3).sin() as f32 * 3.0;
        let cy_ = (h * 0.5 * (1.0 - dot(u) / z / tan)) as f32 + bob;

        let atlas = &self.gui.atlas;
        let style = Style {
            font: song.font.clone(),
            ..Style::default()
        };
        let space = HudBatch::text_width(atlas, " ");
        let widths: Vec<f32> = line
            .words
            .iter()
            .map(|w| batch_width(batch, atlas, &w.text, &style))
            .collect();
        let total = widths.iter().sum::<f32>() + space * (widths.len() as f32 - 1.0);
        let mut x = cx - total * SCALE * 0.5;
        let sung = song.sung.unwrap_or(0xFFD54F);
        let (sr, sg, sb) = ((sung >> 16) as u8, (sung >> 8) as u8, sung as u8);
        for (word, ww) in line.words.iter().zip(&widths) {
            let since = t - word.at;
            let (color, pop, lift) = if since < 0.0 {
                (rgba(255, 255, 255, (150.0 * alpha) as u8), 1.0, 0.0)
            } else {
                // Pop and rise as the word is sung, then settle.
                let k = (since / 0.18).min(1.0) as f32;
                let ease = 1.0 - (1.0 - k) * (1.0 - k);
                (
                    rgba(sr, sg, sb, (255.0 * alpha) as u8),
                    1.0 + 0.35 * (1.0 - ease),
                    -4.0 * (1.0 - ease),
                )
            };
            let start = batch.vertices.len();
            let text = Text::styled(word.text.clone(), style.clone());
            batch.text_styled(atlas, &text, x, cy_ - 4.0 + lift, color, true);
            batch.scale_since(start, x, cy_, SCALE, SCALE);
            batch.scale_since(start, x + ww * SCALE * 0.5, cy_, pop, pop);
            x += (ww + space) * SCALE;
        }
    }
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
