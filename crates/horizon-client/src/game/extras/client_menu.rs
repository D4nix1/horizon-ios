//! The client menu (modded-pc): Right Shift opens a panel of tiles that switch the client's
//! features on and off: the FPS counter, keystrokes, CPS, the sky lyrics' words, card and
//! pill, and Horizon's extras. It is a mod screen like LyricsPlus's (Escape closes it, the
//! cursor is free); the key is rebindable in Options > Mods.
use super::super::Game;
use horizon_client::hud::{HudAtlas, HudBatch, rgba};
use horizon_client::lang;
use horizon_client::mod_api::{NativeMod, NativeScreenView};
use std::sync::Mutex;

pub(in crate::game) const ID: &str = "horizon-client";
const KEY_MENU: (&str, &str, &str) = (
    "open_menu",
    "Open the client menu",
    "key.keyboard.right.shift",
);

const DIM_TOP: u32 = rgba(4, 6, 12, 165);
const DIM_BOTTOM: u32 = rgba(4, 6, 12, 215);
const PANEL: u32 = rgba(14, 16, 24, 238);
const EDGE: u32 = rgba(255, 255, 255, 26);
const TILE: u32 = rgba(255, 255, 255, 14);
const TILE_HOVER: u32 = rgba(255, 255, 255, 30);
const ON: u32 = rgba(88, 204, 128, 255);
const OFF: u32 = rgba(78, 80, 96, 255);
const ACCENT: u32 = rgba(110, 168, 255, 255);
const MUTED: u32 = rgba(150, 156, 175, 255);

const TILE_W: f32 = 92.0;
const TILE_H: f32 = 50.0;
const GAP: f32 = 8.0;
const PAD: f32 = 14.0;
const HEADER: f32 = 34.0;

/// One switch in the menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::game) enum Tile {
    Fps,
    Keystrokes,
    Cps,
    LyricWords,
    SongCard,
    LyricsPill,
    Zoom,
    Fullbright,
    DynamicLights,
    OwnNameTag,
    BlockParticles,
    ChatAnimation,
}

impl Tile {
    const ALL: [Tile; 12] = [
        Tile::Fps,
        Tile::Keystrokes,
        Tile::Cps,
        Tile::LyricWords,
        Tile::SongCard,
        Tile::LyricsPill,
        Tile::Zoom,
        Tile::Fullbright,
        Tile::DynamicLights,
        Tile::OwnNameTag,
        Tile::BlockParticles,
        Tile::ChatAnimation,
    ];
    fn label(self) -> String {
        let (key, plain) = match self {
            Tile::Fps => ("horizon.client.fps", "FPS"),
            Tile::Keystrokes => ("horizon.client.keystrokes", "Keystrokes"),
            Tile::Cps => ("horizon.client.cps", "CPS"),
            Tile::LyricWords => ("horizon.client.lyric_words", "Sky Lyrics"),
            Tile::SongCard => ("horizon.client.song_card", "Song Card"),
            Tile::LyricsPill => ("horizon.client.lyrics_pill", "Lyrics Pill"),
            Tile::Zoom => ("horizon.client.zoom", "Zoom"),
            Tile::Fullbright => ("horizon.client.fullbright", "Fullbright"),
            Tile::DynamicLights => ("horizon.client.dynamic_lights", "Dynamic Lights"),
            Tile::OwnNameTag => ("horizon.client.own_name_tag", "Own Nametag"),
            Tile::BlockParticles => ("horizon.client.block_particles", "Hide Particles"),
            Tile::ChatAnimation => ("horizon.client.chat_animation", "Chat Animation"),
        };
        lang::get_or(key, plain)
    }
    fn get(self, g: &Game<'_>) -> bool {
        let e = &g.settings.extras;
        match self {
            Tile::Fps => e.fps,
            Tile::Keystrokes => e.keystrokes,
            Tile::Cps => e.keystrokes_cps,
            Tile::LyricWords => g.sky_shows().words,
            Tile::SongCard => g.sky_shows().card,
            Tile::LyricsPill => g.sky_shows().pill,
            Tile::Zoom => e.zoom,
            Tile::Fullbright => e.fullbright,
            Tile::DynamicLights => e.dynamic_lights,
            Tile::OwnNameTag => e.own_name_tag,
            Tile::BlockParticles => e.hide_block_particles,
            Tile::ChatAnimation => e.chat_animation,
        }
    }
    fn toggle(self, g: &mut Game<'_>) {
        let e = &mut g.settings.extras;
        let flag = match self {
            Tile::Fps => &mut e.fps,
            Tile::Keystrokes => &mut e.keystrokes,
            Tile::Cps => &mut e.keystrokes_cps,
            Tile::Zoom => &mut e.zoom,
            Tile::Fullbright => &mut e.fullbright,
            Tile::DynamicLights => &mut e.dynamic_lights,
            Tile::OwnNameTag => &mut e.own_name_tag,
            Tile::BlockParticles => &mut e.hide_block_particles,
            Tile::ChatAnimation => &mut e.chat_animation,
            Tile::LyricWords | Tile::SongCard | Tile::LyricsPill => {
                let mut shows = g.sky_shows();
                let (name, flag) = match self {
                    Tile::LyricWords => ("lyric_words", &mut shows.words),
                    Tile::SongCard => ("song_card", &mut shows.card),
                    _ => ("lyrics_pill", &mut shows.pill),
                };
                *flag = !*flag;
                g.native_set_setting(ID, name, if *flag { "true" } else { "false" });
                g.sky_lyrics.shows.set(Some(shows));
                return;
            }
        };
        *flag = !*flag;
        g.settings.save(&Game::options_path());
    }
}

