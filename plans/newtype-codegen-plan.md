# `type` newtypes in capnproto-rust — implementation sketch & work plan

Downstream companion to the C++ `type` (newtype / inline-group) feature. This plans the
capnproto-rust codegen side: giving `type` declarations a semantic presence in generated
Rust, mirroring the C++ wrapper design (concrete zero-cost form + an erased boundary form)
using Rust's native idioms (traits + per-use-site impls, not C++ class templates).

## Premise (verified against the repo)

- **Schema is already synced.** Commits `a20b1677` ("Update schema for newtypes") and
  `f828179c` ("Ignore newtypes for now; remain wire compatible") already added
  `Node.type @36`, `Field.typeId @11`, `Type.typeId @28` to `capnp/schema.capnp` and
  regenerated `capnp/src/schema_capnp.rs`. `node::Which` has `Type(A5)` (schema_capnp.rs:905);
  `type_::Reader::get_type_id()` (schema_capnp.rs:7609) and `field::Reader::get_type_id()`
  (schema_capnp.rs:4715) exist as plain data fields. Back-reference ordinals are **identical**
  to the C++ `schema.capnp` (`@36/@11/@28`), so the two stay wire-compatible.
- **Wire-correct codegen already works.** Feeding a `type`/`@[...]` schema (compiled with the
  new C++ `capnp`) through `capnpc-rust` today produces correct output: each stamped group
  becomes a distinct per-use-site module with literal offsets baked in
  (`pub mod top_left { … get_data_field::<f32>(0/1/2) }`, `pub mod bottom_right { … (3/4/5) }`,
  no aliasing), and union stamps lay down the discriminant at its literal offset. The `type`
  nodes themselves are dropped via the no-op arm at `codegen.rs:2002`.
- **Conclusion:** every task below is *additive ergonomics* on top of already-correct wire
  code — nothing here changes the bytes. This mirrors how the C++ side was built (naive codegen
  first, wrappers layered on).
- **MSRV/idiom headroom.** Workspace pins Rust `1.81`; generated code *already* uses RPITIT
  (interface `Server` traits, `codegen.rs:2634`) and GATs (`Owned::Reader<'a>`,
  `codegen.rs:2202`). So traits, `impl Trait` returns, and associated types are all fair game.

## The load-bearing structural fact

There is **no offset applied at a group's use site** — the parent's group getter is offset-free
(`self.reader.into()`, `codegen.rs:703`). Each stamped group is its own schema node whose field
slots already carry absolute offsets into the shared parent, and those offsets are literal-
interpolated into *that node's own module* (`getter_text`/`generate_setter`/`zero_fields_of_group`
at `codegen.rs:712/729/1042/903/…`). So "the same newtype at N offsets" already exists as N
distinct concrete modules. We do **not** need const generics or a runtime offset carrier to get
the zero-cost form — the per-use-site module *is* the zero-cost concrete type. We only need to
give those modules a shared identity: a trait.

---

## Target generated shape (the sketch)

### 1. Scalar newtype — `type Uuid = Data`, `type Age = UInt16`

A transparent alias. Emit a module of type aliases (parallels the C++ `using`), and make type
positions carrying `Type.typeId` print the alias name.

```rust
pub mod uuid {
    pub type Reader<'a> = ::capnp::data::Reader<'a>;
    pub type Builder<'a> = ::capnp::data::Builder<'a>;
}
pub type age = u16;   // primitive: a bare alias

// use site — shapes.get_id() now returns uuid::Reader<'a> instead of capnp::data::Reader<'a>
```

### 2. Group newtype — `type Vec3 = group { x,y,z :Float32 }`

The trait is the semantic name (per your call: `vec3::Reader`, not `Vec3Reader`). Each use-site
module gains a trait impl. Getters are unchanged (still return the concrete per-site type, which
now *also* impls the trait).

