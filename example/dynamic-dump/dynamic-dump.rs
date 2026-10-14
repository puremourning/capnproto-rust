//! Reads a self-describing message whose payload type is not compiled into this tool, and prints
//! the payload through the dynamic API.
//!
//! ```text
//! dynamic-dump write [--no-schema]    write a sample Envelope to stdout
//! dynamic-dump read [SCHEMA.cgr]      read an Envelope from stdin and print its payload
//! ```
//!
//! Only `envelope.capnp` is compiled into the tool. The payload's schema reaches the reading side
//! at run time: either in the envelope itself (`Envelope.schema`), or as a
//! `CodeGeneratorRequest` file (`capnp compile -o- payload.capnp > payload.cgr`).

use capnp::dynamic_value;
use capnp::introspect::TypeVariant;
use capnp::message::{self, ReaderOptions};
use capnp::schema::{EnumSchema, Field, StructSchema};
use capnp::schema_capnp::{code_generator_request, node};
use capnp::schema_loader::SchemaLoader;
use capnp::{dynamic_list, dynamic_struct, serialize, Error, Result};
use std::io::Write;

capnp::generated_code!(pub mod envelope_capnp);

use envelope_capnp::envelope;

/// The `CodeGeneratorRequest` for `payload.capnp`, produced by `build.rs`. Only `write` uses it,
/// to build a sample payload with the dynamic builder API.
const PAYLOAD_CGR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.cgr"));

/// The id of the struct node whose unqualified name (e.g. `SensorReport`) is `name`.
fn find_struct(nodes: capnp::struct_list::Reader<'_, node::Owned>, name: &str) -> Result<u64> {
    for n in nodes {
        let display_name = n.get_display_name()?.to_str()?;
        let short = display_name
            .get(n.get_display_name_prefix_length() as usize..)
            .unwrap_or_default();
        if short == name && matches!(n.which()?, node::Struct(_)) {
            return Ok(n.get_id());
        }
    }
    Err(Error::failed(format!("no struct named {name:?}")))
}

/// The value of enumerant `name` of the enum-typed `field`.
fn enum_value(field: Field, name: &str) -> Result<dynamic_value::Enum> {
    let TypeVariant::Enum(raw) = field.get_type().which() else {
        return Err(Error::failed("not an enum field".into()));
    };
    let schema = EnumSchema::new(raw);
    for e in schema.get_enumerants()? {
        if e.get_proto().get_name()?.to_str()? == name {
            return Ok(dynamic_value::Enum::new(e.get_ordinal(), schema));
        }
    }
    Err(Error::failed(format!("no enumerant named {name:?}")))
}

