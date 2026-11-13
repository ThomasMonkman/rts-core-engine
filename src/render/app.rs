use crate::debug_ui::DebugUiState;
use std::sync::{Arc, Mutex};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use super::frame;
use super::state::RenderState;

/// Events other threads send the main thread.
#[derive(Debug)]
pub enum UserEvent {
    /// Head n's present thread started a frame: make its next (head 0:
    /// the main window, whose frames run the interface; head 1: FAF's
    /// second head).
    PresentReady(usize),
}

pub struct App {
    state: Option<RenderState>,
    collector: egui_tracing::EventCollector,
    debug_ui: Arc<Mutex<DebugUiState>>,
    snapshots: Option<std::sync::mpsc::Receiver<crate::engine::snapshot::Snapshot>>,
    request: crate::engine::snapshot::SharedDebugRequest,
    control: std::sync::Arc<crate::engine::snapshot::SimControl>,
    options: super::Options,
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
}

impl App {
    pub fn new(
        collector: egui_tracing::EventCollector,
        debug_ui: Arc<Mutex<DebugUiState>>,
        snapshots: std::sync::mpsc::Receiver<crate::engine::snapshot::Snapshot>,
        request: crate::engine::snapshot::SharedDebugRequest,
        control: std::sync::Arc<crate::engine::snapshot::SimControl>,
        options: super::Options,
        proxy: winit::event_loop::EventLoopProxy<UserEvent>,
    ) -> Self {
        Self {
            proxy,
            state: None,
            collector,
            debug_ui,
            snapshots: Some(snapshots),
            request,
            control,
            options,
        }
    }
}

