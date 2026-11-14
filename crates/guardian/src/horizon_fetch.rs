//! Download the exact Minecraft inputs pinned by Horizon from Mojang.
//!
//! Files are private to the panel, verified before publication, and may be
//! reused on later attempts. No game files are included in our releases.
use crate::ScopedFs;
use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::path::{Component, Path, PathBuf};
use tokio::io::AsyncWriteExt;

const LOCK: &str = include_str!("../../../vendor/horizon/research-lock.json");
const MAX_FILE: u64 = 512 * 1024 * 1024;
const MAX_METADATA: u64 = 8 * 1024 * 1024;
const MAX_ASSETS: usize = 100_000;

#[derive(Clone, Debug)]
struct Artifact {
    path: PathBuf,
    url: String,
    sha1: String,
    size: Option<u64>,
}

struct TemporaryDownload<'a> {
    fs: &'a ScopedFs,
    path: PathBuf,
}

impl Drop for TemporaryDownload<'_> {
    fn drop(&mut self) {
        let _ = self.fs.remove(&self.path);
    }
}

fn checksum_valid(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn safe_relative(raw: &str) -> Result<PathBuf> {
    if raw.is_empty() || raw.contains('\\') || raw.contains(':') || raw.starts_with('/') {
        bail!("unsafe Minecraft artifact path");
    }
    let path = Path::new(raw);
    if !path
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
        || raw
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        bail!("unsafe Minecraft artifact path");
    }
    Ok(path.to_path_buf())
}

fn official_url(raw: &str) -> bool {
    reqwest::Url::parse(raw).ok().is_some_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && matches!(
                url.host_str(),
                Some(
                    "piston-meta.mojang.com"
                        | "piston-data.mojang.com"
                        | "libraries.minecraft.net"
                        | "resources.download.minecraft.net"
                )
            )
    })
}

fn artifact(value: &Value, path: PathBuf) -> Result<Artifact> {
    let url = value["url"].as_str().context("missing artifact URL")?;
    let sha1 = value["sha1"]
        .as_str()
        .context("missing artifact checksum")?;
    let size = value["size"].as_u64();
    if !official_url(url) || !checksum_valid(sha1) || size.is_some_and(|n| n > MAX_FILE) {
        bail!("invalid Minecraft artifact metadata");
    }
    Ok(Artifact {
        path,
        url: url.into(),
        sha1: sha1.into(),
        size,
    })
}

fn manifest_plan(bytes: &[u8]) -> Result<(Vec<Artifact>, Artifact)> {
    let lock: Value = serde_json::from_str(LOCK)?;
    let game = &lock["minecraft"];
    let pin = game["manifest_sha1"]
        .as_str()
        .context("missing manifest pin")?;
    let digest = format!("{:x}", Sha1::digest(bytes));
    if !checksum_valid(pin) || digest != pin {
        bail!("Minecraft version manifest checksum mismatch");
    }
    let manifest: Value = serde_json::from_slice(bytes)?;
    let version = game["version"].as_str().context("missing pinned version")?;
    if manifest["id"].as_str() != Some(version)
        || manifest["downloads"]["client"]["sha1"] != game["client_sha1"]
        || manifest["downloads"]["server"]["sha1"] != game["server_sha1"]
        || manifest["javaVersion"]["majorVersion"] != game["java_major"]
    {
        bail!("Minecraft version manifest does not match Horizon's pinned game");
    }
    safe_relative(version)?;
    let mut artifacts = vec![
        artifact(
            &manifest["downloads"]["client"],
            PathBuf::from(format!("versions/{version}/{version}.jar")),
        )?,
        artifact(
            &manifest["downloads"]["server"],
            PathBuf::from("server.jar"),
        )?,
    ];
    let libraries = manifest["libraries"]
        .as_array()
        .context("missing Minecraft libraries")?;
    if libraries.len() > 10_000 {
        bail!("too many Minecraft libraries");
    }
    for library in libraries {
        if let Some(value) = library.get("downloads").and_then(|v| v.get("artifact")) {
            let path = value["path"].as_str().context("missing library path")?;
            artifacts.push(artifact(
                value,
                PathBuf::from("libraries").join(safe_relative(path)?),
            )?);
        }
    }
    let index = &manifest["assetIndex"];
    let id = index["id"].as_str().context("missing asset index id")?;
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || id.is_empty()
    {
        bail!("invalid asset index id");
    }
    let index = artifact(index, PathBuf::from(format!("assets/indexes/{id}.json")))?;
    Ok((artifacts, index))
}

