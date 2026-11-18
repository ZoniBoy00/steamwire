//! Writes a file descriptor as `.proto` source, including custom options.

use std::fmt::Write;

use prost_reflect::{
    Cardinality, DynamicMessage, EnumDescriptor, ExtensionDescriptor, FieldDescriptor, FileDescriptor, Kind,
    MessageDescriptor, ServiceDescriptor, Syntax, Value,
};
use prost_types::field_descriptor_proto::Type;
use prost_types::FieldDescriptorProto;

/// The `.proto` source of `file`.
pub fn render_file(file: &FileDescriptor) -> String {
    let mut out = String::new();
    let proto = file.file_descriptor_proto();
    match file.syntax() {
        Syntax::Proto3 => out.push_str("syntax = \"proto3\";\n\n"),
        Syntax::Proto2 if proto.syntax.is_some() => out.push_str("syntax = \"proto2\";\n\n"),
        Syntax::Proto2 => {}
    }
    if !file.package_name().is_empty() {
        writeln!(out, "package {};\n", file.package_name()).unwrap();
    }
    let public: Vec<_> = proto.public_dependency.iter().map(|&i| i as usize).collect();
    let weak: Vec<_> = proto.weak_dependency.iter().map(|&i| i as usize).collect();
    for (i, dep) in proto.dependency.iter().enumerate() {
        let kind = if public.contains(&i) {
            "public "
        } else if weak.contains(&i) {
            "weak "
        } else {
            ""
        };
        writeln!(out, "import {kind}\"{dep}\";").unwrap();
    }
    if !proto.dependency.is_empty() {
        out.push('\n');
    }
    let options = option_list(&file.options());
    for option in &options {
        writeln!(out, "option {option};").unwrap();
    }
    if !options.is_empty() {
        out.push('\n');
    }

    let mut blocks = Vec::new();
    blocks.extend(group_extensions(file.extensions(), 0));
    blocks.extend(file.messages().map(|m| message(&m, 0)));
    blocks.extend(file.enums().map(|e| enumeration(&e, 0)));
    blocks.extend(file.services().map(|s| service(&s)));
    out.push_str(&blocks.join("\n"));
    out
}

fn indent(level: usize) -> String {
    "\t".repeat(level)
}

/// `name = value` for every set option, custom ones as `(full.name)`.
fn option_list(options: &DynamicMessage) -> Vec<String> {
    let mut list: Vec<String> =
        options.fields().map(|(f, v)| format!("{} = {}", f.name(), value(&f.kind(), v))).collect();
    list.extend(options.extensions().map(|(e, v)| format!("({}) = {}", e.full_name(), value(&e.kind(), v))));
    list
}

fn value(kind: &Kind, v: &Value) -> String {
    match v {
        Value::String(s) => quote(s.as_bytes()),
        Value::Bytes(b) => quote(b),
        Value::EnumNumber(n) => match kind {
            Kind::Enum(e) => e.get_value(*n).map_or(n.to_string(), |v| v.name().to_owned()),
            _ => n.to_string(),
        },
        Value::Message(m) => format!("{{ {} }}", m.to_text_format()),
        Value::List(items) => {
            format!("[{}]", items.iter().map(|i| value(kind, i)).collect::<Vec<_>>().join(", "))
        }
        Value::Bool(b) => b.to_string(),
        Value::I32(n) => n.to_string(),
        Value::I64(n) => n.to_string(),
        Value::U32(n) => n.to_string(),
        Value::U64(n) => n.to_string(),
        Value::F32(n) => float(*n as f64),
        Value::F64(n) => float(*n),
        Value::Map(_) => "{}".to_owned(),
    }
}

fn float(n: f64) -> String {
    if n.is_infinite() {
        if n > 0.0 {
            "inf".into()
        } else {
            "-inf".into()
        }
    } else if n.is_nan() {
        "nan".into()
    } else {
        format!("{n:?}")
    }
}

/// A protobuf string literal.
fn quote(bytes: &[u8]) -> String {
    let mut s = String::from("\"");
    for &b in bytes {
        match b {
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            b'\n' => s.push_str("\\n"),
            b'\r' => s.push_str("\\r"),
            b'\t' => s.push_str("\\t"),
            0x20..=0x7E => s.push(b as char),
            _ => write!(s, "\\{b:03o}").unwrap(),
        }
    }
    s.push('"');
    s
}