```rust
pub mod vec3 {
    // &self (not self) so the trait is object-safe -> `&dyn vec3::Reader` works. Readers are Copy.
    pub trait Reader<'a>: Copy {
        fn get_x(&self) -> f32;
        fn get_y(&self) -> f32;
        fn get_z(&self) -> f32;
    }
    pub trait Builder<'a> {
        fn get_x(&self) -> f32;   fn set_x(&mut self, value: f32);
        fn get_y(&self) -> f32;   fn set_y(&mut self, value: f32);
        fn get_z(&self) -> f32;   fn set_z(&mut self, value: f32);
    }
}

// unchanged: the per-use-site concrete module (already generated today)
pub mod shapes {
    pub mod top_left {
        pub struct Reader<'a> { reader: ::capnp::private::layout::StructReader<'a> }
        impl<'a> Reader<'a> {
            pub fn get_x(self) -> f32 { self.reader.get_data_field::<f32>(0) }  // literal offset
            /* … */
        }
    }
    // NEW: the trait impl that gives top_left::Reader the Vec3 identity
    impl<'a> super::vec3::Reader<'a> for top_left::Reader<'a> {
        fn get_x(&self) -> f32 { self.reader.get_data_field::<f32>(0) }
        fn get_y(&self) -> f32 { self.reader.get_data_field::<f32>(1) }
        fn get_z(&self) -> f32 { self.reader.get_data_field::<f32>(2) }
    }
    // getter unchanged: returns concrete top_left::Reader<'a>, which impls vec3::Reader
}
```

Usage — indistinguishable from a normal group in the common case, trait names it at boundaries:

```rust
let tl = shapes.get_top_left();           // concrete; inherent get_x() works directly
tl.get_x();
fn fill(v: &mut impl vec3::Builder) { v.set_x(1.0); }   // boundary: name the trait
fill(&mut shapes.reborrow().get_top_left());
let anys: Vec<&dyn vec3::Reader> = vec![&tl, &br];       // erased, uniform
```

Nested newtypes (`type OrderPrices = group { limit,stop :Price }`) fall out for free: the
per-site `prices::limit` module is itself a stamped group that impls `price::Reader`. No RPITIT
needed — we return concrete types and attach traits, so nesting composes structurally.

### 3. Union newtype — `type OrderType = union { limit @[0-1] :Price; market :Void; cancel :Int32 }`

Rust's sum-type `Which` makes this *nicer* than C++. The one wrinkle: group arms carry per-use-site
concrete payload types, so the trait uses an **associated type** per group arm to stay zero-cost.

```rust
pub mod order_type {
    pub enum Which<Limit> {      // generic over the group-arm reader type
        Limit(Limit),
        Market(()),
        Cancel(i32),
    }
    pub trait Reader<'a>: Copy {
        type Limit: super::price::Reader<'a>;          // the group arm's concrete reader
        fn which(&self) -> ::core::result::Result<Which<Self::Limit>, ::capnp::NotInSchema>;
    }
    pub trait Builder<'a> {
        fn set_cancel(&mut self, value: i32);          // each setter stamps the discriminant first
        fn set_market(&mut self);
        fn init_limit(&mut self) -> /* concrete */;    // selects arm + returns Price builder
    }
}
// use site: `match order.which()? { Which::Limit(p) => p.get_value(), Which::Cancel(n) => …, … }`
```

Note the single generic `Which<L>` — the trait's `which()` returns `Which<Self::Limit>`, and the
erased carrier below returns `Which<price::AnyReader>`. One enum, used both ways.

`&dyn order_type::Reader` does **not** unify across use sites (the `Limit` associated type must be
pinned to a per-site concrete type, so each site is a different `dyn` type). The same is true of
any newtype that returns *another stamped newtype* — nested groups (`OrderPrices`) as well as
unions. This is the one place Rust's free `&dyn` erasure doesn't reach; the erased carrier in §4
closes it with the same mechanism C++ uses.

### 4. Erased carrier — `AnyReader` / `AnyBuilder` (the runtime offset table)

The faithful mirror of C++'s `AnyReader`: a concrete struct holding the `StructReader` plus a
**runtime** offset slice. Because it is a single concrete type (offsets are data, not type
parameters), it unifies across all use sites, and — critically — its nested-arm accessors return
*other concrete carriers*, so nesting and unions compose with no associated types and no
allocation. This is Kenton's original runtime-offset-table proposal.

