//! RTS camera: looks down at a point on the map from a zoom-dependent
//! height and tilt. Screen-up is map north (-z), screen-right is +x.

use glam::{Mat4, Vec3};

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    /// Point on the map the camera looks at (y follows the terrain).
    pub target: Vec3,
    /// Moho's zoom: the width of ground across the view's longer side at
    /// the target (the eye backs off to fit it: [`Camera::eye_distance`]).
    pub zoom: f32,
    pub min_zoom: f32,
    pub max_zoom: f32,
    /// The view's width / height.
    pub aspect: f32,
    /// Tilt from horizontal at the nearest / farthest zoom, degrees (moho's
    /// `cam_NearPitch` / `cam_FarPitch`).
    pub near_pitch: f32,
    pub far_pitch: f32,
    /// Field of view across the view's longer side at the nearest /
    /// farthest zoom, degrees (`cam_NearFOV` / `cam_FarFOV`).
    pub near_fov: f32,
    pub far_fov: f32,
    /// Zoom per wheel notch (moho's `cam_ZoomAmount`, FAF's zoom-speed
    /// option /100): each notch scales the zoom by 2^-amount.
    pub zoom_amount: f32,
    /// How far past the whole-map view the camera may zoom out (moho's
    /// `cam_MaxZoomMult`; FAF's world views set 1.4 with `SetMaxZoomMult`).
    pub max_zoom_mult: f32,
    /// The zoom at which the whole map fills the view.
    fit_zoom: f32,
    /// Moho's LOD scale (`SetLODScale`, the console's `cam_SetLOD`):
    /// every `LODCutoff` is compared with view depth times this, so below 1
    /// detail reaches further. New cameras take `cam_DefaultLOD`.
    pub lod_scale: f32,
    /// Which way the camera faces, radians about the vertical (moho's
    /// heading, camera +0x34c): pi looks north (-z), the default.
    pub heading: f32,
    /// The pitch while the rotation is held (moho's +0x344).
    held_pitch: f32,
    /// The rotation is held (moho's +0x33d): heading and pitch are the
    /// player's (Space and the mouse, Insert / Delete, `HoldRotation`),
    /// not the zoom's.
    pub rotation_held: bool,
    /// Easing back to the zoom's pitch and the default heading (moho's
    /// +0x33e, set by `RevertRotation`).
    reverting: bool,
}

/// Moho's LOD plane (`GeomCamera3::LODBasis`, the view plane times the LOD
/// scale; MohoEngine.dll 0x1002dff0): what every level-of-detail cutoff
/// (meshes, decals, splats, emitters, beams, trails, resource icons) is
/// measured with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodPlane {
    pub eye: Vec3,
    /// The view direction (unit).
    pub forward: Vec3,
    pub scale: f32,
}

impl LodPlane {
    pub fn new(eye: Vec3, target: Vec3, scale: f32) -> Self {
        Self { eye, forward: (target - eye).normalize_or(Vec3::NEG_Y), scale }
    }

    /// `GeomCamera3::LODMetric` (0x1002df90): `p`'s depth along the view
    /// times the LOD scale.
    pub fn metric(&self, p: Vec3) -> f32 {
        self.forward.dot(p - self.eye) * self.scale
    }
}

