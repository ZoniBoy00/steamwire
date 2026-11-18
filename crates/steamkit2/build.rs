//! Generates the Steam enums (`EMsg`, `EResult`, `EAccountType`, ...) from the
//! SteamKit2 `.steamd` language files in `steamd/`.
//!
//! Every enum becomes a newtype (`pub struct EMsg(pub u32)`) with associated
//! constants instead of a Rust `enum`: the files contain aliases (two names for
//! one value) and Steam sends values that are not listed, and a newtype keeps
//! both cases representable.
//!
//! Rules:
//! * members marked `removed` are skipped;
//! * members marked `obsolete` are emitted with `#[deprecated]`;
//! * a value can be a decimal or hex number, another member's name, or a
//!   `|`-separated combination of those.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;

const FILES: &[&str] = &["emsg.steamd", "eresult.steamd", "enums.steamd", "netheader.steamd"];

struct Member {
    name: String,
    value: i128,
    obsolete: bool,
}

struct Enum {
    name: String,
    repr: &'static str,
    flags: bool,
    members: Vec<Member>,
}

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("steamd");
    let mut enums = Vec::new();
    for file in FILES {
        let path = dir.join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        let text = std::fs::read_to_string(&path).unwrap();
        parse_file(&text, &mut enums);
    }

    let mut out = String::new();
    for e in &enums {
        emit(e, &mut out);
    }
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::write(out_dir.join("enums.rs"), out).unwrap();

    // Message bodies. The headers (`header.steamd`, `gamecoordinator.steamd`)
    // are written by hand in `messages/`.
    let mut classes = Vec::new();
    for file in ["steammsg.steamd", "netheader.steamd"] {
        let path = dir.join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        let text = std::fs::read_to_string(&path).unwrap();
        parse_classes(&text, &mut classes);
    }
    let mut out = String::new();
    for c in &classes {
        emit_class(c, &enums, &mut out);
    }
    emit_describe(&classes, &mut out);
    std::fs::write(out_dir.join("messages.rs"), out).unwrap();
}

// ------------------------------------------------------------------ classes

struct Field {
    name: String,
    ty: String,
    modifier: Option<String>,
    array: Option<usize>,
    default: Option<String>,
}

struct Const {
    name: String,
    ty: String,
    value: String,
}

struct Class {
    name: String,
    emsg: Option<String>,
    consts: Vec<Const>,
    fields: Vec<Field>,
}

fn parse_classes(text: &str, classes: &mut Vec<Class>) {
    let mut lines = text.lines().map(strip_comment);
    while let Some(line) = lines.next() {
        let line = line.trim();
        let Some(header) = line.strip_prefix("class ") else {
            continue;
        };
        let removed = header.split_whitespace().any(|w| w == "removed");
        let decl = header.split_whitespace().next().unwrap();
        let (name, emsg) = match decl.split_once('<') {
            Some((name, rest)) => {
                let emsg = rest.trim_end_matches('>');
                let emsg = emsg.strip_prefix("EMsg::").unwrap_or(emsg).to_string();
                (name.to_string(), Some(emsg))
            }
            None => (decl.to_string(), None),
        };

        let mut consts = Vec::new();
        let mut fields = Vec::new();
        for line in lines.by_ref() {
            let line = line.trim();
            if line == "{" || line.is_empty() {
                continue;
            }
            if line.starts_with('}') {
                break;
            }
            let line = line.trim_end_matches(';').trim();
            let (decl, default) = match line.split_once('=') {
                Some((d, v)) => (d.trim(), Some(v.trim().to_string())),
                None => (line, None),
            };
            let words: Vec<&str> = decl.split_whitespace().collect();
            if words[0] == "const" {
                consts.push(Const { ty: words[1].to_string(), name: words[2].to_string(), value: default.unwrap() });
                continue;
            }
            let (modifier, ty, fname) = match words.len() {
                2 => (None, words[0], words[1]),
                3 => (Some(words[0].to_string()), words[1], words[2]),
                _ => panic!("bad field in {name}: {line}"),
            };
            let (ty, array) = match ty.split_once('<') {
                Some((t, n)) => (t, Some(n.trim_end_matches('>').parse().unwrap())),
                None => (ty, None),
            };
            fields.push(Field { name: fname.to_string(), ty: ty.to_string(), modifier, array, default });
        }
        if !removed {
            classes.push(Class { name, emsg, consts, fields });
        }
    }
}

