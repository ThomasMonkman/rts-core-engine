use crate::engine::vfs::tree::FsDir;
use crate::render::state::RenderState;
use rts_core_engine::engine::sim::command::CommandTarget;

fn path_hash(path: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    h.finish()
}

fn render_dir(
    builder: &mut egui_ltreeview::TreeViewBuilder<u64>,
    name: &str,
    dir: &FsDir,
    path: &str,
) {
    builder.dir(path_hash(path), name);
    for (dname, child) in &dir.dirs {
        let child_path = format!("{path}/{dname}");
        render_dir(builder, dname, child, &child_path);
    }
    for fname in &dir.files {
        let fpath = format!("{path}/{fname}");
        builder.leaf(path_hash(&fpath), fname.as_str());
    }
    builder.close_dir();
}


fn debug_windows(ctx: &egui::Context, scene: &crate::render::world::World, request: &crate::engine::snapshot::SharedDebugRequest, smaa: &mut bool) {
    let Ok(mut r) = request.lock() else { return };
    egui::Window::new("Overlays").default_open(false).anchor(egui::Align2::RIGHT_TOP, [-8.0, 8.0]).show(ctx, |ui| {
        ui.checkbox(smaa, "SMAA");
        ui.checkbox(&mut r.paths, "unit paths");
        ui.checkbox(&mut r.weapon_ranges, "weapon ranges");
        let layers = ["off", "land", "hover", "naval", "amphibious"];
        let mut nav = r.nav_layer.map_or(0, |l| l as usize + 1);
        egui::ComboBox::from_label("nav grid").selected_text(layers[nav]).show_ui(ui, |ui| {
            for (i, name) in layers.iter().enumerate() {
                ui.selectable_value(&mut nav, i, *name);
            }
        });
        r.nav_layer = (nav > 0).then(|| nav as u8 - 1);
        let mut threat = r.threat_army.unwrap_or(0);
        egui::ComboBox::from_label("threat (as army)").selected_text(if threat == 0 { "off".to_string() } else { threat.to_string() }).show_ui(ui, |ui| {
            for a in 0..=8u32 {
                ui.selectable_value(&mut threat, a, if a == 0 { "off".to_string() } else { a.to_string() });
            }
        });
        r.threat_army = (threat > 0).then_some(threat);
        let mut fog = r.fog_army.unwrap_or(0);
        egui::ComboBox::from_label("fog of war (as army)").selected_text(if fog == 0 { "off".to_string() } else { fog.to_string() }).show_ui(ui, |ui| {
            for a in 0..=8u32 {
                ui.selectable_value(&mut fog, a, if a == 0 { "off".to_string() } else { a.to_string() });
            }
        });
        r.fog_army = (fog > 0).then_some(fog);
        if r.nav_layer.is_some() && r.threat_army.is_some() {
            ui.weak("(nav grid shown; threat needs nav off)");
        }
    });
    let Some(id) = r.selected else { return };
    drop(r);
    let sel = scene.debug().and_then(|d| d.selected.clone());
    egui::Window::new("Selected").default_open(false).anchor(egui::Align2::RIGHT_BOTTOM, [-8.0, -40.0]).show(ctx, |ui| {
        let Some(sel) = sel.filter(|s| s.id == id) else {
            ui.label(format!("entity {id} (waiting for the sim / gone)"));
            return;
        };
        ui.heading(format!("{} #{}", sel.blueprint, sel.id));
        ui.label(format!("army {}  layer {}  speed {:.1}", sel.army, sel.layer, sel.speed));
        ui.label(format!("health {:.0} / {:.0}  built {:.0}%", sel.health.0, sel.health.1, sel.build * 100.0));
        if let Some(f) = sel.focus {
            ui.label(format!("working on entity {f}"));
        }
        ui.separator();
        ui.label(format!("orders ({})", sel.orders.len()));
        for o in &sel.orders {
            ui.monospace(o);
        }
        if !sel.weapons.is_empty() {
            ui.separator();
            ui.label("weapons");
            for w in &sel.weapons {
                ui.monospace(w);
            }
        }
    });
}

/// Pans / zooms head `head`'s cameras from this frame's input: the keys, a
/// followed unit and the interface's timed moves move the main view's
/// camera (FAF's `GetsGlobalCameraCommands` view); the wheel and a drag,
/// the camera of the view under the cursor. Each head steps the cameras it
/// shows, at its own rate ([`camera_head`]); the wheel and a drag wait for
/// the frame of the head the cursor is on.
fn update_camera(s: &mut RenderState, head: usize, dt: f32) {
    use winit::keyboard::KeyCode as K;
    let Some(map) = s.world.map_size else { return };
    let two = s.second.is_some();
    let owner = |s: &RenderState, name: &str| if two { camera_head(&s.world, name) } else { 0 };
    let pointer_here = !two || s.cursor_head == head;
    let cursor = s.input.cursor.map(|(x, y)| (x as f32, y as f32)).filter(|_| pointer_here);
    let main_here = owner(s, crate::render::camera::MAIN) == head;
    // The ground under the cursor before a zoom (kept under it after).
    s.zoom_anchor = match (pointer_here && s.input.wheel != 0.0, cursor) {
        (true, Some((x, y))) => s.world.ground_at(x, y),
        _ => None,
    };
    let keys = &s.input.keys;
    let axis = |neg: &[K], pos: &[K]| -> f32 {
        (pos.iter().any(|k| keys.contains(k)) as i32 - neg.iter().any(|k| keys.contains(k)) as i32) as f32
    };
    // Fractions of the zoom (the view's width) a second: the same feel as
    // before zoom meant ground width (0.9 eye distances on a 16:9 view).
    let speed = 0.45 * dt;
    let (px, pz) = match main_here {
        true => (axis(&[K::KeyA, K::ArrowLeft], &[K::KeyD, K::ArrowRight]), axis(&[K::KeyW, K::ArrowUp], &[K::KeyS, K::ArrowDown])),
        false => (0.0, 0.0),
    };
    let turn = if main_here { axis(&[K::Delete], &[K::Insert]) } else { 0.0 };
    let (wheel, (dx, dy)) = match pointer_here {
        true => (std::mem::take(&mut s.input.wheel), std::mem::take(&mut s.input.drag)),
        false => (0.0, (0.0, 0.0)),
    };
    // The view under the cursor (without one, as for a wheel the interface
    // passed on, the main view).
    let pointer = match cursor {
        Some((x, y)) => s.world.view_at(x, y).map(|(v, _)| v.clone()),
        None => s.world.head_views(head).first().cloned(),
    };
    // A camera moved by hand stops following and stops its timed move: the
    // keys move the main one, a drag the one under the cursor. The wheel
    // only zooms: it ends a timed move (which would set the zoom back) but
    // a followed camera keeps following, zooming about what it follows.
    if px != 0.0 || pz != 0.0 {
        forget(s, crate::render::camera::MAIN);
    }
    if let (true, Some(view)) = (dx != 0.0 || dy != 0.0, &pointer) {
        forget(s, &view.camera);
    }
    let pointer_tracked = pointer.as_ref().is_some_and(|v| s.tracks.iter().any(|(n, _)| n.eq_ignore_ascii_case(&v.camera)));
    if let (true, Some(view)) = (wheel != 0.0, &pointer) {
        s.camera_moves.retain(|(n, _)| !n.eq_ignore_ascii_case(&view.camera));
    }
    if main_here {
        if let Some(cam) = s.world.camera_mut() {
            cam.pan(px * speed, pz * speed);
        }
        // Insert / Delete turn the main camera (moho's world view,
        // SupremeCommander.exe 0x876e1c: a mouse spin of
        // `ui_KeyboardRotateSpeed` pixels a frame, twice that with Ctrl;
        // not with Alt, FAF's Alt-Delete is a debug key). It stays turned.
        if turn != 0.0 && !s.input.mods.alt {
            let step = crate::render::camera::KEYBOARD_ROTATE_SPEED * if s.input.mods.ctrl { crate::render::camera::KEYBOARD_ROTATE_ACCELERATE } else { 1.0 } * dt * 60.0;
            let width = s.world.head_views(head).iter().find(|v| v.camera.eq_ignore_ascii_case(crate::render::camera::MAIN)).map_or(1.0, |v| v.size()[0]);
            if let Some(cam) = s.world.camera_mut() {
                cam.spin(turn * step, 0.0, width);
            }
        }
    }
    if pointer_here {
        spin_camera(s, pointer.as_ref(), cursor);
    }
    // Followed entities: the camera looks at their middle (a timed move
    // under way heads there instead); a camera whose entities are all gone
    // stops following. One following a launcher follows what it fires with
    // `CameraFollowsProjectile`, holds where that ended, then goes back.
    let mut tracks = std::mem::take(&mut s.tracks);
    let mut follows = std::mem::take(&mut s.projectile_follows);
    start_follows(&mut tracks, &mut follows, &std::mem::take(&mut s.world.camera_follows));
    tracks.retain_mut(|(name, ids)| {
        if owner(s, name) != head {
            return true;
        }
        if let Some(i) = follows.iter().position(|f| f.camera.eq_ignore_ascii_case(name)) {
            // (Not pruned: the projectile shows in the frame a tick after
            // the launch reaches us.)
            let alive = ids.iter().any(|i| s.world.entities().binary_search_by_key(i, |e| e.id).is_ok());
            match follows[i].step(ids, alive, dt) {
                FollowStep::Following => {}
                FollowStep::Waiting | FollowStep::Holding => return true,
                FollowStep::Back => {
                    follows.remove(i);
                }
            }
        }
        let Some([x, z]) = middle(s.world.entities(), ids) else { return false };
        match s.camera_moves.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            Some((_, m)) => m.retarget(x, z),
            None => {
                if let Some(cam) = s.world.cameras.get_mut(name) {
                    (cam.target.x, cam.target.z) = (x, z);
                }
            }
        }
        true
    });
    follows.retain(|f| tracks.iter().any(|(n, _)| n.eq_ignore_ascii_case(&f.camera)));
    s.tracks = tracks;
    s.projectile_follows = follows;
    let mut moves = std::mem::take(&mut s.camera_moves);
    moves.retain_mut(|(name, m)| owner(s, name) != head || s.world.cameras.get_mut(name).is_some_and(|cam| !m.step(cam, dt)));
    s.camera_moves = moves;
    if let Some(view) = pointer
        && let Some(cam) = s.world.cameras.get_mut(&view.camera) {
        if wheel != 0.0 {
            cam.zoom(wheel);
        }
        if dx != 0.0 || dy != 0.0 {
            // The ground follows the cursor: zoom spans the longer side.
            let [w, h] = view.size();
            let long = w.max(h).max(1.0);
            cam.pan(-(dx as f32) / long, -(dy as f32) / long);
        }
    }
    // Every camera stays over the map, its look-at height easing toward
    // the ground under it.
    let mine: Vec<bool> = s.world.cameras.iter().map(|(name, _)| owner(s, name) == head).collect();
    let heights: Vec<f32> = s.world.cameras.iter().map(|(_, c)| s.world.height_at(c.target.x, c.target.z)).collect();
    for (((_, cam), h), mine) in s.world.cameras.iter_mut().zip(heights).zip(mine) {
        if mine {
            cam.step_revert(dt);
            cam.clamp_to(map.0, map.1);
            cam.target.y += (h - cam.target.y) * (dt * 5.0).min(1.0);
        }
    }
    // Zoom toward the cursor: the ground under it stays under it (not for
    // a followed camera, which stays on what it follows).
    if let (Some((x, y)), Some(before), true, false) = (cursor, s.zoom_anchor.take(), wheel != 0.0, pointer_tracked)
        && let Some(after) = s.world.ground_at(x, y)
        && let Some(cam) = s.world.camera_at_mut(x, y) {
        cam.target.x += before.x - after.x;
        cam.target.z += before.z - after.z;
        cam.clamp_to(map.0, map.1);
    }
}

/// Space held over a world view: the mouse spins its camera (moho's world
/// view, SupremeCommander.exe 0x876543 / 0x876dc4): the cursor hides and
/// stays put, and the camera turns with the mouse's motion while it is
/// zoomed in closer than `ren_BgLowerBound` (or with `cam_Free`). Letting
/// go of Space (or zooming out past that) eases it back to the usual view
/// unless `cam_Free` is on, and puts the cursor back.
fn spin_camera(s: &mut RenderState, pointer: Option<&crate::render::camera::View>, cursor: Option<(f32, f32)>) {
    let motion = std::mem::take(&mut s.input.motion);
    let held = s.input.keys.contains(&winit::keyboard::KeyCode::Space);
    let over_interface = |s: &RenderState| match (&s.ui, cursor) {
        (Some(vm), Some((x, y))) => {
            let (ux, uy) = s.ui_point(x as f64, y as f64);
            vm.hit_interface(ux, uy).unwrap_or(false)
        }
        _ => false,
    };
    let name = match &s.spinning {
        Some((name, _)) => Some(name.clone()),
        None if held && !over_interface(s) => pointer.map(|v| v.camera.clone()),
        None => None,
    };
    let free = s.cam_free;
    let allowed = name.as_deref().and_then(|n| s.world.cameras.get(n)).is_some_and(|c| free || c.zoom < crate::render::camera::SPIN_ZOOM_LIMIT);
    match (held && allowed, name) {
        (true, Some(name)) => {
            if s.spinning.is_none() {
                // The first motion only takes hold (as moho's first event).
                if motion == (0.0, 0.0) {
                    return;
                }
                let at = s.input.cursor.unwrap_or_default();
                tracing::info!(camera = %name, "camera spin: start");
                s.spinning = Some((name, at));
                s.window.set_cursor_visible(false);
                use winit::window::CursorGrabMode;
                if s.window.set_cursor_grab(CursorGrabMode::Locked).is_err() {
                    let _ = s.window.set_cursor_grab(CursorGrabMode::Confined);
                }
                return;
            }
            let width = s.world.views().iter().find(|v| v.camera.eq_ignore_ascii_case(&name)).map_or(1.0, |v| v.size()[0]);
            if let Some(cam) = s.world.cameras.get_mut(&name) {
                cam.spin(motion.0 as f32, motion.1 as f32, width);
            }
        }
        _ => {
            let Some((name, at)) = s.spinning.take() else { return };
            tracing::info!(camera = %name, free, "camera spin: end");
            if !free
                && let Some(cam) = s.world.cameras.get_mut(&name) {
                cam.revert_rotation();
            }
            s.apply_cursor_clip();
            let _ = s.window.set_cursor_position(winit::dpi::PhysicalPosition::new(at.0, at.1));
            s.window.set_cursor_visible(s.os_cursor != rts_core_engine::ui::cursor::Wanted::Hidden);
        }
    }
}