fn assets_plan(bytes: &[u8], expected: &str) -> Result<Vec<Artifact>> {
    if format!("{:x}", Sha1::digest(bytes)) != expected {
        bail!("asset index checksum mismatch");
    }
    let index: Value = serde_json::from_slice(bytes)?;
    let objects = index["objects"]
        .as_object()
        .context("missing asset objects")?;
    if objects.len() > MAX_ASSETS {
        bail!("too many Minecraft assets");
    }
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for value in objects.values() {
        let hash = value["hash"].as_str().context("missing asset checksum")?;
        let size = value["size"].as_u64().context("missing asset size")?;
        if !checksum_valid(hash) || size > MAX_FILE {
            bail!("invalid asset metadata");
        }
        if !seen.insert(hash.to_owned()) {
            continue;
        }
        let prefix = &hash[..2];
        result.push(Artifact {
            path: PathBuf::from(format!("assets/objects/{prefix}/{hash}")),
            url: format!("https://resources.download.minecraft.net/{prefix}/{hash}"),
            sha1: hash.into(),
            size: Some(size),
        });
    }
    Ok(result)
}

async fn fetch_one(client: &reqwest::Client, fs: &ScopedFs, item: &Artifact) -> Result<()> {
    use std::io::Read;
    if let Ok(mut existing) = fs.open_file(&item.path) {
        let mut hash = Sha1::new();
        let mut buffer = [0u8; 65536];
        let mut size = 0u64;
        loop {
            let n = existing.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            size += n as u64;
            if size > MAX_FILE {
                break;
            }
            hash.update(&buffer[..n]);
        }
        if item.size.is_none_or(|n| n == size) && format!("{:x}", hash.finalize()) == item.sha1 {
            return Ok(());
        }
    }
    let parent = item.path.parent().context("artifact has no parent")?;
    fs.create_dir_all(parent)?;
    let temp = parent.join(format!(".mcpanel-download-{}", uuid::Uuid::new_v4()));
    let _cleanup = TemporaryDownload {
        fs,
        path: temp.clone(),
    };
    let result = async {
        let response = client.get(&item.url).send().await?.error_for_status()?;
        if !official_url(response.url().as_str()) {
            bail!("download redirected outside official Minecraft hosts");
        }
        let file = fs.create_new_file(&temp)?;
        let mut file = tokio::fs::File::from_std(file);
        let mut stream = response.bytes_stream();
        let mut hash = Sha1::new();
        let mut count = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            count = count
                .checked_add(chunk.len() as u64)
                .context("download size overflow")?;
            if count > MAX_FILE || item.size.is_some_and(|size| count > size) {
                bail!("Minecraft download exceeds expected size");
            }
            hash.update(&chunk);
            file.write_all(&chunk).await?;
        }
        if item.size.is_some_and(|size| size != count)
            || format!("{:x}", hash.finalize()) != item.sha1
        {
            bail!("Minecraft download checksum mismatch");
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        fs.rename(&temp, &item.path)?;
        Ok(())
    }
    .await;
    result
}