/// The window's starting size: SupCom's minimum resolution, which FAF's
/// main menu is laid out for (anything shorter cuts off its bottom buttons).
const WINDOW_WIDTH: u32 = 1024;
const WINDOW_HEIGHT: u32 = 768;

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // On the system's main display (macOS would open it on the one the
        // cursor is on); the profile's Primary Display moves it once read.
        let mut attributes = Window::default_attributes().with_title("RTS Core Engine").with_inner_size(winit::dpi::LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT)).with_active(self.options.screenshot.is_none());
        if let Some((w, h)) = self.options.window_size {
            attributes = attributes.with_inner_size(winit::dpi::PhysicalSize::new(w, h));
        } else if let Some(m) = event_loop.primary_monitor() {
            let size = winit::dpi::LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT).to_physical(m.scale_factor());
            attributes = attributes.with_position(crate::render::state::centred(&m, size));
        }
        let window = Arc::new(event_loop.create_window(attributes).expect("failed to create window"));
        let Some(snapshots) = self.snapshots.take() else { return };
        let state = pollster::block_on(RenderState::new(
            window,
            self.collector.clone(),
            self.debug_ui.clone(),
            snapshots,
            self.request.clone(),
            self.control.clone(),
            self.options.clone(),
            self.proxy.clone(),
        ));
        let mut state = state;
        // `--second-head`: made now, before any snapshot (else once the
        // interface has read the profile's Secondary Adapter).
        if state.second_head_flag.is_some() {
            state.decide_second_head(None);
            state.make_second_head(event_loop);
        }
        self.state = Some(state);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let s = match self.state.as_mut() {
            Some(s) => s,
            None => return,
        };
        // The second head's window: its own size and scale; the cursor
        // moves onto it; keys and buttons are shared (one keyboard focus).
        let second = s.second.as_ref().and_then(|h| h.window.as_ref()).is_some_and(|w| w.id() == id);
        if second {
            match &event {
                WindowEvent::Resized(size) => {
                    s.resize_second(*size);
                    if let Some(h) = s.second.as_mut() {
                        h.visibility.minimised = size.width == 0 || size.height == 0;
                    }
                    s.apply_visibility();
                    return;
                }
                // Covered or uncovered: a hidden window's surface isn't
                // touched (its present could hold up the main window's).
                WindowEvent::Occluded(occluded) => {
                    if let Some(h) = s.second.as_mut() {
                        h.visibility.occluded = *occluded;
                    }
                    s.apply_visibility();
                    return;
                }
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    if let Some(h) = s.second.as_mut() {
                        h.scale_factor = *scale_factor as f32;
                    }
                    return;
                }
                // Onto another monitor: its refresh paces the head.
                WindowEvent::Moved(_) => {
                    if let Some(h) = s.second.as_ref() {
                        let period = h.window.as_deref().and_then(crate::render::state::vsync_period);
                        h.presenter.command(crate::render::present::Command::VsyncPeriod(period));
                    }
                    return;
                }
                // Only this window closes: the game runs on with one head.
                WindowEvent::CloseRequested => {
                    s.close_second_head();
                    return;
                }
                WindowEvent::RedrawRequested | WindowEvent::Focused(_) => return,
                _ => {}
            }
        }
        let head = usize::from(second);
        if matches!(event, WindowEvent::CursorMoved { .. } | WindowEvent::CursorEntered { .. }) && s.cursor_head != head {
            s.cursor_head = head;
            // A drag doesn't jump between the windows' pixels.
            s.input.cursor = None;
        }

        match &event {
            WindowEvent::CloseRequested => {
                // Moho writes Game.prefs as it shuts down.
                if let Some(vm) = &s.ui {
                    frame::save_prefs(s.prefs.save.as_deref(), vm);
                }
                // The session ends too: its sim stops, even mid-wait for
                // players' turns.
                s.control.close();
                exit_soon();
                event_loop.exit();
                return;
            }
            WindowEvent::Resized(size) => {
                s.resize(*size);
                s.visibility.minimised = size.width == 0 || size.height == 0;
                s.pace_main();
                s.apply_visibility();
                return;
            }
            WindowEvent::Occluded(occluded) => {
                s.visibility.occluded = *occluded;
                s.apply_visibility();
                return;
            }
            // The OS releases a confined cursor when the window loses focus.
            WindowEvent::Focused(true) => s.apply_cursor_clip(),
            // Moved to a screen of another density (a Resized follows).
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                s.scale_factor = *scale_factor as f32;
                s.ui_resized = true;
                // Wayland knows the window's monitor only after it has
                // entered an output: pace by its refresh now.
                s.pace_main();
                // The cursor image is remade at the new size.
                s.os_cursor = rts_core_engine::ui::cursor::Wanted::System;
            }
            // Onto another monitor: with a second head, its refresh paces
            // this window's frames too.
            WindowEvent::Moved(_) => s.pace_main(),
            // Frames are the present thread's (`user_event`).
            WindowEvent::RedrawRequested => return,
            _ => {}
        }

        // (egui's windows are on the main window only.)
        if !second {
            let response = s.egui_state.on_window_event(&s.window, &event);
            if response.repaint {
                s.window.request_redraw();
            }
        }
        // Camera input, unless egui is using the pointer / keyboard.
        use winit::event::{ElementState, MouseButton};
        use winit::keyboard::PhysicalKey;
        match &event {
            WindowEvent::KeyboardInput { event: key, .. } if !s.egui_ctx.egui_wants_keyboard_input() => {
                if let PhysicalKey::Code(code) = key.physical_key {
                    use winit::keyboard::KeyCode as K;
                    // Ctrl+`: the debug windows and status bar (FAF binds no
                    // Ctrl+`; plain ` is moho's console key).
                    if code == K::Backquote && s.input.mods.ctrl {
                        if key.state == ElementState::Pressed && !key.repeat {
                            s.show_debug = !s.show_debug;
                        }
                        return;
                    }
                    // While an interface edit has the focus (chat, filter
                    // boxes) keys type: no speed keys, no camera keys.
                    let typing = s.ui.as_ref().is_some_and(|vm| vm.typing());
                    if typing {
                        if key.state == ElementState::Pressed {
                            s.input.events.push((if key.repeat { super::player::Event::KeyRepeat(code) } else { super::player::Event::Key(code) }, s.input.mods));
                            // A shortcut types nothing: macOS gives Cmd+V
                            // the text "v" (Windows a control character);
                            // the key itself pastes. Ctrl+Alt is AltGr on
                            // Windows (it types `@`, `€`, ...).
                            let shortcut = s.input.mods.ctrl && !s.input.mods.alt;
                            for ch in key.text.as_deref().unwrap_or("").chars().filter(|_| !shortcut) {
                                s.input.events.push((super::player::Event::Char(ch), s.input.mods));
                            }
                        } else {
                            s.input.keys.remove(&code);
                        }
                        return;
                    }
                    // Game speed: the main keyboard's =/- step it one moho step
                    // (most Mac keyboards have no numpad); the numpad's +/-
                    // are FAF's key map's (`WLD_IncreaseSimRate`), so they
                    // aren't handled twice. Space doesn't pause: hit by
                    // accident it stopped the game with no sign why (the
                    // status bar's button and FAF's key map pause).
                    if key.state == ElementState::Pressed && !key.repeat {
                        match code {
                            K::Equal => s.control.step_moho(true),
                            K::Minus => s.control.step_moho(false),
                            _ => {}
                        }
                    }
                    if key.state == ElementState::Pressed && !key.repeat {
                        s.input.events.push((super::player::Event::Key(code), s.input.mods));
                    }
                    if key.state == ElementState::Pressed {
                        s.input.keys.insert(code);
                    } else {
                        s.input.keys.remove(&code);
                    }
                }
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                // Precise scrolls (trackpads, smooth-scroll mice): the OS
                // momentum coast after the lift is a separate phase sequence
                // (issue #3) and doesn't zoom; notched wheels pass through.
                // Tracked before the egui guard: a sequence split across
                // the interface boundary mustn't desync the tracker (its
                // world-side `Ended` would otherwise arm a phantom coast).
                let zoom = match delta {
                    winit::event::MouseScrollDelta::PixelDelta(_) => s.input.scroll.should_zoom(*phase, std::time::Instant::now()),
                    _ => true,
                };
                if s.egui_ctx.egui_wants_pointer_input() {
                    return;
                }
                // A coast over the world zooms nothing — and over a panel it
                // scrolls nothing either (list inertia is the toolkit's own;
                // coasting a panel from a world gesture would scroll it
                // without the cursor ever entering it).
                if !zoom {
                    return;
                }
                let notches = wheel_notches(delta, s.input.mods.shift);
                // Over a panel the wheel is the interface's (its lists
                // scroll); over the world it zooms.
                if let (Some(vm), Some((x, y))) = (&s.ui, s.input.cursor) {
                    let (ux, uy) = s.ui_point(x, y);
                    if vm.hit_interface(ux, uy).unwrap_or(false) {
                        s.input.events.push((super::player::Event::Wheel(x, y, notches), s.input.mods));
                        return;
                    }
                }
                s.input.wheel += notches;
            }
            WindowEvent::MouseInput { state, button: MouseButton::Middle, .. } => {
                s.input.middle_down = *state == ElementState::Pressed && !s.egui_ctx.egui_wants_pointer_input();
            }
            WindowEvent::ModifiersChanged(m) => {
                let st = m.state();
                // Cmd works as Ctrl on macOS (control groups).
                s.input.mods = super::player::Mods { shift: st.shift_key(), ctrl: st.control_key() || st.super_key(), alt: st.alt_key() };
            }
            WindowEvent::MouseInput { state, button: button @ (MouseButton::Left | MouseButton::Right), .. } => {
                use super::player::Event as E;
                let Some((x, y)) = s.input.cursor else { return };
                let pressed = *state == ElementState::Pressed;
                // Presses over egui windows belong to them; releases always
                // pass (they end a drag that started in the world).
                if pressed && s.egui_ctx.egui_wants_pointer_input() {
                    return;
                }
                let ev = match (button, pressed) {
                    (MouseButton::Left, true) => E::LeftDown(x, y),
                    (MouseButton::Left, false) => E::LeftUp(x, y),
                    (_, true) => E::RightDown(x, y),
                    (_, false) => E::RightUp(x, y),
                };
                s.input.events.push((ev, s.input.mods));
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let (Some((x, y)), true) = (s.input.cursor, s.input.middle_down) {
                    s.input.drag.0 += position.x - x;
                    s.input.drag.1 += position.y - y;
                }
                s.input.cursor = Some((position.x, position.y));
                s.input.cursor_moved(position.x, position.y, s.input.mods);
            }
            _ => {}
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        rts_core_engine::ui::clipboard::release();
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _device: winit::event::DeviceId, event: winit::event::DeviceEvent) {
        if let (Some(s), winit::event::DeviceEvent::MouseMotion { delta }) = (self.state.as_mut(), event) {
            s.input.motion.0 += delta.0;
            s.input.motion.1 += delta.1;
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let UserEvent::PresentReady(head) = event;
        let Some(s) = self.state.as_mut() else { return };
        if head == 1 {
            frame::second_frame(s);
            return;
        }
        frame::render_frame(s);
        // The second head the interface's profile asked for.
        s.make_second_head(event_loop);
        apply_cursor(s, event_loop);
        // The interface's Exit (`ExitApplication`).
        if let Some(vm) = s.ui.as_ref().filter(|vm| vm.exit_requested().unwrap_or(false)) {
            frame::save_prefs(s.prefs.save.as_deref(), vm);
            event_loop.exit();
            return;
        }
        // A `--ticks` run's sim has finished and gone: close, once any
        // screenshot is taken.
        if s.exit_when_sim_ends && s.world.sim_ended && s.screenshot.is_none() {
            tracing::info!("sim ticks complete: closing the window");
            event_loop.exit();
        }
    }
}