/// Frames a camera request waits for its camera (a world view opened the
/// frame it asked: its camera is made when the view first shows).
const CAMERA_WAIT_FRAMES: u32 = 30;

/// Camera `name` follows entities `ids` (replacing what it followed).
pub fn track(s: &mut RenderState, name: &str, ids: Vec<u32>) {
    s.tracks.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
    s.projectile_follows.retain(|f| !f.camera.eq_ignore_ascii_case(name));
    s.tracks.push((name.to_owned(), ids));
}

/// Camera `name` was moved by hand: it stops following and its timed move
/// ends.
fn forget(s: &mut RenderState, name: &str) {
    s.tracks.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
    s.projectile_follows.retain(|f| !f.camera.eq_ignore_ascii_case(name));
    s.camera_moves.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
}

/// A followed camera switched to a projectile its entity launched with
/// `Display.CameraFollowsProjectile` (moho's `CameraImpl::CameraFollow`,
/// MohoEngine.dll 0x10370500, and `UpdateTargets` 0x103722e0): it follows
/// the projectile alone; once that's gone it holds still for the
/// projectile's `CameraFollowTimeout` seconds, then follows what it followed
/// before.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectileFollow {
    pub camera: String,
    /// What it followed before.
    pub back: Vec<u32>,
    pub timeout: f32,
    /// Seconds left holding, once the projectile is gone.
    pub holding: Option<f32>,
    /// The projectile has shown in a frame (it's there a tick after its
    /// launch reaches the renderer).
    pub seen: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FollowStep {
    /// The projectile isn't in the frame yet: the camera stays put.
    Waiting,
    /// Still on the projectile.
    Following,
    /// Holding where it ended.
    Holding,
    /// Back on what it followed before (`ids`).
    Back,
}

impl ProjectileFollow {
    /// A frame of `dt` seconds; `alive`: the projectile (`ids`) is still
    /// there.
    pub fn step(&mut self, ids: &mut Vec<u32>, alive: bool, dt: f32) -> FollowStep {
        if alive {
            (self.seen, self.holding) = (true, None);
            return FollowStep::Following;
        }
        let left = self.holding.get_or_insert(self.timeout);
        *left -= dt;
        if *left > 0.0 {
            // Not seen yet: waiting for it, as long as the hold would be.
            return if self.seen { FollowStep::Holding } else { FollowStep::Waiting };
        }
        *ids = std::mem::take(&mut self.back);
        FollowStep::Back
    }
}

/// Cameras following a projectile's launcher switch to it. Moho acts only
/// when the camera's current target is the launcher; ours follow a set, so
/// any set holding it (one already on a projectile doesn't hold it).
fn start_follows(tracks: &mut [(String, Vec<u32>)], follows: &mut Vec<ProjectileFollow>, launches: &[rts_core_engine::engine::sim::presentation::CameraFollow]) {
    for l in launches {
        for (name, ids) in tracks.iter_mut().filter(|(_, ids)| ids.contains(&l.launcher)) {
            follows.retain(|f| !f.camera.eq_ignore_ascii_case(name));
            follows.push(ProjectileFollow { camera: name.clone(), back: std::mem::replace(ids, vec![l.projectile]), timeout: l.timeout, holding: None, seen: false });
        }
    }
}

/// The middle (x, z) of the entities `ids` still in this frame's
/// `entities` (sorted by id), dropping the ones gone; `None` once none
/// are left.
fn middle(entities: &[rts_core_engine::engine::snapshot::EntityView], ids: &mut Vec<u32>) -> Option<[f32; 2]> {
    let mut sum = [0.0f32; 2];
    ids.retain(|id| match entities.binary_search_by_key(id, |e| e.id) {
        Ok(i) => {
            sum[0] += entities[i].position[0];
            sum[1] += entities[i].position[2];
            true
        }
        Err(_) => false,
    });
    let n = ids.len() as f32;
    (n > 0.0).then(|| [sum[0] / n, sum[1] / n])
}

/// The head whose frames step camera `name`: the second head for a camera
/// only it shows (FAF's `CameraHead2`), else the main window (the main
/// camera always: the keys and a followed unit move it).
fn camera_head(world: &crate::render::world::World, name: &str) -> usize {
    let on = |head: usize| world.head_views(head).iter().any(|v| v.camera.eq_ignore_ascii_case(name));
    match !name.eq_ignore_ascii_case(crate::render::camera::MAIN) && on(1) && !on(0) {
        true => 1,
        false => 0,
    }
}

/// FAF's name for a key (`/lua/keymap/defaultKeyMap.lua`).
fn faf_key_name(k: winit::keyboard::KeyCode) -> Option<&'static str> {
    use winit::keyboard::KeyCode as K;
    Some(match k {
        K::KeyA => "A", K::KeyB => "B", K::KeyC => "C", K::KeyD => "D", K::KeyE => "E", K::KeyF => "F", K::KeyG => "G",
        K::KeyH => "H", K::KeyI => "I", K::KeyJ => "J", K::KeyK => "K", K::KeyL => "L", K::KeyM => "M", K::KeyN => "N",
        K::KeyO => "O", K::KeyP => "P", K::KeyQ => "Q", K::KeyR => "R", K::KeyS => "S", K::KeyT => "T", K::KeyU => "U",
        K::KeyV => "V", K::KeyW => "W", K::KeyX => "X", K::KeyY => "Y", K::KeyZ => "Z",
        K::Digit0 => "0", K::Digit1 => "1", K::Digit2 => "2", K::Digit3 => "3", K::Digit4 => "4",
        K::Digit5 => "5", K::Digit6 => "6", K::Digit7 => "7", K::Digit8 => "8", K::Digit9 => "9",
        K::F1 => "F1", K::F2 => "F2", K::F3 => "F3", K::F4 => "F4", K::F5 => "F5", K::F6 => "F6",
        K::F7 => "F7", K::F8 => "F8", K::F9 => "F9", K::F10 => "F10", K::F11 => "F11", K::F12 => "F12",
        K::Escape => "Esc", K::Tab => "Tab", K::Space => "Space", K::Enter => "Enter", K::Backspace => "Backspace",
        K::Delete => "Delete", K::Insert => "Insert", K::Home => "Home", K::End => "End", K::PageUp => "PageUp", K::PageDown => "PageDown",
        K::ArrowUp => "UpArrow", K::ArrowDown => "DownArrow", K::ArrowLeft => "LeftArrow", K::ArrowRight => "RightArrow",
        K::Period => "Period", K::Comma => "Comma", K::Slash => "Slash", K::BracketLeft => "LeftBracket", K::BracketRight => "RightBracket",
        K::Minus => "Minus", K::Equal => "Equals", K::Pause => "Pause",
        K::NumpadAdd => "NumPlus", K::NumpadSubtract => "NumMinus", K::NumpadMultiply => "NumStar", K::NumpadDivide => "NumSlash",
        _ => return None,
    })
}

/// Wheel rotation per notch in interface events (Windows' `WHEEL_DELTA`,
/// which moho passes on as `WheelRotation`).
const WHEEL_DELTA: f32 = 120.0;

/// The interface's view of a player-control event.
fn ui_mouse(ev: crate::render::player::Event) -> Option<(rts_core_engine::ui::controls::MouseEvent, f64, f64)> {
    use crate::render::player::Event as E;
    use rts_core_engine::ui::controls::MouseEvent as M;
    Some(match ev {
        E::LeftDown(x, y) => (M::Press(1), x, y),
        E::LeftUp(x, y) => (M::Release(1), x, y),
        E::RightDown(x, y) => (M::Press(2), x, y),
        E::RightUp(x, y) => (M::Release(2), x, y),
        E::Move(x, y) => (M::Move, x, y),
        E::Wheel(x, y, notches) => (M::Wheel(notches * WHEEL_DELTA), x, y),
        E::Key(_) | E::KeyRepeat(_) | E::Char(_) => return None,
    })
}

/// An interface draw item laid out in logical pixels, in physical pixels
/// (`scale` = the window's scale factor). Text grows in size too, so its
/// glyphs are rasterised sharp at the physical size. Any factor works,
/// fractional ones included (Windows, X11 and Wayland report 1.25, 1.5, ...).
fn to_physical(item: rts_core_engine::ui::controls::DrawItem, scale: f32) -> rts_core_engine::ui::controls::DrawItem {
    use rts_core_engine::ui::controls::DrawItem as D;
    // Edges on whole pixels: at fractional factors (Windows' 125% / 150%)
    // textures stay crisp, and edges two controls share still meet.
    let r = |r: [f32; 4]| r.map(|v| (v * scale).round());
    match item {
        D::Quad { rect, texture, uv, color, tiled } => D::Quad { rect: r(rect), texture, uv, color, tiled },
        D::Minimap { rect } => D::Minimap { rect: r(rect) },
        D::World { rect, camera } => D::World { rect: r(rect), camera },
        D::Text { rect, text, family, size, color, shadow, vertical } => D::Text { rect: r(rect), text, family, size: size * scale, color, shadow, vertical },
        D::Movie { rect, movie, frame, color } => D::Movie { rect: r(rect), movie, frame, color },
    }
}

/// Boots the user VM when the VFS is known, runs its frame and prepares
/// the interface pass. Failures are logged and leave the UI off.
/// Ghost colours: a spot the sim allows, and one it doesn't. FAF's
/// `UnitPlace` lights the instance colour the engine gives it: moho's
/// placement ghost gets `SetColor(0xD800D800)` (ARGB: green 216;
/// MohoEngine.dll 0x10418889). The engine has no red for a refused spot;
/// ours is a guess (docs/research/verifying-against-the-binary.md).
const GHOST_OK: [f32; 3] = [0.0, 216.0 / 255.0, 0.0];
const GHOST_BLOCKED: [f32; 3] = [1.0, 0.15, 0.15];

/// How near the cursor a deposit pulls an extractor onto it (world units).
const DEPOSIT_SNAP: f32 = 3.0;

/// A deposit-bound structure (mass extractor, hydrocarbon plant) goes
/// exactly on the deposit nearest the cursor, as the game's ghost snaps
/// onto it; `None` when there's no such deposit near.
fn deposit_spot(s: &RenderState, vm: &rts_core_engine::ui::UserVm, bp: &str, ground: glam::Vec3) -> Option<glam::Vec3> {
    let kind = vm.deposit_restriction(bp).ok().flatten()?;
    let deposits = s.world.deposits.as_ref()?;
    let (x, z) = deposits
        .iter()
        .filter(|d| d.0 == kind)
        .map(|d| (d.1 as f32, d.2 as f32, d.3 as f32))
        .filter(|&(x, z, _)| (x - ground.x).hypot(z - ground.z) <= DEPOSIT_SNAP)
        .min_by(|a, b| (a.0 - ground.x).hypot(a.1 - ground.z).total_cmp(&(b.0 - ground.x).hypot(b.1 - ground.z)))
        // The centre of the deposit's cells (from round(position - size /
        // 2), halves to even: `Deposit::cells`), which is where the sim
        // lets it stand; a deposit at a whole number takes the even cell.
        .map(|(x, z, size)| ((x - size / 2.0).round_ties_even() + size / 2.0, (z - size / 2.0).round_ties_even() + size / 2.0))?;
    Some(glam::Vec3::new(x, s.world.height_at(x, z), z))
}

/// A waypoint being dragged follows the cursor: over the ground (a build's
/// site snapped as build mode places it), or, for an order on a unit, onto
/// a unit of that unit's army (`commandgraph.lua`: "its node will snap to
/// units/blips in the same army"; off one, it stays put).
fn update_command_drag(s: &mut RenderState) {
    let Some(mut drag) = s.command_drag.take() else {
        s.frame_input.command_drag = command_moved(s).map(|(command, node)| (command, node, false));
        return;
    };
    if let Some((x, y)) = s.input.cursor {
        drag.cursor_at((x, y));
        let (px, py) = (x as f32, y as f32);
        drag.to = match (drag.marker.target, &drag.marker.blueprint) {
            (Some(target), _) => {
                let view = |id: u32| s.world.entities().binary_search_by_key(&id, |e| e.id).ok().map(|i| &s.world.entities()[i]);
                let army = view(target).map(|e| e.army);
                s.world.pick(px, py).and_then(view).filter(|e| e.kind.is_unit() && Some(e.army) == army).map(|e| (CommandTarget::Entity(e.id), glam::Vec3::from_array(e.position)))
            }
            (None, Some(bp)) => s.world.ground_at(px, py).map(|g| {
                let spot = match &s.ui {
                    Some(vm) => deposit_spot(s, vm, bp, g).unwrap_or_else(|| crate::render::player::snap_to_footprint(&s.world, g, vm.footprint(bp).unwrap_or((1.0, 1.0)))),
                    None => g,
                };
                (ground_target(spot), spot)
            }),
            (None, None) => s.world.ground_at(px, py).map(|g| (ground_target(g), g)),
        };
    }
    s.frame_input.command_drag = drag.to.as_ref().filter(|_| drag.moved).map(|(_, p)| (drag.marker.command, p.to_array(), true));
    s.command_drag = Some(drag);
}

/// Snapshot ticks a let-go order keeps drawing where it went while the sim
/// hasn't shown the move (it refused it, or it's still on its way).
const MOVE_SHOW_TICKS: u64 = 30;