/// The type of a field as written in source.
fn type_name(proto: &FieldDescriptorProto) -> String {
    match proto.r#type() {
        Type::Message | Type::Enum | Type::Group => proto.type_name().to_owned(),
        Type::Double => "double".into(),
        Type::Float => "float".into(),
        Type::Int64 => "int64".into(),
        Type::Uint64 => "uint64".into(),
        Type::Int32 => "int32".into(),
        Type::Fixed64 => "fixed64".into(),
        Type::Fixed32 => "fixed32".into(),
        Type::Bool => "bool".into(),
        Type::String => "string".into(),
        Type::Bytes => "bytes".into(),
        Type::Uint32 => "uint32".into(),
        Type::Sfixed32 => "sfixed32".into(),
        Type::Sfixed64 => "sfixed64".into(),
        Type::Sint32 => "sint32".into(),
        Type::Sint64 => "sint64".into(),
    }
}

/// `[default = ..., json_name = ..., options]`, or nothing.
fn field_options(proto: &FieldDescriptorProto, options: &DynamicMessage) -> String {
    let mut list = Vec::new();
    if let Some(default) = &proto.default_value {
        let literal = match proto.r#type() {
            Type::String => quote(default.as_bytes()),
            // Bytes defaults are stored C-escaped already.
            Type::Bytes => format!("\"{default}\""),
            _ => default.clone(),
        };
        list.push(format!("default = {literal}"));
    }
    list.extend(option_list(options));
    if list.is_empty() {
        String::new()
    } else {
        format!(" [{}]", list.join(", "))
    }
}

fn label(proto: &FieldDescriptorProto, cardinality: Cardinality, syntax: Syntax) -> &'static str {
    match cardinality {
        Cardinality::Repeated => "repeated ",
        Cardinality::Required => "required ",
        Cardinality::Optional if syntax == Syntax::Proto2 || proto.proto3_optional() => "optional ",
        Cardinality::Optional => "",
    }
}

fn field(f: &FieldDescriptor, level: usize, in_oneof: bool) -> String {
    let proto = f.field_descriptor_proto();
    let opts = field_options(proto, &f.options());
    if f.is_map() {
        let Kind::Message(entry) = f.kind() else { unreachable!("maps are messages") };
        let key = type_name(entry.map_entry_key_field().field_descriptor_proto());
        let value = type_name(entry.map_entry_value_field().field_descriptor_proto());
        return format!("{}map<{key}, {value}> {} = {}{opts};\n", indent(level), f.name(), f.number());
    }
    let label = if in_oneof { "" } else { label(proto, f.cardinality(), f.parent_file().syntax()) };
    format!("{}{label}{} {} = {}{opts};\n", indent(level), type_name(proto), f.name(), f.number())
}

fn extension(e: &ExtensionDescriptor, level: usize) -> String {
    let proto = e.field_descriptor_proto();
    let opts = field_options(proto, &e.options());
    let label = label(proto, e.cardinality(), e.parent_file().syntax());
    format!("{}{label}{} {} = {}{opts};\n", indent(level), type_name(proto), e.name(), e.number())
}

/// `extend` blocks, one per extended message, in order of appearance.
fn group_extensions(extensions: impl Iterator<Item = ExtensionDescriptor>, level: usize) -> Vec<String> {
    let mut groups: Vec<(String, String)> = Vec::new();
    for e in extensions {
        let extendee = e.field_descriptor_proto().extendee().to_owned();
        let line = extension(&e, level + 1);
        match groups.iter_mut().find(|(name, _)| *name == extendee) {
            Some((_, body)) => body.push_str(&line),
            None => groups.push((extendee, line)),
        }
    }
    groups
        .into_iter()
        .map(|(extendee, body)| format!("{}extend {extendee} {{\n{body}{}}}\n", indent(level), indent(level)))
        .collect()
}

fn ranges(list: impl Iterator<Item = (i64, i64)>, max: i64) -> String {
    list.map(|(start, end)| {
        let last = end - 1;
        if last == start {
            start.to_string()
        } else if end >= max {
            format!("{start} to max")
        } else {
            format!("{start} to {last}")
        }
    })
    .collect::<Vec<_>>()
    .join(", ")
}

