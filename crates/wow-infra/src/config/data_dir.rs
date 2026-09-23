use std::{env, path::{Path, PathBuf}};

const APP_DIR_NAME: &str = "wow-bot-data";
const WORKSPACE_MANIFEST: &str = "Cargo.toml";

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub root: PathBuf,
    pub config: PathBuf,
    pub logs: PathBuf,
    pub generated: PathBuf,
    pub cache: PathBuf,
    pub state: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self, String> {
        let root = if let Some(value) = env::var_os("WOW_BOT_HOME") {
            PathBuf::from(value)
        } else {
            default_repo_data_root()?
        };
        Ok(Self::from_root(root))
    }

    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            config: root.join("config.json"),
            logs: root.join("logs"),
            generated: root.join("generated"),
            cache: root.join("cache"),
            state: root.join("state"),
            root,
        }
    }

    pub fn ensure(&self) -> Result<(), String> {
        for path in [&self.root, &self.logs, &self.generated, &self.cache, &self.state] {
            #[cfg(unix)]
            crate::security::paths::secure_private_dir(path)
                .map_err(|e| format!("failed to secure bot-owned directory {}: {e}", path.display()))?;
            #[cfg(not(unix))]
            std::fs::create_dir_all(path)
                .map_err(|e| format!("failed to create bot-owned directory {}: {e}", path.display()))?;
        }
        Ok(())
    }
}

pub fn resolve(root: impl AsRef<Path>) -> PathBuf { root.as_ref().to_path_buf() }

fn default_repo_data_root() -> Result<PathBuf, String> {
    discover_workspace_root()
        .map(|root| root.join(APP_DIR_NAME))
        .ok_or_else(|| {
            "unable to locate the wow-bot repository root; run the binary from the repository, or set WOW_BOT_HOME to a writable directory".to_owned()
        })
}

fn discover_workspace_root() -> Option<PathBuf> {
    if let Ok(current) = env::current_dir() {
        if let Some(root) = find_workspace_root(&current) {
            return Some(root);
        }
    }

    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            if let Some(root) = find_workspace_root(parent) {
                return Some(root);
            }
        }
    }

    // This is useful for normal workspace builds. It is only a fallback because
    // the repository may be moved after compilation.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    find_workspace_root(&manifest_dir)
}

fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    for candidate in start.ancestors() {
        let manifest = candidate.join(WORKSPACE_MANIFEST);
        if is_wow_bot_workspace(&manifest) {
            return Some(candidate.to_path_buf());
        }
    }
    None
}

fn is_wow_bot_workspace(manifest: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(manifest) else { return false; };
    text.contains("[workspace]")
        && text.contains("apps/wow-supervisor")
        && text.contains("crates/wow-infra")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_paths_use_repo_local_layout() {
        let paths = AppPaths::from_root(PathBuf::from("/repo/wow-bot-data"));
        assert_eq!(paths.config, PathBuf::from("/repo/wow-bot-data/config.json"));
        assert_eq!(paths.logs, PathBuf::from("/repo/wow-bot-data/logs"));
        assert_eq!(paths.generated, PathBuf::from("/repo/wow-bot-data/generated"));
        assert_eq!(paths.cache, PathBuf::from("/repo/wow-bot-data/cache"));
        assert_eq!(paths.state, PathBuf::from("/repo/wow-bot-data/state"));
    }

    #[test]
    fn workspace_manifest_detection_requires_expected_members() {
        let root = std::env::temp_dir().join(format!("wow-bot-workspace-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("apps/wow-supervisor")).expect("create test dir");
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"apps/wow-supervisor\", \"crates/wow-infra\"]\n",
        ).expect("write manifest");
        assert_eq!(find_workspace_root(&root.join("apps/wow-supervisor")), Some(root.clone()));
        let _ = std::fs::remove_dir_all(root);
    }
}
