//! The egui stats bar along the bottom of the window, and its network
//! panel (the peers' links, and the [conditioner](rts_core_engine::net::conditioner)
//! that makes this client's connection bad on purpose).
//!
//! One line of fixed-width readouts (monospace, padded) so nothing moves as
//! the numbers change; the detail is in each readout's tooltip. On a
//! narrow window the readouts that don't fit move to a second line (least
//! useful first) rather than being cut off mid-number.
//!
//! Render thread only: reads snapshots and the network's shared state,
//! never sim state.

use std::time::{Duration, Instant};

use egui::{Color32, RichText, Ui};
use rts_core_engine::net::conditioner::{self, Conditions, Dir};

use crate::engine::snapshot::SimControl;

/// One frame's numbers for the bar (copied out of the render state before
/// egui runs).
pub struct Stats {
    /// Seconds between frames (eased).
    pub frame_interval: f32,
    pub cpu_ms: f32,
    pub cpu_p99: f32,
    pub render_ms: f32,
    /// 0 when the GPU timer isn't available.
    pub gpu_ms: f32,
    pub ram: Option<u64>,
    pub vram: Option<u64>,
    pub model_draws: usize,
    pub particle_draws: usize,
    pub ui_draws: u32,
    /// The sim's last tick's cost (ms), and percentiles of recent ones.
    pub tick_ms: f32,
    pub tick_p50: f32,
    pub tick_p99: f32,
    /// Ticks per second arriving now.
    pub tick_rate: f32,
    pub tick: u64,
    /// Units, structures, projectiles, props.
    pub entity_counts: [usize; 4],
    pub effects: usize,
    pub particles: usize,
    pub log_lines: usize,
    /// Recent frame intervals and tick costs (ms), oldest first.
    pub frame_history: Vec<f32>,
    pub tick_history: Vec<f32>,
}

/// The bar's own state, kept across frames.
#[derive(Default)]
pub struct BarUi {
    /// The network panel is open.
    pub net_open: bool,
    pub net: NetView,
}

/// What the bar shows of the network, refreshed a few times a second (the
/// network thread holds the same locks).
#[derive(Default, Clone)]
pub struct NetView {
    /// In a lobby or a game.
    pub connected: bool,
    pub peers: Vec<PeerRow>,
    /// Our input delay (ticks), once a game runs.
    pub our_delay: Option<u32>,
    pub our_max_speed: Option<u32>,
    /// Players whose turns are late (we're waiting for them).
    pub waiting_on: Vec<String>,
    pub desyncs: u64,
    refreshed: Option<Instant>,
}

#[derive(Clone)]
pub struct PeerRow {
    pub name: String,
    pub path: &'static str,
    pub ping_ms: f64,
    pub ping99_ms: f64,
    pub delay: Option<u32>,
    pub max_speed: Option<u32>,
}

/// A peer's turn this late means we're waiting for it (turns come every tick).
const LATE_TURN_MS: f64 = 500.0;