/// Moho's camera defaults (SupremeCommander.exe .data 0xfb7e98..: the
/// console variables' initial values).
pub const NEAR_ZOOM: f32 = 5.0;
pub const NEAR_FOV: f32 = 65.0;
pub const FAR_FOV: f32 = 60.0;
pub const NEAR_PITCH: f32 = 40.0;
pub const FAR_PITCH: f32 = 89.9;
pub const ZOOM_AMOUNT: f32 = 0.4;
/// `cam_DefaultLOD`'s default (MohoEngine.dll .data 0x1089a0d0), the LOD
/// scale a new camera takes.
pub const DEFAULT_LOD: f32 = 1.0;
/// FAF's Level Of Detail option (`SC_CameraScaleLOD n`, Low / Medium /
/// High): the LOD scale by `n` (SupremeCommander.exe .data 0xfb8d4c, read
/// by the command at 0x912500, which then runs `cam_DefaultLOD` and
/// `cam_SetLOD WorldCamera` with it).
pub const SCALE_LOD: [f32; 3] = [1.2, 1.0, 0.9];
/// `cam_MaxZoomMult`'s default (MohoEngine.dll .data 0x10899280: 1.1, "Extra
/// zoom out buffer so we can see the borders of the map clearly").
pub const MAX_ZOOM_MULT: f32 = 1.1;
/// The heading a camera faces until rotated: north.
pub const DEFAULT_HEADING: f32 = std::f32::consts::PI;
/// `cam_SpinSpeed`'s default (SupremeCommander.exe .data 0xfb7ebc): degrees
/// the camera turns as the mouse crosses the view's width.
pub const SPIN_SPEED: f32 = 360.0;
/// `cam_MinSpinPitch`'s default (.data 0xfb7ecc, radians): a spin tilts
/// the camera no flatter than this.
pub const MIN_SPIN_PITCH: f32 = 0.1;
/// The steepest a spin tilts it (0x7ae32d: .rdata 0xea2b74, radians).
pub const MAX_SPIN_PITCH: f32 = 1.560_796_4;
/// `ren_BgLowerBound`'s default (.data 0xfb7964): Space and the mouse spin
/// the camera only zoomed in closer than this (unless `cam_Free`).
pub const SPIN_ZOOM_LIMIT: f32 = 125.0;
/// `ui_KeyboardRotateSpeed` ("How fast ins/del rotate camera", .data
/// 0xfb7970): pixels of mouse spin Insert / Delete give a frame, times
/// `ui_KeyboardRotateAccelerateMultiplier` (0xfb7974, 2) with Ctrl.
pub const KEYBOARD_ROTATE_SPEED: f32 = 10.0;
pub const KEYBOARD_ROTATE_ACCELERATE: f32 = 2.0;
/// How far a reverting camera turns back in a frame, radians (0x7b0fe2:
/// .rdata 0xe4cea4). Moho steps it each camera update, so it is taken as
/// a 60 Hz frame's here.
pub const REVERT_STEP: f32 = 0.1;

impl Camera {
    pub fn new(map_w: f32, map_h: f32) -> Self {
        let mut c = Self {
            target: Vec3::new(map_w / 2.0, 0.0, map_h / 2.0),
            zoom: 0.0,
            min_zoom: NEAR_ZOOM,
            max_zoom: 0.0,
            aspect: 16.0 / 9.0,
            near_pitch: NEAR_PITCH,
            far_pitch: FAR_PITCH,
            near_fov: NEAR_FOV,
            far_fov: FAR_FOV,
            zoom_amount: ZOOM_AMOUNT,
            max_zoom_mult: MAX_ZOOM_MULT,
            fit_zoom: 0.0,
            lod_scale: DEFAULT_LOD,
            heading: DEFAULT_HEADING,
            held_pitch: 0.0,
            rotation_held: false,
            reverting: false,
        };
        c.fit_map(map_w, map_h);
        c.zoom = c.max_zoom;
        c
    }

    /// Farthest zoom: the map's width, or its height at the view's aspect,
    /// whichever is wider (moho's `GetMaxZoom`, SupremeCommander.exe
    /// 0x7ae8fb), times `max_zoom_mult` to see past the map's edges.
    pub fn fit_map(&mut self, map_w: f32, map_h: f32) {
        self.fit_zoom = map_w.max(map_h * self.aspect);
        self.max_zoom = (self.fit_zoom * self.max_zoom_mult).max(self.min_zoom + 1.0);
    }

    /// `SetMaxZoomMult`: the farthest zoom becomes the whole-map view times
    /// `mult` (the camera keeps its zoom where it still can).
    pub fn set_max_zoom_mult(&mut self, mult: f32) {
        self.max_zoom_mult = mult.max(0.1);
        self.max_zoom = (self.fit_zoom * self.max_zoom_mult).max(self.min_zoom + 1.0);
        self.zoom = self.zoom.min(self.max_zoom);
    }