```rust
pub mod vec3 {
    #[derive(Clone, Copy)]
    pub struct AnyReader<'a> {
        reader: ::capnp::private::layout::StructReader<'a>,
        offsets: &'a [u32],                       // one per leaf, tree order; 0xffff_ffff = unmapped
    }
    impl<'a> AnyReader<'a> {
        pub fn get_x(&self) -> f32 { self.reader.get_data_field::<f32>(self.offsets[0] as usize) }
        // unmapped leaf (incomplete `@[...]`): read the field default instead of the sentinel slot
        // pub fn get_z(&self) -> f32 { if self.offsets[2]==u32::MAX { DEFAULT } else { … } }
    }
    impl<'a> Reader<'a> for AnyReader<'a> { /* same bodies -> one type usable behind `impl Reader` */ }
}

// each per-use-site concrete module gains the offset table + an `as_any()` erasure:
pub mod shapes { pub mod top_left {
    static OFFSETS: [u32; 3] = [0, 1, 2];         // the baked leaf offsets, materialized as data
    impl<'a> Reader<'a> {
        pub fn as_any(self) -> super::super::vec3::AnyReader<'a> {
            super::super::vec3::AnyReader { reader: self.reader, offsets: &OFFSETS }
        }
    }
}}
```

**Nesting composes by slicing** — exactly C++'s `_offsets + leafIdx`:

```rust
impl<'a> order_prices::AnyReader<'a> {
    pub fn get_limit(&self) -> super::price::AnyReader<'a> {
        super::price::AnyReader { reader: self.reader, offsets: &self.offsets[0..2] }
    }
    pub fn get_stop(&self) -> super::price::AnyReader<'a> {
        super::price::AnyReader { reader: self.reader, offsets: &self.offsets[2..4] }
    }
}
```

**Unions erase cleanly** — the arm payload is now the concrete `price::AnyReader`, so `Which<'a>`
is non-generic and unifies:

```rust
impl<'a> order_type::AnyReader<'a> {
    pub fn which(&self) -> Result<Which<super::price::AnyReader<'a>>, ::capnp::NotInSchema> {
        match self.reader.get_data_field::<u16>(self.disc_offset as usize) {
            0 => Ok(Which::Limit(super::price::AnyReader { reader: self.reader, offsets: &self.offsets[0..2] })),
            1 => Ok(Which::Market(())),
            2 => Ok(Which::Cancel(self.reader.get_data_field::<i32>(self.offsets[3] as usize))),
            x => Err(::capnp::NotInSchema(x)),
        }
    }
}
```

Result: `Vec<vec3::AnyReader>`, `Vec<order_type::AnyReader>`, returning any newtype from a
function, all work. Since `AnyReader: Reader`, code written against `impl vec3::Reader` accepts
both the concrete and erased forms — matching C++, where `AnyReader` and `Reader<off…>` share the
accessor interface. `AnyBuilder` mirrors this for mutation (`StructBuilder` + offset slice;
setters write at the runtime offset; not `Copy`).

**Two hygiene requirements** the codegen must honor (both shared with the C++ side):
- *Single leaf-order traversal.* The baked `OFFSETS` array, the `AnyReader` accessors' `offsets[i]`
  indices, and the nested `[start..end]` slices must all agree on leaf order. Drive them from one
  `for_each_leaf(template)` walk so they can't diverge.
- *Incomplete mappings.* A leaf absent from an instance's `@[...]` gets the sentinel `0xffff_ffff`
  in `OFFSETS`; its `AnyReader` getter returns the field default, and its `AnyBuilder` setter
  panics (`unwrap`-style) — the runtime analog of C++'s `unmask` / `static_assert`.

### Cross-language consistency check

| | C++ | Rust |
|---|---|---|
| semantic name | `Vec3::Reader<off…>` (class template) | `vec3::Reader` (trait) |
| a use site | template instantiation | per-site module + trait impl |
| zero-cost path | the templated form | the concrete per-site type |
| erased/boundary | `Vec3::AnyReader` (runtime offset table) | `vec3::AnyReader` (runtime offset table); plus `&dyn vec3::Reader` as a free bonus for *flat* newtypes |
| union | `which()` + `isX()`/`getX()` | `Which` sum type + `match` (nicer) |
| method surface | `getX()` | `get_x()` (snake_case) — identical modulo casing |

Net: Rust reaches full C++ equivalence via the same runtime-offset carrier, and gets `&dyn`
erasure as an extra for flat newtypes on top.

---

## Exact hook sites in `capnpc/src/codegen.rs` (+ `codegen_types.rs`)

1. **Scalar alias name at every type position** — `RustTypeInfo::type_string` for `type_::Reader`,
   `codegen_types.rs:163` (match at :177). Add at the *top*: `if self.get_type_id() != 0 { return
   Ok(ctx.get_qualified_module(self.get_type_id())) }` (mirrors the `Enum` arm at :256). This one
   change routes const types, field types, and `List(Uuid)` through the alias name.