impl NetView {
    pub fn refresh(&mut self) {
        if self.refreshed.is_some_and(|t| t.elapsed() < Duration::from_millis(250)) {
            return;
        }
        let refreshed = Some(Instant::now());
        let Some(net) = rts_core_engine::net::core::current() else {
            *self = NetView { refreshed, ..NetView::default() };
            return;
        };
        let grid = net.ping_grid();
        let paths: std::collections::BTreeMap<u32, &'static str> = grid
            .rows
            .get(&grid.local)
            .map(|r| {
                r.iter()
                    .map(|p| {
                        let path = match p.path {
                            rts_core_engine::net::wire::PathKind::Udp => "udp",
                            rts_core_engine::net::wire::PathKind::IrohDirect => "direct",
                            rts_core_engine::net::wire::PathKind::IrohRelay => "relay",
                        };
                        (p.peer, path)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let peers = net
            .peers()
            .into_iter()
            .map(|p| PeerRow {
                path: paths.get(&p.id).copied().unwrap_or("udp"),
                delay: grid.input_delay.get(&p.id).copied(),
                max_speed: grid.max_speed.get(&p.id).copied(),
                name: p.name,
                ping_ms: p.ping_ms,
                ping99_ms: p.ping99_ms,
            })
            .collect();
        let waiting_on = net.session_clients().iter().filter(|c| net.quiet_ms(c) > LATE_TURN_MS).map(|c| c.name.clone()).collect();
        *self = NetView {
            connected: true,
            peers,
            our_delay: grid.input_delay.get(&grid.local).copied(),
            our_max_speed: grid.max_speed.get(&grid.local).copied(),
            waiting_on,
            desyncs: net.desync_count(),
            refreshed,
        };
    }

    /// The slowest link's round trip (ms).
    fn worst_ping(&self) -> Option<f64> {
        self.peers.iter().map(|p| p.ping_ms).max_by(f64::total_cmp)
    }
}

/// What limits the frame rate: the CPU's or GPU's work per frame when it
/// fills the frame interval, else vsync (both finish with time to spare).
pub fn frame_bound(interval_ms: f32, cpu_ms: f32, gpu_ms: f32) -> &'static str {
    let work = cpu_ms.max(gpu_ms);
    if work < interval_ms * 0.85 {
        "vsync"
    } else if gpu_ms > cpu_ms {
        "gpu"
    } else {
        "cpu"
    }
}

/// How long a frame took to make: the slowest of the main thread, the render
/// thread and the GPU, which work on successive frames in parallel. Unlike
/// the frame interval it leaves out time spent waiting on vsync.
fn frame_work_ms(b: &Stats) -> f32 {
    b.cpu_ms.max(b.render_ms).max(b.gpu_ms)
}

/// The game clock (10 ticks a second).
pub fn game_clock(tick: u64) -> String {
    let secs = tick / 10;
    format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

/// A count in at most 4 characters: `999`, `1.2k`, `12k`, `1.2M`, `12M`.
pub fn compact(n: usize) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..9_950 => format!("{:.1}k", n as f64 / 1e3),
        9_950..999_500 => format!("{:.0}k", n as f64 / 1e3),
        999_500..9_950_000 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{:.0}M", n as f64 / 1e6),
    }
}

/// Milliseconds in 4 characters: `12.3`, ` 0.4`, ` 123`, `9999`.
fn ms4(ms: f32) -> String {
    if ms < 99.95 { format!("{ms:>4.1}") } else { format!("{:>4.0}", ms.min(9999.0)) }
}

/// The ticks a second the sim could run at, from its typical tick cost.
fn capacity(tick_p50: f32) -> Option<f32> {
    (tick_p50 > 0.0).then(|| 1000.0 / tick_p50)
}

/// The tick rate the game speed asks for (`None`: as fast as it goes).
fn target_rate(speed_percent: u32) -> Option<f32> {
    (speed_percent != SimControl::MAX).then(|| speed_percent as f32 / 10.0)
}

/// A readout's colour: ok, warn or bad.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Level {
    Ok,
    Warn,
    Bad,
}

fn level_color(ui: &Ui, level: Level) -> Color32 {
    match level {
        Level::Ok => ui.visuals().weak_text_color(),
        Level::Warn => ui.visuals().warn_fg_color,
        Level::Bad => ui.visuals().error_fg_color,
    }
}

/// The readouts, in the order shown.
struct Readouts {
    fps: String,
    sim: String,
    time: String,
    entities: String,
    net: String,
    net_level: Level,
}

fn readouts(b: &Stats, net: &NetView) -> Readouts {
    let ms = b.frame_interval.max(1e-4);
    let fps = 1.0 / ms;
    let bound = frame_bound(ms * 1000.0, b.cpu_ms.max(b.render_ms), b.gpu_ms);
    let took = frame_work_ms(b);
    let cap = capacity(b.tick_p50).map_or("   -".to_owned(), |c| format!("{:>4}", compact(c as usize)));
    let [units, structures, projectiles, props] = b.entity_counts;
    // Always shown, so the conditions can be set before a game starts.
    let net_text = if !net.connected {
        if conditioner::active() { "offline shaped".to_owned() } else { "offline".to_owned() }
    } else {
        let mut t = match net.worst_ping() {
            Some(p) => format!("{:>3.0}ms", p),
            None => "  -".to_owned(),
        };
        if let Some(d) = net.our_delay {
            t += &format!(" d{d:<2}");
        }
        if conditioner::active() {
            t += " shaped";
        }
        if net.desyncs > 0 {
            t += &format!(" DESYNC x{}", net.desyncs);
        }
        if let Some(first) = net.waiting_on.first() {
            t += &format!(" waiting on {first}");
        }
        t
    };
    let net_level = if net.desyncs > 0 || !net.waiting_on.is_empty() {
        Level::Bad
    } else if conditioner::active() {
        Level::Warn
    } else {
        Level::Ok
    };
    Readouts {
        fps: format!("{fps:>3.0} fps {}ms {bound:<5}", ms4(took)),
        sim: format!("sim {}ms (p99 {}) {:>4.1} t/s (max {cap})", ms4(b.tick_ms), ms4(b.tick_p99), b.tick_rate),
        time: format!("{} {:>5}", game_clock(b.tick), b.tick),
        entities: format!(
            "{} units {} structs {} proj {} props {} fx",
            compact(units),
            compact(structures),
            compact(projectiles),
            compact(props),
            compact(b.effects)
        ),
        net: net_text,
        net_level,
    }
}

/// Which readouts fit on the first line in `room` (px), given each one's
/// width: those in `drop_order` move to the second line first.
fn fitting(widths: &[f32], drop_order: &[usize], room: f32) -> Vec<bool> {
    let mut shown = vec![true; widths.len()];
    let mut total: f32 = widths.iter().sum();
    for &i in drop_order {
        if total <= room {
            break;
        }
        shown[i] = false;
        total -= widths[i];
    }
    shown
}

/// A one-line summary for bug reports (the bar's copy button).
fn summary(b: &Stats, control: &SimControl, net: &NetView) -> String {
    let r = readouts(b, net);
    let mut line = format!(
        "build {} | tick {} ({}) speed {}% | {} | cpu {:.1}ms p99 {:.1} render {:.1}ms gpu {:.1}ms | {} p50 {:.1} | {} | {} particles",
        rts_core_engine::net::core::build_id(),
        b.tick,
        game_clock(b.tick),
        control.effective(),
        r.fps.split_whitespace().collect::<Vec<_>>().join(" "),
        b.cpu_ms,
        b.cpu_p99,
        b.render_ms,
        b.gpu_ms,
        r.sim.split_whitespace().collect::<Vec<_>>().join(" "),
        b.tick_p50,
        r.entities.split_whitespace().collect::<Vec<_>>().join(" "),
        b.particles,
    );
    if net.connected {
        let peers: Vec<String> = net.peers.iter().map(|p| format!("{} {:.0}/{:.0}ms {}", p.name, p.ping_ms, p.ping99_ms, p.path)).collect();
        line += &format!(" | net delay {:?} peers [{}] desyncs {}", net.our_delay, peers.join(", "), net.desyncs);
        for (dir, name) in [(Dir::Out, "out"), (Dir::In, "in")] {
            let c = conditioner::get(dir);
            if !c.is_off() {
                line += &format!(" shaped {name} {}+{}ms loss {:.1}% spike 1/{} +{}ms", c.latency_ms, c.jitter_ms, c.loss_permille as f32 / 10.0, c.spike_every, c.spike_ms);
            }
        }
    }
    line
}

fn mono(text: &str, color: Color32) -> RichText {
    RichText::new(text).monospace().color(color)
}

fn text_width(ui: &Ui, text: &str) -> f32 {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    ui.painter().layout_no_wrap(text.to_owned(), font, Color32::WHITE).size().x
}

/// A line graph of `samples` (ms), scaled to their peak.
fn sparkline(ui: &mut Ui, samples: &[f32], what: &str) {
    if samples.len() < 2 {
        return;
    }
    let (rect, _) = ui.allocate_exact_size(egui::vec2(260.0, 48.0), egui::Sense::hover());
    let peak = samples.iter().copied().fold(0.1f32, f32::max);
    let last = (samples.len() - 1) as f32;
    let points: Vec<egui::Pos2> = samples
        .iter()
        .enumerate()
        .map(|(i, v)| egui::pos2(rect.left() + rect.width() * i as f32 / last, rect.bottom() - rect.height() * (v / peak).min(1.0)))
        .collect();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.0_f32, ui.visuals().text_color())));
    ui.weak(format!("{what}: last {} samples, peak {peak:.1} ms", samples.len()));
}