/// Fills a `SensorReport` through the dynamic builder API.
fn fill_report(mut report: dynamic_struct::Builder<'_>, schema: StructSchema) -> Result<()> {
    report.set_named("station", "Hilltop".into())?;
    report.set_named("takenAt", 1_760_000_000_000u64.into())?;
    let degraded = enum_value(schema.get_field_by_name("status")?, "degraded")?;
    report.set_named("status", degraded.into())?;

    let mut location = report
        .reborrow()
        .get_named("location")?
        .downcast::<dynamic_struct::Builder<'_>>();
    location.set_named("lat", 51.5.into())?;
    location.set_named("lon", (-0.12).into())?;

    let mut readings = report
        .reborrow()
        .initn_named("readings", 3)?
        .downcast::<dynamic_list::Builder<'_>>();
    let samples: [(&str, &str, dynamic_value::Reader<'_>); 3] = [
        ("t0", "celsius", 21.5f32.into()),
        ("h0", "humidity", 0.43f32.into()),
        ("x0", "raw", (&[0xca, 0xfe][..]).into()),
    ];
    for (i, (sensor, which, value)) in samples.into_iter().enumerate() {
        let mut reading = readings
            .reborrow()
            .get(i as u32)?
            .downcast::<dynamic_struct::Builder<'_>>();
        reading.set_named("sensor", sensor.into())?;
        reading.set_named(which, value)?;
    }

    let mut tags = report
        .initn_named("tags", 2)?
        .downcast::<dynamic_list::Builder<'_>>();
    tags.set(0, "outdoor".into())?;
    tags.set(1, "rooftop".into())?;
    Ok(())
}

/// Builds a sample `Envelope` holding a `SensorReport`, with the payload's schema embedded
/// unless `embed_schema` is false.
fn build_sample(embed_schema: bool) -> Result<message::Builder<message::HeapAllocator>> {
    let cgr_message = serialize::read_message(PAYLOAD_CGR, ReaderOptions::new())?;
    let cgr: code_generator_request::Reader<'_> = cgr_message.get_root()?;
    let loader = SchemaLoader::new();
    loader.load_code_generator_request(cgr)?;
    let type_id = find_struct(cgr.get_nodes()?, "SensorReport")?;
    let schema = loader.get_struct(type_id)?;

    let mut message = message::Builder::new_default();
    let mut root: envelope::Builder<'_> = message.init_root();
    root.set_type_id(type_id);
    if embed_schema {
        // All of the request's nodes: a closed set, as `SchemaLoader::load` requires.
        root.set_schema(cgr.get_nodes()?)?;
    }
    fill_report(root.get_payload().init_dynamic(schema)?, schema)?;
    Ok(message)
}

/// Prints the payload of `envelope`, using the schema embedded in it and/or `cgr`.
fn dump(
    envelope: envelope::Reader<'_>,
    cgr: Option<code_generator_request::Reader<'_>>,
    out: &mut impl Write,
) -> Result<()> {
    let loader = SchemaLoader::new();
    if let Some(cgr) = cgr {
        loader.load_code_generator_request(cgr)?;
    }
    if envelope.has_schema() {
        loader.load(envelope.get_schema()?)?;
    }
    let type_id = envelope.get_type_id();
    let schema = loader.get_struct(type_id).map_err(|e| {
        let hint = if cgr.is_none() && !envelope.has_schema() {
            "; the envelope carries no schema, so pass a CodeGeneratorRequest file that has it"
        } else {
            ""
        };
        Error::failed(format!(
            "no schema for payload type {type_id:#018x} ({}){hint}",
            e.extra
        ))
    })?;
    let payload = envelope.get_payload().get_as_dynamic(schema)?;
    let name = schema.get_proto().get_display_name()?.to_str()?;
    writeln!(out, "# {name} ({type_id:#018x})")?;
    writeln!(out, "{payload:#?}")?;
    Ok(())
}

fn read(cgr_path: Option<&str>) -> Result<()> {
    let cgr_message = match cgr_path {
        Some(path) => {
            let file = std::fs::File::open(path)
                .map_err(|e| Error::failed(format!("opening {path}: {e}")))?;
            Some(serialize::read_message(
                std::io::BufReader::new(file),
                ReaderOptions::new(),
            )?)
        }
        None => None,
    };
    let cgr = cgr_message.as_ref().map(|m| m.get_root()).transpose()?;
    let message = serialize::read_message(std::io::stdin().lock(), ReaderOptions::new())?;
    dump(message.get_root()?, cgr, &mut std::io::stdout().lock())
}

const USAGE: &str =
    "usage: dynamic-dump write [--no-schema]\n       dynamic-dump read [SCHEMA.cgr]";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args[..] {
        ["write"] | ["write", "--no-schema"] => build_sample(args.len() == 1)
            .and_then(|m| serialize::write_message(std::io::stdout().lock(), &m)),
        ["read"] => read(None),
        ["read", path] => read(Some(path)),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("dynamic-dump: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(embed_schema: bool, with_cgr: bool) -> Result<String> {
        let mut bytes = Vec::new();
        serialize::write_message(&mut bytes, &build_sample(embed_schema)?)?;
        let message = serialize::read_message(&bytes[..], ReaderOptions::new())?;
        let cgr_message = serialize::read_message(PAYLOAD_CGR, ReaderOptions::new())?;
        let cgr = if with_cgr {
            Some(cgr_message.get_root()?)
        } else {
            None
        };
        let mut out = Vec::new();
        dump(message.get_root()?, cgr, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn self_describing() {
        let text = round_trip(true, false).unwrap();
        assert!(
            text.starts_with("# payload.capnp:SensorReport (0x"),
            "{text}"
        );
        for needle in [
            "station = \"Hilltop\"",
            "status = degraded",
            "celsius = 21.5",
            "raw = 0x\"cafe\"",
            "lat = 51.5",
            "\"outdoor\",\n    \"rooftop\"",
        ] {
            assert!(text.contains(needle), "{needle:?} missing from:\n{text}");
        }
    }

    #[test]
    fn schema_from_file() {
        assert_eq!(
            round_trip(false, true).unwrap(),
            round_trip(true, false).unwrap()
        );
        // Both sources at once: identical nodes are deduplicated.
        assert_eq!(
            round_trip(true, true).unwrap(),
            round_trip(true, false).unwrap()
        );
    }

    #[test]
    fn missing_schema_is_an_error() {
        let e = round_trip(false, false).unwrap_err();
        assert!(e.to_string().contains("no schema for payload type"), "{e}");
    }
}