/// Clicks wait here for the frame: the screen hooks only read the game.
static PENDING: Mutex<Vec<Tile>> = Mutex::new(Vec::new());
/// The tiles as last drawn, for clicks to hit.
static DRAWN: Mutex<Vec<(Tile, [f32; 4])>> = Mutex::new(Vec::new());

/// The panel and each tile's rectangle, centred in a `w` by `h` screen.
fn layout(w: f32, h: f32) -> ([f32; 4], Vec<(Tile, [f32; 4])>) {
    let cols = ((w - 2.0 * PAD - 20.0 + GAP) / (TILE_W + GAP))
        .floor()
        .clamp(2.0, 4.0) as usize;
    let rows = Tile::ALL.len().div_ceil(cols);
    let pw = cols as f32 * (TILE_W + GAP) - GAP + 2.0 * PAD;
    let ph = HEADER + rows as f32 * (TILE_H + GAP) - GAP + 2.0 * PAD;
    let (px, py) = (((w - pw) / 2.0).floor(), ((h - ph) / 2.0).floor().max(4.0));
    let tiles = Tile::ALL
        .iter()
        .enumerate()
        .map(|(i, &t)| {
            let (c, r) = ((i % cols) as f32, (i / cols) as f32);
            (
                t,
                [
                    px + PAD + c * (TILE_W + GAP),
                    py + PAD + HEADER + r * (TILE_H + GAP),
                    TILE_W,
                    TILE_H,
                ],
            )
        })
        .collect();
    ([px, py, pw, ph], tiles)
}

fn inside([x, y, w, h]: [f32; 4], [mx, my]: [f32; 2]) -> bool {
    mx >= x && mx < x + w && my >= y && my < y + h
}

fn mine(g: &Game<'_>) -> bool {
    g.native_mod_screen() == Some(ID)
}

// ---------------------------------------------------------------- hooks

pub(in crate::game) fn listed(mods: &mut Vec<NativeMod>) {
    mods.push(NativeMod {
        id: ID.into(),
        title: lang::get_or("horizon.client.title", "Horizon Client"),
        description: lang::get_or(
            "horizon.client.description",
            "The client menu: overlays, lyrics and extras.",
        ),
        keys: vec![[
            KEY_MENU.0.to_owned(),
            KEY_MENU.1.to_owned(),
            KEY_MENU.2.to_owned(),
        ]],
    });
}

pub(in crate::game) fn key(g: &mut Game<'_>, key: &str, pressed: bool) {
    if !pressed || g.native_mod_key(ID, KEY_MENU.0, KEY_MENU.2) != key {
        return;
    }
    if mine(g) {
        g.native_mod_screen_close(ID);
    } else if g.native_playing() {
        g.native_mod_screen_open(ID);
    }
}

pub(in crate::game) fn frame(g: &mut Game<'_>) {
    let clicks = std::mem::take(&mut *PENDING.lock().unwrap_or_else(|p| p.into_inner()));
    for tile in clicks {
        tile.toggle(g);
    }
}

pub(in crate::game) fn click(
    g: &Game<'_>,
    screen: &NativeScreenView<'_>,
    button: i32,
    canceled: &mut bool,
) {
    if !mine(g) {
        return;
    }
    *canceled = true;
    if button != 0 {
        return;
    }
    let drawn = DRAWN.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((tile, _)) = drawn.iter().find(|(_, r)| inside(*r, screen.mouse)) {
        PENDING
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(*tile);
    }
}

