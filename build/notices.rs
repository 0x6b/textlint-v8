use std::{
    collections::BTreeMap,
    env::var_os,
    fmt::Write as _,
    fs::{canonicalize, copy, create_dir_all, read, read_dir, remove_dir_all, write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde_json::{Value, from_slice};

fn find_package_manifests(directory: &Path, manifests: &mut Vec<PathBuf>) -> Result<()> {
    for entry in read_dir(directory).with_context(|| format!("read {}", directory.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", directory.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("read file type for {}", path.display()))?;
        if file_type.is_dir() {
            find_package_manifests(&path, manifests)?;
        } else if entry.file_name() == "package.json" {
            manifests.push(path);
        }
    }
    Ok(())
}

fn append_rust_inventory(
    notices: &mut String,
    registry_sources: &Path,
    inventory_path: &Path,
) -> Result<()> {
    let inventory = String::from_utf8(read(inventory_path)?)
        .with_context(|| format!("{} is not UTF-8", inventory_path.display()))?;
    for line in inventory.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, version) = line.split_once(' ').with_context(|| {
            format!(
                "invalid runtime Rust crate inventory line in {}: {line}",
                inventory_path.display()
            )
        })?;
        let directory_name = format!("{name}-{version}");
        let mut package_dir = None;
        for registry in read_dir(registry_sources).with_context(|| {
            format!("read Cargo registry sources {}", registry_sources.display())
        })? {
            let candidate = registry?.path().join(&directory_name);
            if candidate.is_dir() {
                package_dir = Some(candidate);
                break;
            }
        }
        let package_dir = package_dir.with_context(|| {
            format!("runtime Rust crate source is unavailable: {name} {version}")
        })?;
        let mut license_files = Vec::new();
        for entry in read_dir(&package_dir)
            .with_context(|| format!("read Rust crate {}", package_dir.display()))?
        {
            let entry = entry.context("read Rust crate entry")?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let filename = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if filename.starts_with("license")
                || filename.starts_with("licence")
                || filename.starts_with("copying")
                || filename.starts_with("notice")
            {
                license_files.push(entry.path());
            }
        }
        license_files.sort();
        if license_files.is_empty() {
            if name == "v8" {
                continue;
            }
            bail!("runtime Rust crate has no license file: {name} {version}");
        }
        writeln!(notices, "\n--- {name} {version} ---")?;
        for path in license_files {
            writeln!(notices, "\n[{}]", path.file_name().unwrap().to_string_lossy())?;
            notices.push_str(
                &String::from_utf8(read(&path)?).with_context(|| {
                    format!("Rust license file is not UTF-8: {}", path.display())
                })?,
            );
            if !notices.ends_with('\n') {
                notices.push('\n');
            }
        }
    }
    Ok(())
}

fn append_rust_notices(notices: &mut String, source_root: &Path) -> Result<()> {
    let cargo_home = var_os("CARGO_HOME").map_or_else(
        || var_os("HOME").map(PathBuf::from).map(|home| home.join(".cargo")),
        |path| Some(PathBuf::from(path)),
    );
    let cargo_home = cargo_home.context("neither CARGO_HOME nor HOME is set")?;
    let registry_sources = cargo_home.join("registry/src");
    append_rust_inventory(
        notices,
        &registry_sources,
        &source_root.join("resources/RUNTIME_RUST_CRATES.txt"),
    )?;
    if var_os("CARGO_FEATURE_CLI").is_some() {
        append_rust_inventory(
            notices,
            &registry_sources,
            &source_root.join("resources/CLI_RUST_CRATES.txt"),
        )?;
    }
    Ok(())
}

pub fn generate_notices(
    root: &Path,
    source_root: &Path,
    out_dir: &Path,
    module_ids: &[PathBuf],
) -> Result<()> {
    let mut manifests = Vec::new();
    find_package_manifests(&root.join("node_modules/.deno"), &mut manifests)?;
    let mut packages = BTreeMap::new();
    for manifest_path in manifests {
        let manifest: Value = from_slice(
            &read(&manifest_path)
                .with_context(|| format!("read npm manifest {}", manifest_path.display()))?,
        )
        .with_context(|| format!("parse npm manifest {}", manifest_path.display()))?;
        let Some(name) = manifest.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(version) = manifest.get("version").and_then(Value::as_str) else {
            continue;
        };
        let Some(license) = manifest.get("license").and_then(Value::as_str).or_else(|| {
            manifest
                .get("licenses")
                .and_then(Value::as_array)
                .and_then(|licenses| licenses.first())
                .and_then(Value::as_object)
                .and_then(|license| license.get("type"))
                .and_then(Value::as_str)
        }) else {
            continue;
        };
        let package_dir = manifest_path
            .parent()
            .context("npm manifest has no parent directory")?;
        let canonical_package_dir = canonicalize(package_dir)
            .with_context(|| format!("canonicalize npm package {}", package_dir.display()))?;
        if !module_ids
            .iter()
            .any(|module_id| module_id.starts_with(&canonical_package_dir))
        {
            continue;
        }
        let author = manifest.get("author").and_then(|author| {
            author.as_str().or_else(|| {
                author
                    .as_object()
                    .and_then(|author| author.get("name"))
                    .and_then(Value::as_str)
            })
        });
        let repository = manifest.get("repository").and_then(|repository| {
            repository.as_str().or_else(|| {
                repository
                    .as_object()
                    .and_then(|repository| repository.get("url"))
                    .and_then(Value::as_str)
            })
        });
        let mut attribution = String::new();
        if let Some(author) = author {
            writeln!(attribution, "Author: {author}")?;
        }
        if let Some(repository) = repository {
            writeln!(attribution, "Source: {repository}")?;
        }
        packages.entry((name.to_owned(), version.to_owned())).or_insert((
            license.to_owned(),
            package_dir.to_path_buf(),
            attribution,
        ));
    }
    for module_id in module_ids {
        if module_id
            .components()
            .any(|component| component.as_os_str() == ".deno")
            && !packages.values().any(|(_, package_dir, _)| {
                module_id.starts_with(package_dir)
                    || canonicalize(package_dir)
                        .is_ok_and(|package_dir| module_id.starts_with(package_dir))
            })
        {
            bail!("bundled npm module has no package license metadata: {}", module_id.display());
        }
    }

    let mut notices = String::from(
        "THIRD-PARTY LICENSES AND NOTICES\n\nThis file is generated at build time from the locked dependencies and tracked attribution sources.\n\n",
    );
    notices.push_str("RUST AND V8 DEPENDENCIES\n========================\n\n");
    notices.push_str(
        &String::from_utf8(read(source_root.join("resources/RUST_THIRD_PARTY_NOTICES.txt"))?)
            .context("Rust third-party notices are not UTF-8")?,
    );
    append_rust_notices(&mut notices, source_root)?;
    notices.push_str("\n\nEMBEDDED NPM PACKAGES\n=====================\n");
    for ((name, version), (license, package_dir, attribution)) in packages {
        writeln!(notices, "\n--- {name} {version} ({license}) ---")?;
        notices.push_str(&attribution);
        let mut license_files = Vec::new();
        for entry in read_dir(&package_dir)
            .with_context(|| format!("read npm package {}", package_dir.display()))?
        {
            let entry = entry.context("read npm package entry")?;
            if !entry
                .file_type()
                .with_context(|| format!("read file type for {}", entry.path().display()))?
                .is_file()
            {
                continue;
            }
            let filename = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if filename.starts_with("license")
                || filename.starts_with("licence")
                || filename.starts_with("copying")
                || filename.starts_with("notice")
            {
                license_files.push(entry.path());
            }
        }
        license_files.sort();
        if license_files.is_empty() {
            notices.push_str(
                "The package does not ship a separate license file. See the identified license text elsewhere in this notice.\n",
            );
        }
        for path in license_files {
            writeln!(notices, "\n[{}]", path.file_name().unwrap().to_string_lossy())?;
            notices.push_str(
                &String::from_utf8(read(&path)?).with_context(|| {
                    format!("npm license file is not UTF-8: {}", path.display())
                })?,
            );
            if !notices.ends_with('\n') {
                notices.push('\n');
            }
        }
    }
    notices.push_str(
        "\nKUROMOJI DICTIONARY\n===================\n\n--- kuromoji 0.1.2 / LICENSE-2.0.txt ---\n",
    );
    notices.push_str(
        &String::from_utf8(read(source_root.join("resources/kuromoji/LICENSE-2.0.txt"))?)
            .context("Kuromoji license is not UTF-8")?,
    );
    notices.push_str("\n--- kuromoji 0.1.2 / NOTICE.md ---\n");
    notices.push_str(
        &String::from_utf8(read(source_root.join("resources/kuromoji/NOTICE.md"))?)
            .context("Kuromoji notice is not UTF-8")?,
    );
    write(out_dir.join("THIRD_PARTY_NOTICES.txt"), notices)
        .context("write embedded third-party notices")?;
    Ok(())
}

pub fn copy_dictionaries(root: &Path, out_dir: &Path) -> Result<()> {
    let source_dir = root.join("node_modules/kuromoji/dict");
    let output_dir = out_dir.join("kuromoji");
    if output_dir.exists() {
        remove_dir_all(&output_dir).with_context(|| format!("remove {}", output_dir.display()))?;
    }
    create_dir_all(&output_dir).with_context(|| format!("create {}", output_dir.display()))?;

    let mut copied = 0;
    for entry in read_dir(&source_dir)
        .with_context(|| format!("read Kuromoji dictionaries from {}", source_dir.display()))?
    {
        let entry = entry.context("read Kuromoji dictionary entry")?;
        let source = entry.path();
        if source.extension().is_none_or(|extension| extension != "gz") {
            continue;
        }
        let output = output_dir.join(entry.file_name());
        copy(&source, &output)
            .with_context(|| format!("copy {} to {}", source.display(), output.display()))?;
        copied += 1;
    }
    if copied == 0 {
        bail!("no Kuromoji dictionaries found in {}", source_dir.display());
    }
    Ok(())
}