/// The bar. `show_status` false: just the button that brings it back.
pub fn show(ui: &mut Ui, b: &Stats, control: &SimControl, bar: &mut BarUi, show_status: &mut bool) {
    if !*show_status {
        egui::Area::new(egui::Id::new("status toggle")).anchor(egui::Align2::RIGHT_BOTTOM, [-4.0, -4.0]).show(ui.ctx(), |ui| {
            if ui.small_button("⏶").on_hover_text("show the stats bar").clicked() {
                *show_status = true;
            }
        });
        net_window(ui.ctx(), bar);
        return;
    }
    let r = readouts(b, &bar.net);
    // Shown in this order; entities last (unpadded: its width moves nothing).
    const ORDER: [usize; 5] = [0, 1, 2, 4, 3];
    let mut overflow = Vec::new();
    egui::Panel::bottom("status").show_inside(ui, |ui| {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("⏷").on_hover_text("hide the stats bar").clicked() {
                    *show_status = false;
                }
                if ui.small_button("copy").on_hover_text("copy these stats (for a bug report)").clicked() {
                    ui.ctx().copy_text(summary(b, control, &bar.net));
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let room = ui.max_rect();
                    ui.set_clip_rect(room.intersect(ui.clip_rect()));
                    speed_controls(ui, control);
                    // Which fit on this line; the rest go on a second.
                    let gap = ui.spacing().item_spacing.x * 2.0 + 2.0;
                    let texts = [&r.fps, &r.sim, &r.time, &r.entities, &r.net];
                    let mut widths: Vec<f32> = texts.iter().map(|t| text_width(ui, t) + gap).collect();
                    widths[4] += text_width(ui, "net ") + ui.spacing().button_padding.x * 2.0 + ui.spacing().item_spacing.x;
                    let shown = fitting(&widths, &[3, 2, 4, 1, 0], ui.available_width());
                    for i in ORDER {
                        if !shown[i] {
                            overflow.push(i);
                        } else {
                            ui.separator();
                            readout(ui, i, &r, b, control, bar);
                        }
                    }
                });
            });
        });
        if !overflow.is_empty() {
            ui.horizontal(|ui| {
                for (n, &i) in overflow.iter().enumerate() {
                    if n > 0 {
                        ui.separator();
                    }
                    readout(ui, i, &r, b, control, bar);
                }
            });
        }
    });
    net_window(ui.ctx(), bar);
}

