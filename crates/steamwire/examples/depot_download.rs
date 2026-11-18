//! Downloads a whole depot, like DepotDownloader: the files of a branch's
//! manifest, several chunks at a time, every file checked by SHA-1.
//!
//! Run again on the same directory, it updates in place: files that already
//! match are skipped, chunks found in the old files (by the manifest saved
//! last time, or at the same place in the file) are reused rather than
//! downloaded, and files no longer in the depot are deleted.
//!
//! Anonymous accounts can download free depots such as Spacewar's (the
//! default).
//!
//! ```sh
//! cargo run --release -p steamwire --example depot_download -- [app] [depot] [dir] [--branch name] [--parallel n]
//! ```

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use futures_util::stream::{self, StreamExt};
use sha1::{Digest, Sha1};
use steamwire::cdn::{CdnClient, ChunkData, DepotManifest, FileData, Server};
use steamwire::enums::EDepotFileFlag;
use steamwire::{Config, Error, SteamClient};

struct Options {
    app_id: u32,
    depot_id: u32,
    dir: PathBuf,
    branch: String,
    parallel: usize,
}

fn options() -> Options {
    let mut positional = Vec::new();
    let mut branch = "public".to_string();
    let mut parallel = 8;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--branch" => branch = args.next().expect("--branch needs a name"),
            "--parallel" => parallel = args.next().and_then(|n| n.parse().ok()).expect("--parallel needs a number"),
            _ => positional.push(arg),
        }
    }
    let app_id = positional.first().map_or(480, |a| a.parse().expect("app ID"));
    let depot_id = positional.get(1).map_or(481, |d| d.parse().expect("depot ID"));
    let dir = positional.get(2).map_or_else(|| PathBuf::from(format!("depot_{depot_id}")), PathBuf::from);
    Options { app_id, depot_id, dir, branch, parallel: parallel.max(1) }
}

/// The local path of a manifest file name, refusing names that would leave
/// the download directory.
fn local_path(dir: &Path, name: &str) -> steamwire::Result<PathBuf> {
    let relative = PathBuf::from(name.replace('\\', "/"));
    if relative.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(Error::InvalidData(format!("unsafe file name in the manifest: {name}")));
    }
    Ok(dir.join(relative))
}

fn sha1_of_file(path: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let mut hasher = Sha1::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buffer).ok()?;
        if n == 0 {
            return Some(hasher.finalize().to_vec());
        }
        hasher.update(&buffer[..n]);
    }
}

/// Reads `len` bytes at `offset` and returns them if their SHA-1 is the
/// chunk ID, i.e. the chunk is already there.
fn read_chunk(path: &Path, offset: u64, chunk: &ChunkData) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut data = vec![0u8; chunk.uncompressed_length as usize];
    file.read_exact(&mut data).ok()?;
    (Sha1::digest(&data).as_slice() == chunk.chunk_id.as_slice()).then_some(data)
}

/// Where each chunk of the previous download is, by chunk ID.
fn old_chunks(dir: &Path, old: Option<&DepotManifest>) -> HashMap<Vec<u8>, (PathBuf, u64)> {
    let mut map = HashMap::new();
    for file in old.into_iter().flat_map(|m| &m.files) {
        let Ok(path) = local_path(dir, &file.file_name) else { continue };
        for chunk in &file.chunks {
            map.entry(chunk.chunk_id.clone()).or_insert_with(|| (path.clone(), chunk.offset));
        }
    }
    map
}

struct Downloader<'a> {
    cdn: CdnClient,
    servers: Vec<Server>,
    depot_id: u32,
    depot_key: &'a [u8],
    parallel: usize,
}

impl Downloader<'_> {
    /// Downloads one chunk, trying the next server when one fails.
    async fn chunk(&self, chunk: &ChunkData) -> steamwire::Result<Vec<u8>> {
        let mut last = None;
        for server in self.servers.iter().take(3) {
            match self.cdn.download_depot_chunk(self.depot_id, chunk, server, Some(self.depot_key), None, None).await {
                Ok(data) => return Ok(data),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or(Error::Other("no content server".into())))
    }

    /// Writes `file` to `path`; returns the bytes downloaded.
    async fn file(
        &self,
        file: &FileData,
        path: &Path,
        old: &HashMap<Vec<u8>, (PathBuf, u64)>,
    ) -> steamwire::Result<u64> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let partial = path.with_extension("steamwire-partial");
        let mut out = File::create(&partial)?;
        out.set_len(file.total_size)?;

        // Chunks already on disk: in the old file at the same offset, or
        // anywhere in the previous download.
        let mut missing = Vec::new();
        for chunk in &file.chunks {
            let reused = read_chunk(path, chunk.offset, chunk)
                .or_else(|| old.get(&chunk.chunk_id).and_then(|(p, offset)| read_chunk(p, *offset, chunk)));
            match reused {
                Some(data) => {
                    out.seek(SeekFrom::Start(chunk.offset))?;
                    out.write_all(&data)?;
                }
                None => missing.push(chunk),
            }
        }

        let mut downloaded = 0;
        let mut results = stream::iter(missing)
            .map(|chunk| async move { self.chunk(chunk).await.map(|data| (chunk.offset, data)) })
            .buffer_unordered(self.parallel);
        while let Some(result) = results.next().await {
            let (offset, data) = result?;
            out.seek(SeekFrom::Start(offset))?;
            out.write_all(&data)?;
            downloaded += data.len() as u64;
        }
        drop(out);

        if sha1_of_file(&partial).as_deref() != Some(file.file_hash.as_slice()) {
            let _ = fs::remove_file(&partial);
            return Err(Error::InvalidData(format!("{}: SHA-1 mismatch after download", file.file_name)));
        }
        fs::rename(&partial, path)?;
        Ok(downloaded)
    }
}

