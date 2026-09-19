mod bundle;
mod docs;
mod notices;
mod npm;

use std::{
    env::var_os,
    fs::{copy, read, write},
    path::PathBuf,
};

use anyhow::{Context, Result, bail};
use bundle::{bundle, generate_registry, rule_packages};
use docs::write_placeholder_assets;
use notices::{copy_dictionaries, generate_notices};
use npm::{
    DENO_INSTALLER_SOURCE, install_dependencies, patch_legacy_style_format, prepare_workspace,
};
use serde_json::{Value, from_slice};
use tokio::runtime::Builder;

pub fn run() -> Result<()> {
    let root = var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .context("CARGO_MANIFEST_DIR is not set")?;
    let out_dir = var_os("OUT_DIR").map(PathBuf::from).context("OUT_DIR is not set")?;
    let config_path = root.join("textlint-v8.config.json");
    println!("cargo:rerun-if-env-changed=DOCS_RS");
    if var_os("DOCS_RS").is_some() {
        write_placeholder_assets(&out_dir)?;
        return Ok(());
    }
    let update_lockfile = match var_os("TEXTLINT_V8_UPDATE_LOCKFILE") {
        None => false,
        Some(value) if value == "1" => true,
        Some(value) => bail!(
            "TEXTLINT_V8_UPDATE_LOCKFILE must be exactly 1 when set, got {}",
            value.to_string_lossy()
        ),
    };

    println!("cargo:rerun-if-env-changed=TEXTLINT_V8_UPDATE_LOCKFILE");
    for path in ["build", "package.json", "deno.lock", "textlint-v8.config.json", "js", "resources"]
    {
        println!("cargo:rerun-if-changed={path}");
    }

    let config: Value = from_slice(
        &read(&config_path)
            .with_context(|| format!("read textlint config {}", config_path.display()))?,
    )
    .with_context(|| format!("parse textlint config {}", config_path.display()))?;
    let manifest: Value = from_slice(
        &read(root.join("package.json")).context("read package.json for rule registry")?,
    )
    .context("parse package.json for rule registry")?;
    let packages = rule_packages(&config, &manifest)?;
    let workspace = prepare_workspace(&root, &out_dir)?;
    let registry_path = generate_registry(&packages, &workspace)?;

    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .context("create build runtime")?;
    let bundle = runtime.block_on(async {
        install_dependencies(&workspace, update_lockfile).await?;
        patch_legacy_style_format(&workspace)?;
        bundle(&workspace, &config, &registry_path).await
    })?;

    if update_lockfile {
        copy(workspace.join("deno.lock"), root.join("deno.lock"))
            .context("copy explicitly updated Deno lockfile to the source tree")?;
    }
    copy_dictionaries(&workspace, &out_dir)?;
    generate_notices(&workspace, &root, &out_dir, &bundle.module_ids)?;
    let output = out_dir.join("textlint-v8.js");
    write(&output, bundle.bytes).with_context(|| format!("write {}", output.display()))?;
    println!("cargo:warning=npm dependencies installed with {DENO_INSTALLER_SOURCE}");
    Ok(())
}