/// Readout `i` (fps, sim, time, entities, net) with its tooltip.
fn readout(ui: &mut Ui, i: usize, r: &Readouts, b: &Stats, control: &SimControl, bar: &mut BarUi) {
    let weak = ui.visuals().weak_text_color();
    match i {
        0 => {
            ui.label(mono(&r.fps, weak)).on_hover_ui(|ui| fps_tooltip(ui, b));
        }
        1 => {
            ui.label(mono(&r.sim, weak)).on_hover_ui(|ui| sim_tooltip(ui, b, control));
        }
        2 => {
            ui.label(mono(&r.time, weak)).on_hover_text("Game time, and the sim tick (10 a second).");
        }
        3 => {
            ui.label(mono(&r.entities, weak)).on_hover_ui(|ui| {
                let [u, s, p, pr] = b.entity_counts;
                ui.label(format!("{u} units, {s} structures, {p} projectiles, {pr} props"));
                ui.label(format!("{} effects ({} particles)", b.effects, b.particles));
                ui.weak("Ctrl+` opens the debug windows.");
            });
        }
        _ => {
            if ui.small_button("net").on_hover_text("the network panel: links and test conditions").clicked() {
                bar.net_open = !bar.net_open;
            }
            ui.label(mono(&r.net, level_color(ui, r.net_level))).on_hover_ui(|ui| net_tooltip(ui, &bar.net));
        }
    }
}

