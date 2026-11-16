//! Visuals (modded-pc): cosmetic effects around your own player, switched in the client menu.
//! Jump circles (a rainbow ring spreads on the ground where you jump), a trail behind your
//! feet, and a world tint. Only your own player and the screen's colour change: nothing here
//! shows anything the game would otherwise hide.
//!
//! The circles and trail are world geometry in the text displays' pass (as LyricsPlus's 3D
//! lyrics are), so blocks in front of them hide them; the tint is the HUD's first layer.
use super::super::Game;
use horizon_client::hud::{HudBatch, rgba};
use horizon_client::mesher::Vertex;
use std::cell::Cell;
use std::collections::VecDeque;
use std::time::Instant;

/// How long a jump circle spreads and fades, in seconds.
const CIRCLE_LIFE: f32 = 0.6;
const CIRCLE_RADIUS: f32 = 1.1;
/// How long a trail point stays, in seconds.
const TRAIL_LIFE: f32 = 0.7;
const TRAIL_WIDTH: f32 = 0.32;
/// Full sky and block light: the effects glow.
const FULL_LIGHT: u32 = 240 | 240 << 8;

/// The tint colours the World Tint tile cycles through (right click); 0 cycles the hue.
pub(in crate::game) const TINTS: [(&str, &str, u32); 5] = [
    ("horizon.client.tint.chroma", "Chroma", 0),
    ("horizon.client.tint.sunset", "Sunset", 0xFF7A3C),
    ("horizon.client.tint.ocean", "Ocean", 0x2E8CFF),
    ("horizon.client.tint.violet", "Violet", 0xA050FF),
    ("horizon.client.tint.mint", "Mint", 0x3CE6AA),
];

/// Which effects show (the client menu's switches), read once from its settings.
#[derive(Clone, Copy, Debug)]
pub(in crate::game) struct VisualShows {
    pub circles: bool,
    pub trail: bool,
    pub tint: bool,
    pub tint_mode: usize,
}

pub(in crate::game) struct Visuals {
    started: Instant,
    circles: Vec<([f64; 3], Instant)>,
    trail: VecDeque<([f64; 3], Instant)>,
    was_on_ground: bool,
    pub(in crate::game) shows: Cell<Option<VisualShows>>,
}

impl Default for Visuals {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            circles: Vec::new(),
            trail: VecDeque::new(),
            was_on_ground: true,
            shows: Cell::new(None),
        }
    }
}