    /// Where the zoom sits between nearest and farthest, on a log scale
    /// (moho interpolates pitch and field of view this way:
    /// SupremeCommander.exe 0x7b0ae0 / 0x7b0f00).
    fn zoom_fraction(&self) -> f32 {
        let (lo, hi) = (self.min_zoom.ln(), self.max_zoom.ln());
        ((self.zoom.clamp(self.min_zoom, self.max_zoom).ln() - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0)
    }

    /// Tilt from horizontal: `cam_NearPitch` fully zoomed in to
    /// `cam_FarPitch` (straight down) fully out.
    pub fn pitch(&self) -> f32 {
        if self.rotation_held { self.held_pitch } else { self.zoom_pitch() }
    }

    /// The pitch the zoom gives (with the rotation not held).
    fn zoom_pitch(&self) -> f32 {
        (self.near_pitch + (self.far_pitch.min(89.9) - self.near_pitch) * self.zoom_fraction()).to_radians()
    }

    /// Field of view across the view's longer side, `cam_NearFOV` in to
    /// `cam_FarFOV` out.
    pub fn fov(&self) -> f32 {
        (self.near_fov + (self.far_fov - self.near_fov) * self.zoom_fraction()).to_radians()
    }

    /// The tangents of half the horizontal and vertical fields of view.
    /// Moho's projection (SupremeCommander.exe 0x4f68d0) puts the field of
    /// view on the longer side, so widening a window keeps the ground's
    /// width and shows less of its depth.
    pub fn half_tangents(&self) -> (f32, f32) {
        let t = (self.fov() * 0.5).tan();
        let a = self.aspect.max(1e-3);
        if a >= 1.0 { (t, t / a) } else { (t * a, t) }
    }

    /// How far the eye sits from the target: far enough for `zoom` to span
    /// the longer side (moho: zoom x 0.5 / tan(fov / 2), 0x7b18c0).
    pub fn eye_distance(&self) -> f32 {
        self.zoom * 0.5 / (self.fov() * 0.5).tan()
    }

    /// The view direction (unit): `heading` about the vertical, `pitch`
    /// down from level (moho 0x7b0a00).
    pub fn forward(&self) -> Vec3 {
        let (p, h) = (self.pitch(), self.heading);
        Vec3::new(h.sin() * p.cos(), -p.sin(), h.cos() * p.cos())
    }

    pub fn eye(&self) -> Vec3 {
        self.target - self.forward() * self.eye_distance()
    }

    /// Spins the camera by a mouse move of `dx`, `dy` pixels over a view
    /// `width` pixels wide (moho's `CameraImpl` vtable +0x6c,
    /// SupremeCommander.exe 0x7ae270): the heading turns `cam_SpinSpeed`
    /// degrees across the width, wrapped to +-pi; the pitch by as much,
    /// between `cam_MinSpinPitch` and nearly straight down. The rotation is
    /// held from then on.
    pub fn spin(&mut self, dx: f32, dy: f32, width: f32) {
        let per_pixel = (SPIN_SPEED / width.max(1.0)).to_radians();
        let pitch = self.pitch();
        let mut heading = (self.heading - dx * per_pixel) % std::f32::consts::TAU;
        if heading < -std::f32::consts::PI {
            heading += std::f32::consts::TAU;
        } else if heading > std::f32::consts::PI {
            heading -= std::f32::consts::TAU;
        }
        self.heading = heading;
        self.held_pitch = (pitch + dy * per_pixel).clamp(MIN_SPIN_PITCH, MAX_SPIN_PITCH);
        self.rotation_held = true;
        self.reverting = false;
    }

    /// `HoldRotation` (0x7ae370): the camera keeps its heading and pitch as
    /// it zooms.
    pub fn hold_rotation(&mut self) {
        if !self.rotation_held {
            self.held_pitch = self.pitch();
        }
        self.rotation_held = true;
        self.reverting = false;
    }

    /// `RevertRotation` (vtable +0x74, 0x7ae3d0): a held rotation eases
    /// back to the zoom's pitch and the default heading.
    pub fn revert_rotation(&mut self) {
        if self.rotation_held {
            self.reverting = true;
        }
    }

    /// `SnapTo` / `MoveTo`'s orientation (the camera's vtable +0x38,
    /// SupremeCommander.exe 0x7b02d0, with no time): the heading (radians,
    /// pi faces north, wrapped to +-pi: 0x7afe30) and pitch (radians down
    /// from level, as given) are held from now on, as `HoldRotation` holds
    /// them, until `RevertRotation` or a reset.
    pub fn set_rotation(&mut self, heading: f32, pitch: f32) {
        self.heading = nearest_heading(0.0, heading);
        self.held_pitch = pitch;
        self.rotation_held = true;
        self.reverting = false;
    }

    /// Drops any rotation at once (a reset view).
    pub fn clear_rotation(&mut self) {
        (self.heading, self.rotation_held, self.reverting) = (DEFAULT_HEADING, false, false);
    }

    /// Eases a reverting camera back by `dt` seconds (moho's camera update,
    /// 0x7b0f7b): the pitch toward the zoom's and the heading toward the
    /// nearer of +-pi, each at most [`REVERT_STEP`] a frame; within a step
    /// of both, it lets go of the rotation.
    pub fn step_revert(&mut self, dt: f32) {
        if !self.reverting {
            return;
        }
        let step = REVERT_STEP * dt * 60.0;
        let goal = self.zoom_pitch();
        self.held_pitch = goal.clamp(self.held_pitch - step, self.held_pitch + step);
        let home = if self.heading > 0.0 { std::f32::consts::PI } else { -std::f32::consts::PI };
        self.heading = home.clamp(self.heading - step, self.heading + step);
        if (self.heading - home).abs() < step && (self.held_pitch - goal).abs() < step {
            self.clear_rotation();
        }
    }

    pub fn lod_plane(&self) -> LodPlane {
        LodPlane::new(self.eye(), self.target, self.lod_scale)
    }

    pub fn view_proj(&self) -> Mat4 {
        let view = glam::camera::rh::view::look_at_mat4(self.eye(), self.target, Vec3::Y);
        let d = self.eye_distance();
        let near = (d * 0.01).max(0.5);
        // Tilted toward the horizon the ground reaches far off: moho's far
        // plane is the eye distance + 17000 (0x7b1925).
        let far = if self.rotation_held { d + 17000.0 } else { d * 4.0 + 500.0 };
        // wgpu clip space: depth 0..1, y up (DirectX convention).
        let (tw, th) = self.half_tangents();
        let range = far / (near - far);
        let proj = Mat4::from_cols(
            glam::Vec4::new(1.0 / tw, 0.0, 0.0, 0.0),
            glam::Vec4::new(0.0, 1.0 / th, 0.0, 0.0),
            glam::Vec4::new(0.0, 0.0, range, -1.0),
            glam::Vec4::new(0.0, 0.0, range * near, 0.0),
        );
        proj * view
    }

    /// Screen-right and screen-down along the ground (unit, level): east
    /// and south at the default heading.
    pub fn ground_axes(&self) -> (Vec3, Vec3) {
        let (sin, cos) = self.heading.sin_cos();
        (Vec3::new(-cos, 0.0, sin), Vec3::new(-sin, 0.0, -cos))
    }

    /// Pan by screen-space amounts (fractions of the view's longer side),
    /// scaled by zoom, along the ground as the camera faces.
    pub fn pan(&mut self, right: f32, down: f32) {
        let (r, d) = self.ground_axes();
        self.target += (r * right + d * down) * self.zoom;
    }

    /// Zoom by wheel steps (positive = in): the zoom scales by
    /// 2^(-cam_ZoomAmount * steps) (moho: exp(amount * wheel * -ln 2),
    /// SupremeCommander.exe 0x7af7f0).
    pub fn zoom(&mut self, steps: f32) {
        self.zoom = (self.zoom * (-self.zoom_amount * steps).exp2()).clamp(self.min_zoom, self.max_zoom);
    }

    /// Keeps the target on the map.
    pub fn clamp_to(&mut self, map_w: f32, map_h: f32) {
        self.target.x = self.target.x.clamp(0.0, map_w);
        self.target.z = self.target.z.clamp(0.0, map_h);
    }

    /// World position -> screen pixels (`None` if behind the camera).
    pub fn project(&self, p: Vec3, screen: [f32; 2]) -> Option<[f32; 2]> {
        let clip = self.view_proj() * p.extend(1.0);
        if clip.w <= 0.0 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        Some([(ndc.x * 0.5 + 0.5) * screen[0], (0.5 - ndc.y * 0.5) * screen[1]])
    }
}

/// The main world view's camera (moho's `WorldCamera`): the one the
/// interface's camera commands, the minimap and sounds follow.
pub const MAIN: &str = "WorldCamera";

/// The world's cameras by name, in the order they were made: `WorldCamera`,
/// FAF's split view's `WorldCamera2`, a second head's `CameraHead2`. Names
/// match without case (moho's console finds `worldcamera` too).
#[derive(Clone, Default)]
pub struct Cameras(Vec<(String, Camera)>);

impl Cameras {
    pub fn main(&self) -> Option<&Camera> {
        self.get(MAIN)
    }