/// The order last let go and where it went, while the latest snapshot
/// still shows it where it was: done once the snapshot has it there (or on
/// that unit), the order is gone, or [`MOVE_SHOW_TICKS`] passed (counted in
/// sim ticks, so a paused game keeps it).
fn command_moved(s: &mut RenderState) -> Option<(u32, [f32; 3])> {
    let (command, target, node, since) = s.command_moved?;
    let latest = s.world.latest.as_ref()?;
    let marker = latest.debug.orders.iter().flat_map(|(_, marks)| marks).find(|m| m.command == command);
    let shown = match (marker, target) {
        (None, _) => true,
        (Some(m), CommandTarget::Entity(id)) => m.target == Some(id),
        (Some(m), _) => (m.node[0] - node[0]).abs() < 0.25 && (m.node[2] - node[2]).abs() < 0.25,
    };
    if shown || latest.tick > since + MOVE_SHOW_TICKS {
        s.command_moved = None;
        return None;
    }
    Some((command, node))
}

fn ground_target(p: glam::Vec3) -> CommandTarget {
    CommandTarget::Ground(rts_core_engine::engine::sim::math::FxVec3::from_f64(p.x as f64, p.y as f64, p.z as f64))
}

/// The drag let go at `at`: the order moves where it was shown going (a
/// build site only where the sim allows the structure: not on a red ghost),
/// and the interface hears the drag end (`OnCommandDragEnd(event, cmdId)`).
fn end_command_drag(s: &mut RenderState, at: (f64, f64)) {
    let Some(drag) = s.command_drag.take() else { return };
    s.frame_input.command_drag = None;
    let allowed = |bp: &str, p: glam::Vec3| placement_answer(&s.world.placement, 0, bp, p).0;
    if let Some((target, p)) = drag.to.filter(|_| drag.moved)
        && drag.marker.blueprint.as_deref().is_none_or(|bp| allowed(bp, p)) {
        s.player.retarget(&s.world, drag.owner, drag.marker.command, target);
        let tick = s.world.latest.as_ref().map_or(0, |l| l.tick);
        s.command_moved = Some((drag.marker.command, target, p.to_array(), tick));
    }
    if let Some(vm) = &s.ui {
        let (ux, uy) = s.ui_point(at.0, at.1);
        if let Err(e) = vm.command_drag_end(ux, uy, drag.marker.command) {
            tracing::warn!("UI OnCommandDragEnd: {e}");
        }
    }
}

