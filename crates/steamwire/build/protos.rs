//! Compiles the `.proto` files in `proto/` with a vendored `protoc`.
//!
//! The Steam protobufs have no `package` line, so every file in a set
//! ends up in one generated module (`steam.rs` / `gc.rs`).

use std::fmt::Write;
use std::path::{Path, PathBuf};

use prost_build::{Service, ServiceGenerator};

pub fn run() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let proto_root = manifest_dir.join("proto");

    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc");
    std::env::set_var("PROTOC", &protoc);
    let protoc_include = protoc_bin_vendored::include_path().expect("vendored protoc include");

    println!("cargo:rerun-if-changed={}", proto_root.display());

    let steam_dir = proto_root.join("steam");
    let mut steam_files: Vec<PathBuf> = std::fs::read_dir(&steam_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "proto"))
        .collect();
    steam_files.sort();

    compile("steam", &steam_files, &[&steam_dir, &protoc_include], &out_dir, Some(Box::new(Services::default())));
    compile("gc", &[proto_root.join("gc.proto")], &[&proto_root, &protoc_include], &out_dir, None);

    // The games' GC messages and the web UI services, as SteamKit2's
    // `SteamKit2.GC.*.Internal` and `SteamKit2.WebUI.Internal`, each behind
    // its feature.
    for set in ["csgo", "dota2", "tf2", "deadlock", "webui"] {
        if std::env::var_os(format!("CARGO_FEATURE_{}", set.to_uppercase())).is_none() {
            continue;
        }
        let services: Option<Box<dyn ServiceGenerator>> =
            if set == "webui" { Some(Box::new(Services::default())) } else { None };
        let stripped = strip_set(&proto_root.join(set), &out_dir.join("stripped").join(set));
        compile(set, &stripped, &[&out_dir.join("stripped").join(set), &protoc_include], &out_dir, services);
    }
}

/// Copies the `.proto` files of `from` to `to` without Valve's own
/// `descriptor.proto` options (`boxed_type`, `synthetic_default`,
/// `additional_includes`), which only their fork of `descriptor.proto`
/// defines; protoc's own include is used instead. Returns the copies.
fn strip_set(from: &Path, to: &Path) -> Vec<PathBuf> {
    std::fs::create_dir_all(to).unwrap();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "proto") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let out = to.join(path.file_name().unwrap());
        std::fs::write(&out, strip_valve_options(&text)).unwrap();
        files.push(out);
    }
    files.sort();
    files
}

