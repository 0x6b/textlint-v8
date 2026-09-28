use std::collections::{BTreeMap, HashMap, VecDeque};

use anyhow::{Context, Result};
use node_semver::{Range, Version};
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct Lockfile {
    pub version: u8,
    pub roots: BTreeMap<String, String>,
    pub packages: BTreeMap<String, LockedPackage>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub resolved: String,
    pub integrity: String,
    pub dependencies: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageManifest {
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default)]
    dev_dependencies: BTreeMap<String, String>,
}

impl PackageManifest {
    pub fn requirements(&self) -> BTreeMap<String, String> {
        self.dependencies
            .iter()
            .chain(&self.dev_dependencies)
            .map(|(name, version)| (name.clone(), version.clone()))
            .collect()
    }
}

#[derive(Clone, Deserialize)]
struct RegistryPackage {
    versions: BTreeMap<String, RegistryVersion>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryVersion {
    version: String,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default)]
    optional_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    peer_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    peer_dependencies_meta: BTreeMap<String, PeerDependencyMeta>,
    dist: RegistryDist,
}

#[derive(Clone, Deserialize)]
struct RegistryDist {
    tarball: String,
    integrity: Option<String>,
}

#[derive(Clone, Deserialize)]
struct PeerDependencyMeta {
    #[serde(default)]
    optional: bool,
}

struct PendingDependency {
    parent: Option<String>,
    name: String,
    requirement: String,
}

async fn fetch_registry_package(client: &Client, name: &str) -> Result<RegistryPackage> {
    let mut url = reqwest::Url::parse("https://registry.npmjs.org/")?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("npm registry URL cannot be a base URL"))?
        .push(name);
    client
        .get(url)
        .header("Accept", "application/vnd.npm.install-v1+json")
        .send()
        .await
        .with_context(|| format!("fetch npm metadata for {name}"))?
        .error_for_status()
        .with_context(|| format!("fetch npm metadata for {name}"))?
        .json()
        .await
        .with_context(|| format!("parse npm metadata for {name}"))
}

fn select_registry_version(
    name: &str,
    requirement: &str,
    package: &RegistryPackage,
) -> Result<RegistryVersion> {
    let range = Range::parse(requirement)
        .with_context(|| format!("unsupported npm version requirement {name}@{requirement}"))?;
    package
        .versions
        .values()
        .filter_map(|candidate| {
            Version::parse(&candidate.version)
                .ok()
                .filter(|version| range.satisfies(version))
                .map(|version| (version, candidate))
        })
        .max_by(|(left, _), (right, _)| left.cmp(right))
        .map(|(_, candidate)| candidate.clone())
        .with_context(|| format!("no npm version satisfies {name}@{requirement}"))
}

pub async fn resolve_lockfile(client: &Client, manifest: &PackageManifest) -> Result<Lockfile> {
    let mut lock = Lockfile {
        version: 1,
        roots: BTreeMap::new(),
        packages: BTreeMap::new(),
    };
    let mut metadata = HashMap::<String, RegistryPackage>::new();
    let mut resolutions = HashMap::<(String, String), String>::new();
    let mut pending = manifest
        .requirements()
        .into_iter()
        .map(|(name, requirement)| PendingDependency {
            parent: None,
            name,
            requirement,
        })
        .collect::<VecDeque<_>>();

    while let Some(dependency) = pending.pop_front() {
        let request = (dependency.name.clone(), dependency.requirement.clone());
        let (id, selected) = if let Some(id) = resolutions.get(&request) {
            (id.clone(), None)
        } else {
            if !metadata.contains_key(&dependency.name) {
                metadata.insert(
                    dependency.name.clone(),
                    fetch_registry_package(client, &dependency.name).await?,
                );
            }
            let selected = select_registry_version(
                &dependency.name,
                &dependency.requirement,
                &metadata[&dependency.name],
            )?;
            let id = format!("{}@{}", dependency.name, selected.version);
            resolutions.insert(request, id.clone());
            (id, Some(selected))
        };

        if let Some(parent) = &dependency.parent {
            lock.packages
                .get_mut(parent)
                .with_context(|| format!("parent npm package is missing from lock: {parent}"))?
                .dependencies
                .insert(dependency.name.clone(), id.clone());
        } else {
            lock.roots.insert(dependency.name.clone(), id.clone());
        }

        let Some(selected) = selected else {
            continue;
        };
        if lock.packages.contains_key(&id) {
            continue;
        }
        let mut requirements = selected.dependencies.clone();
        requirements.extend(selected.optional_dependencies.clone());
        requirements.extend(
            selected
                .peer_dependencies
                .iter()
                .filter(|(name, _)| {
                    !selected
                        .peer_dependencies_meta
                        .get(*name)
                        .is_some_and(|metadata| metadata.optional)
                })
                .map(|(name, requirement)| (name.clone(), requirement.clone())),
        );
        let integrity = selected
            .dist
            .integrity
            .with_context(|| format!("npm package {id} has no sha512 integrity"))?;
        lock.packages.insert(
            id.clone(),
            LockedPackage {
                name: dependency.name,
                version: selected.version,
                resolved: selected.dist.tarball,
                integrity,
                dependencies: BTreeMap::new(),
            },
        );
        pending.extend(
            requirements
                .into_iter()
                .map(|(name, requirement)| PendingDependency {
                    parent: Some(id.clone()),
                    name,
                    requirement,
                }),
        );
    }
    Ok(lock)
}