/// Build mode (FAF's command mode `build`): the structure's mesh as a
/// ghost under the cursor, snapped to the footprint grid, and the sim asked
/// whether it may go there (answered a snapshot later; the ghost shows the
/// latest answer for that structure).
fn update_build_ghost(s: &mut RenderState) {
    let builds = template_builds(s);
    let dragged_site = s.command_drag.as_ref().filter(|d| d.moved).and_then(|d| Some((d.marker.blueprint.clone()?, d.to.as_ref()?.1)));
    let spots: Vec<(String, glam::Vec3)> = match (&s.ui, s.input.cursor) {
        // A queued build's site being dragged: its ghost where it would go.
        _ if dragged_site.is_some() => dragged_site.into_iter().collect(),
        (Some(vm), Some((cx, cy))) => match vm.command_mode() {
            // A build template or a drag build's line: every structure is a
            // ghost the sim checks.
            Ok(Some((mode, _))) if mode == "build" && !builds.is_empty() => builds,
            Ok(Some((mode, name))) if mode == "build" => {
                let fp = vm.footprint(&name).unwrap_or((1.0, 1.0));
                s.world.ground_at(cx as f32, cy as f32).map(|g| (name.to_lowercase(), deposit_spot(s, vm, &name, g).unwrap_or_else(|| crate::render::player::snap_to_footprint(&s.world, g, fp)))).into_iter().collect()
            }
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    let query: Vec<(String, [f32; 3])> = spots.iter().map(|(bp, p)| (bp.clone(), p.to_array())).collect();
    // The army the player sees and hears as (fog of war, sound culling).
    let listener = s.ui.as_ref().and_then(|vm| vm.focus_army());
    if let Ok(mut r) = s.request.lock() {
        if r.placement != query {
            r.placement = query;
        }
        r.view_army = listener;
    }
    let mut ghosts: Vec<crate::render::scene::Ghost> = spots
        .iter()
        .enumerate()
        .filter_map(|(i, (bp, position))| {
            let blueprint = s.world.blueprints.as_ref()?.iter().position(|b| b == bp)? as u32;
            let (ok, site) = placement_answer(&s.world.placement, i, bp, *position);
            Some(crate::render::scene::Ghost { blueprint, position: *position, color: if ok { GHOST_OK } else { GHOST_BLOCKED }, site })
        })
        .collect();
    s.frame_input.ghost = (!ghosts.is_empty()).then(|| ghosts.remove(0));
    s.frame_input.template_ghosts = ghosts;
}

/// The sim's latest verdict on spot `i` of the placement query (blueprint
/// `bp` at `position`): whether it may be built there, and its skirt. The
/// answer for exactly that spot if there is one, else the same slot's from a
/// snapshot ago (the cursor moved since: its skirt moved here); no answer
/// yet: allowed, no skirt.
fn placement_answer(answers: &[(String, [f32; 3], Option<[f32; 4]>, bool)], i: usize, bp: &str, position: glam::Vec3) -> (bool, Option<[f32; 4]>) {
    let here = |a: &&(String, [f32; 3], Option<[f32; 4]>, bool)| a.0 == bp && (a.1[0] - position.x).abs() < 1e-3 && (a.1[2] - position.z).abs() < 1e-3;
    if let Some(a) = answers.iter().find(here) {
        return (a.3, a.2);
    }
    match answers.get(i).filter(|a| a.0 == bp) {
        Some((_, at, site, ok)) => (*ok, site.map(|[x0, z0, x1, z1]| [x0 - at[0] + position.x, z0 - at[2] + position.z, x1 - at[0] + position.x, z1 - at[2] + position.z])),
        None => (true, None),
    }
}

/// The structures of `builds` the sim allows where they are (red ghosts
/// are left out: the rest are still ordered).
fn placeable(s: &RenderState, builds: Vec<(String, glam::Vec3)>) -> Vec<(String, glam::Vec3)> {
    builds.into_iter().enumerate().filter(|(i, (bp, p))| placement_answer(&s.world.placement, *i, bp, *p).0).map(|(_, b)| b).collect()
}

/// The structures build mode would order now: a drag build's line from
/// where the drag began to the ground under the cursor, or the active build
/// template's structures (`SetActiveBuildTemplate`) about it, each snapped to
/// its footprint; empty otherwise.
fn template_builds(s: &RenderState) -> Vec<(String, glam::Vec3)> {
    let (Some(vm), Some((cx, cy))) = (&s.ui, s.input.cursor) else { return Vec::new() };
    let Ok(Some((mode, name))) = vm.command_mode() else { return Vec::new() };
    if mode != "build" {
        return Vec::new();
    }
    let Some(ground) = s.world.ground_at(cx as f32, cy as f32) else { return Vec::new() };
    if let Some(start) = s.build_drag {
        let fp = vm.footprint(&name).unwrap_or((1.0, 1.0));
        let spacing = vm.drag_spacing(&name).unwrap_or(fp);
        let bp = name.to_lowercase();
        return crate::render::player::drag_line(&s.world, start, ground, fp, spacing).into_iter().map(|p| (bp.clone(), p)).collect();
    }
    let Ok(Some(items)) = vm.build_template() else { return Vec::new() };
    items
        .into_iter()
        .map(|(bp, x, z)| {
            let fp = vm.footprint(&bp).unwrap_or((1.0, 1.0));
            let at = crate::render::player::snap_to_footprint(&s.world, glam::Vec3::new(ground.x + x, ground.y, ground.z + z), fp);
            (bp, at)
        })
        .collect()
}

/// Starts the game a lobby launched (or the recording the Replay dialog
/// picked, or the last session again): it goes to the engine thread, and
/// the current interface makes way for the game's.
fn launch_game(s: &mut RenderState, launched: rts_core_engine::engine::session::Launch) {
    use rts_core_engine::engine::session::Launch;
    let Some(launch) = &s.launch else { return };
    let replay = match &launched {
        Launch::Replay(path) => {
            s.timeline = crate::render::state::read_timeline(path);
            s.keyframes = crate::render::state::Keyframes::for_replay(path);
            // The player's keyframe options, for the engine thread's
            // keyframes of this replay.
            if let Some(settings) = s.ui.as_ref().and_then(|vm| vm.keyframe_settings().inspect_err(|e| tracing::warn!("keyframe options: {e}")).ok()) {
                rts_core_engine::engine::keyframe::recorder::set_settings(settings);
            }
            true
        }
        Launch::Restart | Launch::Seek(_) => s.replay,
        Launch::Game { .. } => {
            s.timeline = None;
            s.keyframes = None;
            false
        }
    };
    // The game's interface waits for this game's mods, not the last one's.
    rts_core_engine::engine::mods::clear_session();
    if launch.send(launched).is_err() {
        tracing::error!("launch: the engine thread is gone");
        return;
    }
    // A replay: the interface's `SessionIsReplay`, and no army to command.
    s.replay = replay;
    tracing::info!(replay, "launched a game");
    s.carried_prefs = s.ui.as_ref().and_then(|vm| vm.prefs().inspect_err(|e| tracing::warn!("interface preferences: {e}")).ok());
    s.front_end = false;
    s.ui = None;
    s.ui_booting = None;
    s.ui_failed = false;
}

/// Ends the running session on the render side: its sim runs no more ticks
/// and the engine thread moves on to the next launch (`SimControl::close`);
/// the world, the camera work and the player's selection go with it (the
/// next engine's first snapshot starts the heads' scenes afresh), and on the
/// way to the front end the player's army too.
fn close_session(s: &mut RenderState, end: crate::render::player::SessionEnd) {
    s.control.close();
    s.world.begin_session();
    s.player.end_session(end);
    s.ui_selection.clear();
    s.pending_select.clear();
    s.pending_ui_lua = None;
    s.tracks.clear();
    s.projectile_follows.clear();
    s.camera_moves.clear();
    s.camera_waiting.clear();
    s.minimap.end_session();
}

/// Starts the session again (`RestartSession`): this one closes and the
/// engine launches the last again, with a fresh interface.
fn restart_session(s: &mut RenderState) {
    tracing::info!("restarting the session");
    close_session(s, crate::render::player::SessionEnd::Restart);
    launch_game(s, rts_core_engine::engine::session::Launch::Restart);
}

/// Starts the replay again from its latest keyframe at or before `tick`
/// (from the start without one), with a fresh interface.
fn restart_session_at(s: &mut RenderState, tick: u64) {
    tracing::info!(tick, "restarting the replay at a keyframe");
    close_session(s, crate::render::player::SessionEnd::Restart);
    launch_game(s, rts_core_engine::engine::session::Launch::Seek(tick));
}

/// How far the camera is from the ground when a seek lands on an event.
const SEEK_ZOOM: f32 = 60.0;

/// Starts a replay seek (`Replay.Seek`): the sim runs flat out and holds at
/// the target tick (`SimControl::hold_at`). An earlier tick than now
/// restarts the replay from its latest keyframe at or before the target
/// (from the start without one), as does a later one with a keyframe more
/// than a keyframe interval ahead of now; then it runs flat out from there.
fn start_seek(s: &mut RenderState, request: rts_core_engine::ui::replay::SeekRequest) {
    use rts_core_engine::engine::snapshot::SimControl;
    // A seek on top of a seek keeps the speed from before the first.
    let speed = s.seek.as_ref().map_or_else(|| s.control.speed(), |k| k.speed);
    // No further than the recording goes (the replay ends there).
    let request = match s.timeline.as_ref().filter(|t| t.marked) {
        Some(t) => rts_core_engine::ui::replay::SeekRequest { tick: request.tick.min(t.length), ..request },
        None => request,
    };
    let back = request.tick < s.world.sim_tick;
    let every = rts_core_engine::engine::keyframe::recorder::settings().every;
    let ahead = s.keyframes.as_ref().and_then(|k| k.at_or_before(request.tick)).filter(|&k| k > s.world.sim_tick + every);
    let restart = back || ahead.is_some();
    tracing::info!(from = s.world.sim_tick, to = request.tick, back, keyframe = ?s.keyframes.as_ref().and_then(|k| k.at_or_before(request.tick)), "replay seek");
    s.seek = Some(crate::render::state::Seek { target: request.tick, session: s.world.session + restart as u64, speed, focus: request.focus });
    s.control.set_hold_at(request.tick);
    s.control.set_speed(SimControl::MAX);
    s.control.set_paused(false);
    if restart {
        restart_session_at(s, request.tick);
    }
}

/// A seek arrived (the world shows its session at the target tick): the
/// sim carries on at the speed from before, and the camera looks where the
/// seek asked.
fn update_seek(s: &mut RenderState) {
    // (The replay may end first: its last tick is as far as it goes.)
    let Some(seek) = s.seek.as_ref().filter(|k| s.world.session == k.session && (s.world.sim_tick >= k.target || s.world.game_over)).cloned() else { return };
    tracing::info!(tick = s.world.sim_tick, "replay seek arrived");
    s.control.set_hold_at(u64::MAX);
    s.control.set_speed(seek.speed);
    if let Some(focus) = seek.focus {
        let request = rts_core_engine::ui::CameraRequest { camera: None, focus: Some(focus), zoom: Some(SEEK_ZOOM), reset: false, wheel: None, seconds: 0.0, max_zoom_mult: None, track: None, hold_rotation: false, revert_rotation: false, rotation: None };
        s.camera_waiting.push((0, request));
    }
    s.seek = None;
}

/// Returns to the main menu (`ExitGame`): the session closes, and the
/// game's interface makes way for the front end's, fresh as at start-up.
fn return_to_front_end(s: &mut RenderState, carried: Option<crate::engine::sim::lua_data::LuaData>) {
    tracing::info!("the interface returned to the front end");
    close_session(s, crate::render::player::SessionEnd::FrontEnd);
    s.carried_prefs = carried;
    s.front_end = true;
    s.replay = false;
    s.timeline = None;
    s.keyframes = None;
    s.seek = None;
    s.ui = None;
    s.ui_booting = None;
    s.ui_failed = false;
}

/// Writes the interface's preferences to `path` (`SavePreferences`, and
/// at exit as moho does). No path: a `--screenshot` run, nothing saved.
pub fn save_prefs(path: Option<&std::path::Path>, vm: &rts_core_engine::ui::UserVm) {
    let Some(path) = path else { return };
    let saved = vm.prefs().map_err(|e| e.to_string()).and_then(|p| rts_core_engine::ui::prefs::save_file(path, &p).map_err(|e| e.to_string()));
    match saved {
        Ok(()) => tracing::info!("preferences saved to {}", path.display()),
        Err(e) => tracing::warn!("preferences {}: {e}", path.display()),
    }
}

fn update_ui(s: &mut RenderState, dt: f32) {
    let _span = rts_core_engine::prof_span!("interface");
    let screen = [s.surface_config.width as f32, s.surface_config.height as f32];
    if s.ui.is_none() {
        if s.ui_failed {
            // No interface to read Secondary Adapter from: `--second-head`
            // alone decides.
            s.decide_second_head(None);
            s.ui_items.clear();
            return;
        }
        let Some(vfs) = s.world.models_vfs() else { return };
        // Stage 1, as soon as the file system arrives (the engine sends it
        // before the game starts up): the interface's own blueprint load and
        // `userInit.lua`, while the sim starts up on its thread.
        // A game's interface runs the session's mods: it waits for the sim
        // to say which (it does so before its blueprints load).
        let session_mods = if s.front_end || s.ui_booting.is_some() {
            None
        } else {
            let Some(m) = rts_core_engine::engine::mods::session() else { return };
            Some(m)
        };
        if s.ui_booting.is_none() {
            s.command_heads(|| crate::render::present::Command::UiVfs(vfs.clone()));
            let _s = rts_core_engine::prof_span!("interface init (blueprints, userInit)");
            let vm = match s.world.script_cache.clone() {
                Some(cache) => rts_core_engine::ui::UserVm::with_cache(vfs, cache),
                None => rts_core_engine::ui::UserVm::new(vfs),
            };
            // The player's Game.prefs (options, key bindings) first, or the
            // front end's when a lobby launched this game.
            let carried = s.carried_prefs.take();
            let vm = vm.inspect(|vm| {
                if let Some(prefs) = &carried {
                    if let Err(e) = vm.set_prefs(prefs) {
                        tracing::warn!("front end preferences: {e}");
                    }
                } else if let Some(p) = &s.prefs.load {
                    match vm.load_prefs(p) {
                        Ok(()) => tracing::info!("preferences from {}", p.display()),
                        Err(e) => tracing::warn!("preferences {}: {e}", p.display()),
                    }
                }
            });
            let vm = vm.and_then(|vm| match &session_mods {
                Some(m) => vm.activate_session_mods(m).map(|_| vm),
                None => Ok(vm),
            });
            match vm.and_then(|vm| vm.init().map(|_| vm)) {
                Ok(vm) => {
                    // The second head, decided by the first interface's
                    // profile (Secondary Adapter takes a restart in FAF),
                    // and every interface told of it before it starts.
                    let option = vm.lua().load("local v = GetOptions('secondary_adapter') return v ~= nil and tostring(v) or nil").eval::<Option<String>>().unwrap_or_else(|e| {
                        tracing::warn!("Secondary Adapter: {e}");
                        None
                    });
                    s.ui_booting = Some(vm);
                    crate::render::console::apply_displays(s);
                    let Some(vm) = s.ui_booting.take() else { return };
                    s.decide_second_head(option.as_deref());
                    let head1 = match (&s.second, &s.second_wanted) {
                        (Some(h), _) => Some((h.size.0, h.size.1, h.scale_factor)),
                        (None, Some((_, w, h, scale))) => Some((*w, *h, *scale)),
                        _ => None,
                    };
                    if let Some((w, h, scale)) = head1 {
                        vm.set_heads(2);
                        let _ = vm.set_head_screen(1, w as f32 / scale, h as f32 / scale);
                    }
                    s.ui_booting = Some(vm);
                }
                Err(e) => {
                    tracing::error!("user layer failed to start: {e}");
                    s.ui_failed = true;
                    return;
                }
            }
        }
        // The front end: no session to wait for, the main menu starts now.
        if s.front_end {
            let Some(vm) = s.ui_booting.take() else { return };
            let _s = rts_core_engine::prof_span!("interface start (front end)");
            match vm.start_front_end() {
                Ok(()) => {
                    tracing::info!("front end started");
                    s.ui = Some(vm);
                    crate::render::console::apply_profile(s);
                }
                Err(e) => {
                    tracing::error!("front end failed to start: {e}");
                    s.ui_failed = true;
                }
            }
            return;
        }
        // Stage 2, once the session is up: the game's armies before the
        // interface is built (FAF's CreateUI reads GetArmiesTable: score
        // panel, unit view), then the panels.
        let (Some(armies), Some(scenario)) = (s.world.armies.clone(), s.world.scenario.clone()) else { return };
        // And once the armies' units are in the frame (or a second in, for
        // a game without any): FAF's first update zooms to the ACU.
        if s.world.tick < 10 && !s.world.entities().iter().any(|e| e.army != 0) {
            return;
        }
        let Some(vm) = s.ui_booting.take() else { return };
        let _s = rts_core_engine::prof_span!("interface start (CreateUI)");
        // A lobby's game: the player commands the army the sim gave them
        // (a replay's recorder's army isn't the viewer's).
        s.player.adopt_army(s.world.local_army, s.replay);
        match Ok(vm)
            .and_then(|vm: rts_core_engine::ui::UserVm| vm.set_armies(&armies).map(|_| vm))
            .and_then(|vm| vm.set_army_display(s.world.army_display.as_deref().map_or(&[], |c| c.as_slice())).map(|_| vm))
            .and_then(|vm| vm.set_alliances(s.world.alliances.as_deref().map_or(&[], |a| a.as_slice())).map(|_| vm))
            .and_then(|vm| vm.set_scenario(&scenario).map(|_| vm))
            .and_then(|vm| vm.set_replay_timeline(s.timeline.clone()).map(|_| vm))
            .and_then(|vm| vm.set_session_flags(s.replay, s.replay || s.player.player.or(s.world.local_army).is_none()).map(|_| vm))
            // Before the interface starts: FAF's gamemain keeps it as
            // `OriginalFocusArmy` (pings, ally checks).
            .and_then(|vm| vm.set_focus_army(s.player.player.map_or(-1, |a| a as i64)).map(|_| vm))
            .and_then(|vm| vm.start_hud().map(|_| vm))
        {
            Ok(vm) => {
                tracing::info!("user layer started");
                s.ui_alliances = s.world.alliances.clone();
                s.ui_army_display = s.world.army_display.clone();
                s.ui = Some(vm);
                crate::render::console::apply_profile(s);
                // Its first beat (the units) before its first frame: FAF's
                // OnFirstUpdate zooms to the ACU (GetArmyAvatars).
                s.ui_tick = u64::MAX;
            }
            Err(e) => {
                tracing::error!("user layer failed to start: {e}");
                s.ui_failed = true;
                return;
            }
        }
    }
    let Some(vm) = &s.ui else {
        // No interface: the strategic icons alone.
        s.ui_items.clear();
        return;
    };
    // The interface's screen is logical pixels (as the base game's at this
    // window size); its draw list is scaled back up below.
    let ui_scale = s.ui_scale();
    let _ = vm.set_screen(screen[0] / ui_scale, screen[1] / ui_scale);
    if let Some(h) = &s.second {
        let scale = h.scale_factor.max(0.25);
        let _ = vm.set_head_screen(1, h.size.0 as f32 / scale, h.size.1 as f32 / scale);
    }
    // FAF's camera API: it reads our cameras and asks for moves / zooms,
    // each for the camera it names (the main one when it names none). The
    // requests wait for the world to show, and a request for a camera not
    // made yet (its view opened this frame) waits a few frames for it.
    if let Some(Ok(requests)) = s.world.camera().is_some().then(|| vm.take_camera_requests()) {
        use crate::render::camera::MAIN;
        let start_up = s.camera_override.is_some() && s.world.tick < CAMERA_OVERRIDE_TICKS;
        let requests: Vec<(u32, rts_core_engine::ui::CameraRequest)> = std::mem::take(&mut s.camera_waiting).into_iter().chain(requests.into_iter().map(|r| (0, r))).collect();
        for (_, r) in &requests {
            if let (Some(m), Some(cam)) = (r.max_zoom_mult, s.world.cameras.get_mut(r.camera.as_deref().unwrap_or(MAIN))) {
                cam.set_max_zoom_mult(m);
            }
        }
        for (age, mut r) in requests.into_iter().filter(|_| !start_up) {
            // A wheel the interface passed on (`ZoomScale`): zoom as the
            // wheel over the world does, toward the cursor.
            if let Some(w) = r.wheel.take() {
                s.input.wheel += w;
            }
            if s.world.cameras.get(r.camera.as_deref().unwrap_or(MAIN)).is_none() {
                if age < CAMERA_WAIT_FRAMES {
                    s.camera_waiting.push((age + 1, r));
                }
                continue;
            }
            let name = r.camera.unwrap_or_else(|| MAIN.to_owned());
            // `TrackEntities`: heads for the entities' middle and follows
            // them; another place to look at ends the following.
            let follow = r.track.and_then(|mut ids| middle(s.world.entities(), &mut ids).map(|at| (ids, at)));
            let Some(cam) = s.world.cameras.get_mut(&name) else { continue };
            // Where the request leaves the camera (a timed move starts
            // from where the camera is now).
            let (mut x, mut z, mut zoom) = (cam.target.x, cam.target.z, cam.zoom);
            if r.hold_rotation {
                cam.hold_rotation();
            }
            if r.revert_rotation {
                cam.revert_rotation();
            }
            if r.reset {
                // The default view faces north at the zoom's pitch.
                cam.clear_rotation();
                if let Some((w, h)) = s.world.map_size {
                    let mut fresh = crate::render::camera::Camera::new(w, h);
                    fresh.aspect = cam.aspect;
                    fresh.fit_map(w, h);
                    (x, z, zoom) = (fresh.target.x, fresh.target.z, fresh.max_zoom);
                }
            }
            if let Some(f) = r.focus {
                (x, z) = (f[0], f[2]);
            }
            if let Some(asked) = r.zoom {
                zoom = asked.clamp(cam.min_zoom, cam.max_zoom);
            }
            if let Some((_, [mx, mz])) = follow {
                (x, z) = (mx, mz);
            }
            if r.reset || r.focus.is_some() || r.zoom.is_some() || follow.is_some() || r.rotation.is_some() {
                let timed = (r.seconds > 0.0).then(|| {
                    let m = crate::render::camera::CameraMove::new(cam, x, z, zoom, r.seconds);
                    match r.rotation {
                        Some((heading, pitch)) => m.with_rotation(cam, heading, pitch),
                        None => m,
                    }
                });
                if timed.is_none() {
                    (cam.target.x, cam.target.z, cam.zoom) = (x, z, zoom);
                    // `SnapTo`: the orientation at once, held.
                    if let Some((heading, pitch)) = r.rotation {
                        cam.set_rotation(heading, pitch);
                    }
                }
                s.camera_moves.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
                s.camera_moves.extend(timed.map(|m| (name.clone(), m)));
            }
            match follow {
                Some((ids, _)) => {
                    s.tracks.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
                    s.projectile_follows.retain(|f| !f.camera.eq_ignore_ascii_case(&name));
                    s.tracks.push((name, ids));
                }
                None if r.reset || r.focus.is_some() => {
                    s.tracks.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
                    s.projectile_follows.retain(|f| !f.camera.eq_ignore_ascii_case(&name));
                }
                None => {}
            }
        }
    }
    let states: Vec<rts_core_engine::ui::CameraState> = s
        .world
        .cameras
        .iter()
        .map(|(name, c)| rts_core_engine::ui::CameraState { name: name.to_owned(), focus: c.target.into(), zoom: c.zoom, min: c.min_zoom, max: c.max_zoom, pitch: c.pitch(), heading: c.heading })
        .collect();
    let _ = vm.set_camera_states(&states);
    // Each view's camera (on the cursor's head), for the interface's
    // projections and picks.
    let pick_scale = s.head_scale(s.world.active_head());
    let picks = s
        .world
        .views()
        .iter()
        .filter_map(|v| {
            let cam = s.world.cameras.get(&v.camera)?;
            Some(rts_core_engine::ui::world_pick::PickView {
                camera: v.camera.clone(),
                origin: [v.rect[0], v.rect[1]],
                view_proj: cam.view_proj(),
                inv_view_proj: cam.view_proj().inverse(),
                screen: v.size(),
                ui_scale: pick_scale,
                target_y: cam.target.y,
                distance: cam.eye_distance(),
                terrain: s.world.terrain_view.clone(),
            })
        })
        .collect();
    vm.set_pick_views(picks);
    // The lobby launched a game: the engine starts it, and the game gets a
    // fresh interface (moho switches Lua states), built once it is up.
    let launched = if s.front_end {
        let game = vm.take_launch_request().unwrap_or_else(|e| {
            tracing::error!("lobby launch: {e}");
            None
        });
        let profile = || vm.profile_name().inspect_err(|e| tracing::warn!("profile name: {e}")).ok().flatten();
        match game {
            Some(config) => Some(rts_core_engine::engine::session::Launch::Game { config, profile: profile() }),
            None => vm.take_replay_request().ok().flatten().map(rts_core_engine::engine::session::Launch::Replay),
        }
    } else {
        None
    };
    if let Some(launched) = launched {
        // Dev / scripted tests: `RTS_GAME_UI_LUA=file` runs in the game's
        // interface once it is up (as `--ui-lua` does in the front end).
        if let Some(code) = std::env::var("RTS_GAME_UI_LUA").ok().and_then(|p| std::fs::read_to_string(p).ok()) {
            s.pending_ui_lua = Some(code);
        }
        launch_game(s, launched);
        return;
    }
    // Pause both ways: the interface's pause button reaches the engine, and
    // any pause (the debug panel, the button) reaches the interface.
    // The interface's ground decals (range rings under the cursor).
    if let Ok(Some(decals)) = vm.user_decals() {
        s.frame_input.user_decals = Some(std::sync::Arc::new(decals));
    }
    // Game speed both ways, in moho's steps.
    if let Ok(Some(step)) = vm.take_speed_request() {
        s.control.set_speed(rts_core_engine::engine::snapshot::SimControl::percent_of(step));
    }
    // In a networked game the speed everyone applied, not a request on its way.
    // `GetGameSpeed` is the speed asked for; `GetSimRate` the speed the game
    // really runs at (capped by the slowest machine, or measured flat out),
    // both as moho steps (FAF's clock shows "+asked / +actual", and its
    // `OnBeat` reads `GetSimRate() > 0` as sped up).
    use rts_core_engine::engine::snapshot::SimControl;
    let (asked, actual) = (SimControl::step_of(s.control.shown()), SimControl::step_of(s.control.effective()));
    let _ = vm.set_game_speed(asked, actual as f32, s.control.changed_by());
    if let Ok(Some(p)) = vm.take_pause_request() {
        s.control.set_paused(p);
    }
    if let Err(e) = vm.set_paused(s.control.paused()) {
        tracing::warn!("UI pause: {e}");
    }
    let _ = vm.set_modifiers(s.input.mods.shift, s.input.mods.ctrl, s.input.mods.alt);
    let mouse_order = s.input.cursor.and_then(|at| s.player.right_click_order(&s.world, at));
    // The unit under the cursor (not over a panel), for `GetRolloverInfo`.
    let rollover = s.input.cursor.filter(|&(x, y)| {
        let (ux, uy) = s.ui_point(x, y);
        !vm.over_interface(ux, uy).unwrap_or(true)
    });
    s.rollover = rollover.and_then(|(x, y)| s.world.pick(x as f32, y as f32));
    let _ = vm.set_rollover(s.rollover);
    // The selection's order waypoint nearest the cursor, within 12 pixels
    // (while one is dragged, that one).
    s.hover_command = match &s.command_drag {
        Some(d) => Some((d.owner, d.to.as_ref().map_or_else(|| d.marker.clone(), |(_, p)| crate::render::player::moved_marker(&d.marker, p.to_array())))),
        None => rollover.and_then(|(x, y)| {
            let (view, cam) = s.world.view_at(x as f32, y as f32)?;
            let latest = s.world.latest.as_ref()?;
            let mut best: Option<(f32, u32, &rts_core_engine::engine::snapshot::OrderMarker)> = None;
            for (unit, marks) in &latest.debug.orders {
                for m in marks {
                    let Some(p) = view.project(cam, glam::Vec3::from_array(m.node)) else { continue };
                    let d2 = (p[0] - x as f32).powi(2) + (p[1] - y as f32).powi(2);
                    if d2 <= 144.0 && best.is_none_or(|(b, ..)| d2 < b) {
                        best = Some((d2, *unit, m));
                    }
                }
            }
            best.map(|(_, unit, m)| (unit, m.clone()))
        }),
    };
    let highlight = s.hover_command.as_ref().map(|(_, m)| (m.node, m.kind, m.command, m.blueprint.clone()));
    // Moho's move-to-patrol conversion (`CUIWorldView`'s flag at +0x25c,
    // MohoEngine.dll 0x10434ee3 / 0x103e6d70): hovering a Move (or
    // FormMove) waypoint while a right click would move, with every selected
    // unit holding 2+ orders, all of that kind, the hovered one not its last.
    s.patrol_convert = highlight
        .as_ref()
        .filter(|_| s.command_drag.is_none() && s.player.mode == crate::render::player::Mode::Normal && mouse_order == Some(rts_core_engine::engine::sim::command::CommandKind::Move))
        .and_then(|&(_, kind, command, _)| crate::render::player::patrol_routes(&s.player.selection, &s.world.latest.as_ref()?.debug.orders, kind, command));
    let _ = vm.set_convert_to_patrol(s.patrol_convert.is_some());
    let _ = vm.set_highlight_command(highlight);
    let _ = vm.set_mouse_order(mouse_order);
    {
        use winit::keyboard::KeyCode as K;
        let shift = s.hold_shift || s.input.keys.contains(&K::ShiftLeft) || s.input.keys.contains(&K::ShiftRight);
        if shift != s.ui_shift {
            s.ui_shift = shift;
            // The command graph is the game's (FAF's reclaim labels index the
            // world view, which the menus and lobby don't have).
            if !s.front_end
                && let Err(e) = vm.command_graph_show(shift)
            {
                tracing::warn!("UI OnCommandGraphShow: {e}");
            }
            // And the engine's half: the whole army's queued orders (not
            // only the selection's) draw while Shift is held.
            if let Ok(mut r) = s.request.lock() {
                r.all_orders = shift && !s.front_end;
            }
        }
    }
    if std::mem::take(&mut s.ui_resized)
        && let Err(e) = vm.application_resized(screen[0], screen[1]) {
        tracing::warn!("UI OnApplicationResize: {e}");
    }
    // The sim moved this client to another army (`SetFocusArmy`: common
    // army control merging it into a teammate's).
    if let Some(f) = s.world.focus_army_changed.take() {
        s.player.player = u32::try_from(f).ok().filter(|&a| a > 0);
        if let Err(e) = vm.set_focus_army(f.into()) {
            tracing::warn!("UI focus army: {e}");
        }
    }
    // Alliances changed in the sim (FAF's SetAlliance: teams, sharing).
    if let Some(a) = &s.world.alliances
        && !s.ui_alliances.as_ref().is_some_and(|u| std::sync::Arc::ptr_eq(u, a)) {
        if let Err(e) = vm.set_alliances(a) {
            tracing::warn!("UI alliances: {e}");
        }
        s.ui_alliances = Some(a.clone());
    }
    // How armies are shown changed in the sim (`SetArmyColor`,
    // `SetArmyShowScore`: campaign scripts).
    if let Some(c) = &s.world.army_display
        && !s.ui_army_display.as_ref().is_some_and(|u| std::sync::Arc::ptr_eq(u, c)) {
        if let Err(e) = vm.set_army_display(c) {
            tracing::warn!("UI army display: {e}");
        }
        s.ui_army_display = Some(c.clone());
    }
    // The player's team colours (`TeamColorMode`: FAF's team colour button).
    let (version, team) = vm.team_colors();
    if version != s.ui_team_colors {
        s.world.set_team_colors(team);
        s.ui_team_colors = version;
    }
    // The selection the interface shows: units' data, then FAF's
    // OnSelectionChanged when it changed.
    let infos = |scene: &crate::render::world::World, ids: &[u32]| -> Vec<rts_core_engine::ui::UnitInfo> {
        ids.iter()
            .filter_map(|&id| scene.entities().binary_search_by_key(&id, |e| e.id).ok().map(|i| &scene.entities()[i]))
            .filter(|e| e.is_interface_unit())
            .map(|e| rts_core_engine::ui::UnitInfo {
                detail: scene.debug().and_then(|d| d.units.iter().find(|u| u.id == e.id).cloned()),
                ..rts_core_engine::ui::UnitInfo::from_view(e, scene.blueprint_name(e.blueprint))
            })
            .collect()
    };
    // The sim's Sync tables, in order (FAF's OnSync).
    let sync_span = rts_core_engine::prof_span!("interface sync");
    for v in std::mem::take(&mut s.world.pending_sync) {
        if let Err(e) = vm.sync(&v) {
            tracing::warn!("UI sync: {e}");
        }
    }
    let _ = sync_span;
    // The game ended: after the beat's Sync, as moho's DoBeat.
    if s.world.game_over
        && let Err(e) = vm.set_game_over() {
        tracing::warn!("UI game over: {e}");
    }
    // The player left the session (FAF's score screen, quitting): this
    // client's sim stops, and a networked game's players are told.
    if vm.take_session_left().unwrap_or(false) {
        tracing::info!("the player left the session");
        s.control.leave();
        rts_core_engine::net::core::close_current();
    }
    // The interface asked to return to the main menu (FAF's `ExitGame`, the
    // score screen's Continue): the game's interface makes way for the
    // front end's, as moho switches Lua states back. Game only: the front
    // end's VM has `ExitGame` too, and must not tear itself down.
    if vm.take_save_request().unwrap_or(false) {
        save_prefs(s.prefs.save.as_deref(), vm);
        s.profile = crate::render::console::read_profile_options(vm);
        s.player.switch_right_click = s.profile.switch_right_click;
        s.displays_saved = true;
    }
    if !s.front_end && vm.take_menu_request().unwrap_or(false) {
        let carried = vm.prefs().inspect_err(|e| tracing::warn!("game preferences: {e}")).ok();
        return_to_front_end(s, carried);
        return;
    }
    if !s.front_end && vm.take_restart_request().unwrap_or(false) {
        s.seek = None;
        restart_session(s);
        return;
    }
    vm.set_seeking(s.seek.as_ref().map(|k| k.target));
    if let Some(k) = s.keyframes.as_mut().filter(|_| s.replay)
        && k.refresh() {
        let settings = rts_core_engine::engine::keyframe::recorder::settings();
        vm.set_replay_keyframes(rts_core_engine::ui::replay::ReplayKeyframes { ticks: k.ticks.clone(), every: settings.every, precompute: settings.precompute });
    }
    if let Some(request) = vm.take_seek_request().filter(|_| !s.front_end && s.replay) {
        start_seek(s, request);
        return;
    }
    if s.player.selection != s.ui_selection {
        let _s = rts_core_engine::prof_span!("interface selection");
        let _ = vm.update_units(&infos(&s.world, &s.player.selection));
        if let Err(e) = vm.set_selection(&s.player.selection) {
            tracing::warn!("UI selection: {e}");
        }
        s.ui_selection = s.player.selection.clone();
    }
    // Selections the interface made (FAF's SelectUnits: idle engineers,
    // selection hotkeys, ...).
    if let Ok(Some(ids)) = vm.take_selection_request() {
        s.player.selection = ids;
    }
    if vm.time() > 6.0 {
        if !s.pending_select.is_empty() {
            s.player.selection = std::mem::take(&mut s.pending_select);
        }
        if let Some(code) = s.pending_ui_lua.take()
            && let Err(e) = vm.lua().load(code.as_str()).set_name("=--ui-lua").exec() {
            tracing::warn!(target: "lua", "--ui-lua failed: {e}");
        }
    }
    // Orders given from the panels.
    match vm.take_orders() {
        Ok(orders) => {
            for o in orders {
                if let Some(callback) = o.callback {
                    s.player.sim_callback_ui(o.units.unwrap_or_default(), callback);
                    continue;
                }
                if let Some(t) = o.toggle {
                    s.player.toggle_ui(&s.world, o.units.as_deref(), t);
                    continue;
                }
                if !o.script.is_empty() {
                    // `clear`: replace the queue; else it goes on the end.
                    s.player.script_ui(&s.world, o.units.as_deref(), o.script, !o.clear);
                    continue;
                }
                // `IssueDockCommand`: the interface picks the platform.
                if o.command == "UNITCOMMAND_Dock" {
                    s.player.issue_dock(&s.world, !o.clear);
                    continue;
                }
                match rts_core_engine::ui::command_kind(&o.command) {
                    Some(kind) => {
                        // Blueprint orders (`IssueBlueprintCommand`: factory
                        // builds, upgrades) queue unless told to clear, so an
                        // upgrade clicked on an upgrading structure goes after
                        // the one in progress; other orders replace.
                        use crate::engine::sim::command::CommandKind as K;
                        let queue = if matches!(kind, K::BuildFactory | K::Upgrade) { !o.clear } else { false };
                        s.player.issue_ui(&s.world, o.units.as_deref(), kind, o.blueprint.as_deref(), o.count, queue);
                    }
                    None => tracing::debug!(command = o.command, "interface order not supported yet"),
                }
            }
        }
        Err(e) => tracing::warn!("UI orders: {e}"),
    }
    // A new sim tick: the UI's beat.
    if s.world.tick != s.ui_tick {
        let _s = rts_core_engine::prof_span!("interface beat");
        // Every unit of the frame, for GetUnitById; units the interface
        // holds beyond the selection are refreshed, and gone ones dropped.
        let world_span = rts_core_engine::prof_span!("beat: world units");
        vm.set_world(
            s.world
                .entities()
                .iter()
                // Blips and dying units aren't units to the interface.
                .filter(|e| e.army != 0 && e.is_interface_unit())
                .map(|e| rts_core_engine::ui::UnitInfo::from_view(e, s.world.blueprint_name(e.blueprint)))
                .collect(),
        );
        vm.set_busy(s.world.latest.as_ref().map_or(&[][..], |l| l.busy.as_slice()));
        if let Some(d) = &s.world.deposits {
            vm.set_deposits(d.clone());
        }
        let _ = world_span;
        let known_span = rts_core_engine::prof_span!("beat: held units refresh");
        let _ = vm.refresh_held_units(&s.ui_selection, |others| infos(&s.world, others));
        let _ = known_span;
        {
            let _s = rts_core_engine::prof_span!("beat: selection refresh");
            if vm.update_units(&infos(&s.world, &s.ui_selection)).unwrap_or(false)
                && let Err(e) = vm.refresh_selection() {
                tracing::warn!("UI selection refresh: {e}");
            }
        }
        // The construction panel's factory when it isn't selected (a mobile
        // factory's external factory): the engine sends its queue too.
        let queue_factory = vm.queue_factory();
        if let Ok(mut r) = s.request.lock() {
            r.queue_factory = queue_factory;
        }
        s.ui_tick = s.world.tick;
        let _s = rts_core_engine::prof_span!("beat: Lua (OnBeat)");
        if let Err(e) = vm.beat(s.world.sim_tick, &s.world.economy) {
            tracing::warn!("UI beat: {e}");
        }
    }
    {
        let _s = rts_core_engine::prof_span!("interface frame (Lua)");
        if let Err(e) = vm.frame(dt) {
            tracing::warn!("UI frame: {e}");
        }
    }
    // The interface (its minimap expanded into the map image, unit dots and
    // the camera's view); the strategic icons go under it once the frame's
    // positions are known (`overlays::icon_items`).
    let mut items = Vec::new();
    let Some(vm) = &s.ui else { return };
    let draw_list_span = rts_core_engine::prof_span!("interface draw list");
    let ui = {
        let _l = rts_core_engine::prof_span!("draw list (Lua)");
        match vm.draw_list() {
            Ok(ui) => ui,
            Err(e) => {
                tracing::warn!("UI draw list: {e}");
                Vec::new()
            }
        }
    };
    s.minimap_rect = None;
    // The world views the interface shows (back to front), drawn under it.
    let mut views = Vec::new();
    for item in ui {
        match to_physical(item, ui_scale) {
            rts_core_engine::ui::controls::DrawItem::Minimap { rect } => {
                s.minimap_rect = Some(rect);
                let _m = rts_core_engine::prof_span!("minimap items");
                items.extend(minimap_items(s, rect));
            }
            rts_core_engine::ui::controls::DrawItem::World { rect, camera } => views.push(crate::render::camera::View { camera, rect }),
            other => items.push(other),
        }
    }
    s.world.set_views(views);
    // The second head's: root frame 1's world views and draw list (no
    // minimap: it draws on the main window only).
    if let (Some(vm), Some(scale)) = (&s.ui, s.second.as_ref().map(|h| h.scale_factor.max(0.25))) {
        let ui = vm.head_draw_list(1).unwrap_or_else(|e| {
            tracing::warn!("UI draw list (head 2): {e}");
            Vec::new()
        });
        let (mut views, mut items) = (Vec::new(), Vec::new());
        for item in ui {
            match to_physical(item, scale) {
                rts_core_engine::ui::controls::DrawItem::World { rect, camera } => views.push(crate::render::camera::View { camera, rect }),
                rts_core_engine::ui::controls::DrawItem::Minimap { .. } => {}
                other => items.push(other),
            }
        }
        s.world.set_head_views(1, views);
        if let Some(h) = s.second.as_mut() {
            h.ui_items = items;
        }
    }
    // The range overlays the interface has on, and each kind's own style.
    if let (Some(vm), true) = (&s.ui, s.frame_input.range_styles.is_empty()) {
        match vm.range_styles() {
            Ok(styles) => s.frame_input.range_styles = std::sync::Arc::new(styles),
            Err(e) => {
                tracing::warn!("UI range styles: {e}");
                s.frame_input.range_styles = std::sync::Arc::new(vec![None; rts_core_engine::engine::snapshot::RANGE_KINDS.len()]);
            }
        }
    }
    if let (Some(vm), Some(bps)) = (&s.ui, s.world.blueprints.clone()) {
        match vm.take_overlay_filters(&bps) {
            Ok(Some(filters)) => s.frame_input.range_filters = std::sync::Arc::new(filters),
            Ok(None) => {}
            Err(e) => tracing::warn!("UI range overlays: {e}"),
        }
    }
    // FAF's cursor, made the window's cursor after the frame (`app.rs`).
    if let Some(vm) = &s.ui {
        s.ui_cursor = vm.cursor();
    }
    let _ = draw_list_span;
    s.ui_items = items;
}

/// The minimap's map preview (the `.scmap`'s image, uploaded as
/// `#minimap`): looked for once a session, not every frame.
#[derive(Default)]
pub struct MinimapPreview {
    tried: bool,
}

impl MinimapPreview {
    /// The session closed (`close_session`): the next session's map is
    /// looked for afresh (its upload replaces the last one's image).
    pub fn end_session(&mut self) {
        *self = Self::default();
    }

    /// The world's map preview when it is still to upload this session
    /// (`None` once looked for, found or not).
    pub fn take_new(&mut self, world: &super::world::World) -> Option<rts_core_engine::engine::assets::dds::Texture> {
        if self.tried {
            return None;
        }
        self.tried = true;
        let map = world.terrain_view.as_ref().and_then(|t| t.map.clone());
        let image = map.zip(world.models_vfs()).and_then(|(path, vfs)| {
            let bytes = vfs.read().ok()?.read(&path).ok()?;
            let dds = rts_core_engine::engine::assets::scmap::read_preview(&bytes).ok()?.to_vec();
            rts_core_engine::engine::assets::dds::read(&dds, false).ok()
        });
        match image {
            Some(t) if !t.levels.is_empty() => Some(t),
            _ => {
                tracing::warn!("minimap: no map preview image");
                None
            }
        }
    }
}

/// The minimap in `rect`: the map's preview image (from its `.scmap`,
/// loaded once), a dot per unit and structure in its army's colour, and the
/// outline of what the main camera sees.
fn minimap_items(s: &mut RenderState, rect: [f32; 4]) -> Vec<rts_core_engine::ui::controls::DrawItem> {
    use rts_core_engine::ui::controls::DrawItem;
    let mut out = Vec::new();
    let Some((mw, mh)) = s.world.map_size else { return out };
    if let Some(t) = s.minimap.take_new(&s.world) {
        s.presenter.command(crate::render::present::Command::Image { name: "#minimap".into(), w: t.width, h: t.height, rgba: t.levels[0].clone(), replace: true });
        s.ui_images.insert("#minimap".into());
    }
    let quad = |rect: [f32; 4], texture: Option<String>, color: [f32; 4]| DrawItem::Quad { rect, texture, uv: [0.0, 0.0, 1.0, 1.0], color, tiled: false };
    out.push(quad(rect, Some("#minimap".into()), [1.0; 4]));
    let (rw, rh) = (rect[2] - rect[0], rect[3] - rect[1]);
    let to_mini = |x: f32, z: f32| [rect[0] + x / mw * rw, rect[1] + z / mh * rh];
    // The unit dots: an image the size of the minimap, redrawn each sim
    // tick and drawn as one quad (a quad per unit was 48k quads a frame in
    // a big battle, most of the interface upload).
    let (w, h) = ((rw.round() as u32).max(1), (rh.round() as u32).max(1));
    if s.world.tick != s.minimap_dots_tick || s.minimap_dots_size != (w, h) {
        let _d = rts_core_engine::prof_span!("minimap dots image");
        s.minimap_dots_tick = s.world.tick;
        s.minimap_dots_size = (w, h);
        let scale = s.ui_scale();
        let mut px = std::mem::take(&mut s.minimap_dots);
        px.clear();
        px.resize((w * h * 4) as usize, 0);
        for e in s.world.entities() {
            if e.army == 0 || !matches!(e.kind, crate::engine::snapshot::ViewKind::Unit | crate::engine::snapshot::ViewKind::Structure) {
                continue;
            }
            let (x, y) = (e.position[0] / mw * w as f32, e.position[2] / mh * h as f32);
            // Logical pixels, as the interface around it.
            let half = if e.kind == crate::engine::snapshot::ViewKind::Structure { 1.5 } else { 1.0 } * scale;
            // A blip or a remembered scouted structure: grey, not its army's colour.
            let [r, g, b] = if e.sighting != crate::engine::snapshot::Sighting::Seen { crate::render::overlays::UNIDENTIFIED } else { crate::render::scene::army_color(&s.world.army_rgb, e.army) };
            let rgb = [(r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8];
            let (x0, x1) = ((x - half).floor().max(0.0) as u32, ((x + half).ceil().max(0.0) as u32).min(w));
            let (y0, y1) = ((y - half).floor().max(0.0) as u32, ((y + half).ceil().max(0.0) as u32).min(h));
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let i = ((yy * w + xx) * 4) as usize;
                    px[i..i + 3].copy_from_slice(&rgb);
                    px[i + 3] = 255;
                }
            }
        }
        s.presenter.command(crate::render::present::Command::Image { name: "#minimap-units".into(), w, h, rgba: px.clone(), replace: false });
        s.minimap_dots = px;
    }
    out.push(quad(rect, Some("#minimap-units".into()), [1.0; 4]));
    // What the main view sees: the ground under its corners, boxed.
    let main = s.world.head_views(0).iter().find(|v| v.camera.eq_ignore_ascii_case(crate::render::camera::MAIN));
    let corners: Vec<glam::Vec3> = match main.and_then(|v| Some((v, s.world.cameras.get(&v.camera)?))) {
        Some((v, cam)) => {
            let [l, t, r, b] = v.rect;
            [(l, t), (r, t), (l, b), (r, b)].iter().filter_map(|&(x, y)| s.world.ground_through(v, cam, x, y)).collect()
        }
        None => Vec::new(),
    };
    if corners.len() == 4 {
        let pts: Vec<[f32; 2]> = corners.iter().map(|c| to_mini(c.x, c.z)).collect();
        let (x0, x1) = (pts.iter().map(|p| p[0]).fold(f32::MAX, f32::min).max(rect[0]), pts.iter().map(|p| p[0]).fold(f32::MIN, f32::max).min(rect[2]));
        let (y0, y1) = (pts.iter().map(|p| p[1]).fold(f32::MAX, f32::min).max(rect[1]), pts.iter().map(|p| p[1]).fold(f32::MIN, f32::max).min(rect[3]));
        let white = [1.0, 1.0, 1.0, 0.8];
        // One logical pixel wide.
        let line = s.ui_scale().round().max(1.0);
        for r in [[x0, y0, x1, y0 + line], [x0, y1 - line, x1, y1], [x0, y0, x0 + line, y1], [x1 - line, y0, x1, y1]] {
            out.push(quad(r, None, white));
        }
    }
    out
}

/// The placement ghost and the interface's own meshes (order feedback),
/// from the interface's state this frame.
fn update_ghosts(s: &mut RenderState) {
    let _s = rts_core_engine::prof_span!("build ghost");
    update_build_ghost(s);
    s.frame_input.interface_meshes = s.ui.as_ref().map(|vm| vm.interface_meshes()).unwrap_or_default();
}

/// This frame's input for the GPU side: the world's cameras, views and
/// tables, the cursor, the unit under it, the selection, the settings (and
/// the ghosts and overlays the interface set).
fn fill_input(s: &mut RenderState) {
    let input = &mut s.frame_input;
    s.world.fill_input(0, input);
    // (The cursor on the second head's window isn't over this one.)
    let cursor = s.input.cursor.filter(|_| s.cursor_head == 0);
    input.cursor = cursor.map(|(x, y)| [x as f32, y as f32]);
    input.hovered = s.rollover;
    input.ui_scale = s.scale_factor.max(0.25);
    input.selection.clone_from(&s.player.selection);
    input.settings = s.settings;
    let o = &mut input.overlays;
    o.rules = crate::render::overlays::BarRules { render: s.settings.render_bars, force_enemy: s.settings.force_enemy_bars, show_attached: s.profile.show_attached_bars, viewer: s.player.player, hovered: s.rollover };
    o.display_eta = s.profile.display_eta;
    o.cursor = cursor;
    (o.left_press, o.right_press) = if s.cursor_head == 0 { (s.player.left_press, s.player.right_press) } else { (None, None) };
    o.markers.clone_from(&s.player.markers);
}

/// A frame for the second head, when its present thread asks: the
/// snapshots taken since its last, the main frame's latest input with its
/// own views (and the cursor, while it's on its window), root frame 1's
/// draw list. Both halves at once: the interface runs with the main
/// window's frames.
pub fn second_frame(s: &mut RenderState) {
    let _frame = rts_core_engine::prof_span!("frame (head 2)");
    // Its cameras at its own rate (the main window's frames step the rest).
    let Some(h) = s.second.as_mut() else { return };
    let frame_start = std::time::Instant::now();
    let real = h.last_frame.elapsed().as_secs_f32();
    let dt = real.min(0.1);
    h.last_frame = frame_start;
    // Its FPS readout: the interval eased, as the main window's.
    h.frame_interval += (real.min(5.0) - h.frame_interval) * 0.1;
    s.world.set_active_head(s.cursor_head);
    {
        let _s = rts_core_engine::prof_span!("camera (head 2)");
        update_camera(s, 1, dt);
    }
    let RenderState { second, world, frame_input, cursor_head, input, player, show_status, .. } = s;
    let Some(h) = second.as_mut() else { return };
    if let Some(out) = h.presenter.take_out() {
        world.set_icon_rects(1, out.icon_rects);
        world.add_mesh_bounds(&out.mesh_bounds);
        h.cpu_ms = out.cpu_ms;
        h.captured |= out.captured;
    }
    let mut frame = frame_input.clone();
    world.fill_input(1, &mut frame);
    frame.ui_scale = h.scale_factor.max(0.25);
    // The main frame's cursor and drags are its window's: on this one only
    // while the cursor is here.
    let here = *cursor_head == 1;
    frame.cursor = input.cursor.filter(|_| here).map(|(x, y)| [x as f32, y as f32]);
    frame.overlays.cursor = input.cursor.filter(|_| here);
    (frame.overlays.left_press, frame.overlays.right_press) = if here { (player.left_press, player.right_press) } else { (None, None) };
    h.presenter.early(crate::render::present::Early { deltas: std::mem::take(&mut h.deltas), input: frame.clone() });
    // `--screenshot`: this head's frame first (the main window's exits).
    let capture = match &h.capture {
        Some(path) if h.capture_requested && !h.capture_sent => {
            h.capture_sent = true;
            Some(crate::render::present::Capture { path: path.clone(), exit: false })
        }
        _ => None,
    };
    let (egui, egui_textures) = fps_readout(h, *show_status);
    h.presenter.late(crate::render::present::Late {
        input: frame,
        ui_items: h.ui_items.clone(),
        ui_scale: h.scale_factor.max(0.25),
        egui,
        egui_textures,
        pixels_per_point: h.scale_factor.max(0.25),
        capture,
    });
    h.main_ms = frame_start.elapsed().as_secs_f32() * 1000.0;
    crate::engine::tracy::head2_frame_mark();
}

/// The second head's FPS and CPU time, small in its lower left corner
/// (shown with the main window's stats bar).
fn fps_readout(h: &crate::render::state::SecondHead, show: bool) -> (Vec<egui::ClippedPrimitive>, egui::TexturesDelta) {
    let ppp = h.scale_factor.max(0.25);
    if h.egui_ctx.pixels_per_point() != ppp {
        h.egui_ctx.set_pixels_per_point(ppp);
    }
    let points = egui::vec2(h.size.0 as f32 / ppp, h.size.1 as f32 / ppp);
    let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, points)), ..Default::default() };
    let (ms, cpu, render) = (h.frame_interval.max(1e-4), h.main_ms, h.cpu_ms);
    let output = h.egui_ctx.run_ui(raw, |ui| {
        if !show {
            return;
        }
        egui::Area::new(egui::Id::new("head 2 fps")).anchor(egui::Align2::LEFT_BOTTOM, [4.0, -4.0]).interactable(false).show(ui.ctx(), |ui| {
            egui::Frame::new().fill(egui::Color32::from_black_alpha(160)).inner_margin(egui::Margin::symmetric(6, 2)).corner_radius(3.0).show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(format!("{:.0} FPS  cpu {cpu:.1}ms + render {render:.1}ms", 1.0 / ms)).weak()).wrap_mode(egui::TextWrapMode::Extend));
            });
        });
    });
    (h.egui_ctx.tessellate(output.shapes, output.pixels_per_point), output.textures_delta)
}

