use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs::{
        copy, create_dir_all, read, read_dir, read_to_string, remove_dir_all, remove_file, write,
    },
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::read::GzDecoder;
use reqwest::Client;
use sha2::{Digest, Sha512};
use tar::Archive;

use super::resolver::{Lockfile, PackageManifest, resolve_lockfile};

fn decode_integrity(integrity: &str) -> Result<Vec<u8>> {
    let encoded = integrity
        .strip_prefix("sha512-")
        .with_context(|| format!("unsupported npm integrity algorithm: {integrity}"))?;
    let expected = STANDARD
        .decode(encoded)
        .with_context(|| format!("decode npm integrity: {integrity}"))?;
    if expected.len() != 64 {
        bail!("invalid sha512 npm integrity length: {integrity}");
    }
    Ok(expected)
}

fn verify_integrity(bytes: &[u8], integrity: &str) -> Result<()> {
    let expected = decode_integrity(integrity)?;
    let actual = Sha512::digest(bytes);
    if actual[..] != expected {
        bail!("npm tarball integrity mismatch: {integrity}");
    }
    Ok(())
}

fn cache_filename(digest: &[u8]) -> String {
    let mut filename = String::with_capacity(digest.len() * 2 + 4);
    for byte in digest {
        write!(filename, "{byte:02x}").expect("writing to a String cannot fail");
    }
    filename.push_str(".tgz");
    filename
}

async fn load_tarball(
    client: &Client,
    cache_dir: &Path,
    url: &str,
    integrity: &str,
) -> Result<Vec<u8>> {
    let digest = decode_integrity(integrity)?;
    let cache_path = cache_dir.join(cache_filename(&digest));
    if cache_path.is_file() {
        let bytes = read(&cache_path)
            .with_context(|| format!("read cached npm tarball {}", cache_path.display()))?;
        verify_integrity(&bytes, integrity)
            .with_context(|| format!("verify cached npm tarball {}", cache_path.display()))?;
        return Ok(bytes);
    }

    let parsed =
        reqwest::Url::parse(url).with_context(|| format!("parse npm tarball URL {url}"))?;
    if parsed.scheme() != "https" {
        bail!("npm tarball URL must use HTTPS: {url}");
    }
    let bytes = client
        .get(parsed)
        .send()
        .await
        .with_context(|| format!("download npm tarball {url}"))?
        .error_for_status()
        .with_context(|| format!("download npm tarball {url}"))?
        .bytes()
        .await
        .with_context(|| format!("read npm tarball response {url}"))?
        .to_vec();
    verify_integrity(&bytes, integrity).with_context(|| format!("verify npm tarball {url}"))?;
    write(&cache_path, &bytes)
        .with_context(|| format!("cache npm tarball {}", cache_path.display()))?;
    Ok(bytes)
}

fn extract_tarball(bytes: &[u8], destination: &Path) -> Result<()> {
    create_dir_all(destination)
        .with_context(|| format!("create npm package directory {}", destination.display()))?;
    let mut archive = Archive::new(GzDecoder::new(bytes));
    for entry in archive.entries().context("read npm tarball entries")? {
        let mut entry = entry.context("read npm tarball entry")?;
        let path = entry
            .path()
            .context("read npm tarball entry path")?
            .into_owned();
        let mut components = path.components();
        if !matches!(components.next(), Some(Component::Normal(_))) {
            bail!(
                "npm tarball entry has no package directory: {}",
                path.display()
            );
        }
        let mut relative = PathBuf::new();
        for component in components {
            match component {
                Component::Normal(component) => relative.push(component),
                _ => bail!("unsafe npm tarball entry path: {}", path.display()),
            }
        }
        if relative.as_os_str().is_empty() {
            continue;
        }
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            bail!(
                "npm tarball contains an unsupported link: {}",
                path.display()
            );
        }
        let output = destination.join(&relative);
        if let Some(parent) = output.parent() {
            create_dir_all(parent)
                .with_context(|| format!("create npm package directory {}", parent.display()))?;
        }
        entry
            .unpack(&output)
            .with_context(|| format!("extract npm package file {}", output.display()))?;
    }
    Ok(())
}

fn package_path(name: &str) -> Result<PathBuf> {
    let parts = name.split('/').collect::<Vec<_>>();
    let valid = match parts.as_slice() {
        [name] => !name.is_empty() && !name.starts_with('@'),
        [scope, name] => scope.starts_with('@') && scope.len() > 1 && !name.is_empty(),
        _ => false,
    };
    if !valid
        || parts.iter().any(|part| {
            Path::new(part)
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        })
    {
        bail!("unsafe npm package name in textlint-v8.lock: {name}");
    }
    Ok(parts.into_iter().collect())
}

fn preferred_packages(lock: &Lockfile) -> Result<BTreeMap<String, String>> {
    let mut counts = BTreeMap::<String, BTreeMap<String, usize>>::new();
    for package in lock.packages.values() {
        for (name, id) in &package.dependencies {
            *counts
                .entry(name.clone())
                .or_default()
                .entry(id.clone())
                .or_default() += 1;
        }
    }
    let mut preferred = lock.roots.clone();
    for (name, versions) in counts {
        preferred.entry(name).or_insert_with(|| {
            versions
                .into_iter()
                .max_by(|(left_id, left_count), (right_id, right_count)| {
                    left_count
                        .cmp(right_count)
                        .then_with(|| left_id.cmp(right_id))
                })
                .expect("dependency version counts cannot be empty")
                .0
        });
    }
    for (name, id) in &preferred {
        let package = lock
            .packages
            .get(id)
            .with_context(|| format!("locked npm package is missing: {id}"))?;
        if package.name != *name {
            bail!(
                "locked npm package {id} has unexpected name {}",
                package.name
            );
        }
    }
    Ok(preferred)
}

