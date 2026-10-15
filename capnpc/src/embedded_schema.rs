// Copyright (c) 2026 the Cap'n Proto authors and contributors
// Licensed under the MIT License:
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

//! `SCHEMA_BYTES` (`CompilerCommand::embed_schema`): a generated file's schema as a closed,
//! serialized `CodeGeneratorRequest`, for a `SchemaLoader` in a process without the code.

use std::collections::{HashMap, HashSet};

use capnp::dynamic_value;
use capnp::schema_capnp::{code_generator_request, node};

use crate::codegen::FormattedText::{Branch, Line};
use crate::codegen::{fmt, line, FormattedText, GeneratorContext};

/// The schema `requested_file`'s `SCHEMA_BYTES` hold: the nodes of every file it reaches, which
/// is what a `SchemaLoader` needs for the batch to be closed, as a serialized
/// `CodeGeneratorRequest`.
pub(crate) fn embedded_schema_bytes(
    ctx: &GeneratorContext,
    requested_file: code_generator_request::requested_file::Reader<'_>,
) -> capnp::Result<Vec<u8>> {
    let files = reached_files(ctx, requested_file.get_id())?;

    let mut message = capnp::message::Builder::new_default();
    let mut request = message.init_root::<code_generator_request::Builder<'_>>();
    if ctx.request.has_capnp_version() {
        request.set_capnp_version(ctx.request.get_capnp_version()?)?;
    }
    let included: Vec<node::Reader<'_>> = ctx
        .request
        .get_nodes()?
        .iter()
        .filter(|n| files.contains(&file_of(ctx, n.get_id())))
        .collect();
    let mut nodes = request.reborrow().init_nodes(included.len() as u32);
    for (i, n) in included.into_iter().enumerate() {
        nodes.set_with_caveats(i as u32, n)?;
    }
    request
        .init_requested_files(1)
        .set_with_caveats(0, requested_file)?;
    Ok(capnp::serialize::write_message_to_words(&message))
}

/// Declares `SCHEMA_BYTES`, reading the bytes from `file_name` next to the generated file. They
/// are word-aligned, so they can be read in place.
pub(crate) fn embedded_schema_declaration(
    ctx: &GeneratorContext,
    file_name: &str,
) -> FormattedText {
    Branch(vec![
        line("/// This file's schema and that of every file it refers to, as a serialized"),
        line("/// `CodeGeneratorRequest` (standard framing, 8-byte aligned): load it with"),
        line("/// `SchemaLoader::load_code_generator_request` to read these types dynamically."),
        Line(fmt!(
            ctx,
            "pub static SCHEMA_BYTES: &[u8] = &{capnp}::private::AlignedBytes(*include_bytes!(\"{file_name}\")).0;"
        )),
    ])
}

/// The file `id` is declared in. Ids not in the request (see `referenced_ids`) map to themselves.
fn file_of(ctx: &GeneratorContext, mut id: u64) -> u64 {
    while let Some(&parent) = ctx.node_parents.get(&id) {
        if parent == 0 {
            break;
        }
        id = parent;
    }
    id
}

/// `file` and every file a node in the result refers to, transitively. Whole files are taken,
/// so a node's parent scopes come with it.
fn reached_files(ctx: &GeneratorContext, file: u64) -> capnp::Result<HashSet<u64>> {
    let mut by_file: HashMap<u64, Vec<node::Reader<'_>>> = HashMap::new();
    for n in ctx.request.get_nodes()? {
        by_file.entry(file_of(ctx, n.get_id())).or_default().push(n);
    }
    let mut files = HashSet::from([file]);
    let mut stack = vec![file];
    while let Some(f) = stack.pop() {
        for n in by_file.get(&f).into_iter().flatten() {
            let mut ids = Vec::new();
            referenced_ids((*n).into(), &mut ids)?;
            for id in ids {
                if ctx.node_map.contains_key(&id) {
                    let reached = file_of(ctx, id);
                    if files.insert(reached) {
                        stack.push(reached);
                    }
                }
            }
        }
    }
    Ok(files)
}

/// Every `UInt64` in `value`, which includes every node id a node refers to: references in
/// schema.capnp (types, brands, annotations, superclasses, method structs, newtypes, scopes) are
/// all `UInt64` ids. Values that aren't node ids are dropped by the caller, so over-collecting
/// (a constant's value, say) at worst takes in a file that wasn't needed.
fn referenced_ids(value: dynamic_value::Reader<'_>, out: &mut Vec<u64>) -> capnp::Result<()> {
    match value {
        dynamic_value::Reader::UInt64(v) => out.push(v),
        dynamic_value::Reader::Struct(st) => {
            for field in st.get_schema().get_non_union_fields()? {
                referenced_ids(st.get(field)?, out)?;
            }
            if let Some(field) = st.which()? {
                referenced_ids(st.get(field)?, out)?;
            }
        }
        dynamic_value::Reader::List(list) => {
            for v in list {
                referenced_ids(v?, out)?;
            }
        }
        _ => {}
    }
    Ok(())
}
