//! `--headless-with-ui`: FAF's interface without a window.
//!
//! The sim runs headless on its own thread with a snapshot sink in lockstep
//! with this loop (`snapshot::SnapshotSink::lockstep`). Each snapshot goes
//! into a render-side [`World`] as a window's would (no GPU: its scene
//! deltas are dropped), then the interface gets what a window's frame gives
//! it: the Sync tables, the units, one beat and one frame of
//! [`FRAME_SECONDS`]. Its orders and `SimCallback`s become player commands
//! through [`PlayerControl`], on the window's order channel. Only then is
//! the tick acknowledged, so the sim applies them on the next tick, the
//! same tick in every run of the game.
//!
//! Left out (nothing to look at): cameras moving, picks, the draw list,
//! sounds. A frame-paced window may run several frames a tick or several
//! ticks a frame, so its commands can land on other ticks than here.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use rts_core_engine::engine::sim::input::PlayerCommand;
use rts_core_engine::engine::snapshot::{Snapshot, ViewKind};
use rts_core_engine::ui::{UnitInfo, UserVm};

use super::player::PlayerControl;
use super::world::World;

/// The interface's clock per sim tick: one frame of a game second's tenth.
pub const FRAME_SECONDS: f32 = 0.1;

/// The screen the interface lays out on (logical pixels).
const SCREEN: (u32, u32) = (1920, 1080);

pub struct Options {
    /// `--prefs` (else none: the player's own preferences would make runs
    /// differ between machines). Never written back.
    pub prefs: Option<std::path::PathBuf>,
    /// `--ui-lua`: run in the interface once it has been up 6 s.
    pub ui_lua: Option<String>,
    /// `--player`.
    pub player: Option<u32>,
}

struct Driver {
    world: World,
    player: PlayerControl,
    vm: Option<UserVm>,
    booting: Option<UserVm>,
    failed: bool,
    prefs: Option<std::path::PathBuf>,
    ui_lua: Option<String>,
    ui_tick: u64,
    selection: Vec<u32>,
    alliances: Option<Arc<Vec<(u32, u32, rts_core_engine::engine::sim::army::Alliance)>>>,
    army_display: Option<Arc<Vec<rts_core_engine::engine::snapshot::ArmyDisplay>>>,
    custom_names: Option<Arc<Vec<(u32, String)>>>,
    game_over_told: bool,
}

/// Runs the interface on the snapshots from `rx` until the sim ends (the
/// channel closes), acknowledging each on `handled` once its commands are
/// on `orders`.
pub fn run(rx: Receiver<Snapshot>, handled: Sender<u64>, orders: Sender<PlayerCommand>, options: Options) {
    let mut d = Driver {
        world: World::new(SCREEN.0, SCREEN.1),
        player: PlayerControl::new(options.player, Vec::new(), Some(orders)),
        vm: None,
        booting: None,
        failed: false,
        prefs: options.prefs,
        ui_lua: options.ui_lua,
        ui_tick: u64::MAX,
        selection: Vec::new(),
        alliances: None,
        army_display: None,
        custom_names: None,
        game_over_told: false,
    };
    // `World::receive` takes what's waiting on a channel: one snapshot at a
    // time, handed on through this one.
    let (one_tx, one) = std::sync::mpsc::channel();
    while let Ok(snap) = rx.recv() {
        let tick = snap.tick;
        let _ = one_tx.send(snap);
        drop(d.world.receive(&one, SCREEN.0 as f32 / SCREEN.1 as f32));
        // The interface's units are the snapshot's (a window's are
        // interpolated between two); props aren't units.
        if let Some(latest) = &d.world.latest {
            d.world.frame.entities = Arc::new(latest.entities.iter().filter(|e| e.kind != ViewKind::Prop).copied().collect());
        }
        if !d.failed {
            d.update();
        }
        if handled.send(tick).is_err() {
            break;
        }
    }
}