fn materialize_package(
    lock: &Lockfile,
    cache_dir: &Path,
    id: &str,
    destination: &Path,
    scope: &BTreeMap<String, String>,
) -> Result<()> {
    let package = lock
        .packages
        .get(id)
        .with_context(|| format!("locked npm package is missing: {id}"))?;
    let cache_path = cache_dir.join(cache_filename(&decode_integrity(&package.integrity)?));
    let bytes = read(&cache_path)
        .with_context(|| format!("read cached npm tarball {}", cache_path.display()))?;
    extract_tarball(&bytes, destination)
        .with_context(|| format!("install locked npm package {id}"))?;

    let mut child_scope = scope.clone();
    child_scope.insert(package.name.clone(), id.to_owned());
    for (dependency_name, dependency_id) in &package.dependencies {
        if child_scope.get(dependency_name) == Some(dependency_id) {
            continue;
        }
        let dependency_path = destination
            .join("node_modules")
            .join(package_path(dependency_name)?);
        materialize_package(
            lock,
            cache_dir,
            dependency_id,
            &dependency_path,
            &child_scope,
        )?;
    }
    Ok(())
}

pub async fn install_dependencies(root: &Path, update_lockfile: bool) -> Result<()> {
    let manifest: PackageManifest = serde_json::from_slice(
        &read(root.join("package.json")).context("read package.json for npm install")?,
    )
    .context("parse package.json for npm install")?;
    let lock_path = root.join("textlint-v8.lock");
    let client = Client::new();
    let lock = if update_lockfile {
        let lock = resolve_lockfile(&client, &manifest).await?;
        let mut contents =
            serde_json::to_string_pretty(&lock).context("serialize textlint-v8.lock")?;
        contents.push('\n');
        write(&lock_path, contents).with_context(|| format!("write {}", lock_path.display()))?;
        lock
    } else {
        serde_json::from_slice(
            &read(&lock_path).with_context(|| format!("read {}", lock_path.display()))?,
        )
        .with_context(|| format!("parse {}", lock_path.display()))?
    };
    if lock.version != 1 {
        bail!("unsupported textlint-v8.lock version: {}", lock.version);
    }
    let expected_roots = manifest
        .requirements()
        .into_iter()
        .map(|(name, version)| {
            let id = format!("{name}@{version}");
            (name, id)
        })
        .collect::<BTreeMap<_, _>>();
    if lock.roots != expected_roots {
        bail!("textlint-v8.lock is stale; regenerate it after changing package.json");
    }

    let cache_dir = root
        .parent()
        .context("build workspace has no parent directory")?
        .join("npm-cache");
    create_dir_all(&cache_dir)
        .with_context(|| format!("create npm cache {}", cache_dir.display()))?;
    for package in lock.packages.values() {
        load_tarball(&client, &cache_dir, &package.resolved, &package.integrity).await?;
    }

    let preferred = preferred_packages(&lock)?;
    for (name, id) in &preferred {
        let destination = root.join("node_modules").join(package_path(name)?);
        materialize_package(&lock, &cache_dir, id, &destination, &preferred)?;
    }
    Ok(())
}

pub fn patch_legacy_style_format(root: &Path) -> Result<()> {
    let path = root.join("node_modules/@azu/style-format/ansi-codes.js");
    let source = read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    match source.matches("\\033").count() {
        44 => {
            remove_file(&path).with_context(|| format!("unlink {}", path.display()))?;
            write(&path, source.replace("\\033", "\\x1b"))
                .with_context(|| format!("patch legacy escapes in {}", path.display()))?;
        }
        0 if source.matches("\\x1b").count() == 44 => {}
        _ => bail!("@azu/style-format changed; expected 44 ANSI escape sequences"),
    }
    Ok(())
}

pub fn patch_pluralize_commonjs(root: &Path) -> Result<()> {
    const BEFORE: &str = "typeof require === 'function' && typeof exports === 'object' && typeof module === 'object'";
    const AFTER: &str = "typeof exports === 'object' && typeof module === 'object'";

    let path = root.join("node_modules/pluralize/pluralize.js");
    let source = read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    match source.matches(BEFORE).count() {
        1 => {
            remove_file(&path).with_context(|| format!("unlink {}", path.display()))?;
            write(&path, source.replace(BEFORE, AFTER))
                .with_context(|| format!("patch CommonJS detection in {}", path.display()))?;
        }
        0 if source.matches(AFTER).count() == 1 => {}
        _ => bail!("pluralize changed; expected its CommonJS environment check"),
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    create_dir_all(destination).with_context(|| format!("create {}", destination.display()))?;
    for entry in read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", source.display()))?;
        let source = entry.path();
        let destination = destination.join(entry.file_name());
        if entry
            .file_type()
            .with_context(|| format!("read file type for {}", source.display()))?
            .is_dir()
        {
            copy_tree(&source, &destination)?;
        } else {
            copy(&source, &destination).with_context(|| {
                format!("copy {} to {}", source.display(), destination.display())
            })?;
        }
    }
    Ok(())
}

pub fn prepare_workspace(source_root: &Path, out_dir: &Path) -> Result<PathBuf> {
    let workspace = out_dir.join("npm-workspace");
    if workspace.exists() {
        remove_dir_all(&workspace).with_context(|| format!("remove {}", workspace.display()))?;
    }
    create_dir_all(&workspace).with_context(|| format!("create {}", workspace.display()))?;
    for filename in ["package.json", "textlint-v8.lock"] {
        copy(source_root.join(filename), workspace.join(filename))
            .with_context(|| format!("copy {filename} into the build workspace"))?;
    }
    copy_tree(&source_root.join("js"), &workspace.join("js"))?;
    Ok(workspace)
}
