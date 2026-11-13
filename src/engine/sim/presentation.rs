//! Presentation-only state created by sim scripts (decals, splats, ...).
//!
//! Consumed by the renderer via snapshots; it never feeds back into sim logic
//! and is excluded from the sim checksum, so plain floats are fine here.

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Splat {
    pub position: [f64; 3],
    pub heading: f64,
    pub texture: String,
    pub size: [f64; 2],
    pub lod: f64,
    /// Seconds; 0 = never expires.
    pub duration: f64,
    /// Owning army (visible to its allies), `None` = everyone.
    pub army: Option<u32>,
    /// Sim tick it was created on (`duration` counts from here).
    pub created: u64,
}

/// A textured ground decal (`CreateDecal`), e.g. scorch marks and craters.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Decal {
    pub position: [f64; 3],
    pub heading: f64,
    pub textures: [String; 2],
    /// `DecalType`: "Albedo", "Normals", "Water Mask", ...
    pub kind: String,
    pub size: [f64; 2],
    pub lod: f64,
    /// Seconds; 0 = never expires.
    pub duration: f64,
    /// Visible to this army's allies, `None` = everyone.
    pub army: Option<u32>,
    /// Sim tick it was created on (`duration` counts from here).
    pub created: u64,
}

/// A camera shake around an entity (`Entity:ShakeCamera`), for the user
/// layer's camera.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CameraShake {
    pub entity: u32,
    pub radius: f64,
    pub max_shake: f64,
    pub min_shake: f64,
    /// Seconds.
    pub duration: f64,
}

/// A one-shot world sound (`Entity:PlaySound`, `Weapon:PlaySound`): the
/// next snapshot carries it to the audio thread. Presentation only.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SoundEvent {
    pub entity: Option<u32>,
    pub bank: std::sync::Arc<str>,
    pub cue: std::sync::Arc<str>,
    pub lod_cutoff: Option<std::sync::Arc<str>>,
    /// Where it plays (the entity's position when it was asked for; the
    /// entity may be destroyed the same tick).
    pub position: Option<[f32; 3]>,
}

/// A looping ambient cue attached to an entity (`Entity:SetAmbientSound`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AmbientSound {
    pub bank: std::sync::Arc<str>,
    pub cue: std::sync::Arc<str>,
}

/// Sounds kept for a renderer that is behind (fast game speeds): older
/// ones are dropped, they'd be late anyway.
pub const MAX_PENDING_SOUNDS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EffectKind {
    Emitter,
    /// Flash of light (`CreateLightParticle*`); `intel` lights also grant
    /// the army brief vision.
    Light { intel: bool },
    /// Ribbon following a bone (`CreateTrail`).
    Trail,
    /// Beam from the effect's anchor to `beam_to` (or of fixed length).
    Beam,
}

/// A particle effect (`moho.IEffect`), consumed by the renderer.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Effect {
    pub kind: EffectKind,
    pub blueprint: String,
    pub army: Option<u32>,
    /// World position at creation (bone or entity origin).
    pub position: [f64; 3],
    /// Entity + bone the effect follows (`None` = stays where it spawned).
    pub attached: Option<(u32, Option<u32>)>,
    /// Entity + bone it was spawned at (attached or not), for the renderer
    /// to place it on the posed bone.
    pub source: Option<(u32, Option<u32>)>,
    pub scale: f64,
    pub offset: [f64; 3],
    /// Named emitter/beam parameters (`SetEmitterParam`, `SetBeamParam`).
    pub params: std::collections::BTreeMap<String, f64>,
    /// Curve overrides by parameter: (height, size) (`SetEmitterCurveParam`).
    pub curves: std::collections::BTreeMap<String, (f64, f64)>,
    /// Curves stretched to a new length in ticks (`ResizeEmitterCurve`).
    pub curve_lengths: std::collections::BTreeMap<String, f64>,
    /// Light colour ramp texture (lights only).
    pub ramp: Option<String>,
    /// `blueprint` and `ramp` interned in [`Presentation::assets`]
    /// (`u32::MAX`: none), set as the effect is created.
    pub asset: u32,
    pub ramp_asset: u32,
    /// Beams: the far end (entity, bone).
    pub beam_to: Option<(u32, Option<u32>)>,
    /// Sim tick it was created on.
    pub created: u64,
    /// The blueprint's `Lifetime` in ticks (negative / `None` = forever);
    /// a `LIFETIME` parameter overrides it.
    pub lifetime: Option<f64>,
}

