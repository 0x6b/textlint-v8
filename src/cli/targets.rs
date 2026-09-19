use std::{
    collections::BTreeSet,
    fs::{canonicalize, read_dir},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail};
use glob::{MatchOptions, glob_with};

const EXTENSIONS: [&str; 8] = ["md", "markdown", "mdown", "mkdn", "mkd", "mdwn", "mkdown", "ron"];

pub(crate) fn resolve(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
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

    Ok(files.into_iter().collect())
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

        let actual = resolve(from_ref(&temp.0)).unwrap();

        assert_eq!(actual, [canonicalize(dotfile).unwrap(), canonicalize(markdown).unwrap()]);
    }

    #[test]
    fn expands_recursive_globs_and_deduplicates_files() {
        let temp = TempDir::new();
        let root = temp.write("root.md");
        let nested = temp.write("nested/document.md");
        temp.write("nested/document.ron");
        let pattern = temp.0.join("**/*.md");

        let actual = resolve(&[pattern, nested.clone()]).unwrap();

        assert_eq!(actual, [canonicalize(nested).unwrap(), canonicalize(root).unwrap()]);
    }

    #[test]
    fn processes_matches_when_another_target_is_unmatched() {
        let temp = TempDir::new();
        let matched = temp.write("document.md");

        let actual = resolve(&[temp.0.join("missing-*.md"), matched.clone()]).unwrap();

        assert_eq!(actual, [canonicalize(matched).unwrap()]);
    }

    #[test]
    fn rejects_targets_without_any_matches() {
        let temp = TempDir::new();

        let error = resolve(&[temp.0.join("missing-*.md")]).unwrap_err();

        assert_eq!(error.to_string(), "No files matching the pattern were found");
    }
}