fn snake(name: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = name.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() {
            let prev_lower = i > 0 && (chars[i - 1].is_ascii_lowercase() || chars[i - 1].is_ascii_digit());
            let next_lower = chars.get(i + 1).is_some_and(char::is_ascii_lowercase);
            let prev_upper = i > 0 && chars[i - 1].is_ascii_uppercase();
            if i > 0 && (prev_lower || (prev_upper && next_lower)) {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    match out.as_str() {
        "type" => "r#type".into(),
        _ => out,
    }
}

/// (rust type, kind) for a field. Kinds: "int:<prim>", "enum:<prim>",
/// "steamid", "gameid", "bool", "array".
fn field_type(f: &Field, enums: &[Enum]) -> (String, String) {
    if let Some(n) = f.array {
        return (format!("[u8; {n}]"), "array".into());
    }
    match f.modifier.as_deref() {
        Some("steamidmarshal") => return ("crate::types::SteamId".into(), "steamid".into()),
        Some("gameidmarshal") => return ("crate::types::GameId".into(), "gameid".into()),
        Some("boolmarshal") => return ("bool".into(), "bool".into()),
        Some(other) => panic!("unknown modifier {other}"),
        None => {}
    }
    if let Some(e) = enums.iter().find(|e| e.name == f.ty) {
        return (format!("crate::enums::{}", e.name), format!("enum:{}", e.repr));
    }
    let prim = map_type(&f.ty);
    (prim.to_string(), format!("int:{prim}"))
}

fn default_expr(f: &Field, rust_ty: &str, kind: &str) -> String {
    let Some(value) = &f.default else {
        return match kind {
            "array" => format!("[0u8; {}]", f.array.unwrap()),
            _ => "Default::default()".into(),
        };
    };
    if value == "ulong.MaxValue" {
        return "u64::MAX".into();
    }
    if let Some((owner, member)) = value.split_once("::") {
        if kind.starts_with("enum:") {
            return format!("{rust_ty}::{member}");
        }
        return format!("{owner}::{member}");
    }
    value.clone()
}

fn emit_class(c: &Class, enums: &[Enum], out: &mut String) {
    let name = &c.name;
    let typed: Vec<(String, String, String)> = c
        .fields
        .iter()
        .map(|f| {
            let (ty, kind) = field_type(f, enums);
            (snake(&f.name), ty, kind)
        })
        .collect();

    writeln!(out, "/// `{name}` from SteamKit2's SteamLanguage.").unwrap();
    writeln!(out, "#[derive(Clone, Debug, PartialEq)]\npub struct {name} {{").unwrap();
    for (fname, ty, _) in &typed {
        writeln!(out, "    pub {fname}: {ty},").unwrap();
    }
    writeln!(out, "}}\n").unwrap();

    if !c.consts.is_empty() {
        writeln!(out, "#[allow(non_upper_case_globals)]\nimpl {name} {{").unwrap();
        for k in &c.consts {
            let value = match k.value.split_once("::") {
                Some((owner, member)) => format!("{owner}::{member}"),
                None => k.value.clone(),
            };
            writeln!(out, "    pub const {}: {} = {};", k.name, map_type(&k.ty), value).unwrap();
        }
        writeln!(out, "}}\n").unwrap();
    }

    writeln!(out, "impl Default for {name} {{\n    fn default() -> Self {{\n        Self {{").unwrap();
    for (f, (fname, ty, kind)) in c.fields.iter().zip(&typed) {
        writeln!(out, "            {fname}: {},", default_expr(f, ty, kind)).unwrap();
    }
    writeln!(out, "        }}\n    }}\n}}\n").unwrap();

    writeln!(out, "impl crate::messages::SteamStruct for {name} {{").unwrap();
    writeln!(out, "    fn encode(&self, out: &mut Vec<u8>) {{").unwrap();
    if typed.is_empty() {
        writeln!(out, "        let _ = out;").unwrap();
    }
    for (fname, _, kind) in &typed {
        let line = match kind.as_str() {
            "array" => format!("out.extend_from_slice(&self.{fname});"),
            "steamid" | "gameid" => format!("out.extend_from_slice(&self.{fname}.as_u64().to_le_bytes());"),
            "bool" => format!("out.push(self.{fname} as u8);"),
            k if k.starts_with("enum:") => format!("out.extend_from_slice(&self.{fname}.0.to_le_bytes());"),
            _ => format!("out.extend_from_slice(&self.{fname}.to_le_bytes());"),
        };
        writeln!(out, "        {line}").unwrap();
    }
    writeln!(out, "    }}\n").unwrap();
    writeln!(out, "    fn decode(r: &mut crate::messages::Reader<'_>) -> crate::Result<Self> {{").unwrap();
    if typed.is_empty() {
        writeln!(out, "        let _ = r;").unwrap();
    }
    writeln!(out, "        Ok(Self {{").unwrap();
    for (fname, ty, kind) in &typed {
        let expr = match kind.as_str() {
            "array" => "r.array()?".to_string(),
            "steamid" => "crate::types::SteamId::from_u64(r.u64()?)".to_string(),
            "gameid" => "crate::types::GameId::from_u64(r.u64()?)".to_string(),
            "bool" => "r.u8()? != 0".to_string(),
            k if k.starts_with("enum:") => format!("{ty}(r.{}()?)", &k[5..]),
            k => format!("r.{}()?", &k[4..]),
        };
        writeln!(out, "            {fname}: {expr},").unwrap();
    }
    writeln!(out, "        }})\n    }}\n}}\n").unwrap();

    if let Some(emsg) = &c.emsg {
        writeln!(
            out,
            "impl crate::messages::MsgBody for {name} {{\n    const EMSG: crate::enums::EMsg = crate::enums::EMsg::{emsg};\n}}\n"
        )
        .unwrap();
    }
}

/// A lookup from message type to its struct, for tools like the NetHook
/// analyzer.
fn emit_describe(classes: &[Class], out: &mut String) {
    out.push_str(
        "/// Decodes the body of a non-protobuf message with its `Msg*` struct:
         /// the struct name and either its debug form and the bytes it used, or
         /// the decode error. `None` if no struct is known for `emsg`.
         pub fn describe_body(emsg: crate::enums::EMsg, data: &[u8]) -> Option<(&'static str, crate::Result<(String, usize)>)> {
             use crate::messages::SteamStruct;
             fn run<T: SteamStruct + core::fmt::Debug>(data: &[u8]) -> crate::Result<(String, usize)> {
                 let mut r = crate::messages::Reader::new(data);
                 let body = T::decode(&mut r)?;
                 Ok((format!(\"{body:#?}\"), data.len() - r.remaining().len()))
             }
             Some(match emsg {
",
    );
    let mut seen = std::collections::HashSet::new();
    for c in classes {
        let Some(emsg) = &c.emsg else { continue };
        if !seen.insert(emsg.clone()) {
            continue;
        }
        let name = &c.name;
        writeln!(out, "        crate::enums::EMsg::{emsg} => (\"{name}\", run::<{name}>(data)),").unwrap();
    }
    out.push_str(
        "        _ => return None,
    })
}
",
    );
}

fn parse_file(text: &str, enums: &mut Vec<Enum>) {
    let mut lines = text.lines().map(strip_comment);
    while let Some(line) = lines.next() {
        let line = line.trim();
        let line = line.strip_prefix("public ").unwrap_or(line);
        let Some(header) = line.strip_prefix("enum ") else {
            continue;
        };
        let flags = header.split_whitespace().any(|w| w == "flags");
        let decl = header.split_whitespace().next().unwrap();
        let (name, repr) = match decl.split_once('<') {
            Some((name, ty)) => (name, map_type(ty.trim_end_matches('>'))),
            None if decl == "EMsg" => (decl, "u32"),
            None => (decl, "i32"),
        };

        let mut members: Vec<Member> = Vec::new();
        let mut values: HashMap<String, i128> = HashMap::new();
        for line in lines.by_ref() {
            let line = line.trim();
            if line == "{" || line.is_empty() {
                continue;
            }
            if line.starts_with('}') {
                break;
            }
            let Some((member, rest)) = line.split_once('=') else {
                continue;
            };
            let (expr, attrs) = rest.split_once(';').unwrap_or((rest, ""));
            let member = member.trim().to_string();
            let value = eval(expr, &values).unwrap_or_else(|| panic!("bad value for {name}::{member}: {expr}"));
            values.insert(member.clone(), value);

            let attr = attrs.split_whitespace().next().unwrap_or("");
            if attr == "removed" {
                continue;
            }
            members.push(Member { name: member, value, obsolete: attr == "obsolete" });
        }
        enums.push(Enum { name: name.to_string(), repr, flags, members });
    }
}

fn strip_comment(line: &str) -> &str {
    match line.find("//") {
        Some(i) => &line[..i],
        None => line,
    }
}

fn map_type(ty: &str) -> &'static str {
    match ty {
        "byte" => "u8",
        "short" => "i16",
        "ushort" => "u16",
        "int" => "i32",
        "uint" => "u32",
        "long" => "i64",
        "ulong" => "u64",
        other => panic!("unknown enum type {other}"),
    }
}

fn eval(expr: &str, values: &HashMap<String, i128>) -> Option<i128> {
    let mut result = 0i128;
    for term in expr.split('|') {
        let term = term.trim();
        let value = if let Some(hex) = term.strip_prefix("0x").or_else(|| term.strip_prefix("0X")) {
            i128::from_str_radix(hex, 16).ok()?
        } else if let Ok(n) = term.parse::<i128>() {
            n
        } else {
            *values.get(term)?
        };
        result |= value;
    }
    Some(result)
}

fn emit(e: &Enum, out: &mut String) {
    let Enum { name, repr, flags, members } = e;

    writeln!(out, "/// The `{name}` enumeration from SteamKit2's SteamLanguage.").unwrap();
    writeln!(out, "///").unwrap();
    writeln!(out, "/// A newtype so that unknown values survive a round trip.").unwrap();
    writeln!(
        out,
        "#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]\n#[repr(transparent)]\npub struct {name}(pub {repr});\n"
    )
    .unwrap();

    writeln!(out, "#[allow(non_upper_case_globals)]\nimpl {name} {{").unwrap();
    for m in members {
        if m.obsolete {
            writeln!(out, "    #[deprecated]").unwrap();
        }
        writeln!(out, "    pub const {}: Self = Self({});", m.name, literal(m.value, repr)).unwrap();
    }

    // The name of a value: the first member with it, preferring members that
    // are not obsolete.
    let mut names: Vec<(i128, &str)> = Vec::new();
    for pass_obsolete in [false, true] {
        for m in members.iter().filter(|m| m.obsolete == pass_obsolete) {
            if !names.iter().any(|(v, _)| *v == m.value) {
                names.push((m.value, &m.name));
            }
        }
    }
    names.sort_by_key(|(v, _)| *v);

    writeln!(out, "\n    /// The name of this value, if it is a known member.").unwrap();
    writeln!(out, "    pub const fn name(self) -> Option<&'static str> {{\n        match self.0 {{").unwrap();
    for (value, member) in &names {
        writeln!(out, "            {} => Some(\"{member}\"),", literal(*value, repr)).unwrap();
    }
    writeln!(out, "            _ => None,\n        }}\n    }}").unwrap();

    writeln!(out, "\n    /// Looks a member up by name.").unwrap();
    writeln!(out, "    pub fn from_name(name: &str) -> Option<Self> {{\n        match name {{").unwrap();
    for m in members {
        writeln!(out, "            \"{}\" => Some(Self({})),", m.name, literal(m.value, repr)).unwrap();
    }
    writeln!(out, "            _ => None,\n        }}\n    }}").unwrap();

    if *flags {
        writeln!(
            out,
            "\n    /// Whether every bit of `other` is set in `self`.\n    pub const fn contains(self, other: Self) -> bool {{ self.0 & other.0 == other.0 }}"
        )
        .unwrap();
    }
    writeln!(out, "}}\n").unwrap();

    writeln!(
        out,
        "impl SteamEnum for {name} {{\n    fn from_name(name: &str) -> Option<Self> {{ Self::from_name(name) }}\n    \
         fn from_i64(value: i64) -> Option<Self> {{ <{repr}>::try_from(value).ok().map(Self) }}\n}}\n"
    )
    .unwrap();

    writeln!(
        out,
        r#"impl core::fmt::Debug for {name} {{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {{
        match self.name() {{
            Some(n) => write!(f, "{name}::{{n}}"),
            None => write!(f, "{name}({{}})", self.0),
        }}
    }}
}}

impl core::fmt::Display for {name} {{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {{
        match self.name() {{
            Some(n) => f.write_str(n),
            None => write!(f, "{{}}", self.0),
        }}
    }}
}}

impl From<{repr}> for {name} {{
    fn from(v: {repr}) -> Self {{ Self(v) }}
}}

impl From<{name}> for {repr} {{
    fn from(v: {name}) -> Self {{ v.0 }}
}}
"#
    )
    .unwrap();

    if *flags {
        writeln!(
            out,
            r#"impl core::ops::BitOr for {name} {{
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {{ Self(self.0 | rhs.0) }}
}}

impl core::ops::BitAnd for {name} {{
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {{ Self(self.0 & rhs.0) }}
}}

impl core::ops::BitOrAssign for {name} {{
    fn bitor_assign(&mut self, rhs: Self) {{ self.0 |= rhs.0; }}
}}
"#
        )
        .unwrap();
    }
}

/// Renders a value as a literal of the enum's type. Values above the signed
/// maximum (flags such as `0x80000000` in an `i32` enum) wrap, like C# does.
fn literal(value: i128, repr: &str) -> String {
    match repr {
        "i32" => format!("{}", value as u32 as i32),
        "i16" => format!("{}", value as u16 as i16),
        "i64" => format!("{}", value as u64 as i64),
        _ => format!("{value}"),
    }
}
