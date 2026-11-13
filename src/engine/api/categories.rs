//! `categories` global and the `EntityCategory*` functions.

use std::sync::Arc;

use mlua::prelude::*;

use crate::engine::api::blueprints::{BlueprintKind, BlueprintRegistry};
use crate::engine::sim::category::{CategoryDomain, CategorySet, EntityKind};
use crate::lua_api::lua_fn;
use crate::lua_api::shapes::List;

/// Lua-facing `EntityCategory` userdata.
#[derive(Clone, Debug)]
pub struct EntityCategory(pub CategorySet);

impl crate::lua_api::LuaType for EntityCategory {
    fn ty() -> crate::lua_api::LuaTy {
        crate::lua_api::LuaTy::Class("EntityCategory")
    }
}

impl LuaUserData for EntityCategory {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        crate::engine::api::nil_metatable::add_nil_index(methods);
        methods.add_meta_function(
            LuaMetaMethod::Add,
            |_, (a, b): (LuaUserDataRef<Self>, LuaUserDataRef<Self>)| {
                Ok(EntityCategory(a.0.union(&b.0)))
            },
        );
        methods.add_meta_function(
            LuaMetaMethod::Mul,
            |_, (a, b): (LuaUserDataRef<Self>, LuaUserDataRef<Self>)| {
                Ok(EntityCategory(a.0.intersection(&b.0)))
            },
        );
        methods.add_meta_function(
            LuaMetaMethod::Sub,
            |_, (a, b): (LuaUserDataRef<Self>, LuaUserDataRef<Self>)| {
                Ok(EntityCategory(a.0.difference(&b.0)))
            },
        );
        methods.add_meta_function(
            LuaMetaMethod::Eq,
            |_, (a, b): (LuaUserDataRef<Self>, LuaUserDataRef<Self>)| Ok(a.0 == b.0),
        );
        methods.add_meta_method(LuaMetaMethod::ToString, |_, this, ()| {
            Ok(format!("EntityCategory({} entries)", this.0.iter().count()))
        });
    }
}

/// Builds the category domain from the registered blueprints and installs the
/// `categories` global. Call after `RegisterAllBlueprints`, before simInit.
pub fn build(lua: &Lua) -> LuaResult<()> {
    let mut items: Vec<(u32, String, EntityKind, Vec<String>)> = Vec::new();
    {
        let reg = lua
            .app_data_ref::<BlueprintRegistry>()
            .expect("BlueprintRegistry not in app_data");
        for (bk, ek) in [
            (BlueprintKind::Unit, EntityKind::Unit),
            (BlueprintKind::Projectile, EntityKind::Projectile),
            (BlueprintKind::Prop, EntityKind::Prop),
        ] {
            for (id, bp) in reg.iter(bk) {
                let cats = match bp.table.raw_get::<LuaValue>("Categories")? {
                    LuaValue::Table(t) => t
                        .sequence_values::<String>()
                        .collect::<LuaResult<Vec<_>>>()?,
                    _ => Vec::new(),
                };
                items.push((bp.ordinal, id.to_owned(), ek, cats));
            }
        }
    }
    // Registration order across kinds → deterministic domain indices.
    items.sort_unstable_by_key(|(ordinal, ..)| *ordinal);
    let domain = CategoryDomain::build(items.into_iter().map(|(_, id, k, c)| (id, k, c)));

    let categories = lua.create_table_with_capacity(0, domain.named.len())?;
    for (name, set) in &domain.named {
        categories.raw_set(name.as_str(), EntityCategory(set.clone()))?;
    }
    tracing::info!(
        blueprints = domain.len(),
        categories = domain.named.len(),
        "entity categories built"
    );
    expose_metatable(lua, CategorySet::empty(domain.len()))?;
    lua.set_app_data(Arc::new(domain));
    lua.globals().set("categories", categories)
}

