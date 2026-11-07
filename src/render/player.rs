//! The player's hands: selection and orders (SupCom's controls).
//!
//! Lives on the render thread and never touches sim state. It reads the
//! interpolated entities the renderer draws and sends
//! [`PlayerCommand`]s down the sim's command stream (`sim::input`), which
//! applies them at a later tick like any lockstep input.
//!
//! - Left click selects (shift adds / removes), left drag box-selects,
//!   double-click selects every unit of that type on screen.
//! - Right click gives the whole selection one order, by what is under the
//!   cursor and what the selection can do together (moho's, see
//!   `selection_order`): attack an enemy, capture, reclaim, load, repair a
//!   unit being built, guard a friendly unit, else move. Every unit
//!   beelines for the target (FAF's UI only ever sends `RULEUCC_Move`):
//!   hand-issued orders never form up. Shift queues.
//! - Ctrl+0-9 sets a control group, 0-9 recalls it, pressed twice it centres the camera.
//! - H stops; T and P arm attack-move and patrol for the next click (Alt+right
//!   click is attack-move too); Esc cancels or clears.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use glam::Vec3;
use winit::keyboard::KeyCode;

use crate::engine::sim::command::{CommandKind, CommandTarget};
use crate::engine::sim::input::PlayerCommand;
use crate::engine::sim::math::FxVec3;
use crate::engine::snapshot::{caps, EntityView, ViewKind};
use crate::render::world::World;

/// Formations F cycles through (`formations.lua` names; `None` = none).
/// Unused for now: hand-issued orders always go out plain (FAF's UI only
/// ever sends `RULEUCC_Move`), so this is reserved for a future formation
/// control. Kept (with the `PlayerCommand` fields) so the replay format is
/// stable.
pub const FORMATIONS: [Option<&str>; 3] = [Some("AttackFormation"), Some("GrowthFormation"), None];

/// Pixels a press must move to count as a drag.
const DRAG_PIXELS: f64 = 8.0;
/// How long the right button is held before moho's formation preview
/// starts (its countdown at the session's formation-drag object +0x5c,
/// reset to 0.5 at 0x10400040 and run down by the system timer at
/// 0x104007c0); from then the right-click order offers no guard
/// (0x103e816c).
const FORMATION_PREVIEW_DELAY: Duration = Duration::from_millis(500);
const DOUBLE: Duration = Duration::from_millis(350);
/// Units of a blueprint with no caps (unknown): what most units can do.
const DEFAULT_CAPS: u16 = caps::MOVE | caps::ATTACK | caps::STOP | caps::GUARD | caps::PATROL;

/// Mouse / key input, in event order (cursor in physical pixels).
#[derive(Clone, Copy, Debug)]
pub enum Event {
    LeftDown(f64, f64),
    LeftUp(f64, f64),
    RightDown(f64, f64),
    RightUp(f64, f64),
    /// The cursor moved (the interface's hover; ignored by orders).
    Move(f64, f64),
    Key(KeyCode),
    /// A held key repeating (only an interface edit takes these).
    KeyRepeat(KeyCode),
    /// A character typed (for an interface edit).
    Char(char),
    /// The wheel turned over the interface (cursor, notches).
    Wheel(f64, f64, f32),
}

/// Keyboard modifiers when an event happened.
#[derive(Clone, Copy, Debug, Default)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// An order armed by a hotkey, given by the next click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    AttackMove,
    Patrol,
}

/// A flash where an order was given.
#[derive(Clone)]
pub struct Marker {
    pub at: Vec3,
    pub kind: CommandKind,
    pub when: Instant,
}

/// A queued order's waypoint being dragged in the command graph (moho's
/// `UICommandDragger`): a left press on a waypoint, moved, then let go.
#[derive(Clone, Debug)]
pub struct CommandDrag {
    /// A unit on the order (its army sends the move).
    pub owner: u32,
    pub marker: crate::engine::snapshot::OrderMarker,
    /// Where the press was (screen px).
    pub press: (f64, f64),
    /// Moved past [`DRAG_PIXELS`] (a press that never does moves nothing).
    pub moved: bool,
    /// Where it would go now, and its node there; `None` where it can't (an
    /// order on a unit, off any unit of that unit's army).
    pub to: Option<(CommandTarget, Vec3)>,
}

impl CommandDrag {
    pub fn new(owner: u32, marker: crate::engine::snapshot::OrderMarker, press: (f64, f64)) -> Self {
        Self { owner, marker, press, moved: false, to: None }
    }

    /// The cursor is at `at`: past [`DRAG_PIXELS`] from the press it's a drag.
    pub fn cursor_at(&mut self, at: (f64, f64)) {
        self.moved |= (at.0 - self.press.0).hypot(at.1 - self.press.1) > DRAG_PIXELS;
    }
}

/// `marker` with its waypoint at `node` (a drag's preview): a formation
/// slot keeps its place round it, a build site moves with it.
pub fn moved_marker(marker: &crate::engine::snapshot::OrderMarker, node: [f32; 3]) -> crate::engine::snapshot::OrderMarker {
    let d = [node[0] - marker.node[0], node[1] - marker.node[1], node[2] - marker.node[2]];
    crate::engine::snapshot::OrderMarker {
        at: if marker.target.is_some() { node } else { [marker.at[0] + d[0], marker.at[1] + d[1], marker.at[2] + d[2]] },
        node,
        site: marker.site.map(|[x0, z0, x1, z1]| [x0 + d[0], z0 + d[2], x1 + d[0], z1 + d[2]]),
        ..marker.clone()
    }
}

pub struct PlayerControl {
    /// Army commanded; `None` = observer (any army's units).
    pub player: Option<u32>,
    /// Selected entity ids, sorted.
    pub selection: Vec<u32>,
    groups: [Vec<u32>; 10],
    pub mode: Mode,
    /// Index into [`FORMATIONS`]. Reserved: the sim ignores it for
    /// hand-issued orders (they always go out plain).
    pub formation: usize,
    /// Left press (box selection start), screen px.
    pub left_press: Option<(f64, f64)>,
    /// Right press: screen px and the ground under it (order target).
    pub right_press: Option<((f64, f64), Vec3)>,
    /// When the right button went down: held [`FORMATION_PREVIEW_DELAY`]
    /// it's a formation drag, which offers no guard.
    right_pressed_at: Option<Instant>,
    /// The profile's `switch_right_click_behavior` (moho's
    /// `OPTIONS_GetBool`, MohoEngine.dll 0x103e8072): repair and guard of a
    /// friendly unit asked the other way round.
    pub switch_right_click: bool,
    last_click: Option<(Instant, u32)>,
    last_group: Option<(Instant, usize)>,
    orders: Option<Sender<PlayerCommand>>,
    pub markers: Vec<Marker>,
    /// Camera centre requested by a double-tapped group (x, z).
    pub focus: Option<(f32, f32)>,
    /// Orders sent since the frame loop last took them (the interface hears
    /// each: FAF's `OnCommandIssued`).
    pub issued: Vec<Issued>,
}

/// An order the player gave, as the interface is told of it.
#[derive(Clone, Debug)]
pub struct Issued {
    pub kind: CommandKind,
    pub target: CommandTarget,
    /// Shift-queued (FAF's `Clear` is the opposite).
    pub queue: bool,
    pub blueprint: Option<String>,
    pub units: Vec<u32>,
    /// A script order's `LuaParams` (`TaskName`, `Enhancement`, ...).
    pub script: Vec<(String, String)>,
}