/// How much to enlarge a cursor image so it shows at its logical size: macOS
/// (winit makes the cursor's image `width x height` points) and Wayland (the
/// compositor scales the cursor surface) size it themselves; Windows
/// (`CreateIconIndirect`) and X11 take it in physical pixels.
fn cursor_image_scale(window: &winit::window::Window, ui_scale: f32) -> f32 {
    if cfg!(target_os = "macos") {
        return 1.0;
    }
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Wayland(_)) => 1.0,
        _ => ui_scale,
    }
}

/// `rgba` (`w` x `h`) resampled by `scale` (bilinear); unchanged at 1.
fn scale_rgba(rgba: &[u8], w: u32, h: u32, scale: f32) -> (Vec<u8>, u32, u32) {
    if (scale - 1.0).abs() < 1e-3 || w == 0 || h == 0 {
        return (rgba.to_vec(), w, h);
    }
    let (nw, nh) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
    let texel = |x: u32, y: u32, c: usize| rgba[((y.min(h - 1) * w + x.min(w - 1)) * 4) as usize + c] as f32;
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        let fy = ((y as f32 + 0.5) / scale - 0.5).max(0.0);
        let (y0, ty) = (fy.floor() as u32, fy.fract());
        for x in 0..nw {
            let fx = ((x as f32 + 0.5) / scale - 0.5).max(0.0);
            let (x0, tx) = (fx.floor() as u32, fx.fract());
            for c in 0..4 {
                let top = texel(x0, y0, c) * (1.0 - tx) + texel(x0 + 1, y0, c) * tx;
                let bottom = texel(x0, y0 + 1, c) * (1.0 - tx) + texel(x0 + 1, y0 + 1, c) * tx;
                out[((y * nw + x) * 4) as usize + c] = (top * (1.0 - ty) + bottom * ty).round() as u8;
            }
        }
    }
    (out, nw, nh)
}