/// Lets `getmetatable(categories.X)` return the category metatable, as
/// moho's does: FAF's `/lua/system/categories.lua` (since 3841) replaces
/// its `__add` / `__sub` / `__mul` with ones that cache their results.
/// mlua hides a userdata's metatable behind `__metatable = false`; this
/// takes that field off `EntityCategory`'s. mlua tells its types apart by
/// the metatable's identity, not its fields, so a script that edits them
/// changes only what the operators do.
fn expose_metatable(lua: &Lua, any: CategorySet) -> LuaResult<()> {
    let ud = lua.create_userdata(EntityCategory(any))?;
    // SAFETY: the stack holds [ud]; its metatable is pushed (every mlua
    // userdata has one) and its `__metatable` raw-set to nil (no
    // metamethods run); `exec_raw` restores the stack.
    unsafe {
        lua.exec_raw::<()>(ud, |state| {
            if mlua::ffi::lua_getmetatable(state, -1) != 0 {
                mlua::ffi::lua_pushstring(state, c"__metatable".as_ptr());
                mlua::ffi::lua_pushnil(state);
                mlua::ffi::lua_rawset(state, -3);
            }
        })
    }
}

/// A weapon blueprint restriction string (`TargetRestrictOnlyAllow =
/// "TACTICAL,MISSILE"`, `"UNTARGETABLE AIR"`): comma-separated terms are
/// united, the space-separated names within a term intersected. Unknown
/// names are empty sets; `None` for an empty string.
pub(crate) fn parse_restriction(lua: &Lua, text: &str) -> LuaResult<Option<CategorySet>> {
    let d = domain(lua)?;
    let mut out: Option<CategorySet> = None;
    for term in text.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        let mut set: Option<CategorySet> = None;
        for name in term.split_whitespace() {
            let named = d.named.get(name).cloned().unwrap_or_else(|| CategorySet::empty(d.len()));
            set = Some(match set {
                Some(s) => s.intersection(&named),
                None => named,
            });
        }
        if let Some(set) = set {
            out = Some(match out {
                Some(o) => o.union(&set),
                None => set,
            });
        }
    }
    Ok(out)
}

pub(crate) fn domain(lua: &Lua) -> LuaResult<Arc<CategoryDomain>> {
    lua.app_data_ref::<Arc<CategoryDomain>>()
        .map(|d| d.clone())
        .ok_or_else(|| LuaError::runtime("entity categories not built yet"))
}

/// Lua strings for the keys the category functions read, made once:
/// `raw_get` with a `&str` builds (interns) the key string on every call,
/// which was a large part of `formations.lua`'s cost (one
/// `EntityCategoryContains` per unit per formation category).
struct Keys {
    c_object: LuaString,
    /// The interface's UserUnits (`/src/ui/units.lua`): `{ _id = id }`,
    /// their blueprint id at `__engine_unit_data[id].bp`.
    user_id: LuaString,
    user_data: LuaString,
    user_bp: LuaString,
}

/// Domain index of a unit id string, a blueprint table, or an object with a
/// `Blueprint` table (FAF entity instances). Engine entities resolve
/// through their handle to the category index they were created with, and
/// the interface's UserUnits through their unit data (raw reads: the slow
/// path's `Blueprint` / `GetUnitId` lookups go through the class metatable
/// and back into Lua, per unit, and FAF filters whole selections).
fn index_of(lua: &Lua, domain: &dyn Fn() -> LuaResult<Arc<CategoryDomain>>, value: &LuaValue) -> LuaResult<Option<u32>> {
    if let (LuaValue::Table(t), Some(keys)) = (value, lua.app_data_ref::<Keys>()) {
        if let LuaValue::UserData(ud) = t.raw_get::<LuaValue>(&keys.c_object)?
            && let Ok(h) = ud.borrow::<crate::engine::api::entity::EntityHandle>()
            && let Some(i) = lua.app_data_ref::<crate::engine::sim::entity::EntityWorld>().and_then(|w| w.get(h.0).map(|e| e.category_index)) {
            return Ok(i);
        }
        let id = t.raw_get::<LuaValue>(&keys.user_id)?;
        if let LuaValue::Integer(_) | LuaValue::Number(_) = id
            && let Some(data) = lua.globals().raw_get::<Option<LuaTable>>(&keys.user_data)? {
            // A dead unit (its data dropped) is in no category.
            let Some(unit) = data.raw_get::<Option<LuaTable>>(id)? else { return Ok(None) };
            return Ok(match unit.raw_get::<LuaValue>(&keys.user_bp)? {
                LuaValue::String(s) => domain()?.index_of(&s.to_str()?),
                _ => None,
            });
        }
    }
    index_of_slow(&*domain()?, value)
}