fn sorted(mut v: Vec<u32>) -> Vec<u32> {
    v.sort_unstable();
    v.dedup();
    v
}

/// `p` moved so a `(sx, sz)` footprint centred on it covers whole cells (as
/// structures are placed), on the ground there.
pub fn snap_to_footprint(scene: &World, p: Vec3, (sx, sz): (f32, f32)) -> Vec3 {
    let (x, z) = (snap_centre(p.x, sx), snap_centre(p.z, sz));
    Vec3::new(x, scene.surface_at(x, z), z)
}

/// The selection's routes a right click on waypoint `command` (of kind
/// `kind`) would turn into patrols, grouped by route; `None` when moho
/// wouldn't offer it: the waypoint is a Move / FormMove, and every selected
/// unit has 2+ orders, all of that kind, the hovered one not its last.
pub fn patrol_routes(selection: &[u32], orders: &[(u32, Vec<crate::engine::snapshot::OrderMarker>)], kind: u8, command: u32) -> Option<Vec<(Vec<u32>, Vec<Vec3>)>> {
    if !matches!(CommandKind::from_u8(kind), Some(CommandKind::Move | CommandKind::FormMove)) || selection.is_empty() {
        return None;
    }
    let mut groups: Vec<(Vec<u32>, Vec<Vec3>)> = Vec::new();
    for &id in selection {
        let marks = orders.iter().find(|(u, _)| *u == id).map(|(_, m)| m.as_slice()).unwrap_or(&[]);
        if marks.len() < 2 || marks.iter().any(|m| m.kind != kind) || marks.last().is_some_and(|m| m.command == command) {
            return None;
        }
        let route: Vec<Vec3> = marks.iter().map(|m| Vec3::from_array(m.node)).collect();
        match groups.iter_mut().find(|(_, r)| *r == route) {
            Some((units, _)) => units.push(id),
            None => groups.push((vec![id], route)),
        }
    }
    Some(groups)
}

/// Drag building (a `DRAGBUILD` structure dragged out in build mode): from
/// `start` towards `end`, one structure every `spacing` (its skirt) along
/// the line (the shortest step that clears the last one on either axis, so
/// diagonals touch at the corners), each snapped to the footprint grid.
/// Always at least `start`; at most [`DRAG_BUILD_MAX`].
pub fn drag_line(scene: &World, start: Vec3, end: Vec3, footprint: (f32, f32), spacing: (f32, f32)) -> Vec<Vec3> {
    drag_spots(start, end, footprint, spacing).into_iter().map(|(x, z)| Vec3::new(x, scene.surface_at(x, z), z)).collect()
}

/// The most structures one drag lays.
pub const DRAG_BUILD_MAX: usize = 100;

fn drag_spots(start: Vec3, end: Vec3, (fx, fz): (f32, f32), (sx, sz): (f32, f32)) -> Vec<(f32, f32)> {
    let first = (snap_centre(start.x, fx), snap_centre(start.z, fz));
    let (dx, dz) = (end.x - start.x, end.z - start.z);
    let length = (dx * dx + dz * dz).sqrt();
    let mut spots = vec![first];
    if length < 1e-3 {
        return spots;
    }
    let (ux, uz) = (dx / length, dz / length);
    let along = |size: f32, u: f32| if u.abs() > 1e-4 { size / u.abs() } else { f32::INFINITY };
    let step = along(sx, ux).min(along(sz, uz));
    let count = ((length / step).floor() as usize + 1).min(DRAG_BUILD_MAX);
    for i in 1..count {
        let t = step * i as f32;
        // From the first spot, so snapping never eats into the spacing.
        let spot = (snap_centre(first.0 + ux * t, fx), snap_centre(first.1 + uz * t, fz));
        if spots.last() != Some(&spot) {
            spots.push(spot);
        }
    }
    spots
}

/// The centre nearest `v` of a `size`-wide span starting on a whole cell.
fn snap_centre(v: f32, size: f32) -> f32 {
    (v - size / 2.0).round() + size / 2.0
}

/// Whole degrees of the heading from `from` to `to` (0 = south / +z, 90 =
/// east / +x), the sim's formation facing convention.
pub fn facing_degrees(from: Vec3, to: Vec3) -> i32 {
    (to.x - from.x).atan2(to.z - from.z).to_degrees().round() as i32
}

/// How a session ends (`frame::close_session`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEnd {
    /// The same game again: `RestartSession`, or a replay seek.
    Restart,
    /// Back to the front end (`ExitGame`): the next game is a new lobby's.
    FrontEnd,
}

impl PlayerControl {
    pub fn new(player: Option<u32>, select: Vec<u32>, orders: Option<Sender<PlayerCommand>>) -> Self {
        Self {
            player,
            selection: select,
            groups: Default::default(),
            mode: Mode::Normal,
            formation: 0,
            left_press: None,
            right_press: None,
            right_pressed_at: None,
            switch_right_click: false,
            last_click: None,
            last_group: None,
            orders,
            markers: Vec::new(),
            focus: None,
            issued: Vec::new(),
        }
    }

    /// The session ended: nothing selected, grouped, pressed or marked
    /// carries over into the next (the order link stays). The army stays
    /// for the same game again; a new lobby's game takes the army the sim
    /// gives this client (`adopt_army`).
    pub fn end_session(&mut self, end: SessionEnd) {
        let player = match end {
            SessionEnd::Restart => self.player,
            SessionEnd::FrontEnd => None,
        };
        *self = Self { player, orders: self.orders.take(), ..Self::new(None, Vec::new(), None) };
    }

    /// The interface starts: a lobby's game commands the army the sim gave
    /// this client (a replay's recorder's army isn't the viewer's); an army
    /// already set (`--player`, kept over a restart) stays.
    pub fn adopt_army(&mut self, local_army: Option<u32>, replay: bool) {
        if self.player.is_none() && !replay {
            self.player = local_army;
        }
    }