pub(in crate::game) fn draw(g: &Game<'_>, b: &mut HudBatch, screen: &NativeScreenView<'_>) {
    if !mine(g) {
        return;
    }
    let atlas = &g.gui.atlas;
    let (w, h) = (b.width, b.height);
    b.fill_gradient(atlas, (0.0, 0.0, w, h), DIM_TOP, DIM_BOTTOM);
    let ([px, py, pw, ph], tiles) = layout(w, h);
    b.fill_rounded(atlas, [px - 1.0, py - 1.0, pw + 2.0, ph + 2.0], 11.0, EDGE);
    b.fill_rounded(atlas, [px, py, pw, ph], 10.0, PANEL);
    header(atlas, b, [px, py, pw]);
    let pending = PENDING.lock().unwrap_or_else(|p| p.into_inner()).clone();
    DRAWN
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone_from(&tiles);
    for (tile, rect) in tiles {
        // A click not applied yet shows already.
        let on = tile.get(g) ^ (pending.iter().filter(|t| **t == tile).count() % 2 == 1);
        draw_tile(atlas, b, tile, rect, on, inside(rect, screen.mouse));
    }
}

fn header(atlas: &HudAtlas, b: &mut HudBatch, [px, py, pw]: [f32; 3]) {
    let word = lang::get_or("horizon.title.wordmark", "HORIZON");
    let start = b.vertices.len();
    let ww = b.text(atlas, &word, 0.0, 0.0, u32::MAX, false);
    let (x, y) = (px + PAD, py + PAD);
    for v in &mut b.vertices[start..] {
        v.pos = [x + v.pos[0] * 2.0, y + v.pos[1] * 2.0];
    }
    let sub = lang::get_or("horizon.client.menu", "Client");
    b.text(atlas, &sub, x + ww * 2.0 + 6.0, y + 6.0, ACCENT, false);
    let hint = lang::get_or("horizon.client.close_hint", "Right Shift / Esc to close");
    let hw = HudBatch::text_width(atlas, &hint);
    b.text(atlas, &hint, px + pw - PAD - hw, y + 6.0, MUTED, false);
    b.fill(atlas, px + PAD, py + PAD + 24.0, pw - 2.0 * PAD, 1.0, EDGE);
}

fn draw_tile(
    atlas: &HudAtlas,
    b: &mut HudBatch,
    tile: Tile,
    [x, y, w, h]: [f32; 4],
    on: bool,
    hover: bool,
) {
    b.fill_rounded(
        atlas,
        [x, y, w, h],
        8.0,
        if hover { TILE_HOVER } else { TILE },
    );
    if on {
        // A thin accent along the top marks what is on.
        b.fill_rounded(atlas, [x + 10.0, y, w - 20.0, 2.0], 1.0, ON);
    }
    let label = tile.label();
    let tw = HudBatch::text_width(atlas, &label);
    let ink = if on { u32::MAX } else { MUTED };
    b.text(
        atlas,
        &label,
        x + ((w - tw) / 2.0).floor(),
        y + 12.0,
        ink,
        false,
    );
    // The switch: a pill with its knob right when on.
    let (sw, sh) = (24.0, 12.0);
    let (sx, sy) = (x + ((w - sw) / 2.0).floor(), y + h - sh - 9.0);
    b.fill_rounded(atlas, [sx, sy, sw, sh], sh / 2.0, if on { ON } else { OFF });
    let knob = if on { sx + sw - sh + 2.0 } else { sx + 2.0 };
    b.fill_rounded(
        atlas,
        [knob, sy + 2.0, sh - 4.0, sh - 4.0],
        (sh - 4.0) / 2.0,
        u32::MAX,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_fit_the_screen_and_do_not_overlap() {
        for (w, h) in [(320.0, 240.0), (480.0, 270.0), (960.0, 540.0)] {
            let ([px, py, pw, ph], tiles) = layout(w, h);
            assert!(px >= 0.0 && px + pw <= w, "{w}x{h}");
            assert_eq!(tiles.len(), Tile::ALL.len());
            for (i, (_, a)) in tiles.iter().enumerate() {
                assert!(
                    a[0] >= px && a[0] + a[2] <= px + pw && a[1] >= py && a[1] + a[3] <= py + ph
                );
                for (_, c) in &tiles[i + 1..] {
                    let apart = a[0] + a[2] <= c[0]
                        || c[0] + c[2] <= a[0]
                        || a[1] + a[3] <= c[1]
                        || c[1] + c[3] <= a[1];
                    assert!(apart, "{a:?} {c:?}");
                }
            }
        }
    }
}