fn index_of_slow(domain: &CategoryDomain, value: &LuaValue) -> LuaResult<Option<u32>> {
    Ok(match value {
        LuaValue::String(s) => domain.index_of(&s.to_str()?),
        LuaValue::Table(t) => {
            let bp = match t.raw_get::<LuaValue>("BlueprintId")? {
                LuaValue::String(s) => return Ok(domain.index_of(&s.to_str()?)),
                _ => t.get::<LuaValue>("Blueprint")?,
            };
            match bp {
                LuaValue::Table(bp) => match bp.raw_get::<LuaValue>("BlueprintId")? {
                    LuaValue::String(s) => domain.index_of(&s.to_str()?),
                    _ => None,
                },
                // The user layer's units (`UserUnit`): no `Blueprint`
                // field, but `GetUnitId` (FAF's construction panel tests
                // the selection's first unit this way).
                _ => match t.get::<LuaValue>("GetUnitId")? {
                    LuaValue::Function(f) => match f.call::<LuaValue>(t.clone())? {
                        LuaValue::String(s) => domain.index_of(&s.to_str()?),
                        _ => None,
                    },
                    _ => None,
                },
            }
        }
        _ => None,
    })
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    lua.set_app_data(Keys {
        c_object: lua.create_string(crate::engine::api::script_object::C_OBJECT)?,
        user_id: lua.create_string("_id")?,
        user_data: lua.create_string("__engine_unit_data")?,
        user_bp: lua.create_string("bp")?,
    });
    lua_fn!(lua, global,
        /// The entries of `list` (units, blueprints or ids) in `cat`. A nil
        /// list (no selection) filters to nothing.
        EntityCategoryFilterDown = 
            |lua, (cat, list): (LuaUserDataRef<EntityCategory>, Option<List<LuaValue>>)| {
                let out = List::<LuaValue>::new(lua.create_table()?);
                let Some(list) = list else { return Ok(out) };
                let d = domain(lua)?;
                let mut n = 0;
                for v in list.sequence_values::<LuaValue>() {
                    let v = v?;
                    if index_of(lua, &|| Ok(d.clone()), &v)?.is_some_and(|i| cat.0.contains(i)) {
                        n += 1;
                        out.raw_set(n, v)?;
                    }
                }
                Ok(out)
            })?;
    lua_fn!(lua, global,
        /// The entries of `list` not in `cat` (user layer: orders filtering
        /// silos out of a stop, ...). A nil list filters to nothing. Moho's in
        /// the user VM only (MohoEngine.dll; FA-Binary-Patches fixes its table
        /// insert); ours in both.
        EntityCategoryFilterOut = |lua, (cat, list): (LuaUserDataRef<EntityCategory>, Option<List<LuaValue>>)| {
            let out = List::<LuaValue>::new(lua.create_table()?);
            let Some(list) = list else { return Ok(out) };
            let d = domain(lua)?;
            let mut n = 0;
            for v in list.sequence_values::<LuaValue>() {
                let v = v?;
                if !index_of(lua, &|| Ok(d.clone()), &v)?.is_some_and(|i| cat.0.contains(i)) {
                    n += 1;
                    out.raw_set(n, v)?;
                }
            }
            Ok(out)
        })?;
    lua_fn!(lua, global,
        /// Parses a category expression (`categories.TECH1 * categories.LAND`).
        ParseEntityCategory = |lua, expr: String| {
            let domain = domain(lua)?;
            domain.parse(&expr).map(EntityCategory).map_err(LuaError::runtime)
        })?;
    lua_fn!(lua, global,
        /// Unit blueprint ids in the category, in registration order.
        EntityCategoryGetUnitList = |lua, cat: LuaUserDataRef<EntityCategory>| {
            let domain = domain(lua)?;
            Ok(cat
                .0
                .iter()
                .filter_map(|i| domain.entries.get(i as usize))
                .filter(|(_, kind)| *kind == EntityKind::Unit)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>())
        })?;
    lua_fn!(lua, global,
        /// Whether the unit, blueprint or id is in the category.
        EntityCategoryContains = |lua, (cat, v): (LuaUserDataRef<EntityCategory>, LuaValue)| {
            Ok(index_of(lua, &|| domain(lua), &v)?.is_some_and(|i| cat.0.contains(i)))
        })?;
    lua_fn!(lua, global,
        /// How many entries of `list` (units, blueprints or ids) are in `cat`.
        /// Moho's (SupremeCommander.exe; FAF's annotations: sim); ours in both VMs.
        EntityCategoryCount = |lua, (cat, list): (LuaUserDataRef<EntityCategory>, Option<List<LuaValue>>)| {
            let Some(list) = list else { return Ok(0) };
            let d = domain(lua)?;
            let mut n = 0;
            for v in list.sequence_values::<LuaValue>() {
                if index_of(lua, &|| Ok(d.clone()), &v?)?.is_some_and(|i| cat.0.contains(i)) {
                    n += 1;
                }
            }
            Ok(n)
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::api::blueprints;

    fn lua_with_blueprints() -> Lua {
        let lua = Lua::new();
        blueprints::register(&lua).unwrap();
        register(&lua).unwrap();
        lua.load(
            "__blueprints = {}
             RegisterUnitBlueprint({ BlueprintId = 'uel0001', Categories = { 'LAND', 'COMMAND' } })
             RegisterUnitBlueprint({ BlueprintId = 'uel0105', Categories = { 'LAND', 'ENGINEER' } })
             RegisterProjectileBlueprint({ BlueprintId = '/p/a_proj.bp', Categories = { 'MISSILE' } })",
        )
        .exec()
        .unwrap();
        build(&lua).unwrap();
        lua
    }

    #[test]
    fn categories_algebra_and_filter_down() {
        let lua = lua_with_blueprints();
        let r: String = lua
            .load(
                "local c = categories.ALLUNITS - categories.ENGINEER
                 local f = EntityCategoryFilterDown(c, { 'uel0105', 'uel0001', 'nope', __blueprints.uel0001 })
                 return table.getn(f) .. ':' .. f[1] .. ':' .. tostring(categories.uel0001 == categories.COMMAND)",
            )
            .eval()
            .unwrap();
        assert_eq!(r, "2:uel0001:true");
    }

    /// FAF 3841's `/lua/system/categories.lua` wraps the operators.
    #[test]
    fn scripts_can_wrap_the_operators() {
        let lua = lua_with_blueprints();
        let r: (i64, bool, bool) = lua
            .load(
                "local mt = getmetatable(categories.ALLUNITS)
                 local add, calls = mt.__add, 0
                 function mt.__add(a, b) calls = calls + 1 return add(a, b) end
                 local c = categories.COMMAND + categories.ENGINEER
                 return calls, EntityCategoryContains(c, 'uel0105'), c == categories.LAND",
            )
            .eval()
            .unwrap();
        assert_eq!(r, (1, true, true));
    }

    #[test]
    fn contains() {
        let lua = lua_with_blueprints();
        let r: (bool, bool, bool) = lua
            .load("return EntityCategoryContains(categories.LAND, 'uel0001'), EntityCategoryContains(categories.ENGINEER, __blueprints.uel0001), EntityCategoryContains(categories.LAND, 'missing')")
            .eval()
            .unwrap();
        assert_eq!(r, (true, false, false));
        let list: String = lua
            .load("return table.concat(EntityCategoryGetUnitList(categories.LAND + categories.MISSILE), ',')")
            .eval()
            .unwrap();
        assert_eq!(list, "uel0001,uel0105");
    }
}