/// Makes FAF's cursor the window's (a hardware cursor follows the mouse
/// with no frame of lag): each texture decoded once, from its `.dds`, with
/// its hotspot; one that fails to load leaves the system cursor.
fn apply_cursor(s: &mut super::state::RenderState, event_loop: &ActiveEventLoop) {
    use rts_core_engine::ui::cursor::Wanted;
    use winit::window::{CursorIcon, CustomCursor};
    if s.ui_cursor == s.os_cursor {
        return;
    }
    s.os_cursor = s.ui_cursor.clone();
    let scale = cursor_image_scale(&s.window, s.ui_scale());
    let custom = match &s.os_cursor {
        Wanted::Texture(t) => {
            let vfs = s.world.models_vfs();
            s.os_cursors
                .entry(format!("{}@{scale}", t.path))
                .or_insert_with(|| {
                    let bytes = vfs.as_ref().and_then(|v| v.read().ok()?.read(&t.path.to_lowercase()).ok())?;
                    let tex = rts_core_engine::engine::assets::dds::read(&bytes, false).ok()?;
                    let (rgba, w, h) = scale_rgba(tex.levels.first()?, tex.width, tex.height, scale);
                    let (w, h) = (u16::try_from(w).ok()?, u16::try_from(h).ok()?);
                    let hot = |v: f32, max: u16| ((v * scale).max(0.0) as u16).min(max.saturating_sub(1));
                    match CustomCursor::from_rgba(rgba, w, h, hot(t.hotspot[0], w), hot(t.hotspot[1], h)) {
                        Ok(source) => Some(event_loop.create_custom_cursor(source)),
                        Err(e) => {
                            tracing::warn!(path = %t.path, "cursor image rejected: {e}");
                            None
                        }
                    }
                })
                .clone()
        }
        _ => None,
    };
    let second = s.second.as_ref().and_then(|h| h.window.as_ref());
    for window in std::iter::once(&s.window).chain(second) {
        window.set_cursor_visible(s.os_cursor != Wanted::Hidden && s.spinning.is_none());
        match &custom {
            Some(c) => window.set_cursor(c.clone()),
            None => window.set_cursor(CursorIcon::Default),
        }
    }
}

/// Wheel notches (up is positive) from a scroll event. macOS turns a
/// mouse wheel turned with Shift held into a horizontal scroll (the
/// vertical delta arrives as x, y is 0), so with Shift held and no vertical
/// movement the horizontal delta is the wheel: Shift+wheel still zooms, as
/// in SupCom (placing a queue of structures holds Shift).
fn wheel_notches(delta: &winit::event::MouseScrollDelta, shift: bool) -> f32 {
    use winit::event::MouseScrollDelta;
    let (x, y) = match delta {
        MouseScrollDelta::LineDelta(x, y) => (*x, *y),
        MouseScrollDelta::PixelDelta(p) => (p.x as f32 / 40.0, p.y as f32 / 40.0),
    };
    if shift && y == 0.0 { x } else { y }
}