fn strip_valve_options(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if line.trim_start().starts_with("option additional_includes") {
            continue;
        }
        let mut line = line.to_owned();
        for name in ["boxed_type", "synthetic_default"] {
            while let Some(range) = option_range(&line, name) {
                line.replace_range(range, "");
            }
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Where `name = "…"` is in `line`, with the comma that separates it from
/// another option, or its brackets (and the space before them) when it is
/// the only one.
fn option_range(line: &str, name: &str) -> Option<std::ops::Range<usize>> {
    let start = line.find(name)?;
    let rest = &line[start + name.len()..];
    let value = rest.trim_start().strip_prefix('=')?.trim_start().strip_prefix('"')?;
    let end = line.len() - value.len() + value.find('"')? + 1;
    let before = line[..start].trim_end();
    let after = line[end..].trim_start();
    Some(if let Some(head) = before.strip_suffix(',') {
        head.len()..end
    } else if let Some(next) = after.strip_prefix(',') {
        start..line.len() - next.trim_start().len()
    } else if before.ends_with('[') && after.starts_with(']') {
        before.trim_end_matches('[').trim_end().len()..line.len() - after.len() + 1
    } else {
        start..end
    })
}

fn compile(
    name: &str,
    files: &[PathBuf],
    includes: &[&Path],
    out_dir: &Path,
    services: Option<Box<dyn ServiceGenerator>>,
) {
    let set_out = out_dir.join(name);
    std::fs::create_dir_all(&set_out).unwrap();
    let mut config = prost_build::Config::new();
    if let Some(services) = services {
        config.service_generator(services);
    }
    config
        .out_dir(&set_out)
        .default_package_filename(name)
        .file_descriptor_set_path(set_out.join("descriptors.bin"))
        .compile_protos(files, includes)
        .unwrap_or_else(|e| panic!("failed to compile proto set {name}: {e}"));
}

/// Generates `steam::services`: a typed client per unified service, like
/// the service classes in SteamKit2's `Base/Generated/SteamMsg*.cs`.
///
/// Methods returning `NoResponse` are notifications: they are sent without
/// waiting for a reply, and services that have them also get an enum that
/// decodes the incoming ones by method name.
#[derive(Default)]
struct Services {
    out: String,
}

impl ServiceGenerator for Services {
    fn generate(&mut self, service: Service, _buf: &mut String) {
        let out = &mut self.out;
        let name = &service.name;
        let proto_name = &service.proto_name;
        let is_notification = |m: &prost_build::Method| m.output_proto_type == ".NoResponse";

        writeln!(out, "/// The `{proto_name}` service.").unwrap();
        writeln!(out, "pub struct {name}<'a, T: ?Sized> {{ transport: &'a T }}").unwrap();
        writeln!(out, "impl<T: ?Sized> Clone for {name}<'_, T> {{ fn clone(&self) -> Self {{ *self }} }}").unwrap();
        writeln!(out, "impl<T: ?Sized> Copy for {name}<'_, T> {{}}").unwrap();
        writeln!(out, "impl<'a, T: crate::protos::ServiceTransport + ?Sized> {name}<'a, T> {{").unwrap();
        writeln!(out, "/// The service name.").unwrap();
        writeln!(out, "pub const NAME: &'static str = \"{proto_name}\";").unwrap();
        writeln!(out, "/// Calls the service through `transport`.").unwrap();
        writeln!(out, "pub fn new(transport: &'a T) -> Self {{ Self {{ transport }} }}").unwrap();
        for method in &service.methods {
            let target = format!("{proto_name}.{}#1", method.proto_name);
            let (fn_name, input) = (&method.name, &method.input_type);
            if is_notification(method) {
                writeln!(out, "/// Sends the `{target}` notification.").unwrap();
                writeln!(
                    out,
                    "pub fn {fn_name}(&self, request: super::{input}) -> Result<(), T::Error> {{                      self.transport.notify(\"{target}\", request) }}"
                )
                .unwrap();
            } else {
                let output = &method.output_type;
                writeln!(out, "/// Calls `{target}`.").unwrap();
                writeln!(
                    out,
                    "pub async fn {fn_name}(&self, request: super::{input}) -> Result<super::{output}, T::Error> {{                      self.transport.call(\"{target}\", request).await }}"
                )
                .unwrap();
            }
        }
        writeln!(out, "}}").unwrap();

        let notifications: Vec<_> = service.methods.iter().filter(|m| is_notification(m)).collect();
        if notifications.is_empty() {
            return;
        }
        writeln!(out, "/// An incoming notification of the `{proto_name}` service.").unwrap();
        writeln!(out, "#[derive(Clone, PartialEq, Debug)]").unwrap();
        writeln!(out, "pub enum {name}Notification {{").unwrap();
        for method in &notifications {
            writeln!(out, "/// `{proto_name}.{}#1`.", method.proto_name).unwrap();
            writeln!(out, "{}(super::{}),", method.proto_name, method.input_type).unwrap();
        }
        writeln!(out, "}}").unwrap();
        writeln!(out, "impl {name}Notification {{").unwrap();
        writeln!(
            out,
            "/// Decodes the body of a notification by its method (`{proto_name}.Method#1`);              `None` if the method isn't one of this service's notifications."
        )
        .unwrap();
        writeln!(out, "pub fn decode(method: &str, body: &[u8]) -> Option<Result<Self, prost::DecodeError>> {{")
            .unwrap();
        writeln!(out, "use prost::Message;").unwrap();
        writeln!(out, "Some(match method {{").unwrap();
        for method in &notifications {
            writeln!(
                out,
                "\"{proto_name}.{0}#1\" => super::{1}::decode(body).map(Self::{0}),",
                method.proto_name, method.input_type
            )
            .unwrap();
        }
        writeln!(out, "_ => return None,").unwrap();
        writeln!(out, "}})").unwrap();
        writeln!(out, "}}").unwrap();
        writeln!(out, "}}").unwrap();
    }

    fn finalize_package(&mut self, _package: &str, buf: &mut String) {
        if self.out.is_empty() {
            return;
        }
        buf.push_str(
            "/// Typed clients for the unified services, generated from the              `service` definitions. See `steamwire::services`.
             pub mod services {
",
        );
        buf.push_str(&std::mem::take(&mut self.out));
        buf.push_str(
            "}
",
        );
    }
}
