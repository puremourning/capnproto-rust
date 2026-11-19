use crate::dynamic_value;
use core::fmt::{self, Formatter};

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Indent {
    /// None means everything gets printed on a single line.
    indent: Option<usize>,
}

impl Indent {
    fn no_indent() -> Self {
        Self { indent: None }
    }

    fn enabled() -> Self {
        Self { indent: Some(0) }
    }

    fn next(self) -> Self {
        match self.indent {
            None => self,
            Some(x) => Self {
                indent: Some(x + 1),
            },
        }
    }

    fn maybe_newline(&self, formatter: &mut Formatter) -> Result<(), fmt::Error> {
        match self.indent {
            None => Ok(()),
            Some(indent) => {
                formatter.write_str("\n")?;
                for _ in 0..indent {
                    formatter.write_str("  ")?;
                }
                Ok(())
            }
        }
    }

    fn comma(&self, formatter: &mut Formatter) -> Result<(), fmt::Error> {
        match self.indent {
            None => formatter.write_str(", "),
            Some(_) => formatter.write_str(","),
        }
    }
}

/// Writes a read error in place of the value it stops. Formatting must not fail unless the
/// writer does (`format!` and `to_string` panic otherwise), so a malformed message -- or a value
/// read through a schema that doesn't match it -- prints `<error: ...>` where it went wrong.
fn print_error(formatter: &mut Formatter, error: crate::Error) -> Result<(), fmt::Error> {
    write!(formatter, "<error: {error}>")
}

fn print_result(
    value: crate::Result<dynamic_value::Reader>,
    formatter: &mut Formatter,
    indent: Indent,
) -> Result<(), fmt::Error> {
    match value {
        Ok(value) => print(value, formatter, indent),
        Err(e) => print_error(formatter, e),
    }
}

fn print_field_name(
    field: crate::schema::Field,
    formatter: &mut Formatter,
) -> Result<(), fmt::Error> {
    match field
        .get_proto()
        .get_name()
        .and_then(|name| Ok(name.to_str()?))
    {
        Ok(name) => formatter.write_str(name),
        Err(e) => print_error(formatter, e),
    }
}

