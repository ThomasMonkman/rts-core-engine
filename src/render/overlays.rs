//! What is drawn with the world, laid out on the GPU side at the frame's
//! interpolated positions so it stays on its units: strategic icons over
//! the units and projectiles (drawn in the interface pass, under the
//! interface), and the world overlays (lifebars, order ETAs, the drag box,
//! debug paths) in their own egui context. The main thread's part comes in
//! the frame's input ([`OverlayState`]); the icons' rectangles go back to it
//! for picking.

use std::sync::Arc;

use super::scene::Scene;
use super::world::FrameInput;

/// The main thread's state the overlays draw by (`FrameInput::overlays`).
#[derive(Clone, Default)]
pub struct OverlayState {
    /// Whose lifebars show.
    pub rules: BarRules,
    /// FAF's `display_eta`: order waypoints' ETAs.
    pub display_eta: bool,
    /// The cursor (window pixels), a left press (box selection start) and a
    /// right press (the facing drag: its start and the ground there).
    pub cursor: Option<(f64, f64)>,
    pub left_press: Option<(f64, f64)>,
    pub right_press: Option<((f64, f64), glam::Vec3)>,
    /// Orders just given (their flashes).
    pub markers: Vec<super::player::Marker>,
    /// Each blueprint's lifebar (`Snapshot::blueprint_lifebars`).
    pub lifebars: Option<Arc<Vec<[f32; 3]>>>,
    /// The alliances (whose bars count as allied).
    pub alliances: Option<Arc<Vec<(u32, u32, crate::engine::sim::army::Alliance)>>>,
    /// Projectiles' strategic-view icons, by index.
    pub projectile_icons: Arc<Vec<Option<crate::engine::sim::projectile::ProjectileIcon>>>,
}

/// What the overlays read: the frame's scene (interpolated entities, the
/// views' icon lists, the latest snapshot) and the main thread's input.
pub struct Sources<'a> {
    pub scene: &'a Scene,
    pub input: &'a FrameInput,
}

impl Sources<'_> {
    fn entities(&self) -> &[crate::engine::snapshot::EntityView] {
        &self.scene.frame_entities
    }

    /// The view under window pixel (x, y) and its camera (as
    /// `World::view_at`).
    fn view_at(&self, x: f32, y: f32) -> Option<(&super::camera::View, &super::camera::Camera)> {
        let views = &self.input.views;
        let shown = |v: &&super::camera::View| self.input.cameras.get(&v.camera).is_some();
        let v = views.iter().rev().filter(|v| v.contains(x, y)).find(shown).or_else(|| views.iter().find(shown))?;
        Some((v, self.input.cameras.get(&v.camera)?))
    }

    /// Whether army `b` is `a`'s ally.
    fn allied(&self, a: u32, b: u32) -> bool {
        use crate::engine::sim::army::Alliance;
        self.input.overlays.alliances.as_deref().is_some_and(|l| l.iter().any(|&(f, t, x)| f == a && t == b && x == Alliance::Ally))
    }

    fn debug(&self) -> Option<&crate::engine::snapshot::DebugData> {
        self.scene.latest.as_ref().map(|s| &s.debug)
    }

    fn surface_at(&self, x: f32, z: f32) -> f32 {
        self.scene.surface_at(x, z)
    }
}

/// Per strategic icon name: each state's texture size in texels (rest,
/// over, selected, selected and over: the selected states are 4 texels
/// bigger each way, a frame around the resting icon; a missing state's is
/// the resting one's) and the four textures.
pub struct IconInfo {
    pub sizes: [(u32, u32); 4],
    pub paths: [String; 4],
}

/// The icons' sizes, read once from their texture headers.
#[derive(Default)]
pub struct IconCache {
    info: std::collections::HashMap<String, IconInfo>,
    sizes: std::collections::HashMap<String, (u32, u32)>,
    /// The frame's icon layers and draw order ([`draw_order`]), kept to
    /// reuse their memory.
    layers: Vec<u8>,
    order: Vec<u32>,
}

/// What decides whose lifebars show (moho's `ui_RenderUnitBars`,
/// `UI_ForceLifbarsOnEnemy`, the profile's `show_attached_unit_lifebars`).
#[derive(Clone, Copy, Debug, Default)]
pub struct BarRules {
    pub render: bool,
    pub force_enemy: bool,
    pub show_attached: bool,
    /// The army the player commands (`None`: observing).
    pub viewer: Option<u32>,
    /// The unit under the cursor.
    pub hovered: Option<u32>,
}

/// View depth from which lifebars aren't drawn (moho's `ui_LifebarLOD`,
/// 200: MohoEngine.dll .data 0x10898de0, compared with the view depth at
/// 0x1042087e).
const LIFEBAR_LOD: f32 = 200.0;