2. **Emit the newtype node** — the no-op arm `node::Type(Ok(_t)) => {}` at `codegen.rs:2002`.
   Replace with: scalar RHS → emit the `uuid` alias module; struct/union RHS (detected via
   `_t.which()? == type_::Struct`, whose `.get_type_id()` is the template node) → emit the
   `vec3::Reader`/`Builder` traits (and `order_type::Which` + traits for unions).
3. **Attach the trait impl at each use site** — group getter branch of `getter_text`,
   `codegen.rs:681` (`field::Group(group)`), gated on the field-level `field.get_type_id() != 0`
   (the alias id; distinct from `group.get_type_id()`, the per-instance node). Emit an
   `impl <alias>::Reader for <group_module>::Reader` block into the parent module's
   `nested_output`. The struct getter itself (`codegen.rs:2097`) stays unchanged.
4. **Union arm trait/Which** — `generate_union` (`codegen.rs:1363`) already builds a `Which` enum
   and `which()` with the literal discriminant offset (`codegen.rs:1393/1506`). For a union
   newtype, additionally emit the `order_type` trait + associated types and impl it for the
   per-site union module.
5. **Erased carrier** — emitted alongside the traits in hook (2): the `AnyReader`/`AnyBuilder`
   structs, their trait impls, and (in hook (3)/(4)) the per-use-site `static OFFSETS` table +
   `as_any()`/`as_any_builder()`. A single `for_each_leaf(template)` helper (new) drives the
   `OFFSETS` order, the carrier accessors' indices, and the nested slice bounds so they can't
   diverge. Reuse the C++ side's `collectLeafOffsets` leaf-matching logic (match instance leaves
   to template by name; absent → `0xffff_ffff`).
6. **Naming** — `camel_to_snake_case` (`codegen.rs:403`) for trait module + method names;
   `capitalize_first_letter` (`codegen.rs:416`) for `Which` variants. Trait module name derives
   from the alias node name via `get_last_name`/`get_qualified_module` (`codegen.rs:378`).

No exhaustive match needs a *new* arm to compile — the "ignore" commit already added `_ =>` /
no-op arms everywhere (`zero_fields_of_group` :898, `used_params_of_group` :1260,
`generate_get_field_types` :1632, etc.). We are replacing stubs and adding emission, not
unbreaking the build.

### Representation facts (reverse-engineered from a decoded `CodeGeneratorRequest`)

- **Group/union newtype** = a `type` node whose `type.which() == Struct`, pointing at a template
  node named `X.(template)`. Detection predicate: `node.which()==Type && type.which()==Struct &&
  node_map[type.struct.type_id].get_scope_id() == node_id` (the template is *owned* by the type
  node — distinguishes it from `type Foo = Bar` aliasing a real struct, whose target is scoped to
  the file/parent). The template is a normal `Struct` node (`isGroup=false`) holding the fields.
- **Scalar newtype** = `type` node whose `type.which()` is a primitive / `Data` / `Text` / `List`.
- **Templates are not in any `nestedNodes` list.** The recursion at `codegen.rs:1996` never walks
  them (verified: the smoke test emits no `*_template` module). So R2 reads the template directly
  from `ctx.node_map[&template_id]` and emits only the trait — no separate template module.
- **Trait method signatures are offset-independent.** A field's *return type* (`f32`,
  `capnp::text::Reader<'a>`, `coord::Reader<'a>`) doesn't depend on the use-site offset — only the
  getter *body* does. So R2 can reuse `getter_text`/`generate_setter` two ways: call them on the
  **template** field to get the signature for the trait declaration, and on each **per-instance**
  field (inside the stamped group node, offsets baked) for the trait-impl body.
- **R2 flat-first guard.** A template field that is itself a `Group` (nested newtype, e.g.
  `OrderPrices.limit`) needs an associated type — that's R2-nested/R3. Guard R2 to emit a trait
  only when every template field is a `Slot`; skip (no-op) otherwise, so we never emit an
  incomplete trait. Covers `Vec3`/`Named`/`Boxed` in the first pass.

---

## Work plan (staged, each independently landable + testable)