/// Ticks `--camera` holds the view against the interface's start-up moves.
const CAMERA_OVERRIDE_TICKS: u64 = 10;

/// Makes one frame on the main thread, when the present thread asks for it:
/// snapshots, input and the camera, then the frame's first half to the
/// present thread; the interface's Lua and egui, then the second half.
pub fn render_frame(s: &mut RenderState) {
    // Keep up with the sim even when nothing can be drawn (window hidden).
    let _frame = rts_core_engine::prof_span!("frame");
    // The frame's CPU time: all of this function but its waits on the
    // swapchain (acquire, present), where vsync paces it.
    let frame_start = std::time::Instant::now();
    // What the present thread's last frame worked out.
    if let Some(out) = s.presenter.take_out() {
        take_present_out(s, out);
    }
    let aspect = s.surface_config.width as f32 / s.surface_config.height.max(1) as f32;
    let deltas = {
        let _s = rts_core_engine::prof_span!("receive snapshots");
        s.world.receive(&s.snapshots, aspect)
    };
    update_seek(s);
    // Each head's scene takes every snapshot (a head decided on later
    // takes the ones it missed).
    if let Some(h) = &mut s.second {
        h.deltas.extend(deltas.iter().cloned());
    } else if !s.second_decided {
        s.second_stash.extend(deltas.iter().cloned());
    }
    let start_up = s.world.tick < CAMERA_OVERRIDE_TICKS;
    if let Some(cam) = s.world.camera_mut() {
        // Held through start-up, so the interface's opening camera move
        // (FAF restores its saved view) doesn't replace it.
        if let Some((x, z, d)) = s.camera_override.filter(|_| start_up) {
            cam.target.x = x;
            cam.target.z = z;
            cam.zoom = d.clamp(cam.min_zoom, cam.max_zoom);
        }
    }
    s.world.update_listener();

    // Selection and orders (physical pixels throughout, on the cursor's
    // head's window).
    s.world.set_active_head(s.cursor_head);
    if let Some(vm) = &s.ui {
        vm.set_mouse_head(s.cursor_head);
    }
    {
        for (from, to) in std::mem::take(&mut s.world.upgraded) {
            s.player.upgraded(from, to);
        }
        s.player.prune(&s.world);
        for (ev, mods) in std::mem::take(&mut s.input.events) {
            // The interface sees mouse input first; what it takes doesn't
            // reach the scene. Releases reach both (a drag that began in the
            // world ends there too).
            // Right click on a waypoint that converts moves into a patrol:
            // the routes go out as patrols on release.
            match ev {
                crate::render::player::Event::RightDown(..) if s.patrol_convert.is_some() => {
                    s.patrol_converting = true;
                    continue;
                }
                crate::render::player::Event::RightUp(..) if s.patrol_converting => {
                    s.patrol_converting = false;
                    if let Some(routes) = s.patrol_convert.clone() {
                        s.player.convert_to_patrol(&s.world, &routes);
                    }
                    continue;
                }
                _ => {}
            }
            // A drag build let go: its line, in order, without the spots the
            // sim refused (red).
            if matches!(ev, crate::render::player::Event::LeftUp(..)) && s.build_drag.is_some() {
                let builds = placeable(s, template_builds(s));
                s.build_drag = None;
                if !builds.is_empty() {
                    s.player.issue_builds(&s.world, &builds, mods.shift);
                }
                continue;
            }
            // An order armed in the interface (FAF's command mode): a world
            // click gives it, a right click cancels it.
            if let Some(vm) = &s.ui {
                let mode = match ev {
                    crate::render::player::Event::LeftDown(..) | crate::render::player::Event::RightDown(..) => vm.command_mode().unwrap_or(None),
                    _ => None,
                };
                if let Some((mode, name)) = mode {
                    let (x, y) = match ev {
                        crate::render::player::Event::LeftDown(x, y) | crate::render::player::Event::RightDown(x, y) => (x, y),
                        _ => (0.0, 0.0),
                    };
                    let (ux, uy) = s.ui_point(x, y);
                    let over_ui = vm.hit_interface(ux, uy).unwrap_or(false);
                    if !over_ui {
                        if matches!(ev, crate::render::player::Event::RightDown(..)) {
                            s.build_drag = None;
                            let _ = vm.end_command_mode(false);
                            continue;
                        }
                        let (kind, bp) = if mode == "build" { (Some(crate::engine::sim::command::CommandKind::BuildMobile), Some(name.as_str())) } else { (rts_core_engine::ui::command_kind(&name), None) };
                        let footprint = bp.and_then(|b| vm.footprint(b).ok());
                        // A drag-buildable structure: the order waits for the
                        // button to come up (the line laid by then; it may
                        // start on a spot the sim refuses).
                        if mode == "build" && matches!(ev, crate::render::player::Event::LeftDown(..)) && vm.build_template().ok().flatten().is_none() && vm.drag_buildable(&name).unwrap_or(false) {
                            let fp = vm.footprint(&name).unwrap_or((1.0, 1.0));
                            s.build_drag = s.world.ground_at(x as f32, y as f32).map(|g| deposit_spot(s, vm, &name, g).unwrap_or_else(|| crate::render::player::snap_to_footprint(&s.world, g, fp)));
                            if s.build_drag.is_some() {
                                continue;
                            }
                        }
                        // A build template: every structure that fits.
                        let builds = if mode == "build" { template_builds(s) } else { Vec::new() };
                        if !builds.is_empty() {
                            let builds = placeable(s, builds);
                            if !builds.is_empty() {
                                s.player.issue_builds(&s.world, &builds, mods.shift);
                            }
                            continue;
                        }
                        // A spot the sim refused (the ghost is red): no order.
                        if mode == "build" && s.frame_input.ghost.is_some_and(|g| g.color == GHOST_BLOCKED) {
                            continue;
                        }
                        // An extractor goes on the deposit its ghost snapped to.
                        if mode == "build"
                            && let Some(at) = s.world.ground_at(x as f32, y as f32).and_then(|g| deposit_spot(s, vm, &name, g)) {
                            s.player.issue_builds(&s.world, &[(name.to_lowercase(), at)], mods.shift);
                            continue;
                        }
                        if let Some(kind) = kind {
                            // (OnCommandIssued below ends the mode.)
                            s.player.issue_mode(&s.world, (x, y), kind, bp, footprint, mods.shift);
                            continue;
                        }
                    }
                }
            }
            // A left press on a waypoint drags it (moho's `UICommandDragger`)
            // instead of selecting; letting go moves the order there.
            match ev {
                crate::render::player::Event::LeftDown(x, y) if s.command_drag.is_none() && s.player.mode == crate::render::player::Mode::Normal => {
                    if let Some((owner, marker)) = s.hover_command.clone().filter(|(owner, m)| m.movable && s.player.commands(&s.world, *owner)) {
                        s.command_drag = Some(crate::render::player::CommandDrag::new(owner, marker, (x, y)));
                        if let Some(vm) = &s.ui
                            && let Err(e) = vm.command_drag_begin() {
                            tracing::warn!("UI OnCommandDragBegin: {e}");
                        }
                        continue;
                    }
                }
                crate::render::player::Event::LeftUp(x, y) if s.command_drag.is_some() => {
                    end_command_drag(s, (x, y));
                    continue;
                }
                _ => {}
            }
            // Keys go to FAF's key map first (control groups: FAF's
            // selection sets; Esc: its EscapeHandler). A key the interface
            // used doesn't pan the camera.
            // Typing into an interface edit (chat, filter boxes).
            if let Some(vm) = &s.ui {
                match ev {
                    crate::render::player::Event::Char(ch) => {
                        let _ = vm.text(ch.encode_utf8(&mut [0; 4]));
                        continue;
                    }
                    crate::render::player::Event::KeyRepeat(code) => {
                        if let Some(name) = faf_key_name(code) {
                            let _ = vm.key_repeat(name, mods.ctrl, mods.alt, mods.shift);
                        }
                        continue;
                    }
                    _ => {}
                }
            }
            if let (Some(vm), crate::render::player::Event::Key(code)) = (&s.ui, ev)
                && let Some(name) = faf_key_name(code)
                && vm.key(name, mods.ctrl, mods.alt, mods.shift).unwrap_or(false) {
                s.input.keys.remove(&code);
                continue;
            }
            // A left click (or drag) on the minimap moves the camera there,
            // unless a window over it (the cheat menu) has the click.
            if let (Some(r), crate::render::player::Event::LeftDown(px, py)) = (s.minimap_rect, ev) {
                let (x, y) = (px as f32, py as f32);
                let on_top = s.ui.as_ref().is_none_or(|vm| {
                    let (ux, uy) = s.ui_point(px, py);
                    vm.hit_minimap(ux, uy).unwrap_or(true)
                });
                if on_top && x >= r[0] && x < r[2] && y >= r[1] && y < r[3] {
                    if let (Some((mw, mh)), Some(cam)) = (s.world.map_size, s.world.camera_mut()) {
                        cam.target.x = (x - r[0]) / (r[2] - r[0]) * mw;
                        cam.target.z = (y - r[1]) / (r[3] - r[1]) * mh;
                    }
                    continue;
                }
            }
            if let (Some(vm), Some((ui_ev, x, y))) = (&s.ui, ui_mouse(ev)) {
                use rts_core_engine::ui::controls::{Modifiers, MouseEvent};
                if let MouseEvent::Press(b) = ui_ev {
                    s.ui_buttons |= 1 << b;
                }
                let m = Modifiers { left: s.ui_buttons & 2 != 0, right: s.ui_buttons & 4 != 0, middle: false, shift: mods.shift, ctrl: mods.ctrl, alt: mods.alt };
                if let MouseEvent::Release(b) = ui_ev {
                    s.ui_buttons &= !(1 << b);
                }
                let (ux, uy) = s.ui_point(x, y);
                let taken = vm.mouse(ui_ev, ux, uy, m).unwrap_or_else(|e| {
                    tracing::warn!("UI mouse: {e}");
                    false
                });
                if taken && !matches!(ui_ev, MouseEvent::Release(_)) {
                    continue;
                }
            }
            s.player.handle(&mut s.world, ev, mods, s.input.cursor);
        }
        // Console commands the interface's key map handed the engine.
        let console = s.ui.as_ref().and_then(|vm| vm.take_console().ok()).unwrap_or_default();
        for line in console {
            crate::render::console::run(s, &line);
        }
        // The interface hears every order given (moho's OnCommandIssued);
        // FAF ends command mode there unless the order was shift-queued.
        let issued = std::mem::take(&mut s.player.issued);
        if let Some(vm) = &s.ui {
            for o in issued {
                if let Err(e) = vm.command_issued(o.kind, o.target, o.queue, o.blueprint.as_deref(), &o.units, &o.script) {
                    tracing::warn!("UI OnCommandIssued: {e}");
                    if !o.queue {
                        let _ = vm.end_command_mode(true);
                    }
                }
            }
        }
        if let (Some((x, z)), Some(cam)) = (s.player.focus.take(), s.world.camera_mut()) {
            cam.target.x = x;
            cam.target.z = z;
        }
        if let Ok(mut r) = s.request.lock()
            && r.selection != s.player.selection {
            r.selection = s.player.selection.clone();
            r.selected = s.player.selection.first().copied();
        }
    }
    // Camera input first: the interface below projects strategic icons,
    // labels and picks with this frame's camera (updated after it, they
    // trailed the terrain by a frame, which shows when frames are slow).
    let real = s.last_frame.elapsed().as_secs_f32();
    let dt = real.min(0.1);
    s.last_frame = std::time::Instant::now();
    // The FPS readout: the measured interval, eased (egui's `stable_dt`
    // falls back to its 1/60 s guess for slow frames, so it read 60 FPS at 5).
    s.frame_interval += (real.min(5.0) - s.frame_interval) * 0.1;
    s.frame_times.push(real * 1000.0);
    s.slow_frames.note(real as f64 * 1000.0, 100.0, s.world.sim_tick);
    if s.world.sim_tick != s.timed_tick {
        s.timed_tick = s.world.sim_tick;
        s.tick_times.push(s.world.tick_ms);
    }
    {
        let _s = rts_core_engine::prof_span!("camera");
        update_camera(s, 0, dt);
    }
    // The frame's first half to the presenter (the snapshots and this
    // frame's cameras): its heavy prepare (interpolation, posing, particles)
    // runs on the render pool while the interface runs here.
    fill_input(s);
    s.presenter.early(crate::render::present::Early { deltas, input: s.frame_input.clone() });
    // The user layer: boot it once the game's files arrive, then lay out and
    // draw its controls this frame.
    update_ui(s, dt);
    // The options dialog's OK: its displays (the window may move).
    if std::mem::take(&mut s.displays_saved) {
        crate::render::console::apply_displays(s);
    }
    update_command_drag(s);
    update_ghosts(s);
    // The interface's output this frame (views, ghosts, its meshes).
    fill_input(s);
    // egui: the debug windows, overlays and status bar, then the encoders.
    let ui_span = rts_core_engine::prof_span!("egui + encode");

    let raw_input = s.egui_state.take_egui_input(&s.window);
    let collector = s.collector.clone();
    let debug = s.debug_ui.lock().unwrap();
    let tick_rate = s.world.tick_rate();
    let frame_interval = s.frame_interval;
    s.memory.refresh(&s.device);
    let (cpu_ms, cpu_p99, gpu_ms, render_ms) = (s.cpu_frame_ms, s.cpu_times.get(0.99), s.gpu_frame_ms, s.present_out.cpu_ms);
    let (ram, vram) = (s.memory.ram, s.memory.vram);
    let (tick_p50, tick_p99) = (s.tick_times.get(0.5), s.tick_times.get(0.99));
    let control = s.control.clone();
    let scene = &s.world;
    let request = s.request.clone();
    let player = &s.player;
    let mut smaa = s.settings.smaa;
    let show_debug = s.show_debug;
    let mut show_status = s.show_status;
    let draws_ui = s.present_out.ui_draws;
    let egui_span = rts_core_engine::prof_span!("egui run");
    let full_output = s.egui_ctx.run_ui(raw_input, |ui| {
        // The stats bar (its button at the right hides it; the same button
        // alone in the corner brings it back).
        if show_status {
            egui::Panel::bottom("status").show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("⏷").on_hover_text("hide the stats bar").clicked() {
                            show_status = false;
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            // (Cut off short of the button on narrow windows.)
                            let room = ui.max_rect();
                            ui.set_clip_rect(room.intersect(ui.clip_rect()));
                        // Game speed (+/- keys; the pause button), first: the readouts
                        // after it change width as their numbers do, and would
                        // shift it about.
                        let mut speed = control.speed();
                        let mut paused = control.paused();
                        if ui.selectable_label(paused, "pause").clicked() {
                            paused = !paused;
                            control.set_paused(paused);
                        }
                        egui::ComboBox::from_id_salt("speed").selected_text(crate::engine::snapshot::SimControl::label(speed)).show_ui(ui, |ui| {
                            for s in crate::engine::snapshot::SimControl::STEPS {
                                ui.selectable_value(&mut speed, s, crate::engine::snapshot::SimControl::label(s));
                            }
                        });
                        if speed != control.speed() {
                            control.set_speed(speed);
                        }
                        ui.separator();
                        let ms = frame_interval.max(1e-4);
                        let fps = 1.0 / ms;
                        ui.weak(format!("{fps:.0} FPS")).on_hover_text(format!("{:.1} ms between frames", ms * 1000.0));
                        // The work each frame takes, and what limits it.
                        let gpu = if gpu_ms > 0.0 { format!(" gpu {gpu_ms:.1}ms") } else { String::new() };
                        let bound = frame_bound(ms * 1000.0, cpu_ms.max(render_ms), gpu_ms);
                        ui.weak(format!("cpu {cpu_ms:.1}ms (p99 {cpu_p99:.1}) + render {render_ms:.1}ms{gpu}, {bound}"))
                            .on_hover_text("CPU: the main thread's time per frame (input, the interface's Lua), then the render thread's (prepare, encode, submit; not waiting on vsync). GPU: time the GPU spent drawing it. Bound: whichever (or vsync) sets the frame rate.");
                        let mem: Vec<String> = [ram.map(|b| format!("ram {}", super::memory::format_bytes(b))), vram.map(|b| format!("vram {}", super::memory::format_bytes(b)))].into_iter().flatten().collect();
                        if !mem.is_empty() {
                            ui.weak(mem.join(" "));
                        }
                        ui.separator();
                        ui.weak(format!("sim {:.1}ms/tick (p50 {tick_p50:.1} p99 {tick_p99:.1}), {:.1} ticks/s", scene.tick_ms, tick_rate));
                        ui.separator();
                        ui.weak(format!("{}  tick {}", game_clock(scene.tick), scene.tick));
                        ui.separator();
                        let [units, structures, projectiles, props] = scene.entity_counts;
                        ui.weak(format!(
                            "{} units, {} structures, {} projectiles, {} props, {} effects ({} particles)",
                            units,
                            structures,
                            projectiles,
                            props,
                            scene.effect_count(),
                            scene.frame.particles
                        ));
                        ui.separator();
                        let mode = match player.mode {
                            crate::render::player::Mode::Normal => String::new(),
                            crate::render::player::Mode::AttackMove => " | ATTACK-MOVE: click a target".into(),
                            crate::render::player::Mode::Patrol => " | PATROL: click a point".into(),
                        };
                        ui.weak(format!(
                            "{} selected{} | formation {} (F) | {}",
                            player.selection.len(),
                            mode,
                            player.formation_name(),
                            match player.player { Some(a) => format!("army {a}"), None => "observer".into() }
                        ));
                        ui.separator();
                        ui.weak(format!("draws {} models, {} particles, {} interface", scene.frame.model_draws, scene.frame.particle_draws, draws_ui)).on_hover_text("Draw calls last frame: wgpu's command processing (`CommandEncoder::finish`) is paid per draw.");
                        ui.separator();
                        ui.weak(format!("{} log lines", collector.len()));
                        });
                    });
                });
            });
        } else {
            egui::Area::new(egui::Id::new("status toggle")).anchor(egui::Align2::RIGHT_BOTTOM, [-4.0, -4.0]).show(ui.ctx(), |ui| {
                if ui.small_button("⏶").on_hover_text("show the stats bar").clicked() {
                    show_status = true;
                }
            });
        }
        // The debug windows (Ctrl+` shows them).
        if show_debug {
            debug_windows(ui.ctx(), scene, &request, &mut smaa);
            egui::Window::new("Logs").default_open(false).default_size([700.0, 300.0]).show(ui.ctx(), |ui| {
                ui.add(egui_tracing::Logs::new(collector.clone()));
            });
            if let Some(tree) = &debug.vfs_tree {
                egui::Window::new("VFS").default_open(false).default_size([280.0, 500.0]).show(ui.ctx(), |ui| {
                    egui::ScrollArea::both().show(ui, |ui| {
                        egui_ltreeview::TreeView::new(egui::Id::new("vfs_tree")).show(ui, |builder| {
                            for (dname, child) in &tree.dirs {
                                render_dir(builder, dname, child, dname);
                            }
                            for fname in &tree.files {
                                builder.leaf(path_hash(fname), fname.as_str());
                            }
                        });
                    });
                });
            }
        }
    });
    drop(debug);
    let _ = egui_span;

    s.egui_state
        .handle_platform_output(&s.window, full_output.platform_output);

    let tessellate_span = rts_core_engine::prof_span!("egui tessellate");
    let primitives = s
        .egui_ctx
        .tessellate(full_output.shapes, full_output.pixels_per_point);
    let _ = tessellate_span;

    s.settings.smaa = smaa;
    s.show_status = show_status;
    let _ = ui_span;
    // `--screenshot`: save this frame once the sim is far enough along (the
    // main menu: at --screenshot-tick in interface time, once its opening
    // animations are done).
    let capture = match &s.screenshot {
        // Also past the interface's loading screen and opening animations.
        Some((path, tick)) if s.front_end && s.ui.as_ref().is_some_and(|vm| vm.time() > (*tick as f64 / 10.0).max(6.0)) => Some(path.clone()),
        Some((path, tick)) if !s.front_end && s.world.tick >= *tick && s.world.camera().is_some() && (s.ui_failed || s.ui.as_ref().is_some_and(|vm| vm.time() > 6.0)) => Some(path.clone()),
        // Scripted UI tests: the interface asks for this frame
        // (`__engine_screenshot_now = true`), e.g. while a dialog is up and
        // the sim isn't advancing.
        Some((path, _)) if s.ui.as_ref().is_some_and(|vm| vm.lua().globals().raw_get::<bool>("__engine_screenshot_now").unwrap_or(false)) => Some(path.clone()),
        _ => None,
    };
    // With a second head, its frame is saved first; this one once it is.
    let capture = capture.filter(|_| match s.second.as_mut().filter(|h| h.capture.is_some()) {
        Some(h) => {
            h.capture_requested = true;
            h.captured
        }
        None => true,
    });
    if capture.is_some() {
        tracing::info!(effects = s.world.effect_count(), cpu_ms = s.cpu_frame_ms, "screenshot frame (main thread)");
    }
    let capture = capture.map(|path| crate::render::present::Capture { path, exit: true });
    // The second half: the interface's draw list, egui's output.
    let late = crate::render::present::Late {
        input: s.frame_input.clone(),
        ui_items: std::mem::take(&mut s.ui_items),
        ui_scale: s.ui_scale(),
        egui: primitives,
        egui_textures: full_output.textures_delta,
        pixels_per_point: full_output.pixels_per_point,
        capture,
    };
    s.presenter.late(late);
    let cpu = frame_start.elapsed().as_secs_f32() * 1000.0;
    s.cpu_frame_ms += (cpu - s.cpu_frame_ms) * 0.1;
    s.cpu_times.push(cpu);
    crate::engine::tracy::frame_mark();
}