    pub fn main_mut(&mut self) -> Option<&mut Camera> {
        self.get_mut(MAIN)
    }

    pub fn get(&self, name: &str) -> Option<&Camera> {
        self.0.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, c)| c)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Camera> {
        self.0.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, c)| c)
    }

    /// Adds `camera` as `name` (replacing one of that name).
    pub fn insert(&mut self, name: &str, camera: Camera) {
        match self.get_mut(name) {
            Some(c) => *c = camera,
            None => self.0.push((name.to_owned(), camera)),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Camera)> {
        self.0.iter().map(|(n, c)| (n.as_str(), c))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut Camera)> {
        self.0.iter_mut().map(|(n, c)| (n.as_str(), c))
    }
}

/// A world view this frame: the camera it shows and the part of the window
/// it fills (physical pixels, whole: left, top, right, bottom). FAF's split
/// view is two side by side; without an interface, one fills the window.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub camera: String,
    pub rect: [f32; 4],
    /// Its interface world shapes this frame (`OnRenderWorld`).
    pub shapes: Option<std::sync::Arc<rts_core_engine::engine::api::debug_draw::DebugDraw>>,
}

impl View {
    /// The main camera over the whole `w` x `h` window.
    pub fn full(w: f32, h: f32) -> Self {
        Self { camera: MAIN.to_owned(), rect: [0.0, 0.0, w, h], shapes: None }
    }

