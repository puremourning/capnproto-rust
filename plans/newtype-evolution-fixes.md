# Newtype schema-evolution fixes (prerequisite to runtime schema loading)

**Priority: do this before anything in `runtime-schema-loading.md`.** Inline group/union
newtypes are meant to support schema evolution: explicit use-site ordinals make the layout
stable, and an `@[...]` mapping may leave trailing template ordinals unmapped but must map the
first N (`../capnproto/capnp-type-alias-design.md`). Three bugs break this today. One is in the
compiler and two are in the code generators. All were found on 2026-10-14 with the fork's
`capnp` (2.0-dev, `../capnproto/root/bin/capnp`).

## Reproducer

```capnp
# v1
type Vec3 = group { x @0 :Float32; y @1 :Float32; z @2 :Float32; }
type Rect = group { tl @[0-2] :Vec3; br @[3-5] :Vec3; }
struct S { r @[0-5] :Rect; tag @6 :UInt32; }

# v2: a legal evolution. Vec3 gains w; Rect maps it at new ordinals 6 and 7; S maps those at 7 and 8.
type Vec3 = group { x @0 :Float32; y @1 :Float32; z @2 :Float32; w @3 :Float32; }
type Rect = group { tl @[0-2,6] :Vec3; br @[3-5,7] :Vec3; }
struct S { r @[0-5,7,8] :Rect; tag @6 :UInt32; }
```
Per the design, Rect ordinal *k* maps to the *k*-th listed S ordinal:

| | tl (x,y,z,w) | br (x,y,z,w) |
|---|---|---|
| Rect template, both versions (correct today) | 0,1,2,(6) | 3,4,5,(7) |
| S use site v1, expected | S 0,1,2 | S 3,4,5 |
| S use site v2, expected | S 0,1,2,**7** | S **3,4,5**,8 |
| S use site v2, **actual** | S 0,1,2,3 | S 4,5,7,8 |

## Bug C (compiler, the most severe): `@[...]` is expanded in tree order, not by ordinal

- **Where:** `stampTemplateFields`, `c++/src/capnp/compiler/node-translator.c++:1405`. It walks
  the template's fields recursively (`:1444`) and takes `ordinals[ordinalIndex++]` for each leaf
  (`:1449`). It ignores the leaf's own template ordinal (6 for `tl.w`).
- **Effect:** wire incompatibility between v1 and v2, confirmed with `capnp encode`/`decode`. A
  v1 message decoded with v2 gives `tl.w = <v1 br.x>`, `br = (v1 br.y, v1 br.z, 0, 0)`. This hits
  every backend and the zero-cost forms too, not just the erased ones.
- **The same assumption appears elsewhere:**
  - the incomplete-mapping cutoff (`:1447`, and `:1463` for nested groups), which treats
    "the first N" as the first N leaves *in tree order*;
  - the union "reached" check (`:1428-1436`);
  - the setup in `stampInlineNewtype` (around `:1500-1515`).

  With `S.r @[0-5]` against v2, tl.w lands at S@3, and br.z and br.w are dropped.
- **Single-level templates are fine.** Rect's own `tl @[0-2,6] :Vec3` stamps correctly, because
  Vec3's tree order equals its ordinal order. The bug only shows up when a nested newtype's
  leaves are mapped at non-contiguous outer ordinals.
- **Fix:**
  - a leaf with template ordinal `o` maps to `ordinals[o]`;
  - a leaf is unmapped iff `o >= ordinals.size()`;
  - a nested group is dropped only if *all* of its leaves are unmapped;
  - the union "reached" check uses the arm's ordinals;
  - the incomplete-mapping warning counts unmapped *ordinals*.
- **Compatibility:** any schema where a nested template's tree order differs from its ordinal
  order changes layout under the fix. For example, an interleaved `tl @[0,2,4]; br @[1,3,5]`, or
  any already-evolved nested newtype. The fork is unreleased, so take the fix. Note it in the
  changelog anyway, and consider a one-off compiler warning when the old and new interpretations
  would differ. Today's workaround mapping `r @[0-2,7,3-5,8]` produces the right v2 layout by
  accident. Under the fix it means something else, so it must not be documented as the way to
  evolve.
- **Naming bug (same code path):** nested stamped group nodes are created with
  `newGroupNode(groupMember.node, member)` (`:1468`). `member` is the *use-site* declaration
  (`r`), so both nested nodes are named `S.r.r`. Pass the template member's name, which gives
  `S.r.tl` / `S.r.br`. Display only, but it shows up in generated module names and error messages.
- **Tests (C++ compiler test suite):**
  - v1 and v2 above round-trip both ways through encode/decode;
  - the expected use-site ordinals from the table above;
  - incomplete mappings (`S.r @[0-5]` against v2 leaves only tl.w and br.w unmapped);
  - union newtypes containing nested newtypes;
  - a three-level nest.

## Bug A (both code generators): one flat, tree-ordered offset table per use site

- **Rust** (`capnpc/src/codegen.rs`): `collect_leaf_offsets` (`:2400`) and
  `count_newtype_leaves` (`:2381`) flatten nested newtypes inline, in field-list order. The outer
  `AnyReader` hands a nested one `&self.offsets[start..end]`, with bounds baked from the nested
  leaf count (`:2476-2479`). v1 generates `get_br → offsets[3..6]`; v2 generates `offsets[4..8]`.
- **C++** (`c++/src/capnp/compiler/capnpc-c++.c++`): the same design. `collectLeafOffsets`
  (`:2929-2948`) builds the table, `sliceArgs` (`:2995-2999`) bakes the templated slices, and the
  erased form gets a baked pointer offset (`_offsets + 3`, `:3047-3050`, `:3130-3133`).
