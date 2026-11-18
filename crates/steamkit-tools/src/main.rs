//! `steamkit-tools`: the NetHook analyzer and the protobuf dumper.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use steamkit_tools::nethook::{Analyzer, Dump};
use steamkit_tools::protodump;

const USAGE: &str = "\
usage:
  steamkit-tools nethook <dump dir> [--tree] [--seq N] [--emsg NAME]
      Lists the messages of a NetHook dump; --tree decodes each one,
      --seq / --emsg pick messages (and imply --tree).
  steamkit-tools dump-protos <binary>... [-o <out dir>]
      Finds protobuf descriptors in binaries (like steamclient64.dll) and
      writes them as .proto files, by default to a directory named after
      the first binary.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("nethook") => nethook(&args[1..]),
        Some("dump-protos") => dump_protos(&args[1..]),
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn nethook(args: &[String]) -> Result<(), String> {
    let (mut dir, mut tree, mut seq, mut emsg) = (None, false, None, None);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--tree" => tree = true,
            "--seq" => seq = Some(it.next().and_then(|s| s.parse::<u32>().ok()).ok_or("--seq needs a number")?),
            "--emsg" => emsg = Some(it.next().ok_or("--emsg needs a name")?.to_ascii_lowercase()),
            _ if dir.is_none() => dir = Some(PathBuf::from(arg)),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let dir = dir.ok_or(USAGE)?;
    let dump = Dump::load(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if dump.items.is_empty() {
        return Err(format!("no NetHook messages in {}", dir.display()));
    }
    let analyzer = Analyzer::new();
    let tree = tree || seq.is_some() || emsg.is_some();
    for item in &dump.items {
        if seq.is_some_and(|s| s != item.sequence) {
            continue;
        }
        if emsg.as_ref().is_some_and(|e| !item.emsg.to_string().to_ascii_lowercase().contains(e)) {
            continue;
        }
        let dir = item.direction.as_str();
        if !tree {
            println!("{:>4} {dir:<3} {} ({})", item.sequence, item.emsg, item.emsg.0);
            continue;
        }
        let data = item.read().map_err(|e| format!("{}: {e}", item.path.display()))?;
        print!("#{} {dir} ", item.sequence);
        println!("{}", analyzer.message(&data));
    }
    Ok(())
}

fn dump_protos(args: &[String]) -> Result<(), String> {
    let (mut inputs, mut output) = (Vec::new(), None);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-o" => output = Some(PathBuf::from(it.next().ok_or("-o needs a directory")?)),
            _ => inputs.push(PathBuf::from(arg)),
        }
    }
    let first = inputs.first().ok_or(USAGE)?;
    let output = output.unwrap_or_else(|| PathBuf::from(first.file_stem().unwrap_or_default()));

    let mut files = Vec::new();
    for input in &inputs {
        println!("Loading binary '{}'...", input.display());
        let data = std::fs::read(input).map_err(|e| format!("{}: {e}", input.display()))?;
        let found = protodump::scan(&data);
        println!("  found {} descriptors", found.len());
        files.extend(found);
    }
    let collected = protodump::build_pool(files);
    for (name, reason) in &collected.failed {
        println!("  skipped {name}: {reason}");
    }
    for name in &collected.files {
        let file = collected.pool.get_file_by_name(name).expect("added");
        let path = output.join(Path::new(name));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, protodump::render_file(&file)).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("  wrote {}", path.display());
    }
    println!("{} files written to {}", collected.files.len(), output.display());
    Ok(())
}