/// Moho's lifebar rule (MohoEngine.dll 0x10420869-0x10420911), for a unit
/// that can carry one (`scene::shows_bar`), at any health: bars on, then
/// the player's own and allied units (every unit when observing), or an
/// enemy in sight that is forced (`UI_ForceLifbarsOnEnemy`) or under the
/// cursor; a unit attached to another (on a transport) only with Show
/// Lifebars of Attached Units. `allied`: the unit's army is the viewer's
/// or allied to it.
pub fn bar_visible(e: &crate::engine::snapshot::EntityView, allied: bool, rules: &BarRules) -> bool {
    use crate::engine::snapshot::Sighting;
    if !rules.render || (e.attached && !rules.show_attached) {
        return false;
    }
    rules.viewer.is_none() || allied || (e.sighting == Sighting::Seen && (rules.force_enemy || rules.hovered == Some(e.id)))
}

/// The lifebar's colour at `health` (0-1): moho's `ui_LifeBarGoodColor`
/// above `ui_LifeBarGoodCutoff`, `ui_LifeBarMedColor` above
/// `ui_LifeBarBadCutoff`, else `ui_LifeBarBadColor` (defaults green, yellow,
/// red, 0.75 and 0.25: MohoEngine.dll .data 0x10898e04-0x10898e14, chosen
/// at 0x104216ed).
pub fn lifebar_color(health: f32) -> egui::Color32 {
    if health > 0.75 {
        egui::Color32::from_rgb(0, 255, 0)
    } else if health > 0.25 {
        egui::Color32::from_rgb(255, 255, 0)
    } else {
        egui::Color32::from_rgb(255, 0, 0)
    }
}

/// Health bars, paths, weapon ranges and the selection marker, drawn in
/// their own egui context (over the 3D scene, under the interface).
/// Lifebars: the on-screen units that can carry one (`scene::shows_bar`;
/// the posed frame collects them) that moho's rule shows
/// ([`bar_visible`]), nearer than [`LIFEBAR_LOD`] in view depth.
pub fn draw_world_overlays(ctx: &egui::Context, scene: &Sources) {
    let player = &scene.input.overlays;
    let (cursor, rules) = (player.cursor, player.rules);
    // Each world view's, clipped to it (egui points).
    let ppp = ctx.pixels_per_point();
    for (index, view) in scene.input.views.iter().enumerate() {
        let Some(cam) = scene.input.cameras.get(&view.camera) else { continue };
        let [l, t, r, b] = view.rect.map(|v| v / ppp);
        let clip = egui::Rect::from_min_max(egui::Pos2::new(l, t), egui::Pos2::new(r, b));
        let painter = ctx.layer_painter(egui::LayerId::background()).with_clip_rect(clip);
        let to_screen = |p: glam::Vec3| view.project(cam, p).map(|[x, y]| egui::Pos2::new(x / ppp, y / ppp));
        let started_here = |p: Option<(f64, f64)>| p.is_some_and(|(x, y)| scene.view_at(x as f32, y as f32).is_some_and(|(v, _)| std::ptr::eq(v, view)));
        let here = [started_here(player.left_press), started_here(player.right_press.map(|p| p.0))];
        draw_view_overlays(&painter, clip, scene, index, cam, &to_screen, player, cursor, rules, here);
    }
}