#[tokio::main]
async fn main() -> steamwire::Result<()> {
    let options = options();
    let (client, _events) = SteamClient::connect(Config::default()).await?;
    client.log_on_anonymous().await?;

    let info = client.pics_get_product_info(&[options.app_id.into()], &[]).await?;
    let app = info.apps.get(&options.app_id).ok_or(Error::Other("no product info for the app".into()))?;
    let branch = &app.key_values["depots"][&*options.depot_id.to_string()]["manifests"][&*options.branch];
    let manifest_id: u64 = branch["gid"]
        .as_str()
        .or_else(|| branch.as_str())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| Error::Other(format!("depot {} has no '{}' manifest", options.depot_id, options.branch)))?;

    let depot_key = client.get_depot_decryption_key(options.depot_id, options.app_id).await?;
    let request_code = client
        .get_manifest_request_code(options.depot_id, options.app_id, manifest_id, Some(&options.branch), None)
        .await?;
    let servers: Vec<Server> = client
        .get_servers_for_steam_pipe(None, None)
        .await?
        .into_iter()
        .filter(|s| s.allowed_app_ids.is_empty() || s.allowed_app_ids.contains(&options.app_id))
        .collect();
    client.log_off()?;

    let downloader = Downloader {
        cdn: CdnClient::new(),
        servers,
        depot_id: options.depot_id,
        depot_key: &depot_key,
        parallel: options.parallel,
    };
    let mut manifest = None;
    for server in downloader.servers.iter().take(3) {
        match downloader
            .cdn
            .download_manifest(options.depot_id, manifest_id, request_code, server, Some(&depot_key), None, None)
            .await
        {
            Ok(m) => {
                manifest = Some(m);
                break;
            }
            Err(e) => println!("manifest from {server}: {e}"),
        }
    }
    let manifest = manifest.ok_or(Error::Other("no server gave the manifest".into()))?;
    println!(
        "depot {} manifest {manifest_id}: {} files, {} bytes",
        options.depot_id,
        manifest.files.len(),
        manifest.total_uncompressed_size
    );

    // The manifest of the last download, for reuse and deletions.
    let state = options.dir.join(".steamwire");
    fs::create_dir_all(&state)?;
    let saved = state.join(format!("{}.manifest", options.depot_id));
    let old = DepotManifest::load_from_file(&saved).ok();
    let old_map = old_chunks(&options.dir, old.as_ref());

    let started = Instant::now();
    let (mut downloaded, mut skipped, mut written) = (0u64, 0, 0);
    for file in &manifest.files {
        let path = local_path(&options.dir, &file.file_name)?;
        if file.flags.contains(EDepotFileFlag::Directory) {
            fs::create_dir_all(&path)?;
            continue;
        }
        if file.flags.contains(EDepotFileFlag::Symlink) {
            println!("skipping symlink {} -> {:?}", file.file_name, file.link_target);
            continue;
        }
        let up_to_date = fs::metadata(&path).is_ok_and(|m| m.len() == file.total_size)
            && sha1_of_file(&path).as_deref() == Some(file.file_hash.as_slice());
        if up_to_date {
            skipped += 1;
            continue;
        }
        let bytes = downloader.file(file, &path, &old_map).await?;
        downloaded += bytes;
        written += 1;
        println!("{} ({} bytes, {bytes} downloaded)", file.file_name, file.total_size);
    }

    // Files of the previous version that are gone now.
    let current: std::collections::HashSet<_> = manifest.files.iter().map(|f| f.file_name.as_str()).collect();
    let mut deleted = 0;
    for file in old.iter().flat_map(|m| &m.files) {
        let is_file = !file.flags.contains(EDepotFileFlag::Directory);
        if is_file && !current.contains(file.file_name.as_str()) {
            if let Ok(path) = local_path(&options.dir, &file.file_name) {
                if fs::remove_file(&path).is_ok() {
                    deleted += 1;
                }
            }
        }
    }
    manifest.save_to_file(&saved)?;

    println!(
        "done in {:.1?}: {written} files written ({downloaded} bytes downloaded), {skipped} up to date, {deleted} deleted",
        started.elapsed()
    );
    Ok(())
}
