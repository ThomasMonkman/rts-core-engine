//! Macros for the engine's typed Lua API (`src/lua_api`). They expand to
//! paths under `crate::lua_api`, so they are for use inside the engine crate.
//!
//! - `lua_fn!` / `lua_method!`: register a native function (global, in a
//!   table, or a moho class method) and record its signature, parameter
//!   names, docs and status in the VM's `ApiRegistry`.
//! - `callbacks!`: declares the script callbacks the engine calls (`OnCreate`,
//!   ...) as typed Rust functions, recorded the same way.
//! - `#[derive(LuaTable)]`: a Rust struct <-> Lua table, each key written once.
//! - `#[derive(LuaEnum)]`: a Rust enum <-> Lua string.
//!
//! The registry feeds the generated Luau / EmmyLua definitions (`--dump-api`).

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{Attribute, Data, DeriveInput, Expr, Fields, Ident, LitStr, Pat, Token, Type, parse_macro_input};

// ---------------------------------------------------------------- helpers

/// The `///` docs of an item, one line each.
fn doc_of(attrs: &[Attribute]) -> String {
    let mut lines = Vec::new();
    for a in attrs {
        if !a.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &a.meta
            && let Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value {
            lines.push(s.value().trim().to_string());
        }
    }
    lines.join("\n").trim().to_string()
}

fn rename(s: &str, rule: Option<&str>, span: Span) -> syn::Result<String> {
    use heck::{ToLowerCamelCase, ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
    Ok(match rule {
        None => s.to_string(),
        Some("PascalCase") => s.to_upper_camel_case(),
        Some("camelCase") => s.to_lower_camel_case(),
        Some("snake_case") => s.to_snake_case(),
        Some("SCREAMING_SNAKE_CASE") => s.to_shouty_snake_case(),
        Some("lowercase") => s.to_lowercase(),
        Some("UPPERCASE") => s.to_uppercase(),
        Some(other) => return Err(syn::Error::new(span, format!("unknown rename_all rule '{other}'"))),
    })
}

#[derive(Default)]
struct ContainerAttrs {
    name: Option<String>,
    rename_all: Option<String>,
    raw: bool,
    write_only: bool,
}

fn container_attrs(attrs: &[Attribute]) -> syn::Result<ContainerAttrs> {
    let mut c = ContainerAttrs::default();
    for a in attrs.iter().filter(|a| a.path().is_ident("lua")) {
        a.parse_nested_meta(|m| {
            if m.path.is_ident("name") {
                c.name = Some(m.value()?.parse::<LitStr>()?.value());
            } else if m.path.is_ident("rename_all") {
                c.rename_all = Some(m.value()?.parse::<LitStr>()?.value());
            } else if m.path.is_ident("raw") {
                c.raw = true;
            } else if m.path.is_ident("write_only") {
                c.write_only = true;
            } else {
                return Err(m.error("unknown #[lua(..)] option (name, rename_all, raw, write_only)"));
            }
            Ok(())
        })?;
    }
    Ok(c)
}

enum FieldDefault {
    None,
    Default,
    Expr(Expr),
}

struct FieldAttrs {
    rename: Option<String>,
    default: FieldDefault,
    skip: bool,
}

fn field_attrs(attrs: &[Attribute]) -> syn::Result<FieldAttrs> {
    let mut f = FieldAttrs { rename: None, default: FieldDefault::None, skip: false };
    for a in attrs.iter().filter(|a| a.path().is_ident("lua")) {
        a.parse_nested_meta(|m| {
            if m.path.is_ident("rename") {
                f.rename = Some(m.value()?.parse::<LitStr>()?.value());
            } else if m.path.is_ident("default") {
                f.default = if m.input.peek(Token![=]) {
                    let s: LitStr = m.value()?.parse()?;
                    FieldDefault::Expr(s.parse::<Expr>()?)
                } else {
                    FieldDefault::Default
                };
            } else if m.path.is_ident("skip") {
                f.skip = true;
            } else {
                return Err(m.error("unknown #[lua(..)] option (rename, default, skip)"));
            }
            Ok(())
        })?;
    }
    Ok(f)
}

/// `T` of `Option<T>`.
fn option_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(p) = ty else { return None };
    let seg = p.path.segments.last()?;
    if seg.ident != "Option" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else { return None };
    match args.args.first()? {
        syn::GenericArgument::Type(t) => Some(t),
        _ => None,
    }
}