/// One world view's overlays (`draw_world_overlays`): `clip` is view
/// `index` (egui points), `to_screen` projects through its camera `cam`. `here`:
/// the drag box and the facing arrow were started in this view (they show
/// in that one only).
#[allow(clippy::too_many_arguments)]
fn draw_view_overlays(
    painter: &egui::Painter,
    clip: egui::Rect,
    scene: &Sources,
    index: usize,
    cam: &crate::render::camera::Camera,
    to_screen: &dyn Fn(glam::Vec3) -> Option<egui::Pos2>,
    player: &OverlayState,
    cursor: Option<(f64, f64)>,
    rules: BarRules,
    here: [bool; 2],
) {
    use egui::{Color32, Pos2, Stroke};
    let by_id = |id: u32| scene.entities().binary_search_by_key(&id, |e| e.id).ok().map(|i| &scene.entities()[i]);
    // Lifebars, by moho's rule, nearer than its LOD in view depth.
    {
        let forward = (cam.target - cam.eye()).normalize_or_zero();
        let eye = cam.eye();
        let (right, down) = cam.ground_axes();
        let allied = |e: &crate::engine::snapshot::EntityView| rules.viewer.is_some_and(|v| v == e.army || scene.allied(v, e.army));
        let bars = scene.scene.bars.iter().filter_map(|&i| scene.entities().get(i as usize));
        for e in bars.filter(|e| (glam::Vec3::from_array(e.position) - eye).dot(forward) < LIFEBAR_LOD && bar_visible(e, allied(e), &rules)) {
            // Any work above 0 (moho clamps it to 1 when drawing).
            let working = e.work > 0.0;
            // Moho draws the bars flat under the unit, sized in world units
            // by its blueprint: `LifeBarSize` wide and `LifeBarHeight` tall,
            // `LifeBarOffset` towards the bottom of the screen, along the
            // ground (south unless the camera is turned). Without them, the
            // collision box.
            let life = player.lifebars.as_deref().and_then(|l| l.get(e.blueprint as usize)).copied().unwrap_or_default();
            if life[0] < 0.0 {
                continue;
            }
            let (size, height, offset) = if life[0] > 0.0 { (life[0], life[1].max(0.05), life[2]) } else { (e.half[0] * 2.0, 0.075, e.half[2]) };
            let at = glam::Vec3::from_array(e.position) + down * offset;
            let (Some(l), Some(r)) = (to_screen(at - right * size / 2.0), to_screen(at + right * size / 2.0)) else { continue };
            // Off the view: nothing to draw (a big battle has thousands of
            // damaged units out of view).
            if r.x < clip.min.x || l.x > clip.max.x || l.y < clip.min.y - 16.0 || l.y > clip.max.y {
                continue;
            }
            let w = r.x - l.x;
            let h = (height * w / size).max(2.0);
            // Progress in moho's `ui_ProgressBarColor` (MohoEngine.dll
            // 0x10898e24: 0xFFFF9900, orange): an unfinished unit's
            // construction in place of its health, else its job (building,
            // an upgrade, an enhancement, a missile) under the health bar.
            let progress = Color32::from_rgb(0xFF, 0x99, 0x00);
            let mut y = l.y;
            let mut bar = |frac: f32, color: Color32| {
                let r = egui::Rect::from_min_size(Pos2::new(l.x, y), egui::vec2(w, h));
                painter.rect_filled(r.expand(1.0), 0.0, Color32::from_black_alpha(200));
                painter.rect_filled(egui::Rect::from_min_size(r.min, egui::vec2(w * frac.clamp(0.0, 1.0), h)), 0.0, color);
                y += h + 1.0;
            };
            if e.build < 1.0 {
                bar(e.build, progress);
            } else {
                bar(e.health, lifebar_color(e.health));
                // Its shield under it, in the blue of the interface's
                // shield bar (`unit-build-over-panel/shieldbar.dds`:
                // 0x00C2F7), then its job.
                if e.shield >= 0.0 {
                    bar(e.shield, Color32::from_rgb(0x00, 0xC2, 0xF7));
                }
                if working {
                    bar(e.work, progress);
                }
            }
        }
    }
    let Some(debug) = scene.debug() else { return };
    let ground = |x: f32, z: f32| glam::Vec3::new(x, scene.surface_at(x, z) + 0.3, z);
    for (_, pts) in &debug.paths {
        let line: Vec<Pos2> = pts.iter().filter_map(|p| to_screen(ground(p[0], p[2]))).collect();
        painter.add(egui::Shape::line(line, Stroke::new(1.5_f32, Color32::from_rgb(255, 255, 255))));
    }
    for (unit, range) in &debug.ranges {
        let Some(e) = by_id(*unit) else { continue };
        let c = e.position;
        let ring: Vec<Pos2> = (0..=48)
            .filter_map(|i| {
                let a = i as f32 / 48.0 * std::f32::consts::TAU;
                to_screen(ground(c[0] + range * a.cos(), c[2] + range * a.sin()))
            })
            .collect();
        painter.add(egui::Shape::line(ring, Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 120, 60, 160))));
    }
    let etas = match (player.display_eta, scene.scene.graph(index)) {
        (true, Some(g)) => &g.waypoints[..],
        _ => &[],
    };
    draw_orders(painter, player, cursor, painter.ctx().pixels_per_point(), to_screen, &ground, etas, here);
    if let Some(sel) = &debug.selected {
        let mut line: Vec<Pos2> = by_id(sel.id).and_then(|e| to_screen(ground(e.position[0], e.position[2]))).into_iter().collect();
        line.extend(sel.path.iter().filter_map(|p| to_screen(ground(p[0], p[2]))));
        painter.add(egui::Shape::line(line, Stroke::new(2.0_f32, Color32::from_rgb(120, 255, 120))));
    }
}

/// Colour of an order kind's line / marker (SupCom-like: green move, red
/// attack, yellow reclaim, blue patrol, ...).
fn order_color(kind: u8) -> egui::Color32 {
    use crate::engine::sim::command::CommandKind as K;
    use egui::Color32;
    match K::from_u8(kind) {
        Some(K::Attack | K::FormAttack) => Color32::from_rgb(255, 70, 60),
        Some(K::AggressiveMove | K::FormAggressiveMove) => Color32::from_rgb(255, 150, 50),
        Some(K::Patrol | K::FormPatrol) => Color32::from_rgb(90, 160, 255),
        Some(K::Reclaim) => Color32::from_rgb(240, 220, 60),
        Some(K::Repair | K::Guard) => Color32::from_rgb(80, 220, 220),
        Some(K::Capture) => Color32::from_rgb(220, 120, 255),
        _ => Color32::from_rgb(110, 240, 110),
    }
}

