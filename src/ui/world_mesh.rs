//! Meshes the interface places in the world: order feedback
//! (`AddCommandFeedbackBlip`), `moho.world_mesh_methods` (FAF's
//! `WorldMesh`: rally point flags) and the skin's border round the map
//! (`MapBorderAdd`, FAF's Render World Border option). The renderer draws them each frame from
//! [`meshes`]; they are presentation only and never reach the sim.

use crate::lua_api::shapes;
use std::collections::BTreeMap;

use mlua::prelude::*;

use crate::engine::api::moho::{self, MohoClass};
use crate::lua_api::{LuaTable, Obj, Status, lua_fn, lua_method};
use crate::engine::api::script_object::C_OBJECT;

/// A mesh to draw this frame, in world space.
#[derive(Clone, Debug, PartialEq)]
pub struct InterfaceMesh {
    pub mesh: String,
    pub texture: String,
    /// `mesh.fx` technique (`CommandFeedback`, `RallyPoint`, ...).
    pub shader: String,
    pub position: [f32; 3],
    /// Quaternion (x, y, z, w).
    pub orientation: [f32; 4],
    pub scale: [f32; 3],
    /// Seconds since it appeared (the shaders' `time - material.x`).
    pub age: f32,
    /// The lifetime parameter (`material.y`), seconds.
    pub lifetime: f32,
}

#[derive(Clone, Debug)]
struct WorldMesh {
    mesh: String,
    texture: String,
    shader: String,
    uniform_scale: f32,
    scale: [f32; 3],
    position: Option<[f32; 3]>,
    orientation: [f32; 4],
    hidden: bool,
    lifetime: f32,
    created: f64,
}

#[derive(Clone, Debug)]
struct Blip {
    mesh: InterfaceMesh,
    created: f64,
    duration: f32,
}

#[derive(Default)]
struct WorldMeshes {
    next: u32,
    meshes: BTreeMap<u32, WorldMesh>,
    blips: Vec<Blip>,
    /// `MapBorderAdd`'s meshes, until `MapBorderClear`.
    border: Vec<InterfaceMesh>,
}

#[derive(Clone, Copy)]
struct WorldMeshHandle(u32);
impl LuaUserData for WorldMeshHandle {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        crate::engine::api::nil_metatable::add_nil_index(methods);
    }
}

fn now(lua: &Lua) -> f64 {
    lua.named_registry_value::<Option<f64>>("user.time").ok().flatten().unwrap_or(0.0)
}

fn state(lua: &Lua) -> mlua::AppDataRefMut<'_, WorldMeshes> {
    if lua.app_data_ref::<WorldMeshes>().is_none() {
        lua.set_app_data(WorldMeshes::default());
    }
    lua.app_data_mut::<WorldMeshes>().expect("WorldMeshes")
}

fn id_of(this: &LuaTable) -> LuaResult<u32> {
    Ok(this.raw_get::<LuaAnyUserData>(C_OBJECT)?.borrow::<WorldMeshHandle>()?.0)
}

fn vector(v: &LuaTable) -> LuaResult<[f32; 3]> {
    Ok([v.raw_get::<Option<f32>>(1)?.unwrap_or(0.0), v.raw_get::<Option<f32>>(2)?.unwrap_or(0.0), v.raw_get::<Option<f32>>(3)?.unwrap_or(0.0)])
}

/// `MeshName`, `TextureName`, `ShaderName`, `UniformScale` of a mesh
/// description (`BlueprintID` descriptions aren't drawn yet).
/// A mesh description (FAF's `WorldMesh:SetMesh` / `AddCommandFeedbackBlip` table).
#[derive(LuaTable)]
#[lua(rename_all = "PascalCase")]
struct MeshDesc {
    #[lua(default)]
    mesh_name: String,
    #[lua(default)]
    texture_name: String,
    #[lua(default)]
    shader_name: String,
    #[lua(default = "1.0")]
    uniform_scale: f32,
    /// Where (`AddCommandFeedbackBlip` only).
    position: Option<LuaTable>,
}