/// Where a precise (`PixelDelta`) scroll sequence is in its gesture /
/// momentum cycle (issue #3). macOS reports a smooth scroll and the
/// momentum coast after the lift as adjacent `Started…Ended` sequences
/// (probed on one smooth-scroll mouse, 12 gestures / 9 coasts: every coast
/// onset arrives <1 ms after the gesture's `Ended`, while every genuine
/// new gesture arrives ≥68 ms later — the finger has to touch down again
/// first; trackpads and other platforms still to probe). Dropping a
/// sequence whose onset immediately follows an `Ended` stops the zoom
/// gliding on after the fingers stop; the coast is the OS finishing a
/// gesture, not new input. `LineDelta` (notched wheels) isn't tracked: its
/// phase is ignored by design and it always zooms.
#[derive(Default)]
pub(crate) struct ScrollState {
    phase: ScrollPhase,
    /// When the last sequence ended (to time the next onset against).
    ended_at: Option<std::time::Instant>,
    /// When the live coast started (to time out a lost coast `Ended`: a
    /// coast can't outlast this, so an onset past it classifies fresh).
    coast_started_at: Option<std::time::Instant>,
    /// The coast just closed and its successor hasn't classified yet: the
    /// next onset is either its retake (gesture) or the next coast (drop).
    /// Set on every coast `Ended`, consumed by the very next onset — so it
    /// never survives past one sequence.
    coast_closed: bool,
    /// A retake gesture is live while its superseded coast hasn't sent its
    /// own `Ended` yet (order A). That stale `Ended` must leave the gesture
    /// alone; the gesture's own `Ended` closes it. Only ever set alongside
    /// a live gesture, cleared by the first `Ended` in that state.
    retake_live: bool,
}

#[derive(Default, PartialEq)]
enum ScrollPhase {
    #[default]
    Idle,
    Gesture,
    Momentum,
}

/// A coast onset follows its gesture's `Ended` within this long; a new
/// gesture needs a touch-down first (probed: coasts <1 ms, gestures
/// ≥68 ms, so 10 ms separates them with room on both sides).
const MOMENTUM_ONSET_WITHIN: std::time::Duration = std::time::Duration::from_millis(10);

/// A coast never runs this long (probed: the longest was ~0.9 s), so a
/// sequence older than this whose `Ended` never arrived is a driver-dropped
/// event, not a live coast: the tracker recovers instead of wedging.
const COAST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl ScrollState {
    /// Whether a `PixelDelta` event with this phase zooms. `now` is the
    /// event's arrival time (the caller passes `Instant::now()`; tests pass
    /// explicit times, so no clock stalls can flake them).
    pub(crate) fn should_zoom(&mut self, phase: winit::event::TouchPhase, now: std::time::Instant) -> bool {
        use winit::event::TouchPhase as T;
        match (phase, &self.phase) {
            (T::Started, ScrollPhase::Idle) => {
                // The coast just closed: this onset is either its retake
                // (a gesture after a touch-down gap) or the next coast (a
                // fast continuation). The gap decides; either way the marker
                // is consumed — it never survives past this onset.
                // Accepted tradeoff of the single threshold: a genuine
                // second gesture within 10 ms of a gesture `Ended` (no coast
                // between) also reads as a coast and drops. The probe margin
                // (≥68 ms) makes that implausible for fingers, but fast
                // successive smooth-mouse flicks could hit it.
                if self.coast_closed {
                    self.coast_closed = false;
                    let retake = self.ended_at.is_some_and(|t| now.duration_since(t) > MOMENTUM_ONSET_WITHIN);
                    self.ended_at = None;
                    if retake {
                        self.phase = ScrollPhase::Gesture;
                        return true;
                    }
                    self.coast_started_at = Some(now);
                    self.phase = ScrollPhase::Momentum;
                    return false;
                }
                let coast = self.ended_at.is_some_and(|t| now.duration_since(t) <= MOMENTUM_ONSET_WITHIN);
                if coast {
                    self.coast_started_at = Some(now);
                    self.phase = ScrollPhase::Momentum;
                    false
                } else {
                    self.ended_at = None;
                    self.phase = ScrollPhase::Gesture;
                    true
                }
            }
            (T::Started, ScrollPhase::Momentum) => {
                // A touch-down mid-coast (order A) retakes the zoom; the
                // stale coast's `Ended` below recognises the retake and
                // leaves the gesture alone. Past the coast timeout the
                // `Ended` was lost, not delayed: abandon the dead coast and
                // classify fresh.
                if self.coast_started_at.is_some_and(|t| now.duration_since(t) > COAST_TIMEOUT) {
                    self.coast_started_at = None;
                    self.phase = ScrollPhase::Idle;
                    self.ended_at = None;
                    self.phase = ScrollPhase::Gesture;
                    return true;
                }
                // The coast is superseded, not live: its start time dies
                // with it (the timeout above must not read stale data).
                self.coast_started_at = None;
                self.retake_live = true;
                self.phase = ScrollPhase::Gesture;
                true
            }
            (T::Started, ScrollPhase::Gesture) => {
                // A doubled `Started` just joins the live sequence — the
                // retake marker survives it, so the still-pending stale
                // coast `Ended` is recognised below. (If that `Ended` was
                // lost instead of duplicated, the gesture's own `Ended`
                // consumes the marker and closes normally; a following coast
                // onset then fail-opens — it zooms — rather than wedging the
                // tracker shut. Fail-open beats fail-closed here: at worst
                // one glide gets through after a driver-dropped event.)
                true
            }
            (T::Ended, ScrollPhase::Momentum) => {
                self.phase = ScrollPhase::Idle;
                self.coast_started_at = None;
                self.ended_at = Some(now);
                // The successor onset classifies itself against the gap
                // (retake vs next coast) and consumes this.
                self.coast_closed = true;
                false
            }
            (T::Ended, ScrollPhase::Gesture) => {
                // The stale coast's `Ended` while its retake is live: the
                // gesture continues, nothing closes. (Only a retake sets
                // this flag, and only the first `Ended` consumes it — the
                // gesture's own `Ended` still closes below.)
                if self.retake_live {
                    self.retake_live = false;
                    return true;
                }
                self.phase = ScrollPhase::Idle;
                self.ended_at = Some(now);
                true
            }
            (T::Ended, ScrollPhase::Idle) => {
                // A stray `Ended` (egui swallowed the onset, a driver
                // omitted phases): zoom nothing, arm nothing.
                false
            }
            (T::Cancelled, _) => {
                // A cancel carries no coast information (unprobed): back to
                // idle without arming a coast window, so the next gesture
                // always zooms. Clears the retake/coast markers too — the
                // sequences they belonged to are over.
                self.phase = ScrollPhase::Idle;
                self.coast_started_at = None;
                self.ended_at = None;
                self.coast_closed = false;
                self.retake_live = false;
                false
            }
            (T::Moved, ScrollPhase::Momentum) => false,
            (T::Moved, _) => {
                // A stray `Moved` with no live gesture (phases omitted):
                // apply it rather than freezing the zoom.
                true
            }
        }
    }

}