/// Selection rings, the selected units' queued orders, the selection box,
/// the right-drag facing arrow and order flashes.
#[allow(clippy::too_many_arguments)]
fn draw_orders(
    painter: &egui::Painter,
    player: &OverlayState,
    cursor: Option<(f64, f64)>,
    ppp: f32,
    to_screen: &dyn Fn(glam::Vec3) -> Option<egui::Pos2>,
    ground: &dyn Fn(f32, f32) -> glam::Vec3,
    etas: &[crate::render::command_graph::Waypoint],
    [box_here, arrow_here]: [bool; 2],
) {
    use egui::{Color32, Pos2, Stroke};
    let ring = |c: [f32; 3], r: f32, n: usize| -> Vec<Pos2> {
        (0..=n)
            .filter_map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                to_screen(ground(c[0] + r * a.cos(), c[2] + r * a.sin()))
            })
            .collect()
    };
    // (Selected units' brackets are the scene's.)
    // Queued orders are the scene's command graph; their waypoints' ETAs
    // (moho's "ETA: %d.%02d", minutes and seconds; FAF's `display_eta`
    // option, on by default) go at each icon's upper right.
    // Seconds rounded down (moho's, 0x103f0c75); none for a waypoint with
    // no time to it. Waypoints on one spot with one ETA (a structure's
    // build and the repair orders helping build it) draw one label.
    let mut drawn: Vec<(egui::Pos2, u32)> = Vec::new();
    for w in etas {
        if w.eta < 0.0 {
            continue;
        }
        let corner = w.position + glam::Vec3::new(0.5 * w.size, 0.0, -0.5 * w.size);
        let Some(p) = to_screen(ground(corner.x, corner.z)) else { continue };
        let secs = w.eta.floor() as u32;
        if drawn.iter().any(|&(q, s)| s == secs && q.distance(p) < 4.0) {
            continue;
        }
        drawn.push((p, secs));
        let text = format!("ETA: {}.{:02}", secs / 60, secs % 60);
        let font = egui::FontId::proportional(11.0);
        painter.text(p + egui::vec2(1.0, 1.0), egui::Align2::LEFT_BOTTOM, &text, font.clone(), Color32::from_black_alpha(200));
        painter.text(p, egui::Align2::LEFT_BOTTOM, &text, font, Color32::from_rgb(235, 235, 235));
    }
    let pt = |p: (f64, f64)| Pos2::new(p.0 as f32 / ppp, p.1 as f32 / ppp);
    if let (Some(a), Some(b), true) = (player.left_press, cursor, box_here) {
        let r = egui::Rect::from_two_pos(pt(a), pt(b));
        if r.width() * ppp > 8.0 || r.height() * ppp > 8.0 {
            painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, Color32::from_rgb(235, 235, 235)), egui::StrokeKind::Inside);
        }
    }
    if let (Some((start, g)), Some(c), true) = (player.right_press, cursor, arrow_here)
        && (c.0 - start.0).hypot(c.1 - start.1) > 16.0
        && let Some(a) = to_screen(g) {
        let b = pt(c);
        let color = Color32::from_rgb(110, 240, 110);
        painter.arrow(a, b - a, Stroke::new(2.0_f32, color));
    }
    let now = std::time::Instant::now();
    for m in &player.markers {
        let t = now.duration_since(m.when).as_secs_f32() / 0.7;
        let color = order_color(m.kind as u8).gamma_multiply(1.0 - t.min(1.0));
        let pts = ring(m.at.to_array(), 1.0 + 3.0 * t, 16);
        painter.add(egui::Shape::line(pts, Stroke::new(2.0_f32, color)));
    }
}