    pub fn formation_name(&self) -> &'static str {
        FORMATIONS[self.formation].unwrap_or("NoFormation")
    }

    fn caps_of(scene: &World, e: &EntityView) -> u16 {
        match scene.blueprint_caps.as_ref().and_then(|c| c.get(e.blueprint as usize)) {
            Some(&c) if c != 0 => c,
            _ => DEFAULT_CAPS,
        }
    }

    /// Cargo-slot classes of `e`'s model (bit `class - 1`): where the sim's
    /// `free_slots` finds a bone to board into.
    fn slot_mask(scene: &World, e: &EntityView) -> u8 {
        scene.blueprint_transport.as_ref().and_then(|t| t.get(e.blueprint as usize)).map_or(0, |&(s, _, _)| s)
    }

    /// `e`'s `Transport.ClassGenericUpTo`.
    fn generic_up_to(scene: &World, e: &EntityView) -> u8 {
        scene.blueprint_transport.as_ref().and_then(|t| t.get(e.blueprint as usize)).map_or(0, |&(_, _, g)| g)
    }

    /// The entity's cargo class (its blueprint's `Transport.TransportClass`,
    /// 1 when unset).
    fn transport_class(scene: &World, e: &EntityView) -> u8 {
        scene.blueprint_transport.as_ref().and_then(|t| t.get(e.blueprint as usize)).map_or(1, |&(_, c, _)| c)
    }

    /// Drag-selection priority (1 = highest): the blueprint's
    /// `SelectionPriority`, or 6 for a `LOWSELECTPRIO` unit still being
    /// built (FAF's exe patch, `hooks/selectionPriority.cpp`).
    fn selection_priority(scene: &World, e: &EntityView) -> u8 {
        let v = scene.blueprint_selection_priority.as_ref().and_then(|v| v.get(e.blueprint as usize)).copied().unwrap_or(1);
        if v & 0x80 != 0 && e.build < 1.0 { 6 } else { (v & 0x7f).max(1) }
    }

    fn view(scene: &World, id: u32) -> Option<&EntityView> {
        scene.entities().binary_search_by_key(&id, |e| e.id).ok().map(|i| &scene.entities()[i])
    }

    /// Can the player give orders to `e`?
    fn commandable(&self, e: &EntityView) -> bool {
        matches!(e.kind, ViewKind::Unit | ViewKind::Structure) && !e.in_builder && e.army != 0 && self.player.is_none_or(|p| p == e.army)
    }

    /// Can the player give orders to unit `id` (an ally's waypoints show,
    /// but aren't the player's to drag)?
    pub fn commands(&self, scene: &World, id: u32) -> bool {
        Self::view(scene, id).is_some_and(|e| self.commandable(e))
    }

    /// Structure `from` finished upgrading into `to`: selected or grouped,
    /// the new structure takes its place (moho moves the selection over).
    pub fn upgraded(&mut self, from: u32, to: u32) {
        for ids in std::iter::once(&mut self.selection).chain(self.groups.iter_mut()) {
            if let Some(i) = ids.iter().position(|&id| id == from) {
                ids[i] = to;
                *ids = sorted(std::mem::take(ids));
            }
        }
    }

    /// Drops selected / grouped ids that are gone, and from the selection
    /// units still being built in a factory (however they got there: the
    /// interface's `SelectUnits`, a hotkey, a control group).
    pub fn prune(&mut self, scene: &World) {
        if scene.entities().is_empty() {
            return;
        }
        // (A killed unit still falling or sinking is gone too.)
        let alive = |id: &u32| Self::view(scene, *id).is_some_and(|e| e.kind.is_unit());
        self.selection.retain(|id| Self::view(scene, *id).is_some_and(|e| e.kind.is_unit() && !e.in_builder));
        for g in &mut self.groups {
            g.retain(alive);
        }
        let now = Instant::now();
        self.markers.retain(|m| now.duration_since(m.when) < Duration::from_millis(700));
    }

    pub fn handle(&mut self, scene: &mut World, ev: Event, mods: Mods, cursor: Option<(f64, f64)>) {
        match ev {
            Event::LeftDown(x, y) => {
                if self.mode != Mode::Normal && !self.selection.is_empty() {
                    let kind = if self.mode == Mode::AttackMove { CommandKind::AggressiveMove } else { CommandKind::Patrol };
                    self.order_at(scene, (x, y), None, Some(kind), mods.shift, false);
                    if !mods.shift {
                        self.mode = Mode::Normal;
                    }
                } else {
                    self.left_press = Some((x, y));
                }
            }
            Event::LeftUp(x, y) => {
                let Some(start) = self.left_press.take() else { return };
                if (x - start.0).hypot(y - start.1) > DRAG_PIXELS {
                    self.box_select(scene, start, (x, y), mods.shift);
                } else {
                    self.click_select(scene, (x, y), mods.shift);
                }
            }
            Event::RightDown(x, y) => {
                if self.mode != Mode::Normal {
                    // Right click cancels an armed order, as in SupCom.
                    self.mode = Mode::Normal;
                    return;
                }
                if self.selection.is_empty() {
                    return;
                }
                if let Some(g) = scene.ground_at(x as f32, y as f32) {
                    self.right_press = Some(((x, y), g));
                    self.right_pressed_at = Some(Instant::now());
                }
            }
            Event::RightUp(x, y) => {
                let Some((start, ground)) = self.right_press.take() else { return };
                let facing = ((x - start.0).hypot(y - start.1) > DRAG_PIXELS * 2.0)
                    .then(|| scene.ground_at(x as f32, y as f32))
                    .flatten()
                    .map(|to| facing_degrees(ground, to));
                let forced = mods.alt.then_some(CommandKind::AggressiveMove);
                let held = self.right_pressed_at.take().is_some_and(|t| t.elapsed() >= FORMATION_PREVIEW_DELAY);
                self.order_at(scene, start, facing, forced, mods.shift, held);
            }
            Event::Key(k) => self.key(scene, k, mods, cursor),
            Event::Move(..) | Event::KeyRepeat(_) | Event::Char(_) | Event::Wheel(..) => {}
        }
    }

    fn key(&mut self, scene: &World, k: KeyCode, mods: Mods, _cursor: Option<(f64, f64)>) {
        let digit = match k {
            KeyCode::Digit0 => Some(0),
            KeyCode::Digit1 => Some(1),
            KeyCode::Digit2 => Some(2),
            KeyCode::Digit3 => Some(3),
            KeyCode::Digit4 => Some(4),
            KeyCode::Digit5 => Some(5),
            KeyCode::Digit6 => Some(6),
            KeyCode::Digit7 => Some(7),
            KeyCode::Digit8 => Some(8),
            KeyCode::Digit9 => Some(9),
            _ => None,
        };
        if let Some(d) = digit {
            if mods.ctrl {
                self.groups[d] = self.selection.clone();
            } else if mods.shift {
                self.selection = sorted([self.selection.clone(), self.groups[d].clone()].concat());
            } else {
                let now = Instant::now();
                let again = self.last_group.is_some_and(|(t, g)| g == d && now.duration_since(t) < DOUBLE * 2);
                self.selection = self.groups[d].clone();
                if again {
                    self.focus = self.centre(scene, &self.groups[d]);
                }
                self.last_group = Some((now, d));
            }
            return;
        }
        match k {
            KeyCode::Escape => {
                if self.mode != Mode::Normal {
                    self.mode = Mode::Normal;
                } else {
                    self.selection.clear();
                }
            }
            KeyCode::KeyH => {
                let units = self.selection.clone();
                self.send_split(scene, &units, CommandKind::Stop, CommandTarget::None, false, None, None, None, 0);
            }
            KeyCode::KeyT if !self.selection.is_empty() => self.mode = Mode::AttackMove,
            KeyCode::KeyP if !self.selection.is_empty() => self.mode = Mode::Patrol,
            KeyCode::KeyF => self.formation = (self.formation + 1) % FORMATIONS.len(),
            _ => {}
        }
    }

    /// Mean position of the live units among `ids`.
    fn centre(&self, scene: &World, ids: &[u32]) -> Option<(f32, f32)> {
        let pts: Vec<[f32; 3]> = ids.iter().filter_map(|&id| Self::view(scene, id)).map(|e| e.position).collect();
        (!pts.is_empty()).then(|| {
            let n = pts.len() as f32;
            (pts.iter().map(|p| p[0]).sum::<f32>() / n, pts.iter().map(|p| p[2]).sum::<f32>() / n)
        })
    }

    fn click_select(&mut self, scene: &World, at: (f64, f64), add: bool) {
        let picked = scene.pick(at.0 as f32, at.1 as f32).and_then(|id| Self::view(scene, id)).filter(|e| matches!(e.kind, ViewKind::Unit | ViewKind::Structure));
        let Some(e) = picked.copied() else {
            if !add {
                self.selection.clear();
            }
            return;
        };
        let now = Instant::now();
        let double = self.last_click.is_some_and(|(t, id)| id == e.id && now.duration_since(t) < DOUBLE);
        self.last_click = Some((now, e.id));
        if double {
            // Every unit of that type and army in the view clicked.
            let same: Vec<u32> = self.on_screen(scene, at).filter(|o| o.blueprint == e.blueprint && o.army == e.army && !o.in_builder).map(|o| o.id).collect();
            self.selection = if add { sorted([self.selection.clone(), same].concat()) } else { sorted(same) };
        } else if add {
            match self.selection.binary_search(&e.id) {
                Ok(i) => {
                    self.selection.remove(i);
                }
                Err(i) => self.selection.insert(i, e.id),
            }
        } else {
            self.selection = vec![e.id];
        }
    }

    /// Entities whose position projects inside the world view at window
    /// point `at`.
    fn on_screen<'a>(&'a self, scene: &'a World, at: (f64, f64)) -> impl Iterator<Item = &'a EntityView> + 'a {
        let view = scene.view_at(at.0 as f32, at.1 as f32);
        scene.entities().iter().filter(move |e| {
            view.and_then(|(v, c)| Some((v, v.project(c, Vec3::from_array(e.position))?))).is_some_and(|(v, [x, y])| x >= v.rect[0] && y >= v.rect[1] && x <= v.rect[2] && y <= v.rect[3])
        })
    }

    /// Box select: the commandable units inside of the highest selection
    /// priority among them (moho buckets them by `SelectionPriority`, 1
    /// first, and keeps the first non-empty bucket: MohoEngine.dll
    /// 0x10428a72). With FAF's values: most units 1, transports 2,
    /// engineers / ACU / SACUs 3, structures 5, drones 6. Adding with shift
    /// takes everything in the box. An observer gets one army: the one with
    /// the most units in the box.
    fn box_select(&mut self, scene: &World, a: (f64, f64), b: (f64, f64), add: bool) {
        // The box is in the view it was started in, and ends at its edge.
        let Some((view, cam)) = scene.view_at(a.0 as f32, a.1 as f32) else { return };
        let [l, t, r, bottom] = view.rect;
        let (x0, x1) = ((a.0.min(b.0) as f32).max(l), (a.0.max(b.0) as f32).min(r));
        let (y0, y1) = ((a.1.min(b.1) as f32).max(t), (a.1.max(b.1) as f32).min(bottom));
        // Moho's drag test (`render::selection_box`): the units whose mesh
        // box, scaled by `SelectionMeshScaleX/Y/Z`, meets the volume the box
        // sees, not only those whose centre is in it.
        use super::selection_box::{drag_box, meets, rect_volume, unit_box};
        let ([lx0, ly0], [lx1, ly1], [w, h]) = (view.local(x0, y0), view.local(x1, y1), view.size());
        let ndc = |x: f32, y: f32| (x / w.max(1.0) * 2.0 - 1.0, 1.0 - y / h.max(1.0) * 2.0);
        let ((nx0, ny0), (nx1, ny1)) = (ndc(lx0, ly0), ndc(lx1, ly1));
        let volume = rect_volume(cam.view_proj().inverse(), [nx0, ny0, nx1, ny1]);
        let picks = scene.blueprint_pick.as_deref();
        let inside: Vec<&EntityView> = scene
            .entities()
            .iter()
            .filter(|e| self.commandable(e))
            .filter(|e| {
                let bounds = scene.mesh_bounds.get(e.mesh as usize).copied().flatten();
                let pick = picks.and_then(|p| p.get(e.blueprint as usize).copied()).unwrap_or([0.0, 0.5, 1.0, 1.0, 1.0]);
                meets(&drag_box(&unit_box(e.position, e.rotation, e.scale, e.center, e.half, bounds), pick), &volume)
            })
            .collect();
        let mut pick = inside;
        if !add
            && let Some(best) = pick.iter().map(|e| Self::selection_priority(scene, e)).min() {
            pick.retain(|e| Self::selection_priority(scene, e) == best);
        }
        if self.player.is_none() {
            let mut counts = std::collections::BTreeMap::<u32, usize>::new();
            for e in &pick {
                *counts.entry(e.army).or_default() += 1;
            }
            // Most units; ties to the lowest army.
            if let Some((&army, _)) = counts.iter().max_by_key(|(a, n)| (**n, std::cmp::Reverse(**a))) {
                pick.retain(|e| e.army == army);
            }
        }
        let ids: Vec<u32> = pick.iter().map(|e| e.id).collect();
        self.selection = if add { sorted([self.selection.clone(), ids].concat()) } else { sorted(ids) };
    }

    /// The order a right click at `at` would give (moho's
    /// `GetRightMouseButtonOrder`, which FAF's cursor follows): an order on
    /// the entity there if any selected unit would give one, else the first
    /// unit's order to the ground.
    pub fn right_click_order(&self, scene: &World, at: (f64, f64)) -> Option<CommandKind> {
        let target = scene.pick(at.0 as f32, at.1 as f32).and_then(|id| Self::view(scene, id)).copied();
        let units: Vec<EntityView> = self.selection.iter().filter_map(|&id| Self::view(scene, id)).filter(|e| self.commandable(e)).copied().collect();
        let held = self.right_pressed_at.is_some_and(|t| t.elapsed() >= FORMATION_PREVIEW_DELAY);
        self.selection_order(scene, &units, target.as_ref(), held).map(|o| o.0)
    }

    /// `to`'s standing with army `from` (an army is its own ally; unset
    /// pairs are hostile, as the sim defaults); `None` for no army.
    fn order_categories(scene: &World, e: &EntityView) -> u16 {
        scene.blueprint_order_categories.as_ref().and_then(|c| c.get(e.blueprint as usize)).copied().unwrap_or(0)
    }

    /// Whether `transport` can carry `unit`: the shared rule
    /// ([`crate::engine::sim::carry`]) from the blueprints (classes from
    /// the model's attach points; air staging takes any class).
    fn can_carry(scene: &World, unit: &EntityView, transport: &EntityView) -> bool {
        use crate::engine::sim::carry::{Carrier, Passenger};
        use crate::engine::snapshot::order_categories as oc;
        let (uc, tc) = (Self::order_categories(scene, unit), Self::order_categories(scene, transport));
        let passenger = Passenger { mobile: unit.kind == ViewKind::Unit, air: uc & oc::AIR != 0, commander: uc & oc::COMMAND != 0, class: Self::transport_class(scene, unit) };
        let carrier = Carrier {
            transport: Self::caps_of(scene, transport) & caps::TRANSPORT != 0,
            staging: tc & oc::AIRSTAGINGPLATFORM != 0, carries_commander: tc & oc::CANTRANSPORTCOMMANDER != 0, slot_classes: Self::slot_mask(scene, transport), generic_up_to: Self::generic_up_to(scene, transport), storage: false };
        crate::engine::sim::carry::can_carry(&passenger, &carrier)
    }

    fn alliance(scene: &World, from: u32, to: u32) -> Option<crate::engine::sim::army::Alliance> {
        use crate::engine::sim::army::Alliance;
        if to == 0 {
            return None;
        }
        if from == to {
            return Some(Alliance::Ally);
        }
        let list = scene.alliances.as_deref().map_or([].as_slice(), Vec::as_slice);
        Some(list.iter().find(|&&(f, t, _)| f == from && t == to).map_or(Alliance::Enemy, |&(_, _, a)| a))
    }

    /// moho's right-click order for the whole selection (MohoEngine.dll
    /// 0x103e7a40, behind `GetRightMouseButtonOrder` 0x104365e0): one
    /// order type from the target and the selection's command caps together
    /// (each unit's OR'd), and whether it's on the target entity (else the
    /// ground). The interface sends it to every selected unit (0x103e89c0,
    /// the bit to a command type by 0x103ea8b0) and the sim refuses it to
    /// those that can't take it, which keep their queues
    /// (`command::refuses`). In order, on an entity:
    /// - an enemy, with Attack, that some selected unit can attack -> Attack;
    /// - not allied, not being built, capturable, not carried, with Capture
    ///   -> Capture;
    /// - reclaimable (`RECLAIMABLE` or being built, not carried) with
    ///   Reclaim: an enemy's or one with no army, else a `RECLAIMFRIENDLY`
    ///   one -> Reclaim;
    /// - the player's own unit: one a selected unit can board, with
    ///   CallTransport -> load; one with CallTransport that a selected
    ///   transport can carry -> the transport picks it up;
    /// - not an enemy: Repair a unit being built (not a static `FACTORY`),
    ///   then Guard (not a `CAMPAIGNGATE`, not when the selection is only
    ///   the unit, not once the right button has been held into a formation
    ///   drag), then Repair a damaged ally or an upgrading unit;
    ///   `switch_right_click_behavior`: Repair a damaged ally or upgrading
    ///   unit, else Guard.
    /// Else, and on the ground, Move. An enemy none can attack is no order
    /// in moho; ours counts any unit with Attack as able (moho asks each
    /// unit's weapons, 0x10482530).
    fn selection_order(&self, scene: &World, units: &[EntityView], target: Option<&EntityView>, held: bool) -> Option<(CommandKind, bool)> {
        use crate::engine::sim::army::Alliance;
        use crate::engine::snapshot::{order_categories as oc, order_flags as of};
        let all = units.iter().fold(0u16, |acc, u| acc | Self::caps_of(scene, u));
        let viewer = self.player.or(units.first().map(|u| u.army))?;
        let move_order = (all & caps::MOVE != 0).then_some((CommandKind::Move, false));
        let Some(t) = target.filter(|t| matches!(t.kind, ViewKind::Unit | ViewKind::Structure | ViewKind::Prop)) else { return move_order };
        let on = |kind| Some((kind, true));
        let is_unit = t.kind != ViewKind::Prop;
        // (Props have no blueprint index here: taken as `RECLAIMABLE`.)
        let cats = if is_unit { Self::order_categories(scene, t) } else { oc::RECLAIMABLE };
        let alliance = Self::alliance(scene, viewer, t.army);
        let (enemy, ally) = (alliance == Some(Alliance::Enemy), alliance == Some(Alliance::Ally));
        let being_built = t.build < 1.0;
        let carried = is_unit && t.attached;
        if enemy && all & caps::ATTACK != 0 {
            return on(CommandKind::Attack);
        }
        let capturable = is_unit && !carried && t.order_flags & of::CAPTURABLE != 0;
        if !ally && !being_built && capturable && all & caps::CAPTURE != 0 {
            return on(CommandKind::Capture);
        }
        let reclaimable = !carried && (cats & oc::RECLAIMABLE != 0 || being_built) && all & caps::RECLAIM != 0;
        let reclaim = if !enemy && t.army != 0 { cats & oc::RECLAIMFRIENDLY != 0 && reclaimable } else { reclaimable };
        if reclaim {
            return on(CommandKind::Reclaim);
        }
        if is_unit && t.army == viewer {
            let tc = Self::caps_of(scene, t);
            // Load: moho's 0x103e61d0, a selected unit (alive, finished)
            // boarding the target by its categories (a `TRANSPORTATION` /
            // `TELEPORTATION` one takes non-air units, `AIRSTAGINGPLATFORM`
            // air units; a `COMMAND` unit only a `CANTRANSPORTCOMMANDER`
            // one), and the transport able to carry it (`sim::carry`, the
            // rule the sim's order check and boarding use).
            // A ferry beacon: any non-air unit waits there for the ferry
            // (a commander too), with nothing to carry it yet.
            let beacon = cats & oc::FERRYBEACON != 0;
            let boards = |u: &EntityView| {
                let uc = Self::order_categories(scene, u);
                let air = uc & oc::AIR != 0;
                let by_category = (cats & oc::CANTRANSPORTCOMMANDER != 0 || beacon || uc & oc::COMMAND == 0)
                    && ((cats & (oc::TRANSPORTATION | oc::TELEPORTATION | oc::FERRYBEACON) != 0 && !air) || (cats & oc::AIRSTAGINGPLATFORM != 0 && air));
                u.build >= 1.0 && by_category && (beacon || Self::can_carry(scene, u, t))
            };
            if all & caps::CALL_TRANSPORT != 0 && !being_built && units.iter().any(|u| u.id != t.id && boards(u)) {
                return on(CommandKind::TransportLoadUnits);
            }
            // Factories (all of the selection) onto a ferry beacon: the units
            // they build wait there (0x103e6970; sent as a factory command).
            if beacon && units.iter().all(|u| Self::order_categories(scene, u) & oc::FACTORY != 0) {
                return on(CommandKind::TransportLoadUnits);
            }
            // The transport picks it up: moho's 0x103e6580, a selected
            // `TRANSPORTFOCUS` transport (not `TELEPORTATION`; the target
            // not `EXPERIMENTAL`), the commander rule as above, a
            // `TRANSPORTATION` one taking a non-air unit or an
            // `AIRSTAGINGPLATFORM` an air one, and able to carry it.
            let air = cats & oc::AIR != 0;
            let carries = |u: &EntityView| {
                let uc = Self::order_categories(scene, u);
                let by_category = uc & oc::TELEPORTATION == 0
                    && cats & oc::EXPERIMENTAL == 0
                    && uc & oc::TRANSPORTFOCUS != 0
                    && (uc & oc::CANTRANSPORTCOMMANDER != 0 || cats & oc::COMMAND == 0)
                    && ((uc & oc::TRANSPORTATION != 0 && !air) || (uc & oc::AIRSTAGINGPLATFORM != 0 && air));
                by_category && Self::can_carry(scene, t, u)
            };
            if !being_built && tc & caps::CALL_TRANSPORT != 0 && units.iter().any(|u| u.id != t.id && carries(u)) {
                return on(CommandKind::TransportReverseLoadUnits);
            }
        }
        if !enemy {
            let damaged_or_upgrading = (ally && t.health < 1.0) || (is_unit && t.order_flags & of::UPGRADING != 0);
            if self.switch_right_click {
                if all & caps::REPAIR != 0 && damaged_or_upgrading {
                    return on(CommandKind::Repair);
                }
                if all & caps::GUARD != 0 {
                    return on(CommandKind::Guard);
                }
            } else {
                if all & caps::REPAIR != 0 && being_built && is_unit && (t.kind == ViewKind::Unit || cats & oc::FACTORY == 0) {
                    return on(CommandKind::Repair);
                }
                let only_target = units.iter().all(|u| u.id == t.id);
                if all & caps::GUARD != 0 && cats & oc::CAMPAIGNGATE == 0 && is_unit && !only_target && !held {
                    return on(CommandKind::Guard);
                }
                if all & caps::REPAIR != 0 && damaged_or_upgrading {
                    return on(CommandKind::Repair);
                }
            }
        }
        move_order
    }

    fn order_at(&mut self, scene: &World, at: (f64, f64), facing: Option<i32>, forced: Option<CommandKind>, queue: bool, held: bool) {
        let Some(ground) = scene.ground_at(at.0 as f32, at.1 as f32) else { return };
        let target_view = scene.pick(at.0 as f32, at.1 as f32).and_then(|id| Self::view(scene, id)).copied();
        let units: Vec<EntityView> = self.selection.iter().filter_map(|&id| Self::view(scene, id)).filter(|e| self.commandable(e)).copied().collect();
        if units.is_empty() {
            return;
        }
        let ground_target = CommandTarget::Ground(FxVec3::from_f64(ground.x as f64, ground.y as f64, ground.z as f64));
        let formation = FORMATIONS[self.formation].map(String::from);
        // A plain right click: one order for the whole selection, as moho
        // (`selection_order`); the sim refuses it to those that can't take
        // it (the unit under the cursor among them).
        if forced.is_none() {
            let Some((kind, on_entity)) = self.selection_order(scene, &units, target_view.as_ref(), held) else { return };
            let target = match (on_entity, target_view) {
                (true, Some(t)) => CommandTarget::Entity(t.id),
                _ => ground_target,
            };
            let ids: Vec<u32> = units.iter().map(|u| u.id).collect();
            self.send_split(scene, &ids, kind, target, queue, formation, facing, None, 0);
            let at = match target_view {
                Some(t) if on_entity => Vec3::from_array(t.position),
                _ => ground,
            };
            self.markers.push(Marker { at, kind, when: Instant::now() });
            return;
        }
        // Attack-move / patrol: moho's attack and patrol command modes (the
        // order is the mode's, 0x103e8420), the whole selection's.
        if forced == Some(CommandKind::Patrol) {
            let ids: Vec<u32> = units.iter().map(|u| u.id).collect();
            self.send_split(scene, &ids, CommandKind::Patrol, ground_target, queue, formation, facing, None, 0);
            self.markers.push(Marker { at: ground, kind: CommandKind::Patrol, when: Instant::now() });
        } else {
            self.attack_mode(scene, &units, target_view, ground, queue, formation, facing);
        }
    }

    /// moho's attack command mode clicked (`StartCommandMode order
    /// RULEUCC_Attack`, our attack-move; MohoEngine.dll 0x103e89c0 case 4):
    /// on an entity, the whole selection attacks it (the sim refuses it to
    /// those that can't, an ally's units among them); on the ground the
    /// selection splits (0x103e7960): units that can move and aren't on
    /// ground fire (fire state 2, UserUnit +0x1b8) attack-move there, the
    /// rest attack the ground.
    #[allow(clippy::too_many_arguments)]
    fn attack_mode(&mut self, scene: &World, units: &[EntityView], target: Option<EntityView>, ground: Vec3, queue: bool, formation: Option<String>, facing: Option<i32>) {
        use crate::engine::snapshot::order_flags as of;
        if let Some(t) = target {
            let ids: Vec<u32> = units.iter().map(|u| u.id).collect();
            self.send_split(scene, &ids, CommandKind::Attack, CommandTarget::Entity(t.id), queue, None, None, None, 0);
            self.markers.push(Marker { at: Vec3::from_array(t.position), kind: CommandKind::Attack, when: Instant::now() });
            return;
        }
        let at = CommandTarget::Ground(FxVec3::from_f64(ground.x as f64, ground.y as f64, ground.z as f64));
        let (movers, firers): (Vec<&EntityView>, Vec<&EntityView>) = units.iter().partition(|u| u.kind == ViewKind::Unit && u.order_flags & of::GROUND_FIRE == 0);
        let ids = |v: &[&EntityView]| v.iter().map(|u| u.id).collect::<Vec<u32>>();
        if !movers.is_empty() {
            self.send_split(scene, &ids(&movers), CommandKind::AggressiveMove, at, queue, formation, facing, None, 0);
        }
        if !firers.is_empty() {
            self.send_split(scene, &ids(&firers), CommandKind::Attack, at, queue, None, None, None, 0);
        }
        self.markers.push(Marker { at: ground, kind: CommandKind::AggressiveMove, when: Instant::now() });
    }

    /// An order from the interface's panels (stop, factory builds, upgrades,
    /// ...) for `units` (else the selection).
    pub fn issue_ui(&mut self, scene: &World, units: Option<&[u32]>, kind: CommandKind, blueprint: Option<&str>, count: u32, queue: bool) {
        let ids: Vec<u32> = units.map_or_else(|| self.selection.clone(), <[u32]>::to_vec);
        self.send_split(scene, &ids, kind, CommandTarget::None, queue, None, None, blueprint, count);
    }

    /// A script order from the interface (an enhancement) for the selection.
    pub fn script_ui(&mut self, scene: &World, units: Option<&[u32]>, script: Vec<(String, String)>, queue: bool) {
        let Some(tx) = &self.orders else { return };
        let ids: Vec<u32> = units.map_or_else(|| self.selection.clone(), <[u32]>::to_vec);
        let mut by_army: std::collections::BTreeMap<u32, Vec<u32>> = Default::default();
        for id in ids {
            if let Some(e) = Self::view(scene, id).filter(|e| self.commandable(e)) {
                by_army.entry(e.army).or_default().push(id);
            }
        }
        let mut sent = Vec::new();
        for (army, units) in by_army {
            sent.extend_from_slice(&units);
            let _ = tx.send(PlayerCommand { army, kind: CommandKind::Script, units, target: CommandTarget::None, queue, formation: None, facing: None, blueprint: None, count: 0, toggle: None, script: script.clone(), callback: None, category: None, retarget: None });
        }
        // The interface hears it too: FAF queues an enhancement for the
        // construction panel's progress there (`OnScriptIssued`).
        if !sent.is_empty() {
            sent.sort_unstable();
            self.issued.push(Issued { kind: CommandKind::Script, target: CommandTarget::None, queue, blueprint: None, units: sent, script });
        }
    }

    /// A unit setting from the interface (fire state, pause, toggle bit).
    pub fn toggle_ui(&mut self, scene: &World, units: Option<&[u32]>, toggle: crate::engine::sim::input::Toggle) {
        let Some(tx) = &self.orders else { return };
        let ids: Vec<u32> = units.map_or_else(|| self.selection.clone(), <[u32]>::to_vec);
        let mut by_army: std::collections::BTreeMap<u32, Vec<u32>> = Default::default();
        for id in ids {
            if let Some(e) = Self::view(scene, id).filter(|e| self.commandable(e)) {
                by_army.entry(e.army).or_default().push(id);
            }
        }
        for (army, units) in by_army {
            let _ = tx.send(PlayerCommand { army, kind: CommandKind::Stop, units, target: CommandTarget::None, queue: true, formation: None, facing: None, blueprint: None, count: 0, toggle: Some(toggle), script: Vec::new(), callback: None, category: None, retarget: None });
        }
    }

    /// A `SimCallback` from the interface, with the selection it carries
    /// (any army's: the sim passes on the live ones and FAF's callbacks
    /// check them). Sent as the player's army (an observer's as none).
    pub fn sim_callback_ui(&mut self, units: Vec<u32>, callback: crate::engine::sim::input::SimCallback) {
        let Some(tx) = &self.orders else { return };
        let army = self.player.unwrap_or(0);
        let _ = tx.send(PlayerCommand { army, kind: CommandKind::Stop, units, target: CommandTarget::None, queue: true, formation: None, facing: None, blueprint: None, count: 0, toggle: None, script: Vec::new(), callback: Some(callback), category: None, retarget: None });
    }

    /// An order the interface armed (FAF's command mode) given by a click on
    /// the world at screen point `at`: an entity target under the cursor for
    /// orders that take one, else the ground (builds at the ground point).
    #[allow(clippy::too_many_arguments)]
    pub fn issue_mode(&mut self, scene: &World, at: (f64, f64), kind: CommandKind, blueprint: Option<&str>, footprint: Option<(f32, f32)>, queue: bool) {
        let Some(ground) = scene.ground_at(at.0 as f32, at.1 as f32) else { return };
        // Structures sit on the footprint grid.
        let ground = match footprint {
            Some(fp) => snap_to_footprint(scene, ground, fp),
            None => ground,
        };
        let under = scene.pick(at.0 as f32, at.1 as f32).and_then(|id| Self::view(scene, id)).copied();
        let takes_entity = matches!(
            kind,
            CommandKind::Attack | CommandKind::Guard | CommandKind::Repair | CommandKind::Reclaim | CommandKind::Capture | CommandKind::TransportLoadUnits | CommandKind::TransportReverseLoadUnits
        );
        let target = match under {
            Some(t) if takes_entity => CommandTarget::Entity(t.id),
            _ => CommandTarget::Ground(FxVec3::from_f64(ground.x as f64, ground.y as f64, ground.z as f64)),
        };
        let ids = self.selection.clone();
        let formation = (kind == CommandKind::Move || kind == CommandKind::Patrol || kind == CommandKind::AggressiveMove).then(|| FORMATIONS[self.formation].map(String::from)).flatten();
        // The attack mode on the ground splits the selection (moho's
        // attack command mode: [`Self::attack_mode`]).
        if kind == CommandKind::Attack && matches!(target, CommandTarget::Ground(_)) {
            let units: Vec<EntityView> = ids.iter().filter_map(|&id| Self::view(scene, id)).filter(|e| self.commandable(e)).copied().collect();
            let formation = FORMATIONS[self.formation].map(String::from);
            self.attack_mode(scene, &units, None, ground, queue, formation, None);
            return;
        }
        self.send_split(scene, &ids, kind, target, queue, formation, None, blueprint, 1);
        self.markers.push(Marker { at: ground, kind, when: Instant::now() });
    }

    /// Moves turned into patrols: each group's route reissued as patrol
    /// waypoints, replacing its queue.
    pub fn convert_to_patrol(&mut self, scene: &World, routes: &[(Vec<u32>, Vec<Vec3>)]) {
        let formation = FORMATIONS[self.formation].map(String::from);
        for (units, points) in routes {
            for (i, p) in points.iter().enumerate() {
                let target = CommandTarget::Ground(FxVec3::from_f64(p.x as f64, p.y as f64, p.z as f64));
                self.send_split(scene, units, CommandKind::Patrol, target, i > 0, formation.clone(), None, None, 0);
            }
        }
    }

    /// A build template placed: each structure ordered at its spot (already
    /// snapped), the first queued as `queue` says, the rest after it.
    pub fn issue_builds(&mut self, scene: &World, builds: &[(String, glam::Vec3)], queue: bool) {
        let ids = self.selection.clone();
        for (i, (bp, at)) in builds.iter().enumerate() {
            let target = CommandTarget::Ground(FxVec3::from_f64(at.x as f64, at.y as f64, at.z as f64));
            self.send_split(scene, &ids, CommandKind::BuildMobile, target, queue || i > 0, None, None, Some(bp), 1);
            self.markers.push(Marker { at: *at, kind: CommandKind::BuildMobile, when: Instant::now() });
        }
    }

    /// Moves queued command `command` (an order of `owner`'s) to `target`:
    /// its waypoint or build site dragged in the command graph (moho's
    /// `SetCommandTarget` / `SetCommandCells`).
    pub fn retarget(&mut self, scene: &World, owner: u32, command: u32, target: CommandTarget) {
        let Some(tx) = &self.orders else { return };
        let Some(army) = Self::view(scene, owner).filter(|e| self.commandable(e)).map(|e| e.army) else { return };
        let _ = tx.send(PlayerCommand { army, kind: CommandKind::Stop, units: Vec::new(), target, queue: true, formation: None, facing: None, blueprint: None, count: 0, toggle: None, script: Vec::new(), callback: None, category: None, retarget: Some(command) });
    }

    /// Sends one command per army among `ids` (an observer may hold several).
    #[allow(clippy::too_many_arguments)]
    fn send_split(&mut self, scene: &World, ids: &[u32], kind: CommandKind, target: CommandTarget, queue: bool, formation: Option<String>, facing: Option<i32>, blueprint: Option<&str>, count: u32) {
        let Some(tx) = &self.orders else { return };
        let mut sent = Vec::new();
        let mut by_army: std::collections::BTreeMap<u32, Vec<u32>> = Default::default();
        for &id in ids {
            if let Some(e) = Self::view(scene, id).filter(|e| self.commandable(e)) {
                by_army.entry(e.army).or_default().push(id);
            }
        }
        for (army, units) in by_army {
            sent.extend_from_slice(&units);
            let _ = tx.send(PlayerCommand { army, kind, units, target, queue, formation: formation.clone(), facing, blueprint: blueprint.map(String::from), count, toggle: None, script: Vec::new(), callback: None, category: None, retarget: None });
        }
        // A build with no units still reaches the interface (moho issues it
        // with an empty unit list): FAF's cheat menu spawns the structure
        // there (`OnCommandIssued` -> `SimCallback CheatSpawnUnit`).
        if !sent.is_empty() || kind == CommandKind::BuildMobile {
            sent.sort_unstable();
            self.issued.push(Issued { kind, target, queue, blueprint: blueprint.map(String::from), units: sent, script: Vec::new() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Games one after another in one process: a restart keeps the army,
    /// but after the front end the next lobby's game commands the army the
    /// sim gives this client, not the last game's.
    #[test]
    fn each_lobby_game_commands_its_own_army() {
        let mut p = PlayerControl::new(None, Vec::new(), None);
        p.adopt_army(Some(1), false);
        assert_eq!(p.player, Some(1));
        p.end_session(SessionEnd::Restart);
        p.adopt_army(Some(1), false);
        assert_eq!(p.player, Some(1), "a restart keeps the army");
        p.end_session(SessionEnd::FrontEnd);
        p.adopt_army(Some(2), false);
        assert_eq!(p.player, Some(2), "the second game's army, not the first's");
        p.end_session(SessionEnd::FrontEnd);
        p.adopt_army(Some(1), true);
        assert_eq!(p.player, None, "a replay commands no army");
        // `--player` holds over a restart.
        let mut p = PlayerControl::new(Some(3), Vec::new(), None);
        p.end_session(SessionEnd::Restart);
        p.adopt_army(Some(1), false);
        assert_eq!(p.player, Some(3));
    }

    #[test]
    fn an_upgraded_structure_takes_the_old_ones_place() {
        let mut p = PlayerControl::new(Some(1), vec![3, 9], None);
        p.groups[2] = vec![1, 9];
        p.upgraded(9, 2);
        p.upgraded(4, 5);
        assert_eq!(p.selection, vec![2, 3], "selected: swapped in, sorted");
        assert_eq!(p.groups[2], vec![1, 2], "and in its control group");
        assert!(p.groups[0].is_empty());
    }

    #[test]
    fn footprints_snap_to_whole_cells() {
        // Odd sizes centre on half cells, even sizes on whole ones.
        assert_eq!(snap_centre(10.2, 1.0), 10.5);
        assert_eq!(snap_centre(10.2, 2.0), 10.0);
        assert_eq!(snap_centre(10.7, 3.0), 10.5);
        assert_eq!(snap_centre(10.9, 4.0), 11.0);
    }

    #[test]
    fn drag_lines_lay_structures_footprint_to_footprint() {
        let at = |x: f32, z: f32| Vec3::new(x, 0.0, z);
        // A 2x2 along x: every 2 cells, as far as the drag reached.
        assert_eq!(drag_spots(at(10.0, 10.0), at(17.0, 10.3), (2.0, 2.0), (2.0, 2.0)), vec![(10.0, 10.0), (12.0, 10.0), (14.0, 10.0), (16.0, 10.0)]);
        // Diagonal walls (1x1): corner to corner.
        assert_eq!(drag_spots(at(0.5, 0.5), at(3.5, 3.5), (1.0, 1.0), (1.0, 1.0)), vec![(0.5, 0.5), (1.5, 1.5), (2.5, 2.5), (3.5, 3.5)]);
        // A 1x1 footprint on a 2x2 skirt (T1 power generator): skirt to skirt.
        assert_eq!(drag_spots(at(10.5, 10.5), at(16.5, 10.5), (1.0, 1.0), (2.0, 2.0)), vec![(10.5, 10.5), (12.5, 10.5), (14.5, 10.5), (16.5, 10.5)]);
        // No drag: just the one.
        assert_eq!(drag_spots(at(4.2, 4.2), at(4.2, 4.2), (1.0, 1.0), (1.0, 1.0)), vec![(4.5, 4.5)]);
        // Never two closer than the skirt, whatever the angle.
        for (fp, skirt) in [((2.0, 2.0), (2.0, 2.0)), ((1.0, 1.0), (2.0, 2.0)), ((5.0, 5.0), (8.0, 8.0))] {
            let spots = drag_spots(at(0.0, 0.0), at(40.0, 13.0), fp, skirt);
            for pair in spots.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                assert!((a.0 - b.0).abs() >= skirt.0 || (a.1 - b.1).abs() >= skirt.1, "{fp:?} {skirt:?}: {a:?} {b:?}");
            }
        }
    }

    #[test]
    fn a_dragged_waypoint_previews_where_it_goes() {
        use crate::engine::snapshot::OrderMarker;
        // A formation slot keeps its offset round the moved node; a build's
        // site moves with it; an order on a unit goes onto the new unit.
        let slot = OrderMarker { kind: CommandKind::Move as u8, command: 7, at: [12.0, 0.0, 5.0], node: [10.0, 0.0, 5.0], site: None, blueprint: None, eta: 0.0, target: None, movable: true };
        assert_eq!(moved_marker(&slot, [20.0, 1.0, 0.0]).at, [22.0, 1.0, 0.0]);
        let build = OrderMarker { kind: CommandKind::BuildMobile as u8, at: [10.0, 0.0, 5.0], site: Some([8.0, 3.0, 12.0, 7.0]), blueprint: Some("ueb1101".into()), ..slot.clone() };
        assert_eq!(moved_marker(&build, [14.0, 0.0, 1.0]).site, Some([12.0, -1.0, 16.0, 3.0]));
        let attack = OrderMarker { kind: CommandKind::Attack as u8, at: [10.0, 0.0, 5.0], target: Some(3), ..slot.clone() };
        assert_eq!(moved_marker(&attack, [30.0, 2.0, 30.0]).at, [30.0, 2.0, 30.0]);
        // A press that stays put is a click, not a drag.
        let mut drag = CommandDrag::new(1, slot, (100.0, 100.0));
        drag.cursor_at((104.0, 103.0));
        assert!(!drag.moved);
        drag.cursor_at((120.0, 100.0));
        drag.cursor_at((100.0, 100.0));
        assert!(drag.moved, "once past the threshold it stays a drag");
    }

    #[test]
    fn moves_convert_to_patrol_as_moho_offers_it() {
        use crate::engine::snapshot::OrderMarker;
        let mv = CommandKind::Move as u8;
        let mark = |kind: u8, command: u32, x: f32| OrderMarker { kind, command, at: [x, 0.0, 0.0], node: [x, 0.0, 0.0], site: None, blueprint: None, eta: 0.0, target: None, movable: true };
        let route = |cmds: &[(u8, u32)]| cmds.iter().map(|&(k, c)| mark(k, c, c as f32)).collect::<Vec<_>>();
        // Two units sharing a 3-waypoint move route: hovering the first or
        // the middle one offers it, as one group.
        let orders = vec![(1, route(&[(mv, 10), (mv, 11), (mv, 12)])), (2, route(&[(mv, 10), (mv, 11), (mv, 12)]))];
        let groups = patrol_routes(&[1, 2], &orders, mv, 10).expect("offered");
        assert_eq!(groups, vec![(vec![1, 2], vec![Vec3::new(10.0, 0.0, 0.0), Vec3::new(11.0, 0.0, 0.0), Vec3::new(12.0, 0.0, 0.0)])]);
        assert!(patrol_routes(&[1, 2], &orders, mv, 11).is_some());
        // Not the last waypoint, not a lone move, not a mixed queue.
        assert!(patrol_routes(&[1, 2], &orders, mv, 12).is_none());
        assert!(patrol_routes(&[3], &[(3, route(&[(mv, 20)]))], mv, 20).is_none());
        let attack = CommandKind::Attack as u8;
        assert!(patrol_routes(&[4], &[(4, route(&[(mv, 30), (attack, 31), (mv, 32)]))], mv, 30).is_none());
        // Every selected unit must qualify.
        assert!(patrol_routes(&[1, 5], &orders, mv, 10).is_none());
    }

    #[test]
    fn facing_follows_the_sim_convention() {
        let o = Vec3::ZERO;
        assert_eq!(facing_degrees(o, Vec3::new(0.0, 0.0, 5.0)), 0, "south");
        assert_eq!(facing_degrees(o, Vec3::new(5.0, 0.0, 0.0)), 90, "east");
        assert_eq!(facing_degrees(o, Vec3::new(-5.0, 0.0, 0.0)), -90, "west");
        assert_eq!(facing_degrees(o, Vec3::new(0.0, 0.0, -5.0)).abs(), 180, "north");
    }
}