- **R1 — Scalar newtype aliases. DONE.** Hooks (1) + (2, scalar path). Each scalar newtype emits a
  uniform alias module `pub mod uuid { pub type Reader<'a> = ...; Builder; Owned }` (a value
  newtype like `Age = u16` drops the lifetime); `type_string` short-circuits on `Type.typeId` to
  return `<module>::<leaf>`, so every position (field get/set, `List(Uuid)`, etc.) names the alias.
  Aliases of a pre-existing struct/interface are skipped (deferred). Tested in `capnpc/test`
  (`newtype_scalar_aliases`: `Age`/`Uuid` round-trip) + standalone `List(UInt64)` newtype; full
  suite 81 passing, no regression on the universal `type_string` path.
- **R2 — Group newtype trait + per-site impls. DONE.** Hooks (2, struct path) + (3). Emits
  `vec3::Reader`/`Builder` traits and per-use-site impls that delegate to the inherent accessors
  (`(*self).get_x()` / `self.set_x(v)` — inherent wins over trait in method resolution, so no
  recursion and offsets/defaults live in one place). `generate_setter` was refactored into a
  `setter_methods` producer + a thin assembler (Quirk 1a) so the Builder trait's setter/init
  signatures come from the same source as the inherent methods. Covers flat newtypes over
  primitives, Text/Data, List, and Struct fields; nested-newtype members are guarded out (skip)
  until R3/R4. Tested in `capnpc/test` (4 tests: generic `impl vec3::Builder`/`Reader` fill+read,
  `&dyn vec3::Reader` erasure across two use sites, pointer fields, struct/list fields); full
  suite 79 passing, lint-clean.
  - *Wrinkle A (minor):* the trait carries a uniform `<'a>`, so using it as a bound in a generic
    fn needs a **named** lifetime (`fn f<'a>(v: &impl vec3::Reader<'a>)`) — anonymous `<'_>` is
    unstable in impl-Trait-arg position. Could be avoided by omitting `<'a>` for lifetime-free
    newtypes, at the cost of two trait shapes; left uniform for now.
  - *Consistency (resolved):* the Builder trait now mirrors a normal builder — `get_`/`set_`/
    `init_`/`initn_`/`has_` — and the Reader trait carries `get_`/`has_`. Shared via a
    `newtype_trait_methods` helper so the trait decl and the delegating impl stay in lockstep.
  - *Per-leaf setters take the leaf's own type* (`set_x(f32)`, `set_label(impl SetterInput<Text>)`)
    — consistent with group/struct field setters. There is no whole-`Vec3` setter, same as groups
    (an inline newtype shares the parent's space; you mutate in place via the getter).
  - *Parent-side accessors already match a struct field's, and already match C++.* An inline
    newtype field on the parent gets `get_<field>` (reader + builder) and `init_<field>` (zeroes
    the inline space, returns the newtype builder) on **both** Rust (via the existing group-init
    path) and C++ (`getX`/`initX`), with the same semantics; the returned builder impls the
    newtype's Builder trait. No work needed — this is the struct-like `init` ergonomics we wanted.
    (No `copy_from` — that idea is dropped.)
  - *The one missing struct accessor, symmetric on both sides:* neither C++ nor Rust emits a
    whole-value `set_<field>(value: impl newtype::Reader<'_>)` on the parent (a field-by-field
    copy, since an inline newtype has no pointer to `set_pointer_builder`). If we want the full
    struct-like `init`/`set` pair, add it **symmetrically to both repos** — parent-side
    `set_<field>` taking the newtype Reader as an "any source" bound (a concrete per-site reader
    **or** `AnyReader`). Optional (the two are already consistent without it); would land with R4
    on the Rust side and a matching small addition on the C++ side.
- **R3 — Union newtype trait + `Which`.** Hook (4). Two increments by arm kind:
  - *R3a — all-slot arms* (void/primitive/pointer). **DONE.** Emits `pub mod status { pub enum
    Which<'a> {...} pub trait Reader<'a> { fn which() ...; has_ } pub trait Builder<'a> { set_/
    init_/has_ } }`; the impl's `which()` maps the per-instance `Which` variants to the shared enum
    and the mutators delegate to the inherent setters (which carry the discriminant). Key gotcha:
    union arms have **no inherent getter** (read via `which()`), so arm getters are omitted (an
    `include_getter=false` flag on `newtype_trait_methods`) — emitting them recursed. Detection
    split: `newtype_template_id` (resolve) → `flat_newtype_template_id` (non-union, all-slot) /
    `slot_union_newtype_template_id` (union, all-slot arms). Tested in `capnpc/test`
    (`newtype_union_which`: data/pointer/void arms via `match status::Reader::which()?`); suite 80
    passing, warning-clean. *Deferred within R3a:* builder-side `which()` (mutation is the common
    builder need; add if wanted).
  - *R3b — group arms* (a nested newtype arm, e.g. `limit :Price`). **DONE.** The `Which` enum
    became generic over the group arms (`pub enum Which<Limit> { Limit(Limit), Market(()),
    Cancel(i32) }`), the trait carries `type Limit: price::Reader<'a>` + `which() -> Which<Self::
    Limit>`, and the builder gets `init_limit(self) -> Self::Limit`; the impl sets the associated
    types to the per-instance group and maps the per-instance `Which` variant-for-variant. The
    R3a all-slot path is subsumed (empty `group_params`). `slot_union_newtype_template_id`
    generalized to `union_newtype_template_id`. Tested (`newtype_union_with_group_arm`:
    `match ot.which()? { Which::Limit(p) => p.get_value(), … }` over group/data/void arms); suite
    83 passing, clean.
- **Nested newtype members (group case). DONE.** A group newtype whose members are themselves
  newtypes (`type OrderPrices = group { limit :Price; stop :Price }`) now generates associated
  types bounded by the member's trait (`type Limit: price::Reader<'a>`) plus `get_/init_` returning
  `Self::Limit`; the impl sets `type Limit = <per-instance group>::Reader<'a>` (which impls
  `price::Reader` via its own stamp) and delegates. `flat_newtype_template_id` generalized to
  `group_newtype_template_id` (accepts group fields that carry a `Field.typeId`; a plain anonymous
  group is still deferred). Composes generically: `fn f(op: &impl order_prices::Reader) {
  op.get_limit().get_value() }`. Tested (`newtype_nested_members`); suite 82 passing, clean. The
  union analog (a group *arm*) is R3b, still pending.