/// Units zoomed out past their `IconFadeInZoom` (and past their last LOD
/// cutoff) as their strategic icons (FA's
/// `/textures/ui/common/game/strategicicons/<StrategicIconName>_rest.dds`,
/// `_selected` when selected), at the texture's own pixel size, centred on
/// the unit and tinted with its army's colour.
pub fn icon_items(scene: &Sources, cache: &mut IconCache) -> (Vec<rts_core_engine::ui::controls::DrawItem>, Vec<(u32, [f32; 4])>) {
    let mut out = projectile_icon_items(scene, cache);
    let input = scene.input;
    let Some(names) = input.blueprint_icons.clone() else { return (out, Vec::new()) };
    let vfs = scene.scene.vfs().cloned();
    // Units; blips and remembered structures (both grey: no army colour,
    // identified shapes only for the latter); then the enemy jammers' fake
    // contacts (no entity).
    let fakes = scene.scene.latest.as_ref().map(|l| l.fake_blips.clone()).unwrap_or_default();
    let scale = input.ui_scale;
    let icon_scale = input.settings.icon_scale;
    let margin = 32.0 * scale * icon_scale;
    // Icon names (blips: their generic shape), borrowed or made per kind.
    let blip_names: std::collections::BTreeMap<u32, String> = scene
        .scene
        .icons
        .iter()
        .flatten()
        .filter(|i| i.4 == super::scene::IconKind::Blip)
        .filter_map(|&(_, _, _, bp, _, _)| Some((bp, unidentified_icon(names.get(bp as usize).filter(|n| !n.is_empty())?))))
        .collect();
    let info_of = |icon_info: &mut std::collections::HashMap<String, IconInfo>, name: &str| -> (u32, u32) {
        if let Some(i) = icon_info.get(name) {
            return i.sizes[0];
        }
        let paths = ["rest", "over", "selected", "selectedover"].map(|state| format!("/textures/ui/common/game/strategicicons/{name}_{state}.dds"));
        // Each state's size from its `.dds` header (the textures load into
        // the atlas when first drawn).
        let size_of = |path: &str| -> Option<(u32, u32)> {
            let b = vfs.as_ref()?.read().ok()?.read(path).ok()?;
            (b.get(0..4) == Some(b"DDS ") && b.len() >= 20).then(|| (u32::from_le_bytes(b[16..20].try_into().unwrap()), u32::from_le_bytes(b[12..16].try_into().unwrap())))
        };
        let rest = size_of(&paths[0]).unwrap_or((0, 0));
        let sizes = [rest, size_of(&paths[1]).unwrap_or(rest), size_of(&paths[2]).unwrap_or(rest), size_of(&paths[3]).unwrap_or(rest)];
        icon_info.insert(name.to_owned(), IconInfo { sizes, paths });
        rest
    };
    // Each view's icons (the lists the frame posed, by camera), clipped to
    // the view. Laid out first (at the resting state's size: what the
    // cursor picks), so the one under the cursor (the topmost: drawn last)
    // shows its `_over` state; the selected states' frames draw around that.
    let lists: Vec<(crate::render::camera::View, crate::render::camera::Camera, usize)> = scene
        .scene
        .icon_cameras
        .iter()
        .enumerate()
        .filter_map(|(i, name)| Some((input.views.iter().find(|v| v.camera == *name)?.clone(), *input.cameras.get(name)?, i)))
        .collect();
    let layers = input.blueprint_icon_layer.as_deref();
    let layer_of = |bp: u32| layers.and_then(|l| l.get(bp as usize).copied()).unwrap_or(0);
    // Laid out: (id, icon name, army, rect, alpha, the view's rect, draw layer).
    let mut laid: Vec<(u32, &str, Option<u32>, [f32; 4], f32, [f32; 4], u8)> = Vec::new();
    for (view, cam, i) in &lists {
        let shown = scene.scene.icons[*i].iter().filter_map(|&(id, pos, army, bp, kind, alpha)| {
            let name = names.get(bp as usize).filter(|n| !n.is_empty())?;
            // (A contact's layer would give its kind away: the bottom one.)
            Some(match kind {
                super::scene::IconKind::Blip => (id, pos, None, blip_names.get(&bp)?.as_str(), alpha, 0),
                // Greyed by the tint (no army), its real shape: scouted.
                super::scene::IconKind::Remembered => (id, pos, None, name.as_str(), alpha, layer_of(bp)),
                super::scene::IconKind::Full => (id, pos, Some(army), name.as_str(), alpha, layer_of(bp)),
            })
        });
        let fake = fakes.iter().map(|&pos| (u32::MAX, pos, None, FAKE_BLIP_ICON, 1.0, 0));
        let [l, t, r, b] = view.rect;
        for (id, pos, army, name, alpha, layer) in shown.chain(fake) {
            let Some([x, y]) = view.project(cam, glam::Vec3::from_array(pos)) else { continue };
            if x < l - margin || y < t - margin || x > r + margin || y > b + margin {
                continue;
            }
            let (w, h) = info_of(&mut cache.info, name);
            if w == 0 {
                continue;
            }
            // The icon's texture size is logical pixels.
            let (hw, hh) = icon_half_size((w, h), icon_scale, scale);
            laid.push((id, name, army, [x - hw, y - hh, x + hw, y + hh], alpha, view.rect, layer));
        }
    }
    let (mut layered, mut order) = (std::mem::take(&mut cache.layers), std::mem::take(&mut cache.order));
    layered.clear();
    layered.extend(laid.iter().map(|l| if input.selection.binary_search(&l.0).is_ok() { SELECTED_LAYER } else { l.6 }));
    draw_order(&layered, &mut order);
    let inside = |r: &[f32; 4], x: f32, y: f32| x >= r[0] && x < r[2] && y >= r[1] && y < r[3];
    let over = input.cursor.and_then(|[cx, cy]| {
        order.iter().rev().map(|&i| &laid[i as usize]).find(|(id, _, _, r, _, v, _)| *id != u32::MAX && inside(r, cx, cy) && inside(v, cx, cy)).map(|l| l.0)
    });
    out.reserve(laid.len());
    for &(id, name, army, rect, alpha, clip, _) in hovered_last(&order, |i| over == Some(laid[i as usize].0)).map(|i| &laid[i as usize]) {
        let selected = input.selection.binary_search(&id).is_ok();
        let state = match (selected, over == Some(id)) {
            (false, false) => 0,
            (false, true) => 1,
            (true, false) => 2,
            (true, true) => 3,
        };
        let [r, g, b] = army.map_or(UNIDENTIFIED, |a| crate::render::scene::army_color(&input.army_rgb, a));
        let Some(info) = cache.info.get(name) else { continue };
        // The state's texture at its own size, on the resting icon's centre.
        let (w, h) = info.sizes[state];
        let (cx, cy) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
        let (hw, hh) = icon_half_size((w, h), icon_scale, scale);
        let Some((rect, uv)) = clip_quad([cx - hw, cy - hh, cx + hw, cy + hh], clip) else { continue };
        out.push(rts_core_engine::ui::controls::DrawItem::Quad { rect, texture: Some(info.paths[state].clone()), uv, color: [r, g, b, alpha], tiled: false });
    }
    let rects = laid.iter().filter(|l| l.0 != u32::MAX).filter_map(|l| Some((l.0, clip_quad(l.3, l.5)?.0))).collect();
    (cache.layers, cache.order) = (layered, order);
    (out, rects)
}