/// The sim sent a new `theirs` since the interface was given `mine`.
fn changed<T>(mine: &Option<Arc<T>>, theirs: &Option<Arc<T>>) -> bool {
    theirs.as_ref().is_some_and(|t| !mine.as_ref().is_some_and(|m| Arc::ptr_eq(m, t)))
}

impl Driver {
    /// The interface as `frame::update_ui` starts it: the VM and
    /// `userInit.lua` once the file system and the session's mods are
    /// known, the game interface once the armies' units are in.
    fn boot(&mut self) {
        let Some(vfs) = self.world.models_vfs() else { return };
        if self.booting.is_none() {
            let Some(mods) = rts_core_engine::engine::mods::session() else { return };
            let vm = match self.world.script_cache.clone() {
                Some(cache) => UserVm::with_cache(vfs, cache),
                None => UserVm::new(vfs),
            };
            let vm = vm.inspect(|vm| {
                if let Some(p) = &self.prefs
                    && let Err(e) = vm.load_prefs(p)
                {
                    tracing::warn!("preferences {}: {e}", p.display());
                }
            });
            match vm.and_then(|vm| vm.activate_session_mods(&mods).map(|_| vm)).and_then(|vm| vm.init().map(|_| vm)) {
                Ok(vm) => self.booting = Some(vm),
                Err(e) => {
                    tracing::error!("user layer failed to start: {e}");
                    self.failed = true;
                    return;
                }
            }
        }
        let w = &self.world;
        let (Some(armies), Some(scenario)) = (w.armies.clone(), w.scenario.clone()) else { return };
        if w.tick < 10 && !w.entities().iter().any(|e| e.army != 0) {
            return;
        }
        let Some(vm) = self.booting.take() else { return };
        let campaign_mode = w.pending_sync.iter().any(|v| v.sets_campaign_mode());
        self.player.adopt_army(w.local_army, false);
        let observer = self.player.player.is_none();
        let started = vm
            .set_armies(&armies)
            .and_then(|_| vm.set_army_display(w.army_display.as_deref().map_or(&[], |c| c.as_slice())))
            .and_then(|_| vm.set_alliances(w.alliances.as_deref().map_or(&[], |a| a.as_slice())))
            .and_then(|_| vm.set_scenario(&scenario))
            .and_then(|_| vm.set_session_flags(false, observer))
            .and_then(|_| vm.set_focus_army(self.player.player.map_or(-1, |a| a as i64)))
            .and_then(|_| vm.set_screen(SCREEN.0 as f32, SCREEN.1 as f32))
            .and_then(|_| vm.start_hud_with(campaign_mode));
        match started {
            Ok(()) => {
                tracing::info!(tick = w.tick, "user layer started (headless)");
                self.alliances = w.alliances.clone();
                self.army_display = w.army_display.clone();
                self.vm = Some(vm);
            }
            Err(e) => {
                tracing::error!("user layer failed to start: {e}");
                self.failed = true;
            }
        }
    }