/// Pause and game speed (+/- keys), first: they'd shift about after the
/// readouts.
fn speed_controls(ui: &mut Ui, control: &SimControl) {
    let mut speed = control.speed();
    let paused = control.paused();
    if ui.selectable_label(paused, "⏸").on_hover_text("pause").clicked() {
        control.set_paused(!paused);
    }
    let fixed = control.speed_fixed();
    ui.add_enabled_ui(!fixed, |ui| {
        let combo = egui::ComboBox::from_id_salt("speed").width(52.0).selected_text(SimControl::label(speed)).show_ui(ui, |ui| {
            for s in SimControl::STEPS {
                ui.selectable_value(&mut speed, s, SimControl::label(s));
            }
        });
        combo.response.on_disabled_hover_text("The lobby's Game Speed option fixes the speed (set it to adjustable to change it in game).");
    });
    if speed != control.speed() {
        control.set_speed(speed);
    }
}

fn fps_tooltip(ui: &mut Ui, b: &Stats) {
    let ms = b.frame_interval.max(1e-4) * 1000.0;
    ui.label(format!("{:.1} ms to make a frame (the slowest below), {:.1} ms between frames", frame_work_ms(b), ms));
    ui.label(format!("cpu {:.1} ms (p99 {:.1}): the main thread (input, the interface's Lua)", b.cpu_ms, b.cpu_p99));
    ui.label(format!("render {:.1} ms: the render thread (prepare, encode, submit; not vsync)", b.render_ms));
    if b.gpu_ms > 0.0 {
        ui.label(format!("gpu {:.1} ms: time the GPU spent drawing", b.gpu_ms));
    }
    ui.label(format!("bound: {}", frame_bound(ms, b.cpu_ms.max(b.render_ms), b.gpu_ms)));
    let mem: Vec<String> = [b.ram.map(|v| format!("ram {}", super::memory::format_bytes(v))), b.vram.map(|v| format!("vram {}", super::memory::format_bytes(v)))].into_iter().flatten().collect();
    if !mem.is_empty() {
        ui.label(mem.join("  "));
    }
    ui.label(format!("draws: {} models, {} particles, {} interface", b.model_draws, b.particle_draws, b.ui_draws));
    ui.label(format!("{} log lines", b.log_lines));
    sparkline(ui, &b.frame_history, "frame time");
}

fn sim_tooltip(ui: &mut Ui, b: &Stats, control: &SimControl) {
    ui.label(format!("last tick {:.1} ms, p50 {:.1}, p99 {:.1}", b.tick_ms, b.tick_p50, b.tick_p99));
    match target_rate(control.effective()) {
        Some(t) => ui.label(format!("{:.1} ticks/s now, {t:.1} asked for", b.tick_rate)),
        None => ui.label(format!("{:.1} ticks/s now, as fast as it goes", b.tick_rate)),
    };
    if let Some(c) = capacity(b.tick_p50) {
        ui.label(format!("max: about {c:.0} ticks/s at the typical tick cost"));
    }
    if control.effective() < control.shown() {
        ui.colored_label(ui.visuals().warn_fg_color, format!("running at {}% of {}%: a machine can't keep up", control.effective(), control.shown()));
    }
    sparkline(ui, &b.tick_history, "tick cost");
}