- **R6 — Whole-value parent `set_<field>` (C++ + Rust, done together).** The one struct accessor
  neither side emits (see R2 notes): parent-side `set_<field>(value: impl newtype::Reader<'_>)`, a
  field-by-field copy. Do **both repos in one pass** so the shape lines up (Rust: needs `AnyReader`
  from R4 for the "any source" bound; C++: a `setX(Vec3::AnyReader)` added to `makeFieldText`).
  Depends on R4.
- **R4 — Erased carrier (`AnyReader`/`AnyBuilder`).** The runtime offset table.
  - *Flat `AnyReader`. DONE.* `getter_text` was generalized to `getter_text_at(..., offset_override)`
    so accessor bodies can index `self.offsets[i] as usize` instead of a baked literal (the one
    change; `getter_text` is now a `None` wrapper — no caller churn, existing 83 tests unchanged).
    Each flat newtype module gains `pub struct AnyReader<'a> { reader: StructReader<'a>, offsets:
    &'a [u32] }` with `new()`, runtime-offset getters + `has_`, and an `impl Reader for AnyReader`.
    Each use site emits `static <FIELD>_OFFSETS: [u32; N]` (sentinel `0xffff_ffff` for an unmapped
    leaf) + `as_any()` on the concrete reader (via public `IntoInternalStructReader`, since the
    inner field is private at the impl site). Tested (`newtype_any_reader_erasure`:
    `Vec<vec3::AnyReader>` over two distinct-offset sites, pointer fields through the carrier, and
    `impl Reader` acceptance); suite 84 passing, clean.
  - *Nested + union `AnyReader`. DONE.* `count_newtype_leaves` + `collect_leaf_offsets` build the
    recursive offset table; a nested member's accessor returns the member's `AnyReader` over
    `&self.offsets[start..end]` (`PRICES_OFFSETS = [0,4,2,5]`, `get_limit()` slices `[0..2]`). The
    union carrier adds a runtime `disc_offset`; `which()` reads it and builds `Which<price::
    AnyReader>` (concrete arms, so it unifies where `&dyn` cannot). Tested (nested + union
    round-trips).
  - *`AnyBuilder` (flat + nested). DONE.* `setter_methods` generalized to `setter_methods_at(...,
    offset_override)` (mirrors `getter_text_at`); `AnyBuilder<'a> { builder, offsets }` with
    runtime-offset `set_/init_/get_/has_` + `impl Builder`, and per-site `as_any()`. Reaching the
    private inner builder needed a new `IntoInternalStructBuilder` trait (added to `capnp::traits`,
    impl generated for every struct builder — mirrors `IntoInternalStructReader`). Nested members
    yield a nested `AnyBuilder` over a sub-slice. Tested (`newtype_any_builder`: erased mutation
    over two use sites + nested `init_limit()`). Suite **85 passing, clean**; `capnp` crate builds.
  - *Union `AnyBuilder`. DONE.* Mirrors the union `AnyReader` on the write side (a runtime
    `disc_offset`; each arm setter writes the discriminant via `setter_methods_at(ctx,
    "self.disc_offset as usize", …)`; a group arm's `init_` writes it then hands out a nested
    `AnyBuilder`). `IntoInternalStructBuilder` (new trait in `capnp::traits`, impl generated for
    every struct builder) reaches the private inner builder from the impl site.
  - *Explicit-default fields. DONE (verified).* A defaulted primitive reads via `get_data_field_
    mask`; a defaulted pointer references `_private::DEFAULT_x` (emitted as `mod _private {…}` in
    the newtype module, shared by `AnyReader`/`AnyBuilder`). Confirmed a `Text = "hi"` / `UInt16 =
    100` newtype reads the defaults when unset through the concrete reader, the trait, and the
    carriers.
  - *Incomplete `@[...]` mappings. DONE (matches C++).* An unmapped leaf reads its **default**
    (primitive: sentinel-guarded `prim_default_value`, matching C++'s `unmask<T>(0, mask)`;
    pointer: a null-pointer read via the reader's bounds-checked `get_pointer_field`), and setting
    it **errors** (a compile error on the concrete type — the inherent method is absent, matching
    C++'s `static_assert`; a `panic!("… not mapped …")` through the trait/erased builder, matching
    `KJ_REQUIRE`). Crucial detail: the *builder's* raw reads/writes are **not** bounds-checked, so
    every erased `AnyBuilder` mutator carries a sentinel guard (`unmapped_panic`) — without it an
    unmapped set was UB (SIGBUS) rather than a clean panic. Reads on the concrete trait return the
    default directly (no inherent to delegate to); the erased carrier is always emitted (only the
    concrete trait is gated on a fully-mappable use site). Tested: `partial @[…] :Vec3` /
    `pricedPartial @[…] :Priced` read defaults via trait + `AnyReader`; set panics.
  - *Bootstrap regen: skipped.* Running `regenerate-capnp-schema-code.sh` produces a large, noisy
    diff (the checked-in `schema_capnp.rs` is stale vs `schema.capnp`'s newtype additions + a capnp
    version bump), well beyond the additive `IntoInternalStructBuilder` impls. Left as a separate,
    deliberate maintenance action; the impls' absence from `schema_capnp.rs` is harmless (nothing
    in the capnp crate uses them).

  **Suite: 89 passing, warning-clean. The erased carrier is complete and matches the C++ codegen's
  unmapped-offset behavior.**
- **R5 — Tests + parity.** Port the C++ `newtype-test.c++` cases to a Rust integration test under
  `capnpc/test`; wire a `test-newtype.capnp` into the build; confirm `capnp compile -orust`
  round-trips against the C++-produced messages (cross-impl wire check).

## Design decisions (settled)

1. **`&self` in traits** (readers are `Copy`, keeps traits object-safe for `&dyn`). Builders
   `&mut self` for setters.
2. **Getter returns the concrete per-site type** (consistent with existing group getters;
   nameable; composes for nesting). The trait is the boundary name. (RPIT rejected — hiding the
   module name isn't worth making the return un-nameable.)
3. **Keep inherent accessors alongside the trait impl.** Inherent `get_x(self)` for direct use;
   trait `get_x(&self)` for generic/boundary use. Rationale: having to pull a trait into scope to
   call a method is an annoying Rust quirk that's hard to teach, and duplicating the one-line
   bodies at codegen time is essentially free. So the common path never requires a `use`.
4. **Trait vs struct name collision (doc note, not a blocker).** For a *struct*, `foo::Reader` is
   a struct; for a *newtype*, `vec3::Reader` is a trait. Different node kinds; call it out in the
   generated-code docs so `x::Reader` sometimes being a trait isn't a surprise.