    /// One tick of the interface (the parts of `frame::update_ui` that
    /// don't need a window), then its commands to the sim.
    fn update(&mut self) {
        if self.vm.is_none() {
            self.boot();
        }
        let Some(vm) = &self.vm else { return };
        let w = &self.world;
        let states: Vec<rts_core_engine::ui::CameraState> = w
            .cameras
            .iter()
            .map(|(name, c)| rts_core_engine::ui::CameraState { name: name.to_owned(), focus: c.target.into(), zoom: c.zoom, min: c.min_zoom, max: c.max_zoom, pitch: c.pitch(), heading: c.heading })
            .collect();
        let _ = vm.set_camera_states(&states);
        let _ = vm.take_camera_requests();
        if let Some(f) = self.world.focus_army_changed.take() {
            self.player.player = u32::try_from(f).ok().filter(|&a| a > 0);
            if let Err(e) = vm.set_focus_army(f.into()) {
                tracing::warn!("UI focus army: {e}");
            }
        }
        let w = &self.world;
        if changed(&self.alliances, &w.alliances) {
            if let Some(a) = &w.alliances
                && let Err(e) = vm.set_alliances(a)
            {
                tracing::warn!("UI alliances: {e}");
            }
            self.alliances = w.alliances.clone();
        }
        if changed(&self.army_display, &w.army_display) {
            if let Some(c) = &w.army_display
                && let Err(e) = vm.set_army_display(c)
            {
                tracing::warn!("UI army display: {e}");
            }
            self.army_display = w.army_display.clone();
        }
        if changed(&self.custom_names, &w.custom_names) {
            if let Some(n) = &w.custom_names
                && let Err(e) = vm.set_custom_names(n)
            {
                tracing::warn!("UI custom names: {e}");
            }
            self.custom_names = w.custom_names.clone();
        }
        let infos = |w: &World, ids: &[u32]| -> Vec<UnitInfo> {
            ids.iter()
                .filter_map(|&id| w.entities().binary_search_by_key(&id, |e| e.id).ok().map(|i| &w.entities()[i]))
                .filter(|e| e.is_interface_unit())
                .map(|e| UnitInfo { detail: w.debug().and_then(|d| d.units.iter().find(|u| u.id == e.id).cloned()), ..UnitInfo::from_view(e, w.blueprint_name(e.blueprint)) })
                .collect()
        };
        for v in std::mem::take(&mut self.world.pending_sync) {
            if let Err(e) = vm.sync(&v) {
                tracing::warn!("UI sync: {e}");
            }
        }
        let w = &self.world;
        if w.game_over && !self.game_over_told {
            self.game_over_told = true;
            if let Err(e) = vm.set_game_over() {
                tracing::warn!("UI game over: {e}");
            }
        }
        if let Ok(Some(ids)) = vm.take_selection_request() {
            self.player.selection = ids;
        }
        if self.player.selection != self.selection {
            let _ = vm.update_units(&infos(w, &self.player.selection));
            if let Err(e) = vm.set_selection(&self.player.selection) {
                tracing::warn!("UI selection: {e}");
            }
            self.selection = self.player.selection.clone();
        }
        if w.tick != self.ui_tick {
            vm.set_world(w.entities().iter().filter(|e| e.army != 0 && e.is_interface_unit()).map(|e| UnitInfo::from_view(e, w.blueprint_name(e.blueprint))).collect());
            vm.set_busy(w.latest.as_ref().map_or(&[][..], |l| l.busy.as_slice()));
            vm.set_unselectable(w.entities().iter().filter(|e| e.attached || e.in_builder || w.unselectable(e)).map(|e| e.id).collect());
            if let Some(d) = &w.deposits {
                vm.set_deposits(d.clone());
            }
            let _ = vm.refresh_held_units(&self.selection, |others| infos(w, others));
            if vm.update_units(&infos(w, &self.selection)).unwrap_or(false)
                && let Err(e) = vm.refresh_selection()
            {
                tracing::warn!("UI selection refresh: {e}");
            }
            self.ui_tick = w.tick;
            if let Err(e) = vm.beat(w.sim_tick, &w.economy) {
                tracing::warn!("UI beat: {e}");
            }
        }
        if let Err(e) = vm.frame(FRAME_SECONDS) {
            tracing::warn!("UI frame: {e}");
        }
        if vm.time() > 6.0
            && let Some(code) = self.ui_lua.take()
            && let Err(e) = rts_core_engine::ui::run_source(vm.lua(), &code, "=--ui-lua")
        {
            tracing::warn!(target: "lua", "--ui-lua failed: {e}");
        }
        // What a window does with them (`frame::update_ui`).
        let orders = match vm.take_orders() {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!("UI orders: {e}");
                return;
            }
        };
        for o in orders {
            if let Some(callback) = o.callback {
                tracing::info!(sim_tick = w.sim_tick, func = %callback.func, "interface SimCallback");
                self.player.sim_callback_ui(o.units.unwrap_or_default(), callback);
                continue;
            }
            if let Some(t) = o.toggle {
                self.player.toggle_ui(w, o.units.as_deref(), t);
                continue;
            }
            if !o.script.is_empty() {
                self.player.script_ui(w, o.units.as_deref(), o.script, !o.clear);
                continue;
            }
            if o.command == "UNITCOMMAND_Dock" {
                self.player.issue_dock(w, o.units.as_deref(), !o.clear);
                continue;
            }
            match rts_core_engine::ui::command_kind(&o.command) {
                Some(kind) => {
                    use rts_core_engine::engine::sim::command::CommandKind as K;
                    let queue = if matches!(kind, K::BuildFactory | K::Upgrade) { !o.clear } else { false };
                    self.player.issue_ui(w, o.units.as_deref(), kind, o.blueprint.as_deref(), o.count, queue);
                }
                None => tracing::debug!(command = o.command, "interface order not supported yet"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rts_core_engine::engine;

    /// One headless game of `ticks` with the interface, which sends a ping
    /// (`--ui-lua`) once up: the sim's checksum at the end and the commands
    /// it applied (its recording).
    fn game(ticks: u64, name: &str) -> (u64, Vec<engine::sim::input::Scheduled>) {
        let record = std::env::temp_dir().join(format!("rts-headless-ui-{}-{name}.rec", std::process::id()));
        let (mut sink, rx, _) = engine::snapshot::channel();
        let handled = sink.lockstep();
        let (orders, link) = engine::sim::input::channel();
        let path = record.clone();
        let sim = std::thread::spawn(move || {
            let vault = std::path::Path::new("../faf-vault");
            let mut engine = engine::Engine::new_with_vault(crate::INIT_LUA_PATH, crate::FA_PATH, std::sync::Arc::new(std::sync::Mutex::new(rts_core_engine::debug_ui::DebugUiState::new())), vault.exists().then_some(vault)).expect("engine");
            engine.set_snapshot_sink(sink);
            engine.set_input_link(link);
            engine.record_commands(&path).expect("recording");
            engine.start(&engine::session::SessionConfig { ticks: Some(ticks), create_initial_units: true, ai: true, ai_armies: 2, ..Default::default() }).expect("session");
            engine.checksum()
        });
        let ping = "SimCallback({Func = 'SpawnPing', Args = {Owner = 1, Type = 'alert', Location = {100, 0, 100}}})";
        run(rx, handled, orders, Options { prefs: None, ui_lua: Some(ping.into()), player: None });
        let checksum = sim.join().expect("sim thread");
        let text = std::fs::read_to_string(&record).expect("recording");
        let _ = std::fs::remove_file(&record);
        let applied = text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).map(|l| engine::sim::input::decode(l).expect("replay line")).collect();
        (checksum, applied)
    }

    #[test]
    fn interface_callbacks_reach_the_sim_on_the_same_tick_every_run() {
        if !rts_core_engine::test_data::require_game("headless interface") {
            return;
        }
        let callbacks = |applied: &[engine::sim::input::Scheduled]| applied.iter().filter_map(|s| Some((s.tick, s.command.callback.as_ref()?.func.clone()))).collect::<Vec<_>>();
        let (first, applied) = game(150, "a");
        let sent = callbacks(&applied);
        // `--ui-lua` runs 6 interface seconds in (one frame a tick): the
        // ping goes out with tick 60's reply and runs at the next.
        assert!(sent.contains(&(61, "SpawnPing".to_string())), "the interface's ping reached the sim: {sent:?}");
        let (second, again) = game(150, "b");
        assert_eq!(callbacks(&again), sent, "the same commands on the same ticks");
        assert_eq!(second, first, "the same game");
    }
}