/// `h` (0..1 around the wheel) at full saturation and value, as RGB.
pub(in crate::game) fn hue(h: f32) -> (u8, u8, u8) {
    let h = h.rem_euclid(1.0) * 6.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// One flat-coloured quad (corners in order) into `out`, as two triangles.
fn quad(out: &mut Vec<Vertex>, uv: [f32; 2], corners: [[f32; 3]; 4], colors: [u32; 4]) {
    for i in [0, 1, 2, 0, 2, 3] {
        out.push(Vertex {
            pos: corners[i],
            uv,
            layer: horizon_client::gfx::TEXT_POLYGON_OFFSET_OVERLAY,
            color: colors[i],
            light: FULL_LIGHT,
            normal: [0; 4],
            glint_uv: [0.0, 0.0],
        });
    }
}

/// A flat ring on the ground around `centre` (camera-relative), from radius `inner` to `outer`,
/// its colour going round the hue wheel from `hue0`.
fn ring(
    out: &mut Vec<Vertex>,
    uv: [f32; 2],
    centre: [f32; 3],
    inner: f32,
    outer: f32,
    hue0: f32,
    alpha: u8,
) {
    const SEGMENTS: usize = 40;
    let point = |i: usize, r: f32| {
        let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        [centre[0] + r * a.cos(), centre[1], centre[2] + r * a.sin()]
    };
    let color = |i: usize| {
        let (r, g, b) = hue(hue0 + i as f32 / SEGMENTS as f32);
        rgba(r, g, b, alpha)
    };
    for i in 0..SEGMENTS {
        let j = i + 1;
        quad(
            out,
            uv,
            [
                point(i, inner),
                point(i, outer),
                point(j, outer),
                point(j, inner),
            ],
            [color(i), color(i), color(j), color(j)],
        );
    }
}

impl Game<'_> {
    /// The effects that show; jump circles and the trail until the client menu turns them off.
    pub(in crate::game) fn visual_shows(&self) -> VisualShows {
        if let Some(shows) = self.visuals.shows.get() {
            return shows;
        }
        let id = super::client_menu::ID;
        let read = |key, default: bool| {
            self.native_setting(id, key)
                .map_or(default, |v| v.trim() == "true")
        };
        let tint_mode = self
            .native_setting(id, "tint_mode")
            .and_then(|v| v.trim().parse().ok())
            .filter(|&m: &usize| m < TINTS.len())
            .unwrap_or(0);
        let shows = VisualShows {
            circles: read("jump_circles", true),
            trail: read("trail", true),
            tint: read("world_tint", false),
            tint_mode,
        };
        self.visuals.shows.set(Some(shows));
        shows
    }

    /// Where the player's feet are drawn this frame (between the last two ticks).
    fn visual_feet(&self) -> Option<[f64; 3]> {
        let p = self.player.as_ref()?;
        let t = self.clock.camera_fraction();
        let (a, b) = (p.old_position, p.position);
        Some([
            a.x + (b.x - a.x) * t,
            a.y + (b.y - a.y) * t,
            a.z + (b.z - a.z) * t,
        ])
    }

    /// Once a frame: a jump starts a circle; the trail follows the feet.
    pub(in crate::game) fn visuals_frame(&mut self) {
        let shows = self.visual_shows();
        let now = Instant::now();
        let Some(p) = self.player.as_ref() else {
            self.visuals.circles.clear();
            self.visuals.trail.clear();
            return;
        };
        let on_ground = p.ground.on_ground;
        let jumped = self.visuals.was_on_ground && !on_ground && p.velocity.y > 0.2 && !p.flying;
        let feet = [p.position.x, p.position.y, p.position.z];
        self.visuals.was_on_ground = on_ground;
        if shows.circles && jumped {
            self.visuals.circles.push((feet, now));
        }
        self.visuals
            .circles
            .retain(|(_, born)| now.duration_since(*born).as_secs_f32() < CIRCLE_LIFE);
        if shows.trail
            && let Some(at) = self.visual_feet()
        {
            let moved = self.visuals.trail.back().is_none_or(|(last, _)| {
                let d = [at[0] - last[0], at[1] - last[1], at[2] - last[2]];
                d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > 0.04 * 0.04
            });
            if moved {
                self.visuals.trail.push_back((at, now));
            }
        } else {
            self.visuals.trail.clear();
        }
        while self
            .visuals
            .trail
            .front()
            .is_some_and(|(_, t)| now.duration_since(*t).as_secs_f32() > TRAIL_LIFE)
        {
            self.visuals.trail.pop_front();
        }
    }

    /// The circles and the trail, camera-relative, into the text displays' vertices.
    pub(in crate::game) fn visuals_world(&self, camera: glam::DVec3, out: &mut Vec<Vertex>) {
        let shows = self.visual_shows();
        if self.player.is_none() || !(shows.circles || shows.trail) {
            return;
        }
        let uv = horizon_client::hud_kit::shapes::white(&self.gui.atlas);
        let now = Instant::now();
        let spin = self.visuals.started.elapsed().as_secs_f32() * 0.25;
        let rel = |p: [f64; 3]| {
            [
                (p[0] - camera.x) as f32,
                (p[1] - camera.y) as f32,
                (p[2] - camera.z) as f32,
            ]
        };
        for (at, born) in &self.visuals.circles {
            let k = (now.duration_since(*born).as_secs_f32() / CIRCLE_LIFE).min(1.0);
            let ease = 1.0 - (1.0 - k).powi(3);
            let r = CIRCLE_RADIUS * ease;
            let alpha = ((1.0 - k) * 255.0) as u8;
            let centre = rel([at[0], at[1] + 0.02, at[2]]);
            // A sharp ring with a softer glow inside it.
            ring(out, uv, centre, (r - 0.07).max(0.0), r, spin, alpha);
            ring(
                out,
                uv,
                centre,
                (r - 0.28).max(0.0),
                (r - 0.07).max(0.0),
                spin,
                alpha / 4,
            );
        }
        // The trail: a ribbon facing the camera, thin and clear at its old end.
        let points: Vec<([f64; 3], f32)> = self
            .visuals
            .trail
            .iter()
            .map(|(p, t)| {
                (
                    [p[0], p[1] + 0.12, p[2]],
                    now.duration_since(*t).as_secs_f32() / TRAIL_LIFE,
                )
            })
            .collect();
        for (i, pair) in points.windows(2).enumerate() {
            let ((a, ka), (b, kb)) = (pair[0], pair[1]);
            let (ra, rb) = (rel(a), rel(b));
            // Keep clear of the camera, so the ribbon never covers the view in first person.
            let near = |p: [f32; 3]| p[0] * p[0] + p[1] * p[1] + p[2] * p[2] < 0.8;
            if near(ra) || near(rb) {
                continue;
            }
            let dir = [rb[0] - ra[0], rb[1] - ra[1], rb[2] - ra[2]];
            let mid = [
                (ra[0] + rb[0]) / 2.0,
                (ra[1] + rb[1]) / 2.0,
                (ra[2] + rb[2]) / 2.0,
            ];
            // Across the ribbon: perpendicular to its way and to the line of sight.
            let side = [
                dir[1] * mid[2] - dir[2] * mid[1],
                dir[2] * mid[0] - dir[0] * mid[2],
                dir[0] * mid[1] - dir[1] * mid[0],
            ];
            let len = (side[0] * side[0] + side[1] * side[1] + side[2] * side[2]).sqrt();
            if len < 1e-6 {
                continue;
            }
            let half = |k: f32| TRAIL_WIDTH * 0.5 * (1.0 - k).max(0.0);
            let off = |p: [f32; 3], k: f32, s: f32| {
                let h = half(k) * s / len;
                [p[0] + side[0] * h, p[1] + side[1] * h, p[2] + side[2] * h]
            };
            let color = |k: f32| {
                let (r, g, b) = hue(spin + i as f32 * 0.02);
                rgba(r, g, b, ((1.0 - k).max(0.0) * 200.0) as u8)
            };
            quad(
                out,
                uv,
                [
                    off(ra, ka, 1.0),
                    off(rb, kb, 1.0),
                    off(rb, kb, -1.0),
                    off(ra, ka, -1.0),
                ],
                [color(ka), color(kb), color(kb), color(ka)],
            );
        }
    }

    /// The world tint: a light wash of colour over the world, under the HUD.
    pub(in crate::game) fn draw_world_tint(&self, batch: &mut HudBatch) {
        let shows = self.visual_shows();
        if !shows.tint || self.player.is_none() {
            return;
        }
        let rgb = match TINTS[shows.tint_mode].2 {
            0 => hue(self.visuals.started.elapsed().as_secs_f32() * 0.05),
            c => ((c >> 16) as u8, (c >> 8) as u8, c as u8),
        };
        let (w, h) = (batch.width, batch.height);
        // Stronger in the sky, lighter on the ground.
        let top = rgba(rgb.0, rgb.1, rgb.2, 58);
        let bottom = rgba(rgb.0, rgb.1, rgb.2, 26);
        batch.fill_gradient(&self.gui.atlas, (0.0, 0.0, w, h), top, bottom);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hue_goes_round_the_wheel() {
        assert_eq!(hue(0.0), (255, 0, 0));
        assert_eq!(hue(1.0 / 3.0), (0, 255, 0));
        assert_eq!(hue(2.0 / 3.0), (0, 0, 255));
        assert_eq!(hue(1.0), hue(0.0));
    }

    #[test]
    fn a_ring_is_whole_quads_around_its_centre() {
        let mut out = Vec::new();
        ring(&mut out, [0.0; 2], [1.0, 2.0, 3.0], 0.5, 1.0, 0.0, 255);
        assert_eq!(out.len() % 6, 0);
        for v in &out {
            let (dx, dz) = (v.pos[0] - 1.0, v.pos[2] - 3.0);
            let r = (dx * dx + dz * dz).sqrt();
            assert!((0.49..=1.01).contains(&r), "{r}");
            assert_eq!(v.pos[1], 2.0);
        }
    }
}