    pub fn size(&self) -> [f32; 2] {
        [self.rect[2] - self.rect[0], self.rect[3] - self.rect[1]]
    }

    pub fn aspect(&self) -> f32 {
        let [w, h] = self.size();
        w.max(1.0) / h.max(1.0)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.rect[0] && y >= self.rect[1] && x < self.rect[2] && y < self.rect[3]
    }

    /// Window pixel (x, y) in the view's own pixels.
    pub fn local(&self, x: f32, y: f32) -> [f32; 2] {
        [x - self.rect[0], y - self.rect[1]]
    }

    /// World position -> window pixels through `cam` (`None` if behind it).
    pub fn project(&self, cam: &Camera, p: Vec3) -> Option<[f32; 2]> {
        cam.project(p, self.size()).map(|[x, y]| [x + self.rect[0], y + self.rect[1]])
    }

    /// Draws `pass` into this view only.
    pub fn bind(&self, pass: &mut wgpu::RenderPass<'_>) {
        let [l, t, r, b] = self.rect;
        pass.set_viewport(l, t, r - l, b - t, 0.0, 1.0);
        pass.set_scissor_rect(l as u32, t as u32, (r - l) as u32, (b - t) as u32);
    }
}

/// A timed camera move (FAF's `UIZoomTo(units, seconds)`, `MoveTo`,
/// `heading` as moho aims a camera's heading from `from` (0x7afe30): wrapped
/// to +-pi, then a turn the other way round if that is shorter, so a timed
/// move takes the short way.
fn nearest_heading(from: f32, heading: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut h = heading % TAU;
    if h < -PI {
        h += TAU;
    } else if h > PI {
        h -= TAU;
    }
    if (h - from).abs() > PI {
        if h >= from { h - TAU } else { h + TAU }
    } else {
        h
    }
}

/// `SetZoom(zoom, seconds)`: the game's opening zoom to the ACU): the look-at
/// point and zoom ease from where the camera was to the target. The zoom
/// moves evenly in log zoom, so a fly-in from the whole map slows as it
/// closes in rather than rushing the last part.
#[derive(Clone, Copy, Debug)]
pub struct CameraMove {
    from: (f32, f32, f32),
    to: (f32, f32, f32),
    /// `MoveTo`'s orientation: (heading, pitch) from and to.
    rotation: Option<((f32, f32), (f32, f32))>,
    seconds: f32,
    elapsed: f32,
}

impl CameraMove {
    /// From `cam`'s current view to look at `(x, z)` at `zoom`.
    pub fn new(cam: &Camera, x: f32, z: f32, zoom: f32, seconds: f32) -> Self {
        Self { from: (cam.target.x, cam.target.z, cam.zoom), to: (x, z, zoom), rotation: None, seconds, elapsed: 0.0 }
    }

    /// The move also turns the camera to `heading` and `pitch` (`MoveTo`'s
    /// orientation, 0x7b02d0 -> 0x7aea50 / 0x7afed0): from its heading and
    /// pitch now, the heading the short way round, along the move's own
    /// easing (moho's camera update 0x7b13c4..0x7b14ce: a Hermite curve
    /// whose tangents are the whole turn, so straight along the eased
    /// time). The rotation is held all the way and after.
    pub fn with_rotation(mut self, cam: &Camera, heading: f32, pitch: f32) -> Self {
        self.rotation = Some(((cam.heading, cam.pitch()), (nearest_heading(cam.heading, heading), pitch)));
        self
    }

    /// The move heads for `(x, z)` instead (a followed unit moved).
    pub fn retarget(&mut self, x: f32, z: f32) {
        (self.to.0, self.to.1) = (x, z);
    }

