use std::{
    collections::BTreeSet,
    env::current_dir,
    fs::{canonicalize, read_dir, read_to_string},
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail};
use glob::{MatchOptions, glob_with};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

const EXTENSIONS: [&str; 8] = ["md", "markdown", "mdown", "mkdn", "mkd", "mdwn", "mkdown", "ron"];

pub(crate) fn resolve(paths: &[PathBuf], ignore_path: Option<&Path>) -> Result<Vec<PathBuf>> {
    let cwd = current_dir().context("failed to determine the current directory")?;
    resolve_from(paths, &cwd, ignore_path)
}

fn resolve_from(paths: &[PathBuf], cwd: &Path, ignore_path: Option<&Path>) -> Result<Vec<PathBuf>> {
    let mut files = BTreeSet::new();

    for target in paths {
        if target.exists() {
            collect(target, &mut files)?;
            continue;
        }

        let pattern = target
            .to_str()
            .with_context(|| format!("glob pattern is not valid UTF-8: {}", target.display()))?;
        let options = MatchOptions {
            require_literal_leading_dot: false,
            ..MatchOptions::new()
        };
        for entry in glob_with(pattern, options)
            .with_context(|| format!("invalid glob pattern: {pattern}"))?
        {
            collect(
                &entry.with_context(|| format!("failed to expand glob pattern: {pattern}"))?,
                &mut files,
            )?;
        }
    }

    if files.is_empty() {
        bail!("No files matching the pattern were found")
    }

    let ignores = load_ignores(cwd, ignore_path)?;
    files.retain(|file| !ignores.is_match(file.strip_prefix(cwd).unwrap_or(file)));

    Ok(files.into_iter().collect())
}

fn load_ignores(cwd: &Path, ignore_path: Option<&Path>) -> Result<GlobSet> {
    let path = ignore_path.map_or_else(|| cwd.join(".textlintignore"), Path::to_path_buf);
    let contents = match read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let mut builder = GlobSetBuilder::new();
    for pattern in contents
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim().starts_with('#'))
    {
        builder.add(
            GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .with_context(|| {
                    format!("invalid ignore pattern in {}: {pattern}", path.display())
                })?,
        );
    }
    builder
        .build()
        .with_context(|| format!("failed to compile ignore patterns from {}", path.display()))
}

fn collect(path: &Path, files: &mut BTreeSet<PathBuf>) -> Result<()> {
    if is_always_ignored(path) {
        return Ok(());
    }

    if path.is_file() {
        if is_supported(path) {
            files.insert(
                canonicalize(path)
                    .with_context(|| format!("failed to resolve {}", path.display()))?,
            );
        }
        return Ok(());
    }

    if path.is_dir() {
        let entries = read_dir(path)
            .with_context(|| format!("failed to read directory {}", path.display()))?;
        for entry in entries {
            collect(&entry?.path(), files)?;
        }
    }

    Ok(())
}

fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| EXTENSIONS.contains(&extension))
}

fn is_always_ignored(path: &Path) -> bool {
    path.components().any(|component| {
        let component = component.as_os_str();
        component == ".git" || component == "node_modules"
    })
}

#[cfg(test)]
mod tests {
    #[cfg(test)]
    use std::env::temp_dir;
    #[cfg(test)]
    use std::fs::create_dir;
    #[cfg(test)]
    use std::fs::create_dir_all;
    #[cfg(test)]
    use std::fs::remove_dir_all;
    #[cfg(test)]
    use std::slice::from_ref;
    use std::{fs, time::SystemTime};

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = temp_dir().join(format!("textlint-v8-{unique}"));
            create_dir(&path).unwrap();
            Self(path)
        }

        fn write(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "text").unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn recursively_resolves_supported_directory_entries_including_dotfiles() {
        let temp = TempDir::new();
        let markdown = temp.write("nested/document.markdown");
        let dotfile = temp.write(".hidden.md");
        temp.write("nested/unsupported.txt");
        temp.write("node_modules/dependency.md");
        temp.write(".git/internal.md");

        let actual = resolve_from(from_ref(&temp.0), &temp.0, None).unwrap();

        assert_eq!(actual, [canonicalize(dotfile).unwrap(), canonicalize(markdown).unwrap()]);
    }

    #[test]
    fn expands_recursive_globs_and_deduplicates_files() {
        let temp = TempDir::new();
        let root = temp.write("root.md");
        let nested = temp.write("nested/document.md");
        temp.write("nested/document.ron");
        let pattern = temp.0.join("**/*.md");

        let actual = resolve_from(&[pattern, nested.clone()], &temp.0, None).unwrap();

        assert_eq!(actual, [canonicalize(nested).unwrap(), canonicalize(root).unwrap()]);
    }

    #[test]
    fn processes_matches_when_another_target_is_unmatched() {
        let temp = TempDir::new();
        let matched = temp.write("document.md");

        let actual =
            resolve_from(&[temp.0.join("missing-*.md"), matched.clone()], &temp.0, None).unwrap();

        assert_eq!(actual, [canonicalize(matched).unwrap()]);
    }

    #[test]
    fn rejects_targets_without_any_matches() {
        let temp = TempDir::new();

        let error = resolve_from(&[temp.0.join("missing-*.md")], &temp.0, None).unwrap_err();

        assert_eq!(error.to_string(), "No files matching the pattern were found");
    }

    #[test]
    fn applies_ignore_patterns_to_explicit_and_discovered_files() {
        let temp = TempDir::new();
        let kept = temp.write("nested/kept.md");
        let ignored = temp.write("nested/ignored.md");
        temp.write("ignored/child.md");
        fs::write(temp.0.join(".textlintignore"), "# comment\n\nnested/ignored.md\nignored/**\n")
            .unwrap();

        let actual = resolve_from(&[temp.0.clone(), ignored], &temp.0, None).unwrap();

        assert_eq!(actual, [canonicalize(kept).unwrap()]);
    }

    #[test]
    fn accepts_all_targets_being_ignored() {
        let temp = TempDir::new();
        let ignored = temp.write("ignored.md");
        fs::write(temp.0.join(".textlintignore"), "*.md\n").unwrap();

        let actual = resolve_from(&[ignored], &temp.0, None).unwrap();

        assert!(actual.is_empty());
    }

    #[test]
    fn resolves_custom_ignore_paths_against_the_working_directory() {
        let temp = TempDir::new();
        let ignored = temp.write("nested/ignored.md");
        let kept = temp.write("custom/kept.md");
        let ignore_path = temp.0.join("custom/ignore-patterns");
        fs::write(&ignore_path, "nested/**\n").unwrap();

        let actual = resolve_from(from_ref(&temp.0), &temp.0, Some(&ignore_path)).unwrap();

        assert_eq!(actual, [canonicalize(kept).unwrap()]);
        assert!(!actual.contains(&canonicalize(ignored).unwrap()));
    }

    #[test]
    fn tolerates_a_missing_custom_ignore_path() {
        let temp = TempDir::new();
        let document = temp.write("document.md");

        let actual =
            resolve_from(from_ref(&document), &temp.0, Some(&temp.0.join("missing-ignore-file")))
                .unwrap();

        assert_eq!(actual, [canonicalize(document).unwrap()]);
    }
}