/// A selected unit's icon layer: over the blueprints' layers
/// (`snapshot::icon_layer`).
const SELECTED_LAYER: u8 = 3;

/// The order to draw strategic icons in (indices), from each one's layer,
/// bottom first, as FA's icon pass keeps them (SupremeCommander.exe
/// 0x8616a2): everything else (0), aircraft (1), `StrategicIconSortPriority`
/// under 65 (2), selected units (3); within a layer in the order given. A
/// pass per layer, nothing but indices moved: 48k icons (a zoomed-out
/// 63-army battle) in ~0.07 ms, half a counting scatter's time.
fn draw_order(layers: &[u8], order: &mut Vec<u32>) {
    order.clear();
    for layer in 0..=SELECTED_LAYER {
        order.extend(layers.iter().enumerate().filter(|&(_, &l)| l == layer).map(|(i, _)| i as u32));
    }
}

/// `order` with the hovered icon(s) moved last: FA draws the icon under the
/// cursor over every layer.
fn hovered_last<'a>(order: &'a [u32], hovered: impl Fn(u32) -> bool + Copy + 'a) -> impl Iterator<Item = u32> + 'a {
    order.iter().copied().filter(move |&i| !hovered(i)).chain(order.iter().copied().filter(move |&i| hovered(i)))
}

/// The part of `rect` inside `clip`, and its texture coordinates (of the
/// whole texture over `rect`); `None` if none of it is.
fn clip_quad(rect: [f32; 4], clip: [f32; 4]) -> Option<([f32; 4], [f32; 4])> {
    let out = [rect[0].max(clip[0]), rect[1].max(clip[1]), rect[2].min(clip[2]), rect[3].min(clip[3])];
    if out[2] <= out[0] || out[3] <= out[1] {
        return None;
    }
    let (w, h) = ((rect[2] - rect[0]).max(1e-6), (rect[3] - rect[1]).max(1e-6));
    Some((out, [(out[0] - rect[0]) / w, (out[1] - rect[1]) / h, (out[2] - rect[0]) / w, (out[3] - rect[1]) / h]))
}

/// An icon texture's half width and height on screen: moho halves the
/// texture's size in whole texels, then FAF's `ui_StrategicIconScale`
/// multiplies it (FA-Binary-Patches `hooks/Strategic.hook`: every icon
/// state, blips, the pause / stun overlays and the objective underlay), then
/// the display's scale makes it physical pixels.
fn icon_half_size((w, h): (u32, u32), icon_scale: f32, display: f32) -> (f32, f32) {
    ((w / 2) as f32 * icon_scale * display, (h / 2) as f32 * icon_scale * display)
}

/// Radar and sonar contacts' colour: FAF's `UnidentifiedColor` ("FF808080",
/// `/lua/GameColors.lua`; moho reads it through `GetUnidentifiedColor`,
/// MohoEngine.dll 0x100f2980).
pub const UNIDENTIFIED: [f32; 3] = [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0];

/// Jammer fakes have no unit: drawn as an unidentified land contact.
const FAKE_BLIP_ICON: &str = "icon_land_generic";

