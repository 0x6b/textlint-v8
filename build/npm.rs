use std::{
    fs::{copy, create_dir_all, read_dir, remove_dir_all},
    path::{Path, PathBuf},
    result,
    sync::Arc,
};

use anyhow::{Context, Result};
use deno_config::deno_json::NodeModulesDirMode;
use deno_error::JsErrorBox;
use deno_npm_cache::{
    DownloadError, NpmCacheHttpClient, NpmCacheHttpClientBytesResponse, NpmCacheHttpClientResponse,
    NpmCacheSetting,
};
use deno_npm_installer::{
    LifecycleScriptsConfig, LogReporter, NpmInstallerFactory, NpmInstallerFactoryOptions,
    PackageCaching, graph::NpmCachingStrategy, lifecycle_scripts::NullLifecycleScriptsExecutor,
};
use deno_npmrc::RegistryConfig;
use deno_resolver::factory::{
    ConfigDiscoveryOption, ResolverFactory, ResolverFactoryOptions, WorkspaceFactory,
    WorkspaceFactoryOptions,
};
use reqwest::{Client, Response};
use sys_traits::impls::RealSys;
use url::Url;

pub const DENO_INSTALLER_SOURCE: &str = "deno_npm_installer@0.54.0";

#[derive(Debug)]
struct HttpClient(Client);

#[async_trait::async_trait(?Send)]
impl NpmCacheHttpClient for HttpClient {
    async fn download_with_retries_on_any_tokio_runtime(
        &self,
        url: Url,
        _maybe_auth: Option<String>,
        _maybe_etag: Option<String>,
        _registry_config: Option<&RegistryConfig>,
    ) -> result::Result<NpmCacheHttpClientResponse, DownloadError> {
        let response = self
            .0
            .get(url)
            .send()
            .await
            .and_then(Response::error_for_status)
            .map_err(|error| DownloadError {
                status_code: error.status().map(|status| status.as_u16()),
                error: JsErrorBox::generic(error.to_string()),
            })?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| DownloadError {
                status_code: Some(status.as_u16()),
                error: JsErrorBox::generic(error.to_string()),
            })?
            .to_vec();
        Ok(NpmCacheHttpClientResponse::Bytes(NpmCacheHttpClientBytesResponse { bytes, etag: None }))
    }
}

pub async fn install_dependencies(root: &Path, update_lockfile: bool) -> Result<()> {
    let deno_dir = root
        .parent()
        .context("build workspace has no parent directory")?
        .join("textlint-v8-deno-dir");
    let workspace_factory = Arc::new(WorkspaceFactory::new(
        RealSys,
        root.to_path_buf(),
        WorkspaceFactoryOptions {
            config_discovery: ConfigDiscoveryOption::Disabled,
            is_package_manager_subcommand: true,
            frozen_lockfile: Some(!update_lockfile),
            lock_arg: Some(root.join("deno.lock")),
            maybe_custom_deno_dir_root: Some(deno_dir),
            node_modules_dir: Some(NodeModulesDirMode::Auto),
            ..Default::default()
        },
    ));
    let resolver_factory =
        Arc::new(ResolverFactory::new(workspace_factory, ResolverFactoryOptions::default()));
    let factory = NpmInstallerFactory::new(
        resolver_factory,
        Arc::new(HttpClient(Client::new())),
        Arc::new(NullLifecycleScriptsExecutor),
        LogReporter,
        None,
        NpmInstallerFactoryOptions {
            cache_setting: NpmCacheSetting::Use,
            caching_strategy: NpmCachingStrategy::Eager,
            clean_on_install: true,
            dedup_lockfile_peer_variants: true,
            lifecycle_scripts_config: LifecycleScriptsConfig {
                initial_cwd: root.to_path_buf(),
                root_dir: root.to_path_buf(),
                explicit_install: true,
                ..Default::default()
            },
            production: false,
            resolve_npm_resolution_snapshot: Box::new(|| Ok(None)),
            skip_types: false,
        },
    );
    let installer = factory.npm_installer().await?;
    installer.ensure_no_pkg_json_dep_errors()?;
    installer.ensure_top_level_package_json_install().await?;
    installer.cache_packages(PackageCaching::All).await?;
    factory
        .maybe_lockfile()
        .await?
        .context("Deno did not open deno.lock")?
        .write_if_changed()?;

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
    for filename in ["package.json", "deno.lock"] {
        copy(source_root.join(filename), workspace.join(filename))
            .with_context(|| format!("copy {filename} into the build workspace"))?;
    }
    copy_tree(&source_root.join("js"), &workspace.join("js"))?;
    Ok(workspace)
}