- **Effect:** a carrier from one schema version given a table from another reads the wrong
  leaves, silently. Even with Bug C fixed, a correct v2 table in tree order is `[0,1,2,7,3,4,5,8]`,
  and a v1 `br` slice `[3..6]` reads `[7,3,4]`.
  - *C++:* only hurts when code generated from two versions is linked into one binary. That's
    already an ODR violation there.
  - *Rust:* it's reachable today via `crate_provides` (newtype in crate A, use site in crate B,
    generated from different versions). It will also be reachable routinely through runtime
    schema loading, where use-site tables come from a CGR and carriers from compiled code.
- **Fix: implement what the design doc already specifies.** The design (§"Code generation") says
  the table is indexed by *"the indexes in the group's `fields` list"*. That index is stable under
  evolution (schema.capnp:129: a field's position never changes and new fields are appended).
  So make the tables **per level**:
  - A use site's table for template `T` has one entry per field of `T`'s template, indexed by that
    field's position in `T.(template).fields`.
  - A slot entry holds its offset, or the `0xffff_ffff` sentinel when unmapped.
  - A nested-newtype member's entry refers to **its own table**, built with the same rule for the
    nested template.
  - Each nested carrier gets its own child table instead of a slice of the parent's. No baked
    `start..end` or `+ n`, and no recursive leaf counts.
- **Rust representation** (suggested):
  ```rust
  pub struct UseSiteTable {
      pub entries: &'static [Entry],      // indexed by template field-list position
      pub discriminant_offset: u32,       // union newtypes
  }
  pub enum Entry { Offset(u32), Nested(&'static UseSiteTable) }
  ```
  `AnyReader::from_use_site(reader, &'a UseSiteTable)` replaces `(offsets: &[u32],
  discriminant_offset)`. Generated statics become nested `static`s per use site. `RawStructSchema`'s
  `any_offsets`/`any_discriminant_offset` become `any_table: Option<&'static UseSiteTable>`.
  `LEAF_COUNT` is replaced by `FIELD_COUNT` (this template level only) or removed (see Bug B).
- **C++ representation:** the same shape, a `struct NewtypeTable { const uint32_t* offsets;
  const NewtypeTable* const* nested; uint32_t size; }`, or one table type per level. The
  templated (zero-cost) form keeps baking offsets as template arguments, selecting each nested
  member's offsets by field index rather than by slice. It stays zero-cost.

## Bug B (Rust; C++ differs): table length must be allowed to differ from what the carrier expects

- **Rust:** `check_any_layout` (`capnp/src/dynamic_value.rs:272`) asserts
  `any_offsets.len() == T::LEAF_COUNT`. Generated getters index `self.offsets[i]` directly, so
  `as_any()` over a shorter table panics on out-of-bounds. Under the evolution rules the lengths
  differ legitimately: a newer schema has more template fields, an older one fewer.
- **C++:** `AnyReader` stores a bare `const uint32_t*` with no length (`capnpc-c++.c++:3171-3192`).
  A shorter table from another version is read past its end, which is UB. A longer one is
  silently fine.
- **Fix, per level:** an index past the end of the table is *unmapped*, which the carriers already
  handle correctly within a version:
  - reading gives the template default (Rust `codegen.rs:2499`, C++ `:3018-3021`);
  - setting is a runtime error (Rust panic `codegen.rs:706`, C++ `KJ_REQUIRE` `:3067-3072`);
  - extra entries beyond what the carrier knows are ignored.

  Rust: `self.table.entries.get(i)`, and treat `None` as the sentinel. Drop the `LEAF_COUNT`
  equality assert; keep the `newtype_ids.contains(TYPE_ID)` check. C++: carry the length in the
  table (the `size` field above).

## Other observations (not evolution bugs; recorded for completeness)

- C++ union newtypes have no `AnyReader`/`AnyBuilder` (`capnpc-c++.c++:3157-3158`). Rust has
  them. That's a parity gap, not a bug.
- C++ interface/AnyPointer members and pointer members with explicit defaults get no wrapper and
  fall back to a plain group (`hasInlineNewtypeWrapper`, `:2850-2883`).

## Work split (separate worktrees)

These can run in parallel, with one ordering constraint: the cross-version *tests* for A and B
need a compiler with C fixed, to produce a correct v2 layout.

1. **C++ compiler, Bug C + naming** (`../capnproto` worktree):
   `node-translator.c++` and compiler tests. Small and self-contained. **Land first.**
2. **C++ codegen, Bugs A + B** (`../capnproto` worktree):
   `capnpc-c++.c++`, the per-level tables, the length in the table, and the C++ runtime
   header if the table type lives there. Test by generating v1 and v2 into separate namespaces in
   one test binary, then reading a v2 use site through a v1 erased carrier and vice versa.
3. **Rust codegen + runtime, Bugs A + B** (this repo's worktree):
   `capnpc/src/codegen.rs`, `capnp/src/traits.rs` (`AnyReader`/`AnyBuilder`),
   `capnp/src/introspect.rs` (`RawStructSchema`), and `capnp/src/dynamic_value.rs`
   (`check_any_layout`, downcasts). Tests in `capnpc/test`: generate v1 and v2 as two modules,
   then cross-read through `as_any()` and through the dynamic `downcast::<…::AnyReader>()`, with
   unmapped reads giving defaults and unmapped sets panicking. Regenerate checked-in code if any
   of it contains newtypes.

Once 3 lands, `runtime-schema-loading.md` §3a builds the loader's use-site tables with the
same per-level rule, through the shared `newtype_layout` module.