/// A blip's icon: the shape of its unit's strategic icon without tech level
/// or role (`icon_land2_antiair` -> `icon_land_generic`), from the game's
/// `*_generic` icon set (inferred: which icon moho draws for a radar
/// contact isn't confirmed, see docs/research/verifying-against-the-binary.md).
fn unidentified_icon(name: &str) -> String {
    let shape = name.strip_prefix("icon_").and_then(|r| r.split('_').next()).map(|s| s.trim_end_matches(|c: char| c.is_ascii_digit())).filter(|s| !s.is_empty());
    match shape {
        Some(s) if GENERIC_SHAPES.contains(&s) => format!("icon_{s}_generic"),
        _ => FAKE_BLIP_ICON.to_string(),
    }
}

/// The shapes the game has a tech-less `icon_<shape>_generic` for.
const GENERIC_SHAPES: [&str; 11] = ["bomber", "bot", "commander", "experimental", "factory", "fighter", "gunship", "land", "ship", "structure", "sub"];

/// Zoom from which projectiles are drawn as strategic icons (moho's
/// `UI_StrategicProjectileLOD`, 128 by default: MohoEngine.dll .data
/// 0x10898d8c, compared with `CameraGetZoom` at 0x10427330; its description
/// says "LODMetric" but the code reads the zoom).
const STRATEGIC_PROJECTILE_ZOOM: f32 = 128.0;
/// Their colour without an icon texture: moho's `UI_forceWeaponsToYellow`
/// (on by default) draws them `FromSolidColor(0xFFFFFF00)` (0x10427732).
const PROJECTILE_ICON_YELLOW: [f32; 4] = [1.0, 1.0, 0.0, 1.0];

/// Projectiles on the strategic view, as moho draws them zoomed out
/// (MohoEngine.dll 0x104272f0): a yellow square `StrategicIconSize` pixels
/// across centred on the projectile, or its `StrategicIconName` texture at
/// the texture's own size (moho also pulses a glow over those; not drawn
/// yet). Under the units' strategic icons.
fn projectile_icon_items(scene: &Sources, cache: &mut IconCache) -> Vec<rts_core_engine::ui::controls::DrawItem> {
    let mut out = Vec::new();
    let input = scene.input;
    let vfs = scene.scene.vfs().cloned();
    let px = input.ui_scale;
    for view in &input.views {
        let Some(cam) = input.cameras.get(&view.camera).filter(|c| c.zoom >= STRATEGIC_PROJECTILE_ZOOM) else { continue };
        let [l, t, r, b] = view.rect;
        for e in scene.scene.frame_entities.iter().filter(|e| e.kind == crate::engine::snapshot::ViewKind::Projectile) {
            let Some(Some(icon)) = input.overlays.projectile_icons.get(e.blueprint as usize) else { continue };
            let Some([x, y]) = view.project(cam, glam::Vec3::from_array(e.position)) else { continue };
            let (w, h, texture, color) = match &icon.texture {
                Some(path) => {
                    let (w, h) = *cache.sizes.entry(path.clone()).or_insert_with(|| {
                        let header = vfs.as_ref().and_then(|v| v.read().ok()?.read(path).ok());
                        rts_core_engine::engine::assets::dds::image_size(header.as_deref().unwrap_or(&[])).unwrap_or((0, 0))
                    });
                    (w as f32, h as f32, Some(path.clone()), [1.0; 4])
                }
                None => (icon.size, icon.size, None, PROJECTILE_ICON_YELLOW),
            };
            // Sizes are logical pixels, as the interface's.
            let (w, h) = (w * px, h * px);
            if w <= 0.0 || x < l - w || y < t - h || x > r + w || y > b + h {
                continue;
            }
            let (x, y) = (x.round(), y.round());
            let Some((rect, uv)) = clip_quad([x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0], view.rect) else { continue };
            out.push(rts_core_engine::ui::controls::DrawItem::Quad { rect, texture, uv, color, tiled: false });
        }
    }
    out
}


#[cfg(test)]
mod icon_tests {
    /// Moho halves the texture in whole texels before the patch's scale.
    #[test]
    fn icon_half_size_scales_whole_texel_halves() {
        assert_eq!(super::icon_half_size((16, 16), 1.0, 1.0), (8.0, 8.0));
        assert_eq!(super::icon_half_size((16, 12), 2.0, 1.0), (16.0, 12.0));
        assert_eq!(super::icon_half_size((17, 17), 1.5, 1.0), (12.0, 12.0));
        assert_eq!(super::icon_half_size((16, 16), 1.5, 2.0), (24.0, 24.0));
    }