pub(crate) fn print(
    value: dynamic_value::Reader,
    formatter: &mut Formatter,
    indent: Indent,
) -> Result<(), fmt::Error> {
    match value {
        dynamic_value::Reader::Void => formatter.write_str("()"),
        dynamic_value::Reader::Bool(b) => write!(formatter, "{b}"),
        dynamic_value::Reader::Int8(x) => write!(formatter, "{x}"),
        dynamic_value::Reader::Int16(x) => write!(formatter, "{x}"),
        dynamic_value::Reader::Int32(x) => write!(formatter, "{x}"),
        dynamic_value::Reader::Int64(x) => write!(formatter, "{x}"),
        dynamic_value::Reader::UInt8(x) => write!(formatter, "{x}"),
        dynamic_value::Reader::UInt16(x) => write!(formatter, "{x}"),
        dynamic_value::Reader::UInt32(x) => write!(formatter, "{x}"),
        dynamic_value::Reader::UInt64(x) => write!(formatter, "{x}"),
        dynamic_value::Reader::Float32(x) => write!(formatter, "{x}"),
        dynamic_value::Reader::Float64(x) => write!(formatter, "{x}"),
        dynamic_value::Reader::Enum(e) => match e.get_enumerant() {
            Ok(Some(enumerant)) => match enumerant
                .get_proto()
                .get_name()
                .and_then(|n| Ok(n.to_str()?))
            {
                Ok(name) => formatter.write_str(name),
                Err(err) => print_error(formatter, err),
            },
            Ok(None) => write!(formatter, "{}", e.get_value()),
            Err(err) => print_error(formatter, err),
        },
        dynamic_value::Reader::Text(t) => write!(formatter, "{t:?}"),
        dynamic_value::Reader::Data(d) => {
            formatter.write_str("0x\"")?;
            for b in d {
                write!(formatter, "{:02x}", *b)?;
            }
            formatter.write_str("\"")
        }
        dynamic_value::Reader::List(list) => {
            if list.is_empty() {
                formatter.write_str("[]")
            } else {
                formatter.write_str("[")?;
                let indent2 = indent.next();
                for (idx, value) in list.iter().enumerate() {
                    indent2.maybe_newline(formatter)?;
                    print_result(value, formatter, indent2)?;
                    if idx + 1 < list.len() as usize {
                        indent2.comma(formatter)?;
                    }
                }
                indent.maybe_newline(formatter)?;
                formatter.write_str("]")
            }
        }
        dynamic_value::Reader::Struct(st) => {
            let schema = st.get_schema();
            let (union_fields, non_union_fields) =
                match (schema.get_union_fields(), schema.get_non_union_fields()) {
                    (Ok(u), Ok(n)) => (u, n),
                    (Err(e), _) | (_, Err(e)) => return print_error(formatter, e),
                };
            if union_fields.len() + non_union_fields.len() == 0 {
                return formatter.write_str("()");
            }
            formatter.write_str("(")?;
            let indent2 = indent.next();
            let mut first = true;
            let mut separate = |formatter: &mut Formatter| -> Result<(), fmt::Error> {
                if first {
                    first = false
                } else {
                    indent2.comma(formatter)?;
                }
                indent2.maybe_newline(formatter)
            };
            let print_field = |field: crate::schema::Field,
                               formatter: &mut Formatter|
             -> Result<(), fmt::Error> {
                print_field_name(field, formatter)?;
                formatter.write_str(" = ")?;
                print_result(st.get(field), formatter, indent2)
            };
            // A field whose presence can't be read is printed, so that the error shows.
            let has = |field| st.has(field).unwrap_or(true);
            let mut union_field = match st.which() {
                Ok(None) => None,
                Ok(Some(field)) => {
                    // If it's not the default discriminant, then we always need to print it.
                    if field.get_proto().get_discriminant_value() != 0 || has(field) {
                        Some(field)
                    } else {
                        None
                    }
                }
                Err(e) => {
                    separate(formatter)?;
                    print_error(formatter, e)?;
                    None
                }
            };
            for field in non_union_fields {
                if let Some(ff) = union_field {
                    if ff.get_index() < field.get_index() {
                        // It's time to print the union field.
                        separate(formatter)?;
                        print_field(ff, formatter)?;
                        union_field = None;
                    }
                }
                if has(field) {
                    separate(formatter)?;
                    print_field(field, formatter)?;
                }
            }
            if let Some(ff) = union_field {
                // Union field comes last.
                separate(formatter)?;
                print_field(ff, formatter)?;
            }
            indent.maybe_newline(formatter)?;
            formatter.write_str(")")
        }
        dynamic_value::Reader::AnyPointer(_) => formatter.write_str("<opaque pointer>"),
        dynamic_value::Reader::Capability(_) => formatter.write_str("<external capability>"),
    }
}

impl fmt::Debug for dynamic_value::Reader<'_> {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        let indent = if f.alternate() {
            Indent::enabled()
        } else {
            Indent::no_indent()
        };
        print(*self, f, indent)
    }
}

#[cfg(test)]
mod tests {
    use crate::schema_capnp::{field, node};

    #[test]
    fn read_errors_print_in_place() {
        // A Node's `annotations` list is at the pointer where a Field (read from the same
        // message) has its `slot.type` struct, so that field reads as an error.
        let mut message = crate::message::Builder::new_default();
        message.init_root::<node::Builder<'_>>().init_annotations(1);
        let field: field::Reader<'_> = message.get_root_as_reader().unwrap();
        let field::Slot(slot) = field.which().unwrap() else {
            panic!("expected a slot");
        };
        assert!(slot.get_type().is_err());

        let value: crate::dynamic_value::Reader<'_> = field.into();
        let printed = format!("{value:?}");
        assert!(printed.contains("type = <error: "), "{printed}");
        assert!(printed.ends_with(')'), "{printed}");
        let printed = format!("{value:#?}");
        assert!(printed.contains("type = <error: "), "{printed}");
    }
}