/// The window was closed: the process ends within [`EXIT_GRACE`] even if a
/// thread holds it up (the engine thread stuck in a stalled networked
/// game kept a closed game running until it was killed). A clean shutdown
/// finishes well before.
fn exit_soon() {
    const EXIT_GRACE: std::time::Duration = std::time::Duration::from_secs(3);
    let _ = std::thread::Builder::new().name("exit".into()).spawn(|| {
        std::thread::sleep(EXIT_GRACE);
        tracing::warn!("the window closed but the process didn't end: exiting");
        std::process::exit(0);
    });
}

#[cfg(test)]
mod tests {
    use super::{ScrollState, scale_rgba, wheel_notches};
    use winit::dpi::PhysicalPosition;
    use winit::event::{MouseScrollDelta, TouchPhase as T};

    /// Shift+wheel zooms (issue #8): macOS delivers it as a horizontal
    /// scroll, which still counts as the wheel; unshifted horizontal scroll
    /// doesn't zoom, and an ordinary vertical wheel is unchanged.
    #[test]
    fn shift_wheel_still_zooms() {
        assert_eq!(wheel_notches(&MouseScrollDelta::LineDelta(0.0, 1.0), false), 1.0);
        assert_eq!(wheel_notches(&MouseScrollDelta::LineDelta(0.0, -2.0), true), -2.0);
        assert_eq!(wheel_notches(&MouseScrollDelta::LineDelta(-1.0, 0.0), true), -1.0);
        assert_eq!(wheel_notches(&MouseScrollDelta::LineDelta(1.0, 0.0), false), 0.0);
        assert_eq!(wheel_notches(&MouseScrollDelta::PixelDelta(PhysicalPosition::new(80.0, 0.0)), true), 2.0);
        assert_eq!(wheel_notches(&MouseScrollDelta::PixelDelta(PhysicalPosition::new(80.0, 0.0)), false), 0.0);
    }

    /// Deterministic event times: the base plus whole milliseconds, so no
    /// wall-clock stall can flake the onset-window asserts.
    fn at(base: std::time::Instant, ms: u64) -> std::time::Instant {
        base + std::time::Duration::from_millis(ms)
    }