/// `ty` with every lifetime made `'static` (a borrowed field's type
/// description lives in a `static`).
fn static_lifetimes(ty: &Type) -> Type {
    struct V;
    impl syn::visit_mut::VisitMut for V {
        fn visit_lifetime_mut(&mut self, l: &mut syn::Lifetime) {
            *l = syn::Lifetime::new("'static", l.span());
        }
    }
    let mut ty = ty.clone();
    syn::visit_mut::VisitMut::visit_type_mut(&mut V, &mut ty);
    ty
}

/// The sim's fixed-point number (`Fx`): read from Lua as an `f64` and
/// converted once (`math::from_lua`), so no float reaches sim state.
fn is_fx(ty: &Type) -> bool {
    matches!(ty, Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "Fx"))
}

// ---------------------------------------------------------------- LuaEnum

/// A Rust enum <-> Lua string (`"Land"`, `"Water"`, ...). Options:
/// `#[lua(name = "Layer", rename_all = "PascalCase")]` on the enum,
/// `#[lua(rename = "...")]` on a variant. Generates `lua_str`,
/// `from_lua_str`, `LUA_VARIANTS`, `FromLua`, `IntoLua` and `LuaType` (a
/// string-literal union in the generated types).
#[proc_macro_derive(LuaEnum, attributes(lua))]
pub fn derive_lua_enum(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    lua_enum(&input).unwrap_or_else(syn::Error::into_compile_error).into()
}