fn net_tooltip(ui: &mut Ui, net: &NetView) {
    ui.label("Worst round trip, and our input delay (ticks). The net button opens the network panel.");
    if conditioner::active() {
        ui.colored_label(ui.visuals().warn_fg_color, "shaped: the network panel is adding latency or loss");
    }
    for name in &net.waiting_on {
        ui.colored_label(ui.visuals().error_fg_color, format!("waiting for {name}'s turns"));
    }
    if net.desyncs > 0 {
        ui.colored_label(ui.visuals().error_fg_color, format!("{} desync(s) this game", net.desyncs));
    }
}

/// The network panel: the links, and the conditioner.
fn net_window(ctx: &egui::Context, bar: &mut BarUi) {
    if !bar.net_open {
        return;
    }
    let net = &bar.net;
    let mut open = true;
    egui::Window::new("Network").open(&mut open).default_width(420.0).show(ctx, |ui| {
        if !net.connected {
            ui.weak("Not connected (no lobby or game). Conditions set here apply to the next connection.");
        }
        if !net.peers.is_empty() {
            egui::Grid::new("net peers").striped(true).show(ui, |ui| {
                for h in ["peer", "path", "ping", "p99", "delay", "max speed"] {
                    ui.strong(h);
                }
                ui.end_row();
                for p in &net.peers {
                    ui.label(&p.name);
                    ui.label(p.path);
                    ui.monospace(format!("{:>5.0}ms", p.ping_ms));
                    ui.monospace(format!("{:>5.0}ms", p.ping99_ms));
                    ui.monospace(p.delay.map_or("-".into(), |d| format!("{d}t")));
                    ui.monospace(p.max_speed.map_or("-".into(), |s| format!("{s}%")));
                    ui.end_row();
                }
            });
            let ours = |v: Option<u32>, unit: &str| v.map_or("-".to_owned(), |v| format!("{v}{unit}"));
            ui.label(format!("our input delay {}, our max speed {}", ours(net.our_delay, " ticks"), ours(net.our_max_speed, "%")));
            for name in &net.waiting_on {
                ui.colored_label(ui.visuals().error_fg_color, format!("waiting for {name}'s turns"));
            }
            if net.desyncs > 0 {
                ui.colored_label(ui.visuals().error_fg_color, format!("{} desync(s) this game", net.desyncs));
            }
        }
        ui.separator();
        ui.strong("Conditions (this client only)");
        ui.weak("Latency, jitter, spikes and loss on this client's packets, for testing. The round trip a peer sees is outgoing plus incoming. Not saved: they end with the run.");
        ui.horizontal_wrapped(|ui| {
            for (name, c) in conditioner::PRESETS {
                let on = conditioner::get(Dir::Out) == c && conditioner::get(Dir::In) == c;
                if ui.selectable_label(on, name).clicked() {
                    conditioner::set(Dir::Out, c);
                    conditioner::set(Dir::In, c);
                }
            }
        });
        let (mut out, mut inn) = (conditioner::get(Dir::Out), conditioner::get(Dir::In));
        egui::Grid::new("net conditions").show(ui, |ui| {
            ui.label("");
            ui.strong("outgoing");
            ui.strong("incoming");
            ui.end_row();
            let rows: [(&str, &str, fn(&mut Conditions) -> &mut u32, u32); 5] = [
                ("latency", " ms", |c| &mut c.latency_ms, 2000),
                ("jitter", " ms", |c| &mut c.jitter_ms, 2000),
                ("loss", "%", |c| &mut c.loss_permille, 1000),
                ("spike every", " packets", |c| &mut c.spike_every, 10_000),
                ("spike", " ms", |c| &mut c.spike_ms, 5000),
            ];
            for (name, unit, field, max) in rows {
                ui.label(name);
                for c in [&mut out, &mut inn] {
                    let drag = egui::DragValue::new(field(c)).range(0..=max).suffix(unit);
                    // Loss is stored per thousand and shown as a percentage to 0.1.
                    let drag = if unit == "%" {
                        drag.custom_formatter(|v, _| format!("{:.1}", v / 10.0)).custom_parser(|t| t.trim().trim_end_matches('%').parse::<f64>().ok().map(|p| p * 10.0))
                    } else {
                        drag
                    };
                    ui.add(drag);
                }
                ui.end_row();
            }
            ui.label("packets");
            for dir in [Dir::Out, Dir::In] {
                let (seen, dropped, held) = conditioner::counts(dir);
                ui.weak(format!("{seen} seen, {dropped} lost, {held} held"));
            }
            ui.end_row();
        });
        conditioner::set(Dir::Out, out);
        conditioner::set(Dir::In, inn);
    });
    bar.net_open = open;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_bound_names_the_limit() {
        assert_eq!(frame_bound(16.7, 6.0, 9.0), "vsync");
        assert_eq!(frame_bound(33.0, 31.0, 12.0), "cpu");
        assert_eq!(frame_bound(33.0, 12.0, 31.0), "gpu");
    }

    #[test]
    fn game_clock_formats_hours_minutes_seconds() {
        assert_eq!(game_clock(0), "00:00:00");
        assert_eq!(game_clock(9), "00:00:00");
        assert_eq!(game_clock(12_000), "00:20:00");
        assert_eq!(game_clock(36_615), "01:01:01");
    }

    #[test]
    fn compact_counts_stay_four_characters() {
        assert_eq!(compact(0), "0");
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1_234), "1.2k");
        assert_eq!(compact(9_960), "10k");
        assert_eq!(compact(123_456), "123k");
        assert_eq!(compact(2_500_000), "2.5M");
        for n in [0, 7, 999, 1000, 9_949, 9_950, 99_999, 999_499, 999_500, 9_999_999] {
            assert!(compact(n).chars().count() <= 4, "{n}: {}", compact(n));
        }
    }

    #[test]
    fn target_rate_follows_the_speed() {
        assert_eq!(target_rate(100), Some(10.0));
        assert_eq!(target_rate(SimControl::MAX), None);
    }

    #[test]
    fn narrow_bars_drop_readouts_in_priority_order() {
        let widths = [100.0, 200.0, 80.0, 250.0, 60.0];
        let order = [3, 2, 4, 1, 0];
        assert_eq!(fitting(&widths, &order, 1000.0), vec![true; 5]);
        assert_eq!(fitting(&widths, &order, 450.0), vec![true, true, true, false, true]);
        assert_eq!(fitting(&widths, &order, 300.0), vec![true, true, false, false, false]);
        assert_eq!(fitting(&widths, &order, 10.0), vec![false; 5]);
    }

    #[test]
    fn readouts_keep_their_width_as_numbers_change() {
        let stats = |fps: f32, tick_ms: f32, tick: u64, units: usize| Stats {
            frame_interval: 1.0 / fps,
            cpu_ms: 3.0,
            cpu_p99: 4.0,
            render_ms: 2.0,
            gpu_ms: 0.0,
            ram: None,
            vram: None,
            model_draws: 0,
            particle_draws: 0,
            ui_draws: 0,
            tick_ms,
            tick_p50: tick_ms,
            tick_p99: tick_ms,
            tick_rate: 10.0,
            tick,
            entity_counts: [units, 0, 0, 0],
            effects: 0,
            particles: 0,
            log_lines: 0,
            frame_history: Vec::new(),
            tick_history: Vec::new(),
        };
        let net = NetView::default();
        let a = readouts(&stats(9.0, 0.4, 5, 3), &net);
        let b = readouts(&stats(144.0, 12.3, 54_321, 1_234), &net);
        let c = readouts(&stats(2.0, 250.0, 99_999, 0), &net);
        assert_eq!(a.fps.len(), c.fps.len(), "{:?} {:?}", a.fps, c.fps);
        assert_eq!(a.sim.len(), c.sim.len(), "{:?} {:?}", a.sim, c.sim);
        assert_eq!(a.fps.len(), b.fps.len(), "{:?} {:?}", a.fps, b.fps);
        assert_eq!(a.sim.len(), b.sim.len(), "{:?} {:?}", a.sim, b.sim);
        assert_eq!(a.time.len(), b.time.len());
            }
}