    /// Advances the move by `dt` and places `cam`. True once it has arrived.
    pub fn step(&mut self, cam: &mut Camera, dt: f32) -> bool {
        self.elapsed += dt;
        let t = (self.elapsed / self.seconds.max(1e-6)).clamp(0.0, 1.0);
        let e = t * t * (3.0 - 2.0 * t);
        let (x0, z0, zoom0) = self.from;
        let (x1, z1, zoom1) = self.to;
        if let Some(((h0, p0), (h1, p1))) = self.rotation {
            (cam.heading, cam.held_pitch) = if t >= 1.0 { (h1, p1) } else { (h0 + (h1 - h0) * e, p0 + (p1 - p0) * e) };
            (cam.rotation_held, cam.reverting) = (true, false);
        }
        if t >= 1.0 {
            (cam.target.x, cam.target.z, cam.zoom) = (x1, z1, zoom1);
            return true;
        }
        cam.target.x = x0 + (x1 - x0) * e;
        cam.target.z = z0 + (z1 - z0) * e;
        cam.zoom = zoom0 * (zoom1 / zoom0.max(1e-3)).powf(e);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Moho's LOD metric: view depth times the camera's LOD scale; the
    /// camera's target is the eye distance deep whatever the pitch.
    #[test]
    fn lod_metric_is_depth_times_scale() {
        let mut c = Camera::new(1024.0, 1024.0);
        c.zoom = 230.0;
        let d = c.eye_distance();
        assert!((c.lod_plane().metric(c.target) - d).abs() < 1e-2);
        // Beside the target, off the view's axis: the same depth, unlike
        // the distance to the eye.
        let side = c.target + Vec3::X * 150.0;
        assert!((c.lod_plane().metric(side) - d).abs() < 1e-2);
        assert!(c.eye().distance(side) > d + 40.0);
        c.lod_scale = SCALE_LOD[2];
        assert!((c.lod_plane().metric(c.target) - d * 0.9).abs() < 1e-2);
    }

    /// Ground across the view at the target, in world units: (width, height)
    /// through the screen's middle row and column.
    fn ground_span(c: &Camera) -> (f32, f32) {
        let vp = c.view_proj();
        let inv = vp.inverse();
        let ground = |ndc: glam::Vec2| {
            let near = inv.project_point3(ndc.extend(0.0));
            let far = inv.project_point3(ndc.extend(1.0));
            let d = far - near;
            near + d * ((c.target.y - near.y) / d.y)
        };
        let w = ground(glam::Vec2::new(1.0, 0.0)).distance(ground(glam::Vec2::new(-1.0, 0.0)));
        let h = ground(glam::Vec2::new(0.0, 1.0)).distance(ground(glam::Vec2::new(0.0, -1.0)));
        (w, h)
    }

    /// Moho's projection (0x4f68d0) puts the field of view on the longer
    /// side, and the zoom is the ground across it: looking straight down,
    /// a wide view shows `zoom` across and `zoom / aspect` up; widening the
    /// window keeps the width and loses height. A tall view is the other
    /// way round.
    #[test]
    fn zoom_spans_the_longer_side() {
        for aspect in [4.0 / 3.0, 16.0 / 9.0, 1574.0 / 595.0, 9.0 / 16.0] {
            let mut c = Camera::new(1024.0, 1024.0);
            c.aspect = aspect;
            c.fit_map(1024.0, 1024.0);
            c.zoom = c.max_zoom;
            assert!((c.pitch().to_degrees() - 89.9).abs() < 0.01);
            let (w, h) = ground_span(&c);
            let long = w.max(h);
            assert!((long - c.zoom).abs() < c.zoom * 2e-3, "aspect {aspect}: longer side {long} vs zoom {}", c.zoom);
            assert!((w / h - aspect).abs() < 1e-2, "aspect {aspect}: ground {w} x {h} not stretched");
            let (tw, th) = c.half_tangents();
            let fov = if aspect >= 1.0 { tw } else { th };
            assert!((fov - (c.fov() * 0.5).tan()).abs() < 1e-6, "aspect {aspect}: field of view on the longer side");
        }
    }

    /// The default camera faces north, down at the zoom's pitch.
    #[test]
    fn default_heading_faces_north() {
        let c = Camera::new(512.0, 512.0);
        let f = c.forward();
        assert!(f.x.abs() < 1e-5 && f.z < 0.0 && f.y < 0.0, "{f}");
        let (right, down) = c.ground_axes();
        assert!((right - Vec3::X).length() < 1e-5 && (down - Vec3::Z).length() < 1e-5);
    }

    /// Moho's mouse spin (0x7ae270): `cam_SpinSpeed` (360) degrees across
    /// the view's width, the heading wrapped to +-pi; the pitch by as much,
    /// between `cam_MinSpinPitch` and nearly straight down; panning turns
    /// with the camera.
    #[test]
    fn spin_turns_by_the_mouse_across_the_view() {
        let mut c = Camera::new(512.0, 512.0);
        c.zoom = 60.0;
        let pitch = c.pitch();
        // A quarter of the width: a quarter turn.
        c.spin(250.0, 0.0, 1000.0);
        assert!(c.rotation_held);
        assert!((c.heading - std::f32::consts::FRAC_PI_2).abs() < 1e-4, "{}", c.heading);
        assert!((c.pitch() - pitch).abs() < 1e-6);
        // Facing east now: screen-right is south.
        let (right, _) = c.ground_axes();
        assert!((right - Vec3::Z).length() < 1e-4, "{right}");
        let at = c.target;
        c.pan(0.5, 0.0);
        assert!((c.target - at - Vec3::Z * 30.0).length() < 1e-3);
        // Three more quarters: back round, wrapped.
        c.spin(750.0, 0.0, 1000.0);
        assert!((c.heading.abs() - std::f32::consts::PI).abs() < 1e-4, "{}", c.heading);
        // Down the screen tilts it toward straight down, up toward level,
        // within the limits.
        c.spin(0.0, 10_000.0, 1000.0);
        assert!((c.pitch() - MAX_SPIN_PITCH).abs() < 1e-6);
        c.spin(0.0, -10_000.0, 1000.0);
        assert!((c.pitch() - MIN_SPIN_PITCH).abs() < 1e-6);
        // Held, zooming leaves the pitch alone.
        c.zoom = 30.0;
        assert!((c.pitch() - MIN_SPIN_PITCH).abs() < 1e-6);
    }

    /// `RevertRotation` eases back at most 0.1 radians a (60 Hz) frame in
    /// heading and pitch (0x7b0f7b), then lets go of the rotation.
    #[test]
    fn revert_eases_back_to_the_usual_view() {
        let mut c = Camera::new(512.0, 512.0);
        c.zoom = 60.0;
        let (pitch, eye) = (c.pitch(), c.eye());
        c.spin(100.0, -50.0, 1000.0);
        let turned = std::f32::consts::PI - c.heading;
        c.revert_rotation();
        c.step_revert(1.0 / 60.0);
        assert!(c.rotation_held, "one frame turns it back a step");
        assert!((std::f32::consts::PI - c.heading - (turned - REVERT_STEP)).abs() < 1e-4);
        let mut frames = 1;
        while c.rotation_held {
            c.step_revert(1.0 / 60.0);
            frames += 1;
            assert!(frames < 100);
        }
        // It snaps home once less than a step is left.
        assert_eq!(frames, (turned / REVERT_STEP) as i32);
        assert!((c.pitch() - pitch).abs() < 1e-6 && (c.eye() - eye).length() < 1e-3);
        // Not held: nothing to revert.
        c.revert_rotation();
        c.step_revert(1.0 / 60.0);
        assert!(!c.rotation_held && c.heading == DEFAULT_HEADING);
    }

    /// `HoldRotation` keeps the pitch the zoom gave as the camera zooms.
    #[test]
    fn hold_rotation_keeps_the_pitch() {
        let mut c = Camera::new(512.0, 512.0);
        c.zoom = 60.0;
        let pitch = c.pitch();
        c.hold_rotation();
        c.zoom = 400.0;
        assert!((c.pitch() - pitch).abs() < 1e-6);
        c.clear_rotation();
        assert!(c.pitch() > pitch + 0.1, "the zoom's pitch again");
    }

    /// `SnapTo`'s orientation is held as the camera zooms, its heading
    /// wrapped to +-pi (0x7b02d0, 0x7afe30), until `RevertRotation` eases it
    /// home.
    #[test]
    fn snap_rotation_is_held_until_reverted() {
        use std::f32::consts::{FRAC_PI_2, PI};
        let mut c = Camera::new(512.0, 512.0);
        c.zoom = 30.0;
        c.set_rotation(FRAC_PI_2 + 2.0 * PI, 0.2);
        assert!(c.rotation_held);
        assert!((c.heading - FRAC_PI_2).abs() < 1e-5, "{}", c.heading);
        assert_eq!(c.pitch(), 0.2);
        // Side-on: the view looks along +x (heading pi / 2), nearly level.
        let f = c.forward();
        assert!(f.x > 0.97 && f.z.abs() < 1e-4 && f.y < 0.0, "{f}");
        c.zoom = 300.0;
        assert_eq!(c.pitch(), 0.2, "held as it zooms");
        c.revert_rotation();
        for _ in 0..100 {
            c.step_revert(1.0 / 60.0);
        }
        assert!(!c.rotation_held && c.heading == DEFAULT_HEADING);
    }

    /// `MoveTo`'s orientation turns along the move (the short way round) and
    /// is held from its first step to after it arrives.
    #[test]
    fn timed_move_turns_the_short_way_and_holds() {
        use std::f32::consts::PI;
        let mut cam = Camera::new(512.0, 512.0);
        cam.zoom = 100.0;
        let pitch0 = cam.pitch();
        // From north (pi) to -0.75 pi: a quarter turn on through +-pi, not
        // three quarters back.
        let mut m = CameraMove::new(&cam, 200.0, 200.0, 100.0, 1.0).with_rotation(&cam, -0.75 * PI, 0.3);
        assert!(!m.step(&mut cam, 0.0));
        assert!(cam.rotation_held && (cam.heading - PI).abs() < 1e-6 && (cam.pitch() - pitch0).abs() < 1e-6);
        assert!(!m.step(&mut cam, 0.5));
        assert!((cam.heading - 1.125 * PI).abs() < 1e-4, "halfway: {}", cam.heading);
        assert!((cam.pitch() - (pitch0 + 0.3) / 2.0).abs() < 1e-4);
        assert!(m.step(&mut cam, 0.5));
        assert!((cam.heading - 1.25 * PI).abs() < 1e-5 && cam.pitch() == 0.3 && cam.rotation_held);
        // 1.25 pi faces the same way as -0.75 pi.
        let f = cam.forward();
        assert!((f.x - (-0.75 * PI).sin() * 0.3f32.cos()).abs() < 1e-5);
    }

    /// `GetMaxZoom` (0x7ae8fb): the map's width or its height at the view's
    /// aspect, whichever is more, times the multiplier, so the whole map
    /// fits.
    #[test]
    fn farthest_zoom_fits_the_map() {
        let mut c = Camera::new(1024.0, 512.0);
        c.aspect = 16.0 / 9.0;
        c.fit_map(1024.0, 512.0);
        assert!((c.max_zoom - 1024.0f32.max(512.0 * 16.0 / 9.0) * MAX_ZOOM_MULT).abs() < 1e-3);
        c.aspect = 4.0 / 3.0;
        c.fit_map(1024.0, 512.0);
        assert!((c.max_zoom - 1024.0 * MAX_ZOOM_MULT).abs() < 1e-3);
        c.zoom = c.max_zoom;
        let (w, h) = ground_span(&c);
        assert!(w >= 1024.0 && h >= 512.0, "the whole map shows: {w} x {h}");
    }

    /// The farthest zoom is the whole-map view times the zoom-out room
    /// (moho's 1.1 by default, FAF's 1.4 through `SetMaxZoomMult`).
    #[test]
    fn max_zoom_mult_gives_room_past_the_map() {
        let mut cam = Camera::new(512.0, 512.0);
        let fit = cam.max_zoom / MAX_ZOOM_MULT;
        cam.set_max_zoom_mult(1.4);
        assert!((cam.max_zoom - fit * 1.4).abs() < 0.01);
        cam.zoom(-100.0);
        assert!((cam.zoom - fit * 1.4).abs() < 0.01, "zooms out to the new limit");
        cam.set_max_zoom_mult(1.0);
        assert!((cam.zoom - fit).abs() < 0.01, "a lower limit pulls the camera in");
    }

    /// A timed move (the opening zoom to the ACU) starts where the camera is,
    /// is halfway in log zoom at half time and arrives at the end.
    #[test]
    fn timed_move_eases_from_the_whole_map_to_the_target() {
        let mut cam = Camera::new(512.0, 512.0);
        let (x0, d0) = (cam.target.x, cam.zoom);
        let mut m = CameraMove::new(&cam, 100.0, 400.0, 40.0, 1.0);
        assert!(!m.step(&mut cam, 0.0));
        assert_eq!((cam.target.x, cam.zoom), (x0, d0), "starts from the current view");
        assert!(!m.step(&mut cam, 0.5));
        assert!((cam.zoom - (d0 * 40.0).sqrt()).abs() < 0.01, "halfway in log zoom: {}", cam.zoom);
        assert!((cam.target.x - (x0 + 100.0) / 2.0).abs() < 0.01);
        assert!(m.step(&mut cam, 0.5), "arrived");
        assert_eq!((cam.target.x, cam.target.z, cam.zoom), (100.0, 400.0, 40.0));
    }

    /// Straight down zoomed out, `cam_NearPitch` fully in, linear in log
    /// zoom; the field of view widens from 60 to 65 degrees on the way in.
    #[test]
    fn pitch_tilts_as_it_zooms_in() {
        let mut c = Camera::new(1024.0, 1024.0);
        c.zoom = c.max_zoom;
        assert!((c.pitch().to_degrees() - 89.9).abs() < 0.01);
        assert!((c.fov().to_degrees() - 60.0).abs() < 0.01);
        c.zoom = c.min_zoom;
        assert!((c.pitch().to_degrees() - 40.0).abs() < 0.01);
        assert!((c.fov().to_degrees() - 65.0).abs() < 0.01);
        // Halfway on the log scale is halfway in pitch.
        c.zoom = (c.min_zoom * c.max_zoom).sqrt();
        assert!((c.pitch().to_degrees() - (40.0 + 89.9) / 2.0).abs() < 0.05);
        let mut last = 0.0;
        for i in 0..=20 {
            c.zoom = c.min_zoom + (c.max_zoom - c.min_zoom) * i as f32 / 20.0;
            assert!(c.pitch() >= last);
            last = c.pitch();
        }
    }
}