fn lua_enum(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let ident = &input.ident;
    let c = container_attrs(&input.attrs)?;
    let name = c.name.clone().unwrap_or_else(|| ident.to_string());
    let doc = doc_of(&input.attrs);
    let Data::Enum(e) = &input.data else {
        return Err(syn::Error::new_spanned(ident, "LuaEnum needs an enum"));
    };
    let mut variants = Vec::new();
    let mut strs = Vec::new();
    for v in &e.variants {
        if !matches!(v.fields, Fields::Unit) {
            return Err(syn::Error::new_spanned(v, "LuaEnum variants must be unit variants"));
        }
        let fa = field_attrs(&v.attrs)?;
        let s = match fa.rename {
            Some(s) => s,
            None => rename(&v.ident.to_string(), c.rename_all.as_deref(), v.ident.span())?,
        };
        variants.push(&v.ident);
        strs.push(s);
    }
    let expected = format!("expected one of {}", strs.join("|"));
    let list = strs.join("|");
    Ok(quote! {
        impl #ident {
            /// The Lua strings, in declaration order.
            pub const LUA_VARIANTS: &'static [&'static str] = &[#(#strs),*];
            /// The Lua string for this value.
            pub fn lua_str(&self) -> &'static str {
                match self { #(Self::#variants => #strs,)* }
            }
            /// The value for a Lua string (`None`: not one of [`Self::LUA_VARIANTS`]).
            pub fn from_lua_str(s: &str) -> ::core::option::Option<Self> {
                match s { #(#strs => ::core::option::Option::Some(Self::#variants),)* _ => ::core::option::Option::None }
            }
        }
        impl ::mlua::FromLua for #ident {
            fn from_lua(value: ::mlua::Value, _lua: &::mlua::Lua) -> ::mlua::Result<Self> {
                match &value {
                    ::mlua::Value::String(s) => {
                        let s = s.to_str()?;
                        Self::from_lua_str(&s).ok_or_else(|| ::mlua::Error::FromLuaConversionError {
                            from: "string",
                            to: #name.into(),
                            message: ::core::option::Option::Some(::std::format!("'{}' is not one of {}", &*s, #list)),
                        })
                    }
                    other => Err(::mlua::Error::FromLuaConversionError {
                        from: other.type_name(),
                        to: #name.into(),
                        message: ::core::option::Option::Some(#expected.into()),
                    }),
                }
            }
        }
        impl ::mlua::IntoLua for #ident {
            fn into_lua(self, lua: &::mlua::Lua) -> ::mlua::Result<::mlua::Value> {
                ::mlua::IntoLua::into_lua(self.lua_str(), lua)
            }
        }
        impl crate::lua_api::LuaType for #ident {
            fn ty() -> crate::lua_api::LuaTy {
                static INFO: crate::lua_api::EnumInfo = crate::lua_api::EnumInfo { name: #name, doc: #doc, variants: &[#(#strs),*] };
                crate::lua_api::LuaTy::Enum(&INFO)
            }
        }
        impl crate::lua_api::FieldIntoLua for #ident {
            fn set_field<K: ::mlua::IntoLua>(&self, _lua: &::mlua::Lua, t: &::mlua::Table, key: K) -> ::mlua::Result<()> {
                t.raw_set(key, self.lua_str())
            }
        }
        crate::lua_api::inventory::submit! { crate::lua_api::TypeDecl(<#ident as crate::lua_api::LuaType>::ty) }
    })
}

// ---------------------------------------------------------------- LuaTable

/// A Rust struct <-> Lua table, each key written once (on its field).
///
/// On the struct: `#[lua(name = "AirBlueprint", rename_all = "PascalCase", raw)]`
/// (`raw`: read with `raw_get`, ignoring metatables; `write_only`: only
/// written to Lua, so no `FromLua`, and fields need only `FieldIntoLua`). On a field:
/// `#[lua(rename = "Key")]`, `#[lua(default)]` (`Default::default()` when
/// missing), `#[lua(default = "expr")]`, `#[lua(skip)]` (never read or
/// written). A missing key is an error unless the field is an `Option` or
/// has a default. `Fx` fields are read as numbers and converted with
/// `math::from_lua` (no float reaches sim state).
///
/// Generates `FromLua`, `IntoLua`, `from_lua_table`, `write_into` (updates a
/// table in place: no allocation), `unknown_keys`, `LUA_KEYS` and `LuaType`.
#[proc_macro_derive(LuaTable, attributes(lua))]
pub fn derive_lua_table(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    lua_table(&input).unwrap_or_else(syn::Error::into_compile_error).into()
}

fn lua_table(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let ident = &input.ident;
    let c = container_attrs(&input.attrs)?;
    let name = c.name.clone().unwrap_or_else(|| ident.to_string());
    let doc = doc_of(&input.attrs);
    let Data::Struct(s) = &input.data else {
        return Err(syn::Error::new_spanned(ident, "LuaTable needs a struct"));
    };
    let Fields::Named(fields) = &s.fields else {
        return Err(syn::Error::new_spanned(ident, "LuaTable needs named fields"));
    };
    let only_lifetimes = input.generics.params.iter().all(|p| matches!(p, syn::GenericParam::Lifetime(_)));
    if !input.generics.params.is_empty() && !(c.write_only && only_lifetimes) {
        return Err(syn::Error::new_spanned(&input.generics, "LuaTable structs can't be generic (write_only ones may borrow: lifetimes only)"));
    }
    let (impl_g, ty_g, _) = input.generics.split_for_impl();
    let get = if c.raw { quote!(raw_get) } else { quote!(get) };
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    let mut keys = Vec::new();
    let mut infos = Vec::new();
    for f in &fields.named {
        let fid = f.ident.as_ref().unwrap();
        let fa = field_attrs(&f.attrs)?;
        if fa.skip {
            reads.push(quote!(#fid: ::core::default::Default::default()));
            continue;
        }
        let key = match &fa.rename {
            Some(k) => k.clone(),
            None => rename(&fid.to_string(), c.rename_all.as_deref(), fid.span())?,
        };
        let ty = &f.ty;
        let fdoc = doc_of(&f.attrs);
        let opt = option_inner(ty);
        let has_default = !matches!(fa.default, FieldDefault::None);
        let missing = match &fa.default {
            FieldDefault::None => quote!(return Err(crate::lua_api::missing_field(#name, #key))),
            FieldDefault::Default => quote!(::core::default::Default::default()),
            FieldDefault::Expr(e) => quote!((#e)),
        };
        let fetch = |t: TokenStream2| quote!(t.#get::<::core::option::Option<#t>>(#key).map_err(|e| crate::lua_api::field_error(#name, #key, e))?);
        let read = match opt {
            Some(inner) if is_fx(inner) => {
                let v = fetch(quote!(f64));
                match &fa.default {
                    FieldDefault::None => quote!(#v.map(crate::lua_api::fx)),
                    _ => quote!(match #v { Some(v) => Some(crate::lua_api::fx(v)), None => #missing }),
                }
            }
            Some(inner) => {
                let v = fetch(quote!(#inner));
                match &fa.default {
                    FieldDefault::None => v,
                    _ => quote!(match #v { Some(v) => Some(v), None => #missing }),
                }
            }
            None if is_fx(ty) => {
                let v = fetch(quote!(f64));
                quote!(match #v { Some(v) => crate::lua_api::fx(v), None => #missing })
            }
            None => {
                let v = fetch(quote!(#ty));
                quote!(match #v { Some(v) => v, None => #missing })
            }
        };
        reads.push(quote!(#fid: #read));
        writes.push(quote!(crate::lua_api::FieldIntoLua::set_field(&self.#fid, lua, t, #key)?;));
        let optional = opt.is_some() || has_default;
        let sty = static_lifetimes(ty);
        infos.push(quote!(crate::lua_api::FieldInfo { key: #key, doc: #fdoc, ty: <#sty as crate::lua_api::LuaType>::ty, optional: #optional }));
        keys.push(key);
    }
    let n = keys.len();
    let reading = if c.write_only {
        quote!()
    } else {
        quote! {
            impl #ident {
                /// Reads the struct from table `t`.
                pub fn from_lua_table(t: &::mlua::Table, _lua: &::mlua::Lua) -> ::mlua::Result<Self> {
                    Ok(Self { #(#reads,)* })
                }
            }
            impl ::mlua::FromLua for #ident {
                fn from_lua(value: ::mlua::Value, lua: &::mlua::Lua) -> ::mlua::Result<Self> {
                    match &value {
                        ::mlua::Value::Table(t) => Self::from_lua_table(t, lua),
                        other => Err(::mlua::Error::FromLuaConversionError { from: other.type_name(), to: #name.into(), message: Some("expected a table".into()) }),
                    }
                }
            }
        }
    };
    // The type is listed for the definitions under 'static lifetimes.
    let static_ty = if input.generics.params.is_empty() { quote!(#ident) } else { let ls = input.generics.params.iter().map(|_| quote!('static)); quote!(#ident<#(#ls),*>) };
    let submit = quote!(crate::lua_api::inventory::submit! { crate::lua_api::TypeDecl(<#static_ty as crate::lua_api::LuaType>::ty) });
    Ok(quote! {
        #reading
        impl #impl_g #ident #ty_g {
            /// The Lua keys this struct reads and writes.
            pub const LUA_KEYS: &'static [&'static str] = &[#(#keys),*];
            /// Writes every field into `t` (an existing table: no allocation
            /// for the table itself; `None` fields clear their key).
            pub fn write_into(&self, lua: &::mlua::Lua, t: &::mlua::Table) -> ::mlua::Result<()> {
                #(#writes)*
                Ok(())
            }
            /// String keys of `t` this struct doesn't know (sorted): typos in
            /// data, or fields not implemented yet.
            pub fn unknown_keys(t: &::mlua::Table) -> ::mlua::Result<::std::vec::Vec<::std::string::String>> {
                crate::lua_api::unknown_keys(t, Self::LUA_KEYS)
            }
        }
        impl #impl_g ::mlua::IntoLua for #ident #ty_g {
            fn into_lua(self, lua: &::mlua::Lua) -> ::mlua::Result<::mlua::Value> {
                let t = lua.create_table_with_capacity(0, #n)?;
                self.write_into(lua, &t)?;
                Ok(::mlua::Value::Table(t))
            }
        }
        impl #impl_g crate::lua_api::FieldIntoLua for #ident #ty_g {
            fn set_field<K: ::mlua::IntoLua>(&self, lua: &::mlua::Lua, t: &::mlua::Table, key: K) -> ::mlua::Result<()> {
                let inner = lua.create_table_with_capacity(0, #n)?;
                self.write_into(lua, &inner)?;
                t.raw_set(key, inner)
            }
        }
        impl #impl_g crate::lua_api::LuaType for #ident #ty_g {
            fn ty() -> crate::lua_api::LuaTy {
                static INFO: crate::lua_api::StructInfo = crate::lua_api::StructInfo { name: #name, doc: #doc, fields: &[#(#infos),*] };
                crate::lua_api::LuaTy::Struct(&INFO)
            }
        }
        #submit
    })
}

// ---------------------------------------------------------------- lua_fn! / lua_method!

/// `#[lua(stub)]`, `#[lua(partial = "...")]`, `#[lua(internal)]` on a
/// registered function.
fn status_of(attrs: &[Attribute]) -> syn::Result<TokenStream2> {
    let mut status = quote!(crate::lua_api::Status::Implemented);
    for a in attrs.iter().filter(|a| a.path().is_ident("lua")) {
        a.parse_nested_meta(|m| {
            if m.path.is_ident("stub") {
                status = quote!(crate::lua_api::Status::Stub);
            } else if m.path.is_ident("internal") {
                status = quote!(crate::lua_api::Status::Internal);
            } else if m.path.is_ident("partial") {
                let s: LitStr = m.value()?.parse()?;
                status = quote!(crate::lua_api::Status::Partial(#s));
            } else {
                return Err(m.error("unknown #[lua(..)] option (stub, partial = \"..\", internal)"));
            }
            Ok(())
        })?;
    }
    Ok(status)
}

/// Parameter names from a callback closure `|lua, (a, b): (A, B)|`:
/// `Some(names)` when the pattern names them, `None` when unknown.
fn param_names(f: &Expr) -> Option<Vec<Option<String>>> {
    let Expr::Closure(c) = f else { return None };
    let arg = c.inputs.iter().nth(1)?;
    let (pat, ty) = match arg {
        Pat::Type(pt) => (&*pt.pat, Some(&*pt.ty)),
        p => (p, None),
    };
    let name = |p: &Pat| match p {
        Pat::Ident(i) => Some(i.ident.to_string().trim_start_matches('_').to_string()).filter(|s| !s.is_empty()),
        _ => None,
    };
    // `Arity((a, b)): Arity<..>`: the names are the inner tuple's.
    let pat = match pat {
        Pat::TupleStruct(ts) if ts.elems.len() == 1 => &ts.elems[0],
        p => p,
    };
    match pat {
        Pat::Tuple(t) => Some(t.elems.iter().map(name).collect()),
        // `args: (A, B)`: one name for several values; leave them unnamed.
        Pat::Ident(_) if matches!(ty, Some(Type::Tuple(t)) if !t.elems.is_empty()) => None,
        p => Some(vec![name(p)]),
    }
}

fn params_tokens(f: &Expr) -> TokenStream2 {
    match param_names(f) {
        Some(names) => {
            let items = names.iter().map(|n| match n {
                Some(n) => quote!(::core::option::Option::Some(#n)),
                None => quote!(::core::option::Option::None),
            });
            quote!(::core::option::Option::Some(&[#(#items),*]))
        }
        None => quote!(::core::option::Option::None),
    }
}

enum Target {
    Global { raw: bool },
    Table { table: Expr, scope: LitStr, raw: bool },
}

struct FnInput {
    lua: Expr,
    target: Target,
    attrs: Vec<Attribute>,
    name: Ident,
    f: Expr,
}

impl Parse for FnInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let lua: Expr = input.parse()?;
        input.parse::<Token![,]>()?;
        let kind: Ident = input.parse()?;
        let target = match kind.to_string().as_str() {
            "global" => Target::Global { raw: false },
            "global_raw" => Target::Global { raw: true },
            k @ ("table" | "table_raw") => {
                let content;
                syn::parenthesized!(content in input);
                let table: Expr = content.parse()?;
                content.parse::<Token![,]>()?;
                let scope: LitStr = content.parse()?;
                Target::Table { table, scope, raw: k == "table_raw" }
            }
            _ => return Err(syn::Error::new(kind.span(), "expected `global`, `global_raw`, `table(t, \"Scope\")` or `table_raw(t, \"Scope\")`")),
        };
        input.parse::<Token![,]>()?;
        let attrs = input.call(Attribute::parse_outer)?;
        let name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let f: Expr = input.parse()?;
        let _ = input.parse::<Option<Token![,]>>()?;
        Ok(Self { lua, target, attrs, name, f })
    }
}

/// Registers a native function and records it in the VM's API registry.
///
/// ```ignore
/// lua_fn!(lua, global,
///     /// Draws the map border.
///     MapBorderAdd = |lua, (desc, duration): (LuaTable, Option<f32>)| { ... })?;
/// lua_fn!(lua, table(t, "GpgNetSend"), Send = |_, ()| Ok(()))?;
/// ```
/// `global` / `table` set with `set` (honouring metatables), `global_raw` /
/// `table_raw` with `raw_set`. `#[lua(stub)]`, `#[lua(partial = "...")]`,
/// `#[lua(internal)]` mark the status. Evaluates to `LuaResult<()>`.
#[proc_macro]
pub fn lua_fn(input: TokenStream) -> TokenStream {
    let FnInput { lua, target, attrs, name, f } = parse_macro_input!(input as FnInput);
    let status = match status_of(&attrs) {
        Ok(s) => s,
        Err(e) => return e.into_compile_error().into(),
    };
    let doc = doc_of(&attrs);
    let name_s = name.to_string();
    let params = params_tokens(&f);
    let (table, scope, raw) = match target {
        Target::Global { raw } => (quote!(__lua.globals()), quote!(""), raw),
        Target::Table { table, scope, raw } => (quote!(::core::clone::Clone::clone(&#table)), quote!(#scope), raw),
    };
    quote! {{
        let __lua: &::mlua::Lua = #lua;
        let __target: ::mlua::Table = #table;
        crate::lua_api::register_fn(__lua, &__target, #scope, #name_s, #params, #doc, #status, #raw, #f)
    }}
    .into()
}

struct MethodInput {
    lua: Expr,
    class: Type,
    attrs: Vec<Attribute>,
    name: Ident,
    f: Expr,
}

impl Parse for MethodInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let lua: Expr = input.parse()?;
        input.parse::<Token![,]>()?;
        let class: Type = input.parse()?;
        input.parse::<Token![,]>()?;
        let attrs = input.call(Attribute::parse_outer)?;
        let name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let f: Expr = input.parse()?;
        let _ = input.parse::<Option<Token![,]>>()?;
        Ok(Self { lua, class, attrs, name, f })
    }
}

/// Adds a native method to a moho class (a `MohoClass` marker type) and
/// records it. The closure's first argument is `self`.
///
/// ```ignore
/// lua_method!(lua, Unit,
///     /// The unit's build rate.
///     GetBuildRate = |lua, this: Obj<Unit>| ...)?;
/// ```
#[proc_macro]
pub fn lua_method(input: TokenStream) -> TokenStream {
    let MethodInput { lua, class, attrs, name, f } = parse_macro_input!(input as MethodInput);
    let status = match status_of(&attrs) {
        Ok(s) => s,
        Err(e) => return e.into_compile_error().into(),
    };
    let doc = doc_of(&attrs);
    let name_s = name.to_string();
    let params = params_tokens(&f);
    quote! {
        crate::lua_api::register_method::<#class, _, _, _>(#lua, #name_s, #params, #doc, #status, #f)
    }
    .into()
}

// ---------------------------------------------------------------- callbacks!

struct CallbackDecl {
    attrs: Vec<Attribute>,
    name: Ident,
    params: Vec<(Ident, Type)>,
}

struct CallbackBlock {
    attrs: Vec<Attribute>,
    class: syn::Path,
    callbacks: Vec<CallbackDecl>,
}

struct CallbacksInput(Vec<CallbackBlock>);

impl Parse for CallbacksInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut blocks = Vec::new();
        while !input.is_empty() {
            let attrs = input.call(Attribute::parse_outer)?;
            let class: syn::Path = input.parse()?;
            let body;
            syn::braced!(body in input);
            let mut callbacks = Vec::new();
            while !body.is_empty() {
                let attrs = body.call(Attribute::parse_outer)?;
                let name: Ident = body.parse()?;
                let args;
                syn::parenthesized!(args in body);
                let mut params = Vec::new();
                while !args.is_empty() {
                    let p: Ident = args.parse()?;
                    args.parse::<Token![:]>()?;
                    let t: Type = args.parse()?;
                    params.push((p, t));
                    if args.is_empty() {
                        break;
                    }
                    args.parse::<Token![,]>()?;
                }
                body.parse::<Token![;]>()?;
                callbacks.push(CallbackDecl { attrs, name, params });
            }
            blocks.push(CallbackBlock { attrs, class, callbacks });
        }
        Ok(Self(blocks))
    }
}

/// Declares the script callbacks the engine calls on a moho class's script
/// objects, as typed functions (the name is written once, here):
///
/// ```ignore
/// callbacks! {
///     /// Unit script callbacks.
///     Unit {
///         /// After the unit's script object is created.
///         OnCreate();
///         OnStopBeingBuilt(builder: Option<LuaTable>, layer: Layer);
///     }
/// }
/// // -> UnitCallbacks::on_stop_being_built(lua, &obj, what, builder, layer)
/// // and the block is collected at link time for --dump-api.
/// ```
/// Each call goes through `script_object::run_script` (a missing callback
/// is a no-op; an error is logged, not propagated; `what` names the object in
/// the log).
#[proc_macro]
pub fn callbacks(input: TokenStream) -> TokenStream {
    let CallbacksInput(blocks) = parse_macro_input!(input as CallbacksInput);
    let mut out = TokenStream2::new();
    for b in blocks {
        let class = &b.class;
        let last = &class.segments.last().unwrap().ident;
        let holder = format_ident!("{}Callbacks", last);
        let block_doc = doc_of(&b.attrs);
        let block_doc = if block_doc.is_empty() { format!("Script callbacks of `{last}` objects.") } else { block_doc };
        let mut fns = Vec::new();
        let mut infos = Vec::new();
        for c in &b.callbacks {
            let lua_name = c.name.to_string();
            let fn_name = format_ident!("{}", heck::ToSnakeCase::to_snake_case(lua_name.as_str()));
            let doc = doc_of(&c.attrs);
            let doc_attr = if doc.is_empty() { format!("Calls `obj:{lua_name}(...)` if the script defines it.") } else { doc.clone() };
            let pnames: Vec<&Ident> = c.params.iter().map(|(p, _)| p).collect();
            let ptypes: Vec<&Type> = c.params.iter().map(|(_, t)| t).collect();
            let pname_strs: Vec<String> = pnames.iter().map(|p| p.to_string().trim_start_matches('_').to_string()).collect();
            fns.push(quote! {
                #[doc = #doc_attr]
                #[allow(clippy::too_many_arguments)]
                pub fn #fn_name(lua: &::mlua::Lua, obj: &::mlua::Table, what: &str #(, #pnames: #ptypes)*) -> ::mlua::Result<()> {
                    crate::engine::api::script_object::run_script(lua, obj, #lua_name, what, (#(#pnames,)*))
                }
            });
            infos.push(quote! {
                crate::lua_api::CallbackInfo {
                    name: #lua_name,
                    params: &[#(#pname_strs),*],
                    args: <(#(#ptypes,)*) as crate::lua_api::LuaArgs>::types,
                    doc: #doc,
                }
            });
        }
        out.extend(quote! {
            #[doc = #block_doc]
            pub struct #holder;
            impl #holder {
                #(#fns)*
            }
            crate::lua_api::inventory::submit! {
                crate::lua_api::CallbackBlock { class: <#class as crate::lua_api::MohoClass>::CLASS, callbacks: &[#(#infos),*] }
            }
        });
    }
    out.into()
}

// ---------------------------------------------------------------- signatures!

struct SigDecl {
    attrs: Vec<Attribute>,
    name: Ident,
    params: Vec<(Ident, Type)>,
    ret: Option<Type>,
}

struct SigBlock {
    scope: syn::Path,
    sigs: Vec<SigDecl>,
}

struct SignaturesInput(Vec<SigBlock>);

impl Parse for SignaturesInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut blocks = Vec::new();
        while !input.is_empty() {
            let _ = input.call(Attribute::parse_outer)?;
            let scope: syn::Path = input.parse()?;
            let body;
            syn::braced!(body in input);
            let mut sigs = Vec::new();
            while !body.is_empty() {
                let attrs = body.call(Attribute::parse_outer)?;
                let name: Ident = body.parse()?;
                let args;
                syn::parenthesized!(args in body);
                let mut params = Vec::new();
                while !args.is_empty() {
                    let p: Ident = args.parse()?;
                    args.parse::<Token![:]>()?;
                    let t: Type = args.parse()?;
                    params.push((p, t));
                    if args.is_empty() {
                        break;
                    }
                    args.parse::<Token![,]>()?;
                }
                let ret = if body.peek(Token![->]) {
                    body.parse::<Token![->]>()?;
                    Some(body.parse::<Type>()?)
                } else {
                    None
                };
                body.parse::<Token![;]>()?;
                sigs.push(SigDecl { attrs, name, params, ret });
            }
            blocks.push(SigBlock { scope, sigs });
        }
        Ok(Self(blocks))
    }
}

/// Declares the Lua signatures of functions registered without types: stubs
/// and functions that take `LuaMultiValue` (a stub accepts anything, so its
/// signature can't come from its closure). Only the generated definitions
/// use them; nothing changes at run time. A block is `global` or a moho
/// class; the same name twice in a row declares overloads.
///
/// ```ignore
/// signatures! {
///     global {
///         /// Draws a circle (debug).
///         DrawCircle(center: Vector, diameter: f64, color: Color);
///         GetUnitsInRect(rect: Rect) -> Option<Vec<Obj<moho::Unit>>>;
///         GetUnitsInRect(x0: f64, z0: f64, x1: f64, z1: f64) -> Option<Vec<Obj<moho::Unit>>>;
///     }
///     moho::Unit {
///         SetCustomName(name: String);
///     }
/// }
/// ```
#[proc_macro]
pub fn signatures(input: TokenStream) -> TokenStream {
    let SignaturesInput(blocks) = parse_macro_input!(input as SignaturesInput);
    let mut out = TokenStream2::new();
    for b in blocks {
        let class = if b.scope.is_ident("global") {
            quote!("")
        } else {
            let p = &b.scope;
            quote!(<#p as crate::lua_api::MohoClass>::CLASS)
        };
        let one = |d: &SigDecl, overloads: TokenStream2| {
            let name = d.name.to_string();
            let doc = doc_of(&d.attrs);
            let pnames: Vec<String> = d.params.iter().map(|(p, _)| p.to_string().trim_start_matches('_').to_string()).collect();
            let ptypes: Vec<&Type> = d.params.iter().map(|(_, t)| t).collect();
            let ret = d.ret.as_ref().map_or(quote!(()), |t| quote!(#t));
            quote! {
                crate::lua_api::Signature {
                    name: #name,
                    params: &[#(#pnames),*],
                    args: <(#(#ptypes,)*) as crate::lua_api::LuaArgs>::param_types,
                    rets: <#ret as crate::lua_api::LuaArgs>::types,
                    doc: #doc,
                    overloads: #overloads,
                }
            }
        };
        let mut sigs = Vec::new();
        let mut i = 0;
        while i < b.sigs.len() {
            let mut j = i + 1;
            while j < b.sigs.len() && b.sigs[j].name == b.sigs[i].name {
                j += 1;
            }
            let rest: Vec<TokenStream2> = b.sigs[i + 1..j].iter().map(|d| one(d, quote!(&[]))).collect();
            sigs.push(one(&b.sigs[i], quote!(&[#(#rest),*])));
            i = j;
        }
        out.extend(quote! {
            crate::lua_api::inventory::submit! {
                crate::lua_api::SignatureBlock { class: #class, sigs: &[#(#sigs),*] }
            }
        });
    }
    out.into()
}