/// A trail whose entity was destroyed this tick, with where the entity
/// ended: the snapshot shows it once more there, so the renderer lays its
/// last segment to the impact point rather than stopping a tick short.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EndedTrail {
    pub id: u32,
    pub effect: Box<Effect>,
    pub position: crate::engine::sim::math::FxVec3,
    pub rotation: crate::engine::sim::math::FxQuat,
}

/// A launch a camera following `launcher` switches to (moho's
/// `SCamFollowParams`): it follows `projectile`, then holds where that ended
/// for `timeout` seconds before going back.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CameraFollow {
    pub launcher: u32,
    pub projectile: u32,
    pub timeout: f32,
}

#[derive(Default, Debug, serde::Serialize, serde::Deserialize)]
pub struct Presentation {
    /// Trails ended this tick (`api::effect::expire`), for this tick's
    /// snapshot only.
    pub ended_trails: Vec<EndedTrail>,
    /// Permanent splats (part of the ground decals).
    pub splats: Vec<Splat>,
    /// Timed splats (footprints, tread marks from scripts) not yet sent to
    /// the renderer, which fades them itself; dropped once their duration
    /// is up (headless runs never take them).
    pub fresh_splats: Vec<Splat>,
    /// Sound events since the last delivered snapshot (headless runs drop
    /// them each tick).
    pub sounds: Vec<SoundEvent>,
    /// Pending camera shakes, drained like `sounds`.
    pub camera_shakes: Vec<CameraShake>,
    /// Projectiles launched that a camera following their launcher
    /// follows (`Display.CameraFollowsProjectile`), drained like `sounds`.
    pub camera_follows: Vec<CameraFollow>,
    /// Projectiles launched since the last delivered snapshot, with where
    /// they were launched (projectile id, position), in id order. A shot
    /// flies its first tick in the same beat it is fired (moho's motion
    /// stage runs after the script stage), so the renderer, which has no
    /// last-tick position for it, lerps it from here instead of showing it
    /// a tick of flight in front of the muzzle. Drained like `sounds`; not
    /// in keyframes.
    #[serde(skip)]
    pub launches: Vec<(u32, [f32; 3])>,
    /// Upgrades completed since the last delivered snapshot (old structure,
    /// new one; `NotifyUpgrade`): the interface moves its selection and
    /// control groups over. Drained like `sounds`.
    pub upgrades: Vec<(u32, u32)>,
    /// Current looping ambient cue per entity id (absent = silent).
    pub ambient: std::collections::BTreeMap<u32, AmbientSound>,
    /// Live effects by id.
    /// Boxed: big maps of them (tens of thousands of emitters in large
    /// air battles) move only pointers when nodes split or shrink.
    /// Effect assets (blueprint / texture paths, lowercased) by index, and
    /// the index of each path as given (`intern_asset`). Render
    /// bookkeeping: the sim never reads them.
    pub assets: Vec<String>,
    asset_ids: std::collections::BTreeMap<String, u32>,
    /// Private so every change goes through [`Presentation::add_effect`],
    /// [`Presentation::effect_mut`] and [`Presentation::remove_effect`],
    /// which keep [`EffectIndex`] in step; read with [`Presentation::effects`].
    effects: std::collections::BTreeMap<u32, Box<Effect>>,
    pub next_effect_id: u32,
    /// Which effects end when (derived from `effects`). Not in keyframes: a
    /// loaded game rebuilds it on first use, and its ending effects come
    /// out sorted, so the same.
    #[serde(skip)]
    effect_index: EffectIndex,
    /// Live decals by id (`CDecalHandle:Destroy` removes one).
    pub decals: std::collections::BTreeMap<u32, Decal>,
    pub next_decal_id: u32,
    /// Bumped whenever a decal or splat comes or goes (snapshots resend
    /// the ground decals only then).
    pub decal_version: u64,
    /// Bumped whenever a unit's `SetCustomName` runs (snapshots resend the
    /// names only then; ids are never reused, so a dead unit's name is
    /// harmless).
    pub custom_name_version: u64,
}