/// What a frame worked out, for the main thread: its entities and icons'
/// rectangles (picking, selection, the interface's units), its timings.
fn take_present_out(s: &mut RenderState, mut out: crate::render::present::PresentOut) {
    if let Some(r) = out.results.take() {
        s.world.frame = r;
    }
    s.world.set_icon_rects(0, std::mem::take(&mut out.icon_rects));
    s.world.add_mesh_bounds(&std::mem::take(&mut out.mesh_bounds));
    s.gpu_frame_ms = out.gpu_ms;
    s.present_out = out;
}

/// What limits the frame rate: the CPU's or GPU's work per frame when it
/// fills the frame interval, else vsync (both finish with time to spare).
fn frame_bound(interval_ms: f32, cpu_ms: f32, gpu_ms: f32) -> &'static str {
    let work = cpu_ms.max(gpu_ms);
    if work < interval_ms * 0.85 {
        "vsync bound"
    } else if gpu_ms > cpu_ms {
        "gpu bound"
    } else {
        "cpu bound"
    }
}


/// In-game time for a sim tick (10 ticks per game second), as SupCom shows
/// it: `HH:MM:SS`.
fn game_clock(tick: u64) -> String {
    let secs = tick / 10;
    format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

#[cfg(test)]
mod clock_tests {
    #[test]
    fn frame_bound_names_the_limit() {
        assert_eq!(super::frame_bound(16.7, 6.0, 9.0), "vsync bound");
        assert_eq!(super::frame_bound(33.0, 31.0, 12.0), "cpu bound");
        assert_eq!(super::frame_bound(33.0, 10.0, 30.0), "gpu bound");
    }

    /// A followed group's middle counts the entities still there and
    /// forgets the rest; none left, nothing to follow.
    #[test]
    fn middle_follows_the_entities_left() {
        use rts_core_engine::engine::snapshot::{EntityView, Sighting, ViewKind};
        let at = |id, x, z| EntityView { id, kind: ViewKind::Unit, army: 1, position: [x, 0.0, z], rotation: [0.0, 0.0, 0.0, 1.0], center: [0.0; 3], half: [1.0; 3], health: 1.0, build: 1.0, work: 0.0, shield: -1.0, blueprint: 0, mesh: 0, scale: [1.0; 3], pose_start: 0, pose_len: 0, sighting: Sighting::Seen, in_builder: false, attached: false, scroll: [0.0; 4], order_flags: 0 };
        let entities = [at(2, 10.0, 20.0), at(5, 30.0, 40.0)];
        let mut ids = vec![2, 4, 5];
        assert_eq!(super::middle(&entities, &mut ids), Some([20.0, 30.0]));
        assert_eq!(ids, [2, 5]);
        let mut gone = vec![1, 3];
        assert_eq!(super::middle(&entities, &mut gone), None);
        assert!(gone.is_empty());
    }

    #[test]
    fn game_clock_formats_hours_minutes_seconds() {
        assert_eq!(super::game_clock(0), "00:00:00");
        assert_eq!(super::game_clock(9), "00:00:00");
        assert_eq!(super::game_clock(12_000), "00:20:00");
        assert_eq!(super::game_clock(36_615), "01:01:01");
    }
}

#[cfg(test)]
mod projectile_follow_tests {
    use super::{start_follows, FollowStep};
    use rts_core_engine::engine::sim::presentation::CameraFollow;

    /// A camera following a launcher switches to its projectile, follows
    /// it, holds `CameraFollowTimeout` seconds where it ended, then goes
    /// back; one following something else is left alone.
    #[test]
    fn a_tracked_launcher_hands_the_camera_to_its_projectile_and_back() {
        let mut tracks = vec![("WorldCamera".to_owned(), vec![7]), ("Other".to_owned(), vec![9])];
        let mut follows = Vec::new();
        start_follows(&mut tracks, &mut follows, &[CameraFollow { launcher: 7, projectile: 40, timeout: 2.0 }]);
        assert_eq!(tracks[0].1, vec![40]);
        assert_eq!(tracks[1].1, vec![9], "not following the launcher");
        assert_eq!(follows.len(), 1);
        let f = &mut follows[0];
        let ids = &mut tracks[0].1;
        // Not in the frame yet: wait.
        assert_eq!(f.step(ids, false, 0.1), FollowStep::Waiting);
        assert_eq!(f.step(ids, true, 0.5), FollowStep::Following);
        // Gone: hold 2 s (frames of 0.5 s), then back to the launcher.
        assert_eq!(f.step(ids, false, 0.5), FollowStep::Holding);
        assert_eq!(f.step(ids, false, 0.5), FollowStep::Holding);
        assert_eq!(f.step(ids, false, 0.5), FollowStep::Holding);
        assert_eq!(f.step(ids, false, 0.5), FollowStep::Back);
        assert_eq!(*ids, vec![7]);
        // Not following its launcher: nothing changes.
        let mut tracks = vec![("WorldCamera".to_owned(), vec![3])];
        let mut follows = Vec::new();
        start_follows(&mut tracks, &mut follows, &[CameraFollow { launcher: 7, projectile: 40, timeout: 2.0 }]);
        assert_eq!((tracks[0].1.clone(), follows.len()), (vec![3], 0));
    }
}