/// Fetch the Minecraft inputs expected by Horizon into a private launcher-like directory.
/// The supplied callback receives a human-readable phase and completed/total item counts.
pub async fn download_inputs(root: &Path, progress: impl Fn(&str, usize, usize)) -> Result<()> {
    std::fs::create_dir_all(root)?;
    let fs = ScopedFs::open(root)?;
    fs.set_private()?;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .read_timeout(std::time::Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() < 5 && official_url(attempt.url().as_str()) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()?;
    let lock: Value = serde_json::from_str(LOCK)?;
    let game = &lock["minecraft"];
    let version = game["version"].as_str().context("missing version")?;
    safe_relative(version)?;
    let pin = game["manifest_sha1"]
        .as_str()
        .context("missing manifest pin")?;
    if !checksum_valid(pin) {
        bail!("invalid pinned manifest checksum");
    }
    let manifest = Artifact {
        path: PathBuf::from(format!("versions/{version}/{version}.json")),
        url: format!("https://piston-meta.mojang.com/v1/packages/{pin}/{version}.json"),
        sha1: pin.into(),
        size: None,
    };
    progress("Minecraft version manifest", 0, 1);
    fetch_one(&client, &fs, &manifest).await?;
    if fs.metadata(&manifest.path)?.len > MAX_METADATA {
        bail!("Minecraft version manifest is too large");
    }
    let bytes = fs.read_file(&manifest.path)?;
    let (files, index) = manifest_plan(&bytes)?;
    fetch_one(&client, &fs, &index).await?;
    if fs.metadata(&index.path)?.len > MAX_METADATA {
        bail!("Minecraft asset index is too large");
    }
    let bytes = fs.read_file(&index.path)?;
    let assets = assets_plan(&bytes, &index.sha1)?;
    let total = files.len() + assets.len();
    let downloads =
        futures_util::stream::iter(files.iter().chain(assets.iter()).map(|item| async {
            fetch_one(&client, &fs, item)
                .await
                .with_context(|| format!("downloading {}", item.path.display()))
        }));
    let mut downloads = downloads.buffer_unordered(8);
    let mut completed = 0;
    while let Some(result) = downloads.next().await {
        result?;
        completed += 1;
        progress("Downloading Minecraft files", completed, total);
    }
    progress("Minecraft files ready", total, total);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_untrusted_paths_and_hosts() {
        for path in ["../x", "/x", "a/../x", "a\\b", "C:/x", "a//b"] {
            assert!(safe_relative(path).is_err(), "{path}");
        }
        assert!(safe_relative("some/library.jar").is_ok());
        assert!(!official_url("https://libraries.minecraft.net.evil.test/a"));
        assert!(!official_url("http://libraries.minecraft.net/a"));
        assert!(official_url("https://libraries.minecraft.net/a"));
    }
    #[test]
    fn rejects_modified_manifests_and_indices() {
        assert!(manifest_plan(b"{}").is_err());
        assert!(assets_plan(b"{}", "0000000000000000000000000000000000000000").is_err());
    }
    #[test]
    fn asset_plan_deduplicates_content_and_rejects_bad_hashes() {
        let json = br#"{"objects":{"a":{"hash":"0123456789012345678901234567890123456789","size":1},"b":{"hash":"0123456789012345678901234567890123456789","size":1}}}"#;
        let hash = format!("{:x}", Sha1::digest(json));
        assert_eq!(assets_plan(json, &hash).unwrap().len(), 1);
        let invalid = br#"{"objects":{"a":{"hash":"../../etc","size":1}}}"#;
        let hash = format!("{:x}", Sha1::digest(invalid));
        assert!(assets_plan(invalid, &hash).is_err());
    }

    #[tokio::test]
    #[ignore = "requires official Minecraft download servers"]
    async fn official_metadata_matches_pinned_horizon_and_can_be_reused() {
        let root = tempfile::tempdir().unwrap();
        let fs = ScopedFs::open(root.path()).unwrap();
        let lock: Value = serde_json::from_str(LOCK).unwrap();
        let game = &lock["minecraft"];
        let version = game["version"].as_str().unwrap();
        let pin = game["manifest_sha1"].as_str().unwrap();
        let manifest = Artifact {
            path: PathBuf::from(format!("versions/{version}/{version}.json")),
            url: format!("https://piston-meta.mojang.com/v1/packages/{pin}/{version}.json"),
            sha1: pin.into(),
            size: None,
        };
        let client = reqwest::Client::new();
        fetch_one(&client, &fs, &manifest).await.unwrap();
        let (files, index) = manifest_plan(&fs.read_file(&manifest.path).unwrap()).unwrap();
        assert!(files.len() > 2);
        fetch_one(&client, &fs, &index).await.unwrap();
        let assets = assets_plan(&fs.read_file(&index.path).unwrap(), &index.sha1).unwrap();
        assert!(!assets.is_empty());
        let library = files
            .iter()
            .find(|item| item.path.starts_with("libraries"))
            .unwrap();
        fetch_one(&client, &fs, library).await.unwrap();
        let small_asset = assets.iter().min_by_key(|item| item.size).unwrap();
        fetch_one(&client, &fs, small_asset).await.unwrap();
        std::fs::write(root.path().join(&small_asset.path), b"corrupt").unwrap();
        fetch_one(&client, &fs, small_asset).await.unwrap();
        // A second call succeeds from the verified cache even with a dead client.
        let dead = reqwest::Client::builder()
            .timeout(std::time::Duration::from_nanos(1))
            .build()
            .unwrap();
        fetch_one(&dead, &fs, &manifest).await.unwrap();
    }
}