fn message(m: &MessageDescriptor, level: usize) -> String {
    let mut out = format!("{}message {} {{\n", indent(level), m.name());
    let inner = level + 1;
    for option in option_list(&m.options()) {
        writeln!(out, "{}option {option};", indent(inner)).unwrap();
    }

    let mut done_oneofs = Vec::new();
    // Declaration order; `fields()` is sorted by number.
    let fields: Vec<FieldDescriptor> =
        m.descriptor_proto().field.iter().filter_map(|f| m.get_field_by_name(f.name())).collect();
    for f in &fields {
        match f.containing_oneof().filter(|o| !o.is_synthetic()) {
            Some(oneof) => {
                if done_oneofs.contains(&oneof.name().to_owned()) {
                    continue;
                }
                done_oneofs.push(oneof.name().to_owned());
                writeln!(out, "{}oneof {} {{", indent(inner), oneof.name()).unwrap();
                for option in option_list(&oneof.options()) {
                    writeln!(out, "{}option {option};", indent(inner + 1)).unwrap();
                }
                for f in fields.iter().filter(|f| f.containing_oneof().as_ref() == Some(&oneof)) {
                    out.push_str(&field(f, inner + 1, true));
                }
                writeln!(out, "{}}}", indent(inner)).unwrap();
            }
            None => out.push_str(&field(f, inner, false)),
        }
    }

    let mut nested = Vec::new();
    nested.extend(m.child_messages().filter(|c| !c.is_map_entry()).map(|c| message(&c, inner)));
    nested.extend(m.child_enums().map(|e| enumeration(&e, inner)));
    nested.extend(group_extensions(m.child_extensions(), inner));
    if !nested.is_empty() {
        if !fields.is_empty() {
            out.push('\n');
        }
        out.push_str(&nested.join("\n"));
    }

    let proto = m.descriptor_proto();
    if !proto.extension_range.is_empty() {
        let list = ranges(proto.extension_range.iter().map(|r| (r.start() as i64, r.end() as i64)), 536_870_912);
        writeln!(out, "{}extensions {list};", indent(inner)).unwrap();
    }
    if !proto.reserved_range.is_empty() {
        let list = ranges(proto.reserved_range.iter().map(|r| (r.start() as i64, r.end() as i64)), 536_870_912);
        writeln!(out, "{}reserved {list};", indent(inner)).unwrap();
    }
    if !proto.reserved_name.is_empty() {
        let names: Vec<_> = proto.reserved_name.iter().map(|n| quote(n.as_bytes())).collect();
        writeln!(out, "{}reserved {};", indent(inner), names.join(", ")).unwrap();
    }
    writeln!(out, "{}}}", indent(level)).unwrap();
    out
}

fn enumeration(e: &EnumDescriptor, level: usize) -> String {
    let mut out = format!("{}enum {} {{\n", indent(level), e.name());
    let inner = level + 1;
    for option in option_list(&e.options()) {
        writeln!(out, "{}option {option};", indent(inner)).unwrap();
    }
    // Declaration order; `values()` is sorted by number.
    for v in e.enum_descriptor_proto().value.iter().filter_map(|v| e.get_value_by_name(v.name())) {
        let opts = option_list(&v.options());
        let opts = if opts.is_empty() { String::new() } else { format!(" [{}]", opts.join(", ")) };
        writeln!(out, "{}{} = {}{opts};", indent(inner), v.name(), v.number()).unwrap();
    }
    let proto = e.enum_descriptor_proto();
    if !proto.reserved_range.is_empty() {
        // Enum reserved ranges are inclusive.
        let list = ranges(proto.reserved_range.iter().map(|r| (r.start() as i64, r.end() as i64 + 1)), i64::MAX);
        writeln!(out, "{}reserved {list};", indent(inner)).unwrap();
    }
    if !proto.reserved_name.is_empty() {
        let names: Vec<_> = proto.reserved_name.iter().map(|n| quote(n.as_bytes())).collect();
        writeln!(out, "{}reserved {};", indent(inner), names.join(", ")).unwrap();
    }
    writeln!(out, "{}}}", indent(level)).unwrap();
    out
}

fn service(s: &ServiceDescriptor) -> String {
    let mut out = format!("service {} {{\n", s.name());
    for option in option_list(&s.options()) {
        writeln!(out, "\toption {option};").unwrap();
    }
    for m in s.methods() {
        let proto = m.method_descriptor_proto();
        let stream = |on: bool| if on { "stream " } else { "" };
        let head = format!(
            "\trpc {} ({}{}) returns ({}{})",
            m.name(),
            stream(m.is_client_streaming()),
            proto.input_type(),
            stream(m.is_server_streaming()),
            proto.output_type()
        );
        let opts = option_list(&m.options());
        if opts.is_empty() {
            writeln!(out, "{head};").unwrap();
        } else {
            writeln!(out, "{head} {{").unwrap();
            for option in opts {
                writeln!(out, "\t\toption {option};").unwrap();
            }
            out.push_str("\t}\n");
        }
    }
    out.push_str("}\n");
    out
}