/// What an effect is indexed under: the tick it ends on (by its lifetime)
/// and the entities it hangs on (attached to, beaming to).
type IndexKey = (Option<u64>, [Option<u32>; 2]);

/// When effects end, so [`Presentation::ending_effects`] needn't look at
/// every effect each tick (a 63-army air battle has ~170k live): timed
/// effects by end tick, attached / beam ones by the entity they hang on.
/// Kept cheap per effect: in a big air fight a tenth of all effects end
/// every tick (projectile trails), so the bookkeeping costs as much as a
/// scan would save.
#[derive(Default, Debug)]
struct EffectIndex {
    /// Built from `effects` (rebuilt on first use if not).
    built: bool,
    /// End tick -> the timed effects ending then. An entry whose effect has
    /// since gone or changed its end is passed over when due.
    ends: std::collections::BTreeMap<u64, Vec<u32>>,
    /// Entity -> the effects hanging on it (looked up only; unordered).
    anchored: crate::engine::sim::hpa::CellMap<Vec<u32>>,
    /// Effects newly hung on entities: checked once that those are alive.
    check: Vec<u32>,
}

impl EffectIndex {
    /// The tick `e` ends on by its lifetime (the blueprint's, or a
    /// `LIFETIME` parameter; 0 ends at once, negative / none never).
    fn end_of(e: &Effect) -> Option<u64> {
        let life = if e.params.is_empty() { e.lifetime } else { e.params.get("LIFETIME").copied().or(e.lifetime) };
        life.filter(|&l| l >= 0.0).map(|l| e.created + l.ceil() as u64)
    }

    fn key(e: &Effect) -> IndexKey {
        let (a, b) = (e.attached.map(|a| a.0), e.beam_to.map(|b| b.0));
        (Self::end_of(e), [a, b.filter(|&b| Some(b) != a)])
    }

    /// Effect `id` was indexed as `old` and is now `new` (`None`: absent).
    fn update(&mut self, id: u32, old: Option<IndexKey>, new: Option<IndexKey>) {
        if !self.built || old == new {
            return;
        }
        let (old_end, old_anchors) = old.unwrap_or((None, [None; 2]));
        let (new_end, new_anchors) = new.unwrap_or((None, [None; 2]));
        if new_end != old_end
            && let Some(t) = new_end {
            self.ends.entry(t).or_default().push(id);
        }
        if new_anchors != old_anchors {
            for entity in old_anchors.into_iter().flatten() {
                if let Some(list) = self.anchored.get_mut(&entity) {
                    if let Some(i) = list.iter().position(|&x| x == id) {
                        list.swap_remove(i);
                    }
                    if list.is_empty() {
                        self.anchored.remove(&entity);
                    }
                }
            }
            for entity in new_anchors.into_iter().flatten() {
                self.anchored.entry(entity).or_default().push(id);
            }
            if new_anchors.iter().any(Option::is_some) {
                self.check.push(id);
            }
        }
    }