    /// The momentum coast after a precise scroll (issue #3) doesn't zoom:
    /// a gesture applies, the coast onset immediately after its `Ended`
    /// drops, and the next gesture — after the finger touches down again —
    /// applies. Notched wheels aren't tracked (out of scope here).
    #[test]
    fn momentum_coast_does_not_zoom() {
        let (mut s, t0) = (ScrollState::default(), std::time::Instant::now());
        assert!(s.should_zoom(T::Started, at(t0, 0)));
        assert!(s.should_zoom(T::Moved, at(t0, 8)));
        assert!(s.should_zoom(T::Ended, at(t0, 16)));
        assert!(!s.should_zoom(T::Started, at(t0, 17)));
        assert!(!s.should_zoom(T::Moved, at(t0, 25)));
        assert!(!s.should_zoom(T::Ended, at(t0, 600)));
        assert!(s.should_zoom(T::Started, at(t0, 668)));
        assert!(s.should_zoom(T::Moved, at(t0, 676)));
        assert!(s.should_zoom(T::Ended, at(t0, 684)));
    }

    /// A tap with no gesture before it applies (the onset only drops right
    /// after an `Ended`).
    #[test]
    fn onset_without_history_applies() {
        let (mut s, t0) = (ScrollState::default(), std::time::Instant::now());
        assert!(s.should_zoom(T::Started, at(t0, 0)));
        assert!(s.should_zoom(T::Ended, at(t0, 8)));
    }

    /// A touch-down mid-coast retakes the zoom, whichever order the stale
    /// coast `Ended` arrives in: the gesture applies throughout, the stale
    /// `Ended` changes nothing, and the gesture's own `Ended` closes it.
    #[test]
    fn touch_down_mid_coast_retakes_zoom() {
        for order_a in [true, false] {
            let (mut s, t0) = (ScrollState::default(), std::time::Instant::now());
            assert!(s.should_zoom(T::Started, at(t0, 0)));
            assert!(s.should_zoom(T::Moved, at(t0, 8)));
            assert!(s.should_zoom(T::Ended, at(t0, 16)));
            assert!(!s.should_zoom(T::Started, at(t0, 17)));
            assert!(!s.should_zoom(T::Moved, at(t0, 25)));
            if order_a {
                // Retake `Started` first, stale coast `Ended` mid-gesture.
                assert!(s.should_zoom(T::Started, at(t0, 40)));
                assert!(s.should_zoom(T::Moved, at(t0, 48)));
                assert!(s.should_zoom(T::Ended, at(t0, 50)));
                assert!(s.should_zoom(T::Moved, at(t0, 56)));
                assert!(s.should_zoom(T::Ended, at(t0, 64)));
            } else {
                // Stale coast `Ended` first, retake `Started` after a
                // touch-down gap: a gesture, not a coast (a real retake
                // needs the finger back down; probed gaps ≥68 ms, while a
                // coast onset lands <1 ms after the gesture `Ended`).
                assert!(!s.should_zoom(T::Ended, at(t0, 33)));
                assert!(s.should_zoom(T::Started, at(t0, 101)));
                assert!(s.should_zoom(T::Moved, at(t0, 109)));
                assert!(s.should_zoom(T::Ended, at(t0, 117)));
            }
            // Back to idle: the next gesture zooms (no phantom window).
            assert!(s.should_zoom(T::Started, at(t0, 200)));
            assert!(s.should_zoom(T::Ended, at(t0, 208)));
        }
    }

    /// Back-to-back coasts (a flick, coast, then another flick with no
    /// gesture between) each drop: the second onset classifies by timing,
    /// not by the first coast's handoff marker.
    #[test]
    fn consecutive_coasts_each_drop() {
        let (mut s, t0) = (ScrollState::default(), std::time::Instant::now());
        assert!(s.should_zoom(T::Started, at(t0, 0)));
        assert!(s.should_zoom(T::Ended, at(t0, 8)));
        assert!(!s.should_zoom(T::Started, at(t0, 9)));
        assert!(!s.should_zoom(T::Ended, at(t0, 400)));
        assert!(!s.should_zoom(T::Started, at(t0, 401)));
        assert!(!s.should_zoom(T::Ended, at(t0, 700)));
        assert!(s.should_zoom(T::Started, at(t0, 900)));
        assert!(s.should_zoom(T::Ended, at(t0, 908)));
    }