    /// FA's four icon lists, bottom to top: everything else, aircraft,
    /// `StrategicIconSortPriority` under 65, selected; the hovered icon
    /// over them all. Entity order within a list.
    #[test]
    fn icons_draw_in_layers_hovered_on_top() {
        use crate::engine::snapshot::icon_layer;
        // (id, layer) in entity order: an ACU, a tank, an air scout, a
        // second tank (selected), a mass extractor (hovered), a T1 engineer.
        let acu = icon_layer(Some(15.0), false);
        let tank = icon_layer(Some(135.0), false);
        let scout = icon_layer(Some(207.0), true);
        let extractor = icon_layer(Some(175.0), false);
        let engineer = icon_layer(Some(64.0), false);
        assert_eq!((acu, tank, scout, extractor, engineer), (2, 0, 1, 0, 2));
        assert_eq!(icon_layer(Some(65.0), false), 0, "65 and over: no priority layer");
        assert_eq!(icon_layer(None, false), 2, "unset is 0: under 65");
        // Units 1-6 at indices 0-5; the second tank (index 3) selected.
        let layers = [acu, tank, scout, super::SELECTED_LAYER, extractor, engineer];
        let mut order = Vec::new();
        super::draw_order(&layers, &mut order);
        assert_eq!(order, vec![1, 4, 2, 0, 5, 3]);
        // The extractor (index 4) hovered: last of all.
        let drawn: Vec<u32> = super::hovered_last(&order, |i| i == 4).map(|i| i + 1).collect();
        assert_eq!(drawn, vec![2, 3, 1, 6, 4, 5]);
    }
}

#[cfg(test)]
mod bar_tests {
    use crate::engine::snapshot::{EntityView, Sighting, ViewKind};

    fn unit(health: f32, build: f32) -> EntityView {
        EntityView { id: 7, kind: ViewKind::Unit, army: 1, position: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], center: [0.0; 3], half: [1.0; 3], health, max_health: 100.0, build, work: 0.0, shield: -1.0, blueprint: 0, mesh: 0, scale: [1.0; 3], pose_start: 0, pose_len: 0, sighting: Sighting::Seen, in_builder: false, attached: false, scroll: [0.0; 4], order_flags: 0 }
    }

    use crate::render::scene::shows_bar;

    /// A killed unit (an aircraft falling: health 0) has no bar;
    /// any other unit can, at any health, finished or not.
    #[test]
    fn killed_units_show_no_health_bar() {
        assert!(!shows_bar(&unit(0.0, 1.0)), "killed, falling");
        assert!(shows_bar(&unit(0.4, 1.0)), "damaged");
        assert!(shows_bar(&unit(1.0, 1.0)), "unhurt: moho draws every unit's bar");
        assert!(shows_bar(&unit(0.0, 0.3)), "being built (health grows with it)");
        assert!(!shows_bar(&EntityView { half: [0.0; 3], ..unit(0.0, 0.3) }), "an upgrade inside its structure");
        assert!(!shows_bar(&EntityView { sighting: Sighting::Blip, ..unit(1.0, 1.0) }), "a radar blip");
    }

    fn rules(viewer: Option<u32>) -> super::BarRules {
        super::BarRules { render: true, force_enemy: false, show_attached: true, viewer, hovered: None }
    }

    /// Moho's rule: own and allied bars always, everyone's when observing;
    /// an enemy's only in sight and forced or hovered; attached units by the
    /// option; none with bars off.
    #[test]
    fn lifebars_follow_moho_visibility() {
        use super::bar_visible;
        let enemy = unit(1.0, 1.0);
        assert!(bar_visible(&enemy, true, &rules(Some(1))), "own / allied");
        assert!(bar_visible(&enemy, false, &rules(None)), "observing");
        assert!(!bar_visible(&enemy, false, &rules(Some(2))), "enemy");
        assert!(bar_visible(&enemy, false, &super::BarRules { force_enemy: true, ..rules(Some(2)) }), "enemy, forced");
        assert!(bar_visible(&enemy, false, &super::BarRules { hovered: Some(7), ..rules(Some(2)) }), "enemy under the cursor");
        let remembered = EntityView { sighting: Sighting::Remembered, ..enemy };
        assert!(!bar_visible(&remembered, false, &super::BarRules { force_enemy: true, ..rules(Some(2)) }), "enemy out of sight, even forced");
        let carried = EntityView { attached: true, ..enemy };
        assert!(bar_visible(&carried, true, &rules(Some(1))), "attached, option on");
        assert!(!bar_visible(&carried, true, &super::BarRules { show_attached: false, ..rules(Some(1)) }), "attached, option off");
        assert!(!bar_visible(&enemy, true, &super::BarRules { render: false, ..rules(Some(1)) }), "ui_RenderUnitBars off");
    }

    #[test]
    fn lifebar_colours_follow_moho_cutoffs() {
        use egui::Color32;
        assert_eq!(super::lifebar_color(1.0), Color32::from_rgb(0, 255, 0));
        assert_eq!(super::lifebar_color(0.75), Color32::from_rgb(255, 255, 0), "at the good cutoff: medium");
        assert_eq!(super::lifebar_color(0.3), Color32::from_rgb(255, 255, 0));
        assert_eq!(super::lifebar_color(0.25), Color32::from_rgb(255, 0, 0), "at the bad cutoff: bad");
    }
}