/// What to draw now; drops blips whose duration has passed.
pub fn meshes(lua: &Lua) -> Vec<InterfaceMesh> {
    let t = now(lua);
    let mut s = state(lua);
    s.blips.retain(|b| t - b.created <= b.duration as f64);
    let mut out: Vec<InterfaceMesh> = s.blips.iter().map(|b| InterfaceMesh { age: (t - b.created) as f32, ..b.mesh.clone() }).chain(s.border.iter().cloned()).collect();
    for m in s.meshes.values() {
        let Some(position) = m.position.filter(|_| !m.hidden && !m.mesh.is_empty()) else { continue };
        out.push(InterfaceMesh {
            mesh: m.mesh.clone(),
            texture: m.texture.clone(),
            shader: m.shader.clone(),
            position,
            orientation: m.orientation,
            scale: m.scale.map(|s| s * m.uniform_scale),
            age: (t - m.created) as f32,
            lifetime: m.lifetime,
        });
    }
    out
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua_fn!(lua, global,
        /// FAF's order feedback (`commandmode.lua`): a mesh shown at
        /// `desc.Position` for `duration` seconds (default 1).
        AddCommandFeedbackBlip = |lua, (desc, duration): (MeshDesc, Option<f32>)| {
            let position = desc.position.as_ref().map(vector).transpose()?;
            let (Some(position), false) = (position, desc.mesh_name.is_empty()) else { return Ok(()) };
            let duration = duration.unwrap_or(1.0);
            let created = now(lua);
            let scale = desc.uniform_scale;
            state(lua).blips.push(Blip {
                mesh: InterfaceMesh { mesh: desc.mesh_name, texture: desc.texture_name, shader: desc.shader_name, position, orientation: [0.0, 0.0, 0.0, 1.0], scale: [scale; 3], age: 0.0, lifetime: duration },
                created,
                duration,
            });
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// A skin's border mesh round the map (FAF's Render World Border
        /// option; `/meshes/game/map-border_squ_uef_mesh`), drawn with
        /// `MapImager`. Its model is in map widths: centred on the map,
        /// scaled by the map's width times the blueprint's `UniformScale`
        /// (1/512: the map spans 512 model units, and FAF picks the square
        /// or 2:1 mesh to fit). Files by moho's naming (`<blueprint minus
        /// _mesh>_lod0.scm`, `_albedo.dds`).
        MapBorderAdd = |lua, blueprint: String| {
            let id = blueprint.to_lowercase();
            let bp: Option<LuaTable> = lua.globals().get::<LuaTable>("__blueprints")?.get(id.as_str())?;
            let uniform = bp.map(|b| b.get::<Option<f32>>("UniformScale")).transpose()?.flatten().unwrap_or(1.0 / 512.0);
            let size: Option<LuaTable> = lua.load("local s = SessionGetScenarioInfo() return s and s.size").eval()?;
            let Some(size) = size else { return Ok(()) };
            let (w, h) = (size.get::<Option<f32>>(1)?.unwrap_or(0.0), size.get::<Option<f32>>(2)?.unwrap_or(0.0));
            let stem = id.strip_suffix("_mesh").unwrap_or(&id).to_owned();
            state(lua).border.push(InterfaceMesh {
                mesh: format!("{stem}_lod0.scm"),
                texture: format!("{stem}_albedo.dds"),
                shader: "MapImager".into(),
                position: [w / 2.0, 0.0, h / 2.0],
                orientation: [0.0, 0.0, 0.0, 1.0],
                scale: [w * uniform; 3],
                age: 0.0,
                lifetime: 0.0,
            });
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Removes the map border meshes.
        MapBorderClear = |lua, ()| {
            state(lua).border.clear();
            Ok(())
        })?;
    lua_fn!(lua, global,
        /// Binds a new `WorldMesh` script object (FAF's `WorldMesh.__init`)
        /// to an engine mesh, hidden until `SetMesh` / `SetStance`.
        InternalCreateWorldMesh = |lua, this: Obj<moho::WorldMesh>| {
            let created = now(lua);
            let id = {
                let mut s = state(lua);
                let id = s.next;
                s.next += 1;
                s.meshes.insert(
                    id,
                    WorldMesh {
                        mesh: String::new(),
                        texture: String::new(),
                        shader: String::new(),
                        uniform_scale: 1.0,
                        scale: [1.0; 3],
                        position: None,
                        orientation: [0.0, 0.0, 0.0, 1.0],
                        hidden: true,
                        lifetime: 0.0,
                        created,
                    },
                );
                id
            };
            this.raw_set(C_OBJECT, lua.create_userdata(WorldMeshHandle(id))?)
        })?;
    fn with(lua: &Lua, this: &LuaTable, f: &mut dyn FnMut(&mut WorldMesh)) -> LuaResult<()> {
        let id = id_of(this)?;
        if let Some(m) = state(lua).meshes.get_mut(&id) {
            f(m);
        }
        Ok(())
    }
    lua_method!(lua, moho::WorldMesh,
        /// The mesh, texture, shader and uniform scale to draw.
        SetMesh = |lua, (this, desc): (Obj<moho::WorldMesh>, MeshDesc)| {
            with(lua, &this, &mut |m| {
                m.mesh = desc.mesh_name.clone();
                m.texture = desc.texture_name.clone();
                m.shader = desc.shader_name.clone();
                m.uniform_scale = desc.uniform_scale;
            })
        })?;
    lua_method!(lua, moho::WorldMesh,
        /// Where the mesh is, and optionally its orientation (a quaternion).
        SetStance = |lua, (this, position, orientation): (Obj<moho::WorldMesh>, Option<shapes::Vector>, Option<shapes::Quaternion>)| {
            let position = position.map(|p| vector(&p)).transpose()?;
            let orientation = match orientation {
                Some(q) => Some([q.raw_get::<Option<f32>>(1)?.unwrap_or(0.0), q.raw_get::<Option<f32>>(2)?.unwrap_or(0.0), q.raw_get::<Option<f32>>(3)?.unwrap_or(0.0), q.raw_get::<Option<f32>>(4)?.unwrap_or(1.0)]),
                None => None,
            };
            with(lua, &this, &mut |m| {
                m.position = position;
                if let Some(q) = orientation {
                    m.orientation = q;
                }
            })
        })?;
    lua_method!(lua, moho::WorldMesh,
        /// Hides (default) or shows the mesh.
        SetHidden = |lua, (this, hidden): (Obj<moho::WorldMesh>, Option<bool>)| with(lua, &this, &mut |m| m.hidden = hidden.unwrap_or(true)))?;
    lua_method!(lua, moho::WorldMesh,
        IsHidden = |lua, this: Obj<moho::WorldMesh>| {
            let id = id_of(&this)?;
            Ok(state(lua).meshes.get(&id).is_none_or(|m| m.hidden))
        })?;
    lua_method!(lua, moho::WorldMesh,
        /// Per-axis scale (a vector).
        SetScale = |lua, (this, scale): (Obj<moho::WorldMesh>, shapes::Vector)| {
            let v = vector(&scale)?;
            with(lua, &this, &mut |m| m.scale = v)
        })?;
    lua_method!(lua, moho::WorldMesh,
        /// The shaders' lifetime parameter (`material.y`), seconds.
        SetLifetimeParameter = |lua, (this, seconds): (Obj<moho::WorldMesh>, f32)| with(lua, &this, &mut |m| m.lifetime = seconds))?;
    // Team colour and the other shader parameters: accepted; the feedback
    // techniques don't read them. Names from a list: `add_class_fn` directly.
    for name in ["SetColor", "SetAuxiliaryParameter", "SetFractionCompleteParameter", "SetFractionHealthParameter"] {
        crate::lua_api::add_class_fn(lua, moho::WorldMesh::CLASS, name, None, "", Status::Partial("accepted; the feedback shaders don't read it"), |_, _args: LuaMultiValue| Ok(()))?;
    }
    lua_method!(lua, moho::WorldMesh,
        Destroy = |lua, this: Obj<moho::WorldMesh>| {
            let id = id_of(&this)?;
            state(lua).meshes.remove(&id);
            Ok(())
        })?;
    Ok(())
}