    fn ending(&mut self, tick: u64, destroyed: &[u32], effects: &std::collections::BTreeMap<u32, Box<Effect>>, alive: impl Fn(u32) -> bool) -> Vec<u32> {
        let mut out = Vec::new();
        while let Some(due) = self.ends.first_entry() {
            if *due.key() > tick {
                break;
            }
            let (t, ids) = due.remove_entry();
            out.extend(ids.into_iter().filter(|id| effects.get(id).is_some_and(|e| Self::end_of(e) == Some(t))));
        }
        for entity in destroyed {
            if let Some(list) = self.anchored.get(entity) {
                out.extend_from_slice(list);
            }
        }
        for id in std::mem::take(&mut self.check) {
            if effects.get(&id).is_some_and(|e| Self::key(e).1.into_iter().flatten().any(|entity| !alive(entity))) {
                out.push(id);
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

impl Presentation {
    pub fn add_decal(&mut self, decal: Decal) -> u32 {
        let id = self.next_decal_id;
        self.next_decal_id += 1;
        self.decals.insert(id, decal);
        self.decal_version += 1;
        id
    }

    pub fn remove_decal(&mut self, id: u32) {
        if self.decals.remove(&id).is_some() {
            self.decal_version += 1;
        }
    }

    pub fn add_splat(&mut self, splat: Splat) {
        if splat.duration > 0.0 {
            self.fresh_splats.push(splat);
            return;
        }
        self.splats.push(splat);
        self.decal_version += 1;
    }

    /// Drops decals and splats whose `duration` (seconds; 0 = forever) ran
    /// out by `tick`.
    pub fn expire_decals(&mut self, tick: u64) {
        let live = |created: u64, duration: f64| duration <= 0.0 || tick < created + (duration * 10.0).ceil() as u64;
        let (d, s) = (self.decals.len(), self.splats.len());
        self.decals.retain(|_, x| live(x.created, x.duration));
        self.splats.retain(|x| live(x.created, x.duration));
        self.fresh_splats.retain(|x| live(x.created, x.duration));
        if (d, s) != (self.decals.len(), self.splats.len()) {
            self.decal_version += 1;
        }
    }

    /// The live effects by id (read-only: see the field).
    pub fn effects(&self) -> &std::collections::BTreeMap<u32, Box<Effect>> {
        &self.effects
    }

    pub fn add_effect(&mut self, mut effect: Effect) -> u32 {
        let id = self.next_effect_id;
        self.next_effect_id += 1;
        effect.asset = self.intern_asset(&effect.blueprint);
        let key = EffectIndex::key(&effect);
        self.effects.insert(id, Box::new(effect));
        self.effect_index.update(id, None, Some(key));
        id
    }

    /// Runs `f` on effect `id` (if it's live) and reindexes it: its
    /// lifetime, attachment or beam may have changed.
    pub fn effect_mut<R>(&mut self, id: u32, f: impl FnOnce(&mut Effect) -> R) -> Option<R> {
        let e = self.effects.get_mut(&id)?;
        let old = EffectIndex::key(e);
        let r = f(e);
        let new = EffectIndex::key(e);
        self.effect_index.update(id, Some(old), Some(new));
        Some(r)
    }

    pub fn remove_effect(&mut self, id: u32) -> Option<Box<Effect>> {
        let e = self.effects.remove(&id)?;
        self.effect_index.update(id, Some(EffectIndex::key(&e)), None);
        Some(e)
    }

    /// The effects that end by `tick`, ascending: those whose lifetime has
    /// run out, and those attached to or beaming to an entity that isn't
    /// `alive` (`destroyed`: the entities destroyed since the last call;
    /// any other entity an effect hangs on was alive then, or the effect
    /// was queued to be checked when it was attached). Doesn't remove them.
    pub fn ending_effects(&mut self, tick: u64, destroyed: &[u32], alive: impl Fn(u32) -> bool) -> Vec<u32> {
        if !self.effect_index.built {
            self.effect_index = EffectIndex { built: true, ..Default::default() };
            for (&id, e) in &self.effects {
                self.effect_index.update(id, None, Some(EffectIndex::key(e)));
            }
        }
        self.effect_index.ending(tick, destroyed, &self.effects, alive)
    }

    /// The index of effect asset `path` (blueprint or texture, lowercased)
    /// in [`Presentation::assets`], adding it on first use. Interned once
    /// when an effect is made, so snapshots copy an index rather than look
    /// up a path for every effect every tick.
    pub fn intern_asset(&mut self, path: &str) -> u32 {
        if let Some(&i) = self.asset_ids.get(path) {
            return i;
        }
        let lower = path.to_lowercase();
        let i = match self.asset_ids.get(&lower) {
            Some(&i) => i,
            None => {
                let i = self.assets.len() as u32;
                self.assets.push(lower.clone());
                self.asset_ids.insert(lower, i);
                i
            }
        };
        self.asset_ids.insert(path.to_owned(), i);
        i
    }
}

#[cfg(test)]
mod effect_index_tests {
    use super::*;

    fn effect(created: u64, lifetime: Option<f64>, attached: Option<u32>, beam_to: Option<u32>) -> Effect {
        Effect {
            kind: EffectKind::Emitter,
            blueprint: String::new(),
            army: None,
            position: [0.0; 3],
            attached: attached.map(|a| (a, None)),
            source: None,
            scale: 1.0,
            offset: [0.0; 3],
            params: Default::default(),
            curves: Default::default(),
            curve_lengths: Default::default(),
            ramp: None,
            asset: u32::MAX,
            ramp_asset: u32::MAX,
            beam_to: beam_to.map(|b| (b, None)),
            created,
            lifetime,
        }
    }

    /// The old full scan: every effect whose lifetime ran out by `tick`,
    /// or that hangs on an entity that isn't alive.
    fn scan(p: &Presentation, tick: u64, alive: impl Fn(u32) -> bool) -> Vec<u32> {
        p.effects
            .iter()
            .filter(|(_, e)| {
                let life = if e.params.is_empty() { e.lifetime } else { e.params.get("LIFETIME").copied().or(e.lifetime) };
                let expired = life.is_some_and(|l| l >= 0.0 && tick >= e.created + l.ceil() as u64);
                expired || e.attached.is_some_and(|(id, _)| !alive(id)) || e.beam_to.is_some_and(|(id, _)| !alive(id))
            })
            .map(|(&id, _)| id)
            .collect()
    }

    /// Random adds, lifetime / attachment / beam changes, removals and
    /// entity deaths: the index ends exactly what the scan would, tick by
    /// tick (including effects hung on entities already dead).
    #[test]
    fn the_index_ends_what_a_full_scan_would() {
        let mut rng = crate::engine::sim::rng::SimRng::new(7);
        let mut p = Presentation::default();
        let mut dead = std::collections::BTreeSet::new();
        let mut destroyed = Vec::new();
        for tick in 1..400u64 {
            let mut r = |n: u64| rng.next_u64() % n;
            for _ in 0..20 {
                match r(6) {
                    0 | 1 => {
                        let life = match r(3) { 0 => None, 1 => Some(r(30) as f64 + 0.5), _ => Some(-1.0) };
                        let attached = (r(2) == 0).then(|| r(50) as u32);
                        let beam = (r(4) == 0).then(|| r(50) as u32);
                        p.add_effect(effect(tick, life, attached, beam));
                    }
                    2 if !p.effects.is_empty() => {
                        let id = *p.effects.keys().nth(r(p.effects.len() as u64) as usize).unwrap();
                        let life = r(20) as f64;
                        p.effect_mut(id, |e| { e.params.insert("LIFETIME".into(), life); });
                    }
                    3 if !p.effects.is_empty() => {
                        let id = *p.effects.keys().nth(r(p.effects.len() as u64) as usize).unwrap();
                        let to = (r(3) != 0).then(|| r(50) as u32);
                        p.effect_mut(id, |e| e.attached = to.map(|a| (a, None)));
                    }
                    4 if !p.effects.is_empty() => {
                        let id = *p.effects.keys().nth(r(p.effects.len() as u64) as usize).unwrap();
                        p.remove_effect(id);
                    }
                    5 if r(4) == 0 => {
                        let entity = r(50) as u32;
                        if dead.insert(entity) {
                            destroyed.push(entity);
                        }
                    }
                    _ => {}
                }
            }
            let alive = |id: u32| !dead.contains(&id);
            let want = scan(&p, tick, alive);
            let got = p.ending_effects(tick, &std::mem::take(&mut destroyed), alive);
            assert_eq!(got, want, "tick {tick}");
            for id in got {
                p.remove_effect(id);
            }
        }
    }
}
