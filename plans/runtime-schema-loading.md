# Runtime schema loading (`SchemaLoader`) — design & work plan

Goal: given schema nodes that are **not compiled into the binary** (typically a
`CodeGeneratorRequest` produced by `capnp compile -o-`), produce `StructSchema` /
`EnumSchema` / `introspect::Type` values that drive the existing dynamic API
(`dynamic_struct`, `dynamic_list`, `dynamic_value`, `stringify`) exactly as the compiled-in
ones do. A generic tool can then read a framed message whose concrete type is only known at
run time (e.g. from an envelope's type id) and walk it dynamically.

Schema *compilation* stays with C++ `capnp compile`. We only consume its output (a CGR,
or any `List(Node)` / individual `Node`s from it).

**Prerequisite (done):** `plans/newtype-evolution-fixes.md`. It fixed the newtype layout bugs in
the compiler (`../capnproto` 6d4efa4f), the C++ codegen (81063fd2) and the Rust codegen/runtime
(7dd34914: per-level `capnp::traits::UseSiteTable<'t>`, `RawStructSchema::any_table`). §3a
builds on that representation. The crate is published as `capnp-newtype` (and `capnpc-newtype`),
so cargo commands use `-p capnp-newtype`; the library is still `capnp` in code.

## What we have today (verified)

- `introspect::RawStructSchema` (`capnp/src/introspect.rs:248`): per-node static data:
  `arena: &'static GeneratedCodeArena` (the encoded `Node`), `nonunion_members`,
  `members_by_discriminant`, `members_by_name`, plus newtype tables. All `pub(crate)`,
  built through `const fn new(..)` + `with_*` builders. **A loader inside the crate can build
  one from leaked data with no changes.**
- `introspect::RawBrandedStructSchema` (`introspect.rs:327`): one per *instantiation*:
  ```rust
  pub struct RawBrandedStructSchema {
      pub generic: &'static RawStructSchema,
      pub field_types: fn(u16) -> Type,
      pub annotation_types: fn(Option<u16>, u32) -> Type,
      pub type_id: core::any::TypeId,          // e2332219: Eq/Hash key
  }
  ```
  All fields are `pub` and generated code builds it **by struct literal**
  (`capnpc/src/codegen.rs:3805, 3816, 3867, 3970`).
- Branding is encoded *in Rust's monomorphisation*: `get_field_types::<A, B>` is a different
  `fn` per instantiation. There is no way to keep it in data, because Rust has no generic statics.
  This is the core obstacle. A plain `fn(u16) -> Type` has **no context pointer**, so a loader
  cannot supply one per loaded node or instantiation.
- `RawEnumSchema` (`introspect.rs:376`): `arena` + `annotation_types: fn(..)`, fields
  `pub(crate)`, built via `RawEnumSchema::new`. Eq/Hash = arena pointer identity. Loaded
  enums get their own leaked arena, so **they are automatically never equal to compiled enums**;
  only `annotation_types` needs a context-carrying alternative.
- `AnnotationList` stores `get_annotation_type: fn(Option<u16>, u32) -> Type`
  (`schema.rs:528`), and `FieldList::get` / `FieldSubset::get` call `(raw.field_types)(index)`
  (`schema.rs:261, 308`). These are the only two places the fn pointers are invoked.
- Equality is load-bearing for the dynamic API, not just for maps:
  - `dynamic_struct::{Reader,Builder}::downcast<T>` asserts
    `Type::Struct(self.schema.raw) == T::introspect()` (`dynamic_struct.rs:240, 791`).
  - `dynamic_list` set-struct asserts `ss == s.get_schema().raw` (`dynamic_list.rs:329`).
  - `struct_list`/`list_list`/`enum_list`/`data_list` downcasts assert `element_type() == T::introspect()`.
- There is **no public way** to make a `dynamic_struct::Reader` from an
  `any_pointer::Reader` + `StructSchema`. `dynamic_struct::Reader::new` takes a
  `private::layout::StructReader`.
- `capnpc::CompilerCommand::raw_code_generator_request_path` (`capnpc/src/lib.rs:287`)
  already dumps the raw CGR. Tests can get CGR bytes for `test.capnp` for free.
- The CGR's `nodes` list contains every node the compiler parsed, **including imports**, so a
  CGR is a closed set (apart from interfaces we don't model anyway).

### What C++ does (`../capnproto/c++/src/capnp/schema-loader.{h,c++}`)

- `Schema::operator==` is `raw == other.raw`, i.e. **pointer identity of `RawBrandedSchema`**.
  The loader allocates its own `RawSchema`s, so a loaded schema is never `==` to a native one,
  even with the same id.
- To allow `DynamicStruct::as<T>()` on loaded data, the loaded `RawSchema` carries
  `canCastTo = &nativeSchema`, set by `loader.loadCompiledTypeAndDependencies<T>()`
  (`loadNative`, schema-loader.c++:1312). `requireUsableAs<T>()` accepts
  `*this == from<T>()` **or** `canCastTo == native(T)`.
- Brand instances are interned per `(generic, bindings)` so that repeated lookups produce the
  same `RawBrandedSchema*`. That is what makes identity equality usable.
- Unbound brand parameters resolve to `AnyPointer` (unless `getUnbound`).
- `load()` validates: struct sizes, member kinds, field offsets in-bounds, ordinals/codeOrders
  sequential, discriminants, value union cases match types. Unknown dependencies become
  stub schemas, and the dynamic API reports `UNKNOWN`.

## Design

### 1. Equality model (replacing `TypeId`-only identity)

The user's proposal, matching C++:

| lhs \ rhs | compiled | loaded |
|---|---|---|
| **compiled** | `TypeId` equality (unchanged from e2332219) | never equal |
| **loaded** | never equal | pointer identity of the interned brand instance |

- Two lookups of the same `(node id, bindings)` in **one** loader return the same pointer, so
  they are equal. The same type loaded by two different loaders is unequal, as in C++.
- "Usable as" is a *separate, weaker* relation used only by downcasts (§5). `Eq` stays a true
  identity relation, so `Hash` and `HashMap<Field, _>` (the JSON-codec use case behind #675)
  keep working with loaded fields.

`TypeId` stays the right key for compiled schemas. It is the only sound way to tell
`Foo<Text>` from `Foo<Data>` without comparing fn pointers, which Miri rejected. We don't
remove it; we put it under one variant of a sum type.

### 2. Representation change: make the brand a sum type

```rust
// introspect.rs
#[derive(Copy, Clone)]
pub struct RawBrandedStructSchema {
    pub generic: &'static RawStructSchema,
    pub(crate) brand: StructBrand,
}

#[derive(Copy, Clone)]
pub(crate) enum StructBrand {
    Compiled {
        field_types: fn(u16) -> Type,
        annotation_types: fn(Option<u16>, u32) -> Type,
        brand_binding: fn(u64, u16) -> Option<Type>,   // (scope id, param index); see below
        type_id: core::any::TypeId,
    },
    #[cfg(feature = "std")]
    Loaded(&'static crate::schema_loader::LoadedBrand),
}

impl RawBrandedStructSchema {
    /// Used by generated code.
    pub fn compiled(
        generic: &'static RawStructSchema,
        field_types: fn(u16) -> Type,
        annotation_types: fn(Option<u16>, u32) -> Type,
        brand_binding: fn(u64, u16) -> Option<Type>,
        type_id: core::any::TypeId,
    ) -> Self { .. }

    pub(crate) fn field_type(&self, index: u16) -> Type { match self.brand { .. } }
    pub(crate) fn annotation_type(&self, child: Option<u16>, i: u32) -> Type { .. }
    pub(crate) fn brand_binding(&self, scope_id: u64, index: u16) -> Option<Type> { .. }
    pub fn is_loaded(&self) -> bool { .. }
}
```

**Brand bindings as data (`brand_binding`).** Compiled branding currently exists only through
monomorphisation: nothing can ask a compiled `Foo<Text>` "what is `T`?". Codegen emits one more
function next to `get_field_types`:
```rust
pub(crate) fn get_brand_binding<T: Owned>(scope_id: u64, index: u16) -> Option<Type> {
    match (scope_id, index) {
        (0xabcd_..., 0) => Some(<T as Introspect>::introspect()),
        _ => None,
    }
}
```
- It can't be a `const BINDINGS: &[Type]`: `introspect()` and `TypeId::of` aren't `const fn`, and
  a `&'static` slice for each instantiation would need generic statics. It's a function for the
  same reason `field_types` is.
- It's keyed by `(scope_id, index)` rather than a flat list, because a nested generic binds
  several scopes: `Outer(T).Inner(U)` has both `T` and `U`, and since 42ba874a the Rust `Inner`
  `Owned` carries both. This matches C++ `Schema::getBrandBinding(scopeId, index)`.
- Non-generic structs all share one `capnp::introspect::no_brand_bindings` (`|_, _| None`).
- Expose it publicly as `StructSchema::get_brand_binding(scope_id, index) -> Option<Type>` for
  compiled and loaded schemas alike, and `StructSchema::get_brand_scopes()` for enumerating them
  (from the node's `parameters` and the scope chain). This is C++ parity and useful in its own
  right, e.g. for a generic tool printing `Message(Request(Text))`.
- Loaded brands answer it from their `Bindings`; unbound parameters return `AnyPointer`.

- `PartialEq`/`Hash`: match on the variant. `Compiled` uses `type_id`, and `Loaded` uses
  `ptr::eq` / pointer hash. Mixed variants are unequal. The hash includes the discriminant.
- `schema.rs` call sites use `raw.field_type(i)` / `raw.annotation_type(..)`.
  `AnnotationList` stores an `AnnotationSource` (a `Copy` enum carrying either the fn pointer
  or the `&'static Loaded*`) instead of a bare fn pointer.
- `RawEnumSchema` gets the same treatment for `annotation_types` only
  (`pub(crate) annotations: EnumAnnotations`). `RawEnumSchema::new` keeps its signature, so
  generated enum code is unaffected. Eq stays arena-pointer identity, which already gives the
  desired semantics.
- `Debug for StructSchema` (`schema.rs:157`) currently prints `raw.type_id`. Print the
  variant instead.

**Compatibility cost:** replacing the `pub` fields with `pub(crate) brand` is a breaking change
to `RawBrandedStructSchema`. Generated code must switch from the struct literal to
`RawBrandedStructSchema::compiled(..)`. That means four `fmt!` sites in `codegen.rs`, plus
regenerating `capnp/src/schema_capnp.rs` and `capnp-rpc` schema code
(`regenerate-capnp-schema-code.sh`, `regenerate-rpc-schema-code.sh`). capnp/capnpc are
version-locked anyway (e2332219 was the same kind of break). Ship as **0.28**.

*Alternatives rejected:*
- Changing the fn signature to `fn(ctx: *const (), u16) -> Type` keeps a struct literal but
  needs raw pointers and unsafety, and still needs a variant tag for Eq.
- `&'static dyn BrandSource` for compiled code needs a `static` per *generic instantiation*,
  which Rust cannot express.
- Keeping the `pub` fields and adding a `loaded: Option<..>` field breaks struct literals just
  the same, and leaves meaningless fn pointers in loaded schemas.

### 3. The loader (`capnp/src/schema_loader.rs`, `std` feature)

```rust
pub struct SchemaLoader { inner: std::sync::Mutex<Inner> }

impl SchemaLoader {
    pub fn new() -> Self;

    /// Validate, copy and resolve a batch of nodes. The batch plus everything previously
    /// loaded must be closed: any reference to an id not present fails the whole batch and
    /// loads nothing.
    pub fn load<'n>(&self, nodes: impl IntoIterator<Item = schema_capnp::node::Reader<'n>>) -> Result<()>;
    /// Convenience: load every node in a CGR (`capnp compile -o- foo.capnp`). Always closed.
    pub fn load_code_generator_request(&self, cgr: code_generator_request::Reader) -> Result<()>;

    /// Look up by id. Generic params unbound → AnyPointer (C++ semantics).
    pub fn get_struct(&self, id: u64) -> Result<StructSchema>;
    pub fn get_enum(&self, id: u64) -> Result<EnumSchema>;
    pub fn get_node(&self, id: u64) -> Result<node::Reader<'static>>;   // any kind, incl. interfaces/consts
    /// Explicit branding, as in `SchemaLoader::get(id, brand, scope)`.
    pub fn get_struct_branded(&self, id: u64, brand: brand::Reader, scope: Option<StructSchema>) -> Result<StructSchema>;
    /// Resolve a schema `Type` (e.g. from an annotation or a user's own envelope).
    pub fn get_type(&self, ty: type_::Reader, scope: Option<StructSchema>) -> Result<Type>;

    /// Like C++ loadCompiledTypeAndDependencies<T>(): permits `downcast::<T>()` on loaded values.
    pub fn load_compiled<T: crate::traits::Owned>(&self) -> Result<()>;
}
```

**Lifetime:** everything the loader hands out is `'static`, and the loader `Box::leak`s its
allocations. Dropping a `SchemaLoader` must *not* free them. See "Memory" below for why there's
no practical alternative under the current API, and for the dedup mitigation.

**Per-node storage (`LoadedNode`, leaked once per id):**
1. **Copy the node into a fresh single-segment buffer** by `canonicalize()`-ing a message whose
   root is the node. That gives a flat `Vec<Word>`, leaked to `&'static [Word]`. The copy goes
   through the bounds-checked reader, so the leaked words are structurally sound, and
   `GeneratedCodeArena` (no read limit) is then safe to reuse as the arena. `StructSchema::new`'s
   `get_root_from_arena(..).unwrap()` can't fail on these words.
2. Compute `nonunion_members`, `members_by_discriminant`, `members_by_name` exactly as
   `codegen.rs` `generate_members_by_*` does. Then leak them and build a `RawStructSchema` with
   `RawStructSchema::new(..)`.
3. Newtypes: compute `field_newtypes`, and for use-site groups `newtype_ids` / `any_table`, using the same shared code codegen uses (§3a). Then
   `Field::get_newtype_ids()`, `StructSchema::get_newtype_ids()` and AnyReader/AnyBuilder
   downcasts all work on loaded schemas.

**Brand instances (`LoadedBrand`, interned):**
```rust
pub(crate) struct LoadedBrand {
    node: &'static LoadedNode,
    bindings: &'static Bindings,              // scopeId -> [Type] | Inherit, canonicalised
    field_types: &'static [FieldType],        // computed at load / at instantiation
    annotation_types: &'static [((Option<u16>, u32), Type)],
    can_cast_to: Mutex<Vec<TypeId>>,          // §5: memoised positive checks
}

enum FieldType {
    Resolved(Type),
    /// Only for a field whose type is a *different generic instantiation* (see below).
    Deferred { ty: type_::Reader<'static>, scope: &'static LoadedBrand, cell: OnceLock<Type> },
}
```
- Interned in `Inner.brands: HashMap<(u64, Bindings), &'static LoadedBrand>`. `Bindings` contains
  `Type`s, which are already `Eq + Hash`, so the key works as-is. **Interning is required, not an
  optimisation.** Without it, `List(Foo)` built from one lookup would fail the
  `dynamic_list.rs:329` assertion against a `Foo` from another.

**Resolution is done at load, eagerly. Every possible failure happens there:**
- `load()` validates the batch (§4) and checks that every id referenced anywhere in it (field
  types, brand bindings, group nodes, annotation ids) exists and has the right kind. After
  that, nothing downstream can fail: resolving a type is pure computation over validated data.
- For every non-generic struct, and for the default (all-`AnyPointer`) brand of every generic
  struct, the full `field_types` and `annotation_types` tables are computed at load. Recursive
  and mutually recursive types (`struct A { next @0 :A; }`) need a two-phase build: first
  allocate (leak) one `LoadedBrand` per node so every target address exists, then fill the tables.
  The tables are written before anything is published, so this can be done with `MaybeUninit`
  or `OnceLock::set` during load. Readers then see plain immutable data either way.
- **The one exception is generic instantiations.** The set of instantiations reachable from a
  schema can be **infinite** (polymorphic recursion). This compiles fine with `capnp`:
  ```capnp
  struct Box(T) { v @0 :T; }
  struct Foo(T) { value @0 :T; child @1 :Foo(Box(T)); }   # Foo(Text) -> Foo(Box(Text)) -> …
  ```
  So instances with non-default bindings are interned **on first reach** (from `get_struct_branded`,
  or the first time a field typed `Foo(Box(T))` is read). The instance's own table is computed
  eagerly at that point. A field pointing to a *further* distinct instantiation is the only
  `FieldType::Deferred`. That step is infallible because load already validated it: it's a
  cache fill, not a lazy load. Non-generic schemas never go through this path.

Resolution follows C++ `makeBranded` / `getBrandBinding`:
  - `struct`/`enum` `typeId` + `brand`: get or intern that node's brand with bindings resolved
    in the current scope.
  - `anyPointer.parameter{scopeId, parameterIndex}`: look up in `bindings`, unbound
    becomes `AnyPointer`.
  - `anyPointer.implicitMethodParameter`: `AnyPointer`.
  - `interface`: `Capability`. `Type` carries no interface schema today.
  - group field: the group node's brand with the **parent's bindings** (groups share the
    parent's scope), mirroring 42ba874a for compiled code.
- `LoadedBrand` must be `Sync` (`OnceLock`/`Mutex`, not `Cell`), so `Type`/`StructSchema` stay
  `Send + Sync` like the compiled ones. Check that with a `static_assertions`-style test.
- The interner `Mutex` is held only while loading or interning, never while user code runs.

**Missing dependencies:** the load fails. `Type` has no `Unknown` variant (adding one is a large
public break), and `Field::get_type` returns `Type`, not `Result`. So an incomplete batch is
rejected by `load()` as a whole. A CGR is always closed.

**Memory.** Leaking is forced by the current API, not chosen:
- `'static` is in the public surface. `Type`/`TypeVariant`/`StructSchema`/`Field` are `Copy` and
  hold `&'static` data, `StructSchema::get_proto()` returns `node::Reader<'static>`, and
  `Annotation::get_value()` returns `dynamic_value::Reader<'static>`.
- `Arc` doesn't fit: it isn't `Copy`, so it would remove `Copy` from `Type` and every type
  holding one (including dynamic readers). It would add a refcount bump to every `get_type`.
  And `node::Reader<'static>` borrowing from inside the `Arc` can't be expressed without a
  self-referential type.
- An `unsafe fn unload()` ("caller promises no schema or value is alive") is possible but an
  unchecked footgun. Not proposed.
- The real fix is a schema lifetime: `StructSchema<'s>`, `Type<'s>`, and so on, with compiled code
  using `'static`. That would let a loader own its memory. It touches every public dynamic and
  schema type, so it's a separate breaking project. Keeping all loader data behind `&'static`
  references, never handed-out raw pointers, keeps that migration open.
- **Mitigation in v1:** content-addressed dedup. Reloading a node whose `(id, canonical
  bytes)` is already loaded reuses the existing allocation, so a long-running tool that reloads
  the same schemas leaks nothing new. Memory is bounded by the number of *distinct schema
  versions* seen, and nodes are a few KB each. A different version of an already-loaded id is
  an error in v1 (see §4), so this is also the only way a batch can overlap previous ones.

### 3a. Group/union newtypes on loaded schemas (AnyReader / AnyBuilder downcasts)

`downcast::<dec19::AnyReader>()` (64cebfba) must work on a group read through a loaded schema.

**Depends on `newtype-evolution-fixes.md` landing first.** That plan fixes the compiler's
`@[...]` expansion (Bug C), replaces the flat tree-ordered offset tables with per-level
`UseSiteTable`s indexed by template field-list position (Bug A), and treats a table shorter or
longer than the carrier expects as unmapped/ignored instead of asserting (Bug B). Everything
below assumes that representation.

**It's mostly a data problem.** `downcast_any_reader`/`downcast_any_builder`
(`dynamic_value.rs`) never look at Rust identity. They read the group's `RawStructSchema`:
- `newtype_ids`, checked to contain `T::TYPE_ID`, a schema node id;
- `any_table: Option<&'static UseSiteTable<'static>>`, the use site's per-level offsets plus
  the union discriminant offset.

Both are pure functions of CGR nodes: the use-site group, its parent field, the `type` node and
its template. If the loader fills them with the values codegen would emit, the existing downcast
path works unchanged. Loaded schemas are no special case.

**Share the algorithm instead of duplicating it.** Move the layout code (`field_newtype_ids`,
`newtype_template_id`, `group_/union_newtype_template_id`, the table builder and
`use_site_any_layout`) out of `capnpc/src/codegen.rs` into the `capnp` crate as a
`#[doc(hidden)] pub mod private::newtype_layout`, generic over a lookup:
```rust
pub trait NodeLookup<'a> { fn node(&self, id: u64) -> Option<node::Reader<'a>>; }
pub fn field_newtype_ids(nodes: &impl NodeLookup, field: field::Reader) -> Result<Vec<u64>>;
pub fn use_site_layout(nodes: &impl NodeLookup, group_node: node::Reader)
    -> Result<Option<UseSiteLayout>>;   // { newtype_ids, table: owned tree of entries }
```
`capnpc` implements `NodeLookup` for `node_map` and renders `UseSiteLayout` as nested
`static`s. The loader implements it over the batch plus previously loaded nodes and leaks the
tree. One implementation means a loaded use site and a compiled use site from the same CGR
produce identical tables by construction. Test this directly: for every use site in
`test-newtype.capnp`, the loaded table equals the compiled one.

**Version skew is evolution, not an error.** We assume the capnp evolution rules are followed:
existing fields keep their type and their position in the template's `fields` list, new fields
are appended, and renames are free. Under those rules a per-level table from a *different*
schema version than the compiled carrier is still correct:
- leaves the carrier doesn't know about are ignored;
- leaves the table lacks read as defaults, and panic if set.

So there's no fingerprint and no leaf-count check: id plus per-level tables is enough. Renames
are harmless because the loader matches template leaves to the use site by name *within one
CGR*, and carriers index by position. A schema that breaks the evolution rules gives
wrong-but-bounds-checked reads, the same as reading any struct with an incompatible schema.

**Nothing else is needed.** The carrier reads through the group's `StructReader`, which is the
same whether the schema is loaded or compiled. `load_compiled::<T>()` isn't required for newtype
downcasts, because the check is by id, not by schema identity.

### 4. Validation (untrusted input)

A malformed node must never cause UB, and should not cause panics in `schema.rs`/dynamic code.
Validate at `load()`, covering at least C++'s list plus everything
our code currently `unwrap()`s or `panic!()`s on:
- the node is a struct/enum/interface/const/annotation/`type` node; `StructSchema` methods
  `panic!()` on non-struct.
- struct: `dataWordCount`/`pointerCount` sane; `discriminantCount` matches the number of
  fields with a discriminant; discriminant values are dense `0..n` and unique, because
  `members_by_discriminant` is indexed by them; slot offsets fit within the declared section
  sizes for their type; group fields point at nodes whose `scopeId` is this node and which are
  `isGroup`; field count < `u16::MAX`.
- enum: enumerant count < `u16::MAX`.
- `Type` references point at nodes of the right kind; brand `scopeId`s are ancestors and
  parameter indices are in range.
- default `Value`s have the union case matching the slot type (`dynamic_value::Reader::new`
  matches on `(value, type)`; verify a mismatch errors rather than panics).
- re-loading an id: identical canonical bytes are a no-op; otherwise `Err`. Skip C++'s
  "pick the newer compatible version" in v1, because swapping a node invalidates outstanding
  `'static` schemas.

### 5. Downcasting loaded values to compiled types (`can_cast_to`)

As in C++, add a "usable as" check alongside `==`:
```rust
impl Type { pub(crate) fn usable_as(self, target: Type) -> bool }  // == , or the structural check below
```
Use it in `dynamic_struct::{Reader,Builder}::downcast` and in the list `downcast_*` impls in
place of `==`.

**The check.** `usable_as(loaded, compiled)` for struct types holds iff:
1. both have the same node id;
2. the node is **compatible**. v1 means the canonical node bytes are equal. C++'s
   `CompatibilityChecker` (layout-compatible evolution) is a later improvement. This is checked
   once per node id and memoised in the loader;
3. for every brand scope and parameter of the node, `loaded.brand_binding(s, i)` is `usable_as`
   `compiled.brand_binding(s, i)`, recursing into struct bindings.

Enums use (1) and (2). Primitives, `Text`, `Data`, `AnyPointer` and `Capability` compare with
`==`. `List(x)` recurses into `x`.

Why bindings and not field types: in a protocol, a `Message(T)` is easily used as both
`Message(Request(T))` and `Message(Reply(T))`. These have to be told apart exactly, including
when a parameter is phantom or is used only deep inside a nested group. Comparing bindings is
exact: compiled `Foo<Text>` and `Foo<Data>` stay distinct even when no field uses `T`. It is
also cheap, because bindings are finite trees: `Message(Request(Text))` is three nodes deep, so
the recursion terminates without a visited set. Positive results are memoised per loaded brand
(`can_cast_to: Vec<TypeId>`).

**`load_compiled::<T>()`** (C++ `loadCompiledTypeAndDependencies<T>()`) walks the graph from
`T::introspect()` through `field_type` and `brand_binding`. For each compiled struct or enum it
reaches:
- if the loader has a node with that id, it runs the compatibility check (2) now and records the
  result. A mismatch is an `Err` from `load_compiled`, not a later downcast panic.
- if it doesn't, it inserts the compiled node's encoded words as a loaded node. A loaded parent
  type can then reference it and still downcast.

Because compiled bindings are now data, `load_compiled` can also translate `T`'s bindings into
loaded `Type`s and intern **that exact instance** (`Message(Request(Text))`). A later loaded value
of that type then hits the memoised path on its first downcast.

`downcast::<T>()` still asserts, as today. Add `try_downcast::<T>() -> Result<T::Reader<'a>>` for
tools whose input types aren't trusted.

### 6. Public entry points into the dynamic API

There is no public way to point a schema at bytes today. Add:
```rust
impl<'a> any_pointer::Reader<'a> {
    pub fn get_as_dynamic(&self, schema: StructSchema) -> Result<dynamic_struct::Reader<'a>>;
}
impl<'a> any_pointer::Builder<'a> {
    pub fn get_as_dynamic(self, schema: StructSchema) -> Result<dynamic_struct::Builder<'a>>;
    pub fn init_dynamic(self, schema: StructSchema) -> dynamic_struct::Builder<'a>;  // uses struct_size_from_schema
}
impl<S: ReaderSegments> message::Reader<S> {
    pub fn get_root_as_dynamic(&self, schema: StructSchema) -> Result<dynamic_struct::Reader<'_>>;
}
```
These help compiled schemas too: `T::introspect()` → `StructSchema` → read an `AnyPointer` as `T`
dynamically.

### 7. "Discovering" the schema: out of scope for the crate, provided as an example

The library gives you id → schema. How a tool learns the id and finds the nodes is policy:
- **Schema source:** `capnp compile -o- foo.capnp > foo.cgr`, shipped alongside the tool, or
  embedded with `include_bytes!`, or fetched from a registry. `load_code_generator_request`
  takes it directly.
- **Envelope:** a convention such as
  ```capnp
  struct Envelope {
    typeId  @0 :UInt64;          # node id of the payload struct
    payload @1 :AnyPointer;
    schema  @2 :List(Schema.Node);   # optional: make the stream self-describing
  }
  ```
  The tool reads `Envelope` (compiled), loads `schema` if present, then calls
  `loader.get_struct(typeId)?` and `payload.get_as_dynamic(schema)?`.
- Later, a `LazyLoadCallback` equivalent (`trait SchemaSource { fn find(&self, id) -> Option<..> }`)
  can be added without API breaks. `load()` would consult it for missing ids before
  rejecting the batch, so the "complete at load" rule still holds.

Deliver this as `example/dynamic-dump` (reads an `Envelope` + CGR file, prints the payload via
`stringify`/`Debug`) to prove the end-to-end story.

## Work plan

1. **Representation refactor (no behaviour change).** Add the `StructBrand` enum with only
   `Compiled`, the `RawBrandedStructSchema::compiled` constructor, the `field_type`/`annotation_type`
   accessors, and `AnnotationSource`/`EnumAnnotations`. Switch the codegen's four `fmt!` sites
   to the constructor and regenerate the bootstrap/RPC schema code. Existing tests (incl.
   `schema.rs` equality/hash tests, `capnpc/test/dynamic.rs`) must pass unchanged. Run Miri on
   the `schema.rs` tests.
1b. **Compiled brand bindings** (§2): emit `get_brand_binding::<..>` in codegen, pass it to the
   constructor, and add `StructSchema::get_brand_binding` / `get_brand_scopes`. Test on compiled
   code only, using `test.capnp`'s generics: nested scopes (`Outer(T).Inner(U)`), inherited group
   brands, and a phantom parameter (`Foo<Text>` and `Foo<Data>` report different bindings).
   This is useful on its own, so it can land before the loader.
2. **Dynamic entry points** (§6), with tests using compiled schemas.
3. **Loader core** (§3, non-generic): `LoadedNode` copy + member tables, the `Loaded` variant,
   interning, batch closure check, two-phase eager field tables, content dedup, `get_struct`/`get_enum`/`get_node`.
   Equality tests: loaded == loaded (same loader), loaded != compiled, two loaders unequal,
   `HashMap<Field, _>` with loaded fields.
4. **Conformance tests.** In `capnpc/test/build.rs`, set `raw_code_generator_request_path`
   to dump `test.capnp`'s CGR. For a fully populated `TestAllTypes` (from `test_util`), compare
   `stringify` of the **compiled** dynamic reader against the **loaded** dynamic reader
   (same text), including defaults (`TestDefaults`), unions, groups, lists of structs/enums,
   newtype ids (`test-newtype.capnp`), and dynamic *builders* (set fields through a loaded
   builder, then read back through compiled code).
5. **Generics/branding** (§3 resolution): `TestGenerics`, `TestUseGenerics` from `test.capnp`.
   Check that the field types of loaded `Foo<Text>` resolve to `Text`, unbound params to
   `AnyPointer`, inherited group brands, and that interned brands are pointer-equal across
   lookups. Loaded and compiled `get_brand_binding` must agree in shape for the same
   instantiation.
6. **Validation** (§4) plus negative tests (hand-built malformed nodes: bad discriminants, wrong
   kind references, missing deps, conflicting reload). Consider a `cargo fuzz` target feeding
   arbitrary `Node` bytes into `load` and then `stringify`-ing a fixed message.
6b. **Newtype layouts** (§3a; after `newtype-evolution-fixes.md`): move the layout functions
   into `capnp::private::newtype_layout` behind `NodeLookup`, and switch codegen to them
   (generated output must be byte-identical). Fill `newtype_ids`/`any_table` in the loader.
   Tests:
   - for every use site in `test-newtype.capnp`, the loaded table equals the compiled one;
   - `downcast::<X::AnyReader>()` and `AnyBuilder` work on loaded groups, including union
     newtypes, nested newtypes, aliases (`type Bar = Foo`) and incomplete `@[...]` mappings;
   - a v2 CGR (the evolved `Vec3`/`Rect` reproducer) loaded against v1-compiled carriers reads
     the v1 fields correctly and ignores the new ones, and a v1 CGR against v2 carriers reads the
     new fields as defaults.
7. **`load_compiled::<T>` + `usable_as` + `try_downcast`** (§5): downcast a loaded value to
   compiled `T`. Must-have cases: `Message(Request(Text))` downcasts to
   `message::Owned<request::Owned<text::Owned>>` but **not** to `message::Owned<reply::Owned<..>>`;
   a phantom-parameter mismatch is rejected; mismatched node bytes give an `Err` from
   `load_compiled`; lists of generic structs downcast through `struct_list`.
8. **Example tool** (§7) and docs (`capnp/src/lib.rs` module docs; CHANGELOG entry noting
   the `RawBrandedStructSchema` break).

## Decisions

- **Polymorphic recursion** (`Foo(T) { child :Foo(Box(T)) }`): the loader handles it, through
  instantiations created on first reach (§3). capnpc-rust codegen probably can't monomorphise
  it. That's out of scope.
- **Group/union newtype downcasts** are required (§3a).
- **Interfaces:** out of scope. `Type::Capability` carries no schema, so loaded interface nodes
  are reachable only through `get_node`. No dynamic RPC.
- **no_std:** the loader and the `Loaded` variant are `std`-only.
- **Memory:** leaking is accepted, with content dedup (§3).

## Status (2026-10-15)

All work-plan steps are implemented on branch `schema-loader`:

| Step | Commits |
|---|---|
| 1, 1b: brand sum type, compiled brand bindings | c0621343, b9e1c8a7 |
| 2: dynamic entry points | df8e7127, c5dd794b |
| 6b (codegen half): `capnp::private::newtype_layout` | 592f20ae |
| 3–6, 6b (loader half): `SchemaLoader` | d7d3d292, 27ddc040 |
| 7: `load_compiled`, `Type::usable_as`, `try_downcast` | 0b004e0f, 44e8485e |
| 8: `example/dynamic-dump`, docs, changelogs | 1f1f3662, 1e0d25f7 |
| codegen `MEMBERS_BY_NAME` sorted by schema name (found by the loader) | e05eda64 |

Deviations from the design above:
- `init_dynamic` / `init_root_dynamic` return `Result`, because a malformed loaded schema must not panic there.
- `StructBrand::Loaded` holds a `BrandRef` (a `NonNull` wrapper around a never-freed
  `&'static LoadedBrand`). Holding the reference directly would trip clippy's
  `mutable_key_type` on every `HashMap<Field, _>`.
- Leaked loader memory stays reachable through a process-wide registry of loaders, so Miri
  sees no leaks. A failed batch leaks nothing.
- Compiled `StructSchema`/`Field` were never `Send + Sync`, because they hold a `node::Reader`.
  The `Send + Sync` assertion therefore covers `SchemaLoader`, `Type` and the raw schemas.
- `Type::usable_as` is public, since list downcasts have no fallible variant. A downcast
  requires `load_compiled::<T>()` first, as in C++.
- `Deferred` field types cover any non-default instantiation not yet interned, not only
  polymorphic recursion.

Known gaps:
- Compatibility means identical canonical node bytes. There is no C++-style `CompatibilityChecker`.
- `load_compiled` can't insert compiled nodes that refer to annotation, `type` or interface
  nodes generated code doesn't carry. Load those from a CGR first, or it returns `Err`.
- `dynamic_list`'s set-struct check still uses `==`.
- A generic field bound to an interface reads as `FieldAndDefaultMismatch`. That limitation of the
  dynamic API predates the loader and applies to compiled schemas too.
- Annotation values aren't type-checked at load. A mismatch surfaces as `TypeMismatch` from
  `get_value`.
- Changelogs say "Unreleased". This is a breaking release, so it should probably be 0.28.0.

TODO (deferred, 2026-10-15):
- `SchemaLoader::get_struct_bound(id, &[Type])`: binds a struct's own parameters in order, so a
  caller needn't build a `Brand` message by hand. Deferred because a `&[Type]` can't be sent over
  a wire; a CGR or an explicit `Brand`/`Type` message is the practical form for that.
- Exporting a compiled type's schema for a remote loader: there is no public way to collect the
  nodes a compiled type reaches (`CompiledGraph` is private to `load_compiled`). There is also no
  encoder from a compiled branded `introspect::Type` to a `schema.capnp` `Type`, which the
  receiver would pass to `get_type`. The collected nodes aren't closed either: generated code
  doesn't carry annotation, `type` newtype or interface nodes, so a batch with any of those
  references is rejected.