    /// A cancel resets without arming a coast window: the next gesture
    /// always zooms. A doubled `Started` joins the live sequence, and
    /// stray phases zoom nothing (but also never wedge the tracker).
    #[test]
    fn cancel_doubled_and_stray_phases() {
        let (mut s, t0) = (ScrollState::default(), std::time::Instant::now());
        assert!(s.should_zoom(T::Started, at(t0, 0)));
        assert!(!s.should_zoom(T::Cancelled, at(t0, 8)));
        assert!(s.should_zoom(T::Started, at(t0, 9)));
        assert!(s.should_zoom(T::Ended, at(t0, 17)));

        assert!(s.should_zoom(T::Started, at(t0, 100)));
        assert!(s.should_zoom(T::Started, at(t0, 101)));
        assert!(s.should_zoom(T::Moved, at(t0, 108)));
        assert!(s.should_zoom(T::Ended, at(t0, 116)));

        assert!(!s.should_zoom(T::Ended, at(t0, 200)));
        assert!(s.should_zoom(T::Moved, at(t0, 208)));
        assert!(s.should_zoom(T::Started, at(t0, 300)));
        assert!(s.should_zoom(T::Ended, at(t0, 308)));
    }

    /// A cancel mid-retake clears everything: the next onset starts fresh
    /// (a gesture), not classified against the cancelled sequences — for
    /// both retake orders (stale `Ended` still pending, or already seen).
    #[test]
    fn cancel_mid_retake_resets() {
        for order_a in [true, false] {
            let (mut s, t0) = (ScrollState::default(), std::time::Instant::now());
            assert!(s.should_zoom(T::Started, at(t0, 0)));
            assert!(s.should_zoom(T::Ended, at(t0, 8)));
            assert!(!s.should_zoom(T::Started, at(t0, 9)));
            if order_a {
                assert!(s.should_zoom(T::Started, at(t0, 20)));
            } else {
                assert!(!s.should_zoom(T::Ended, at(t0, 17)));
            }
            assert!(!s.should_zoom(T::Cancelled, at(t0, 28)));
            assert!(s.should_zoom(T::Started, at(t0, 29)));
            assert!(s.should_zoom(T::Ended, at(t0, 37)));
        }
    }

    /// A lost stale coast `Ended` (driver dropped it) fail-opens rather
    /// than wedging: the gesture's own `Ended` consumes the retake marker
    /// and closes, and a following coast onset zooms once (fail-open)
    /// instead of freezing the zoom — then the tracker is back to idle.
    #[test]
    fn lost_stale_ended_fail_opens() {
        let (mut s, t0) = (ScrollState::default(), std::time::Instant::now());
        assert!(s.should_zoom(T::Started, at(t0, 0)));
        assert!(s.should_zoom(T::Ended, at(t0, 8)));
        assert!(!s.should_zoom(T::Started, at(t0, 9)));
        assert!(s.should_zoom(T::Started, at(t0, 20)));
        // The coast's `Ended` never arrives; the gesture's own `Ended`
        // consumes the retake marker and leaves the gesture open (the
        // marker is spent, so the next `Ended` will close it).
        assert!(s.should_zoom(T::Ended, at(t0, 28)));
        // Still open from the lost `Ended`: this coast onset joins the live
        // gesture and fail-opens (zooms) instead of wedging shut.
        assert!(s.should_zoom(T::Started, at(t0, 29)));
        assert!(s.should_zoom(T::Ended, at(t0, 400)));
        // Back to idle: the next gesture classifies normally.
        assert!(s.should_zoom(T::Started, at(t0, 900)));
        assert!(s.should_zoom(T::Ended, at(t0, 908)));
    }

    /// A lost coast `Ended` can't wedge the tracker shut: past the coast
    /// timeout the dead coast is abandoned and the onset zooms.
    #[test]
    fn lost_coast_ended_times_out() {
        let (mut s, t0) = (ScrollState::default(), std::time::Instant::now());
        assert!(s.should_zoom(T::Started, at(t0, 0)));
        assert!(s.should_zoom(T::Ended, at(t0, 8)));
        assert!(!s.should_zoom(T::Started, at(t0, 9)));
        // The coast's `Ended` never arrives; six seconds later the coast is
        // dead and this onset classifies fresh.
        assert!(s.should_zoom(T::Started, at(t0, 6009)));
        assert!(s.should_zoom(T::Ended, at(t0, 6017)));
    }

    /// A cursor image grows by the scale (Windows / X11 at 150%: 32 -> 48
    /// pixels); at 1 it is untouched.
    #[test]
    fn cursor_images_scale_bilinearly() {
        let img: Vec<u8> = (0..4u8).flat_map(|i| [i * 60, 0, 0, 255]).collect();
        assert_eq!(scale_rgba(&img, 2, 2, 1.0), (img.clone(), 2, 2));
        let (out, w, h) = scale_rgba(&img, 2, 2, 1.5);
        assert_eq!((w, h, out.len()), (3, 3, 36));
        assert_eq!(out[0], 0, "corners keep their texel");
        assert_eq!(out[(8 * 4) as usize], 180);
        assert!(out.chunks(4).all(|p| p[3] == 255));
    }
}
