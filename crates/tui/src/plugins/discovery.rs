use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::manifest::{LoadedPlugin, PluginManifest};
use super::registry::PluginRegistry;

const PLUGIN_MANIFEST: &str = "plugin.toml";
const OVERRIDES_FILE: &str = "overrides.json";

pub fn default_user_plugins_dir() -> PathBuf {
    codewhale_config::codewhale_home()
        .map(|p| p.join("plugins"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/codewhale/plugins"))
}

/// 记录 `/plugin enable|disable` 选择的 JSON 文件路径，以便在重启后仍然保留。
pub fn default_overrides_path() -> PathBuf {
    default_user_plugins_dir().join(OVERRIDES_FILE)
}

/// 读取已持久化的启用/禁用覆盖记录。文件缺失或格式错误时返回空映射——
/// 用户只需获得默认的启用状态。
pub fn load_overrides(path: &Path) -> HashMap<String, bool> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

/// 持久化启用/禁用覆盖记录，必要时创建父目录。
pub fn save_overrides(path: &Path, overrides: &HashMap<String, bool>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(overrides)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, json)
}

pub fn discover_all(builtin_dirs: &[&str]) -> PluginRegistry {
    let mut registry = PluginRegistry::new();

    let overrides_path = default_overrides_path();
    let overrides = load_overrides(&overrides_path);
    registry.set_overrides_store(overrides_path, overrides);

    for dir in builtin_dirs {
        let path = PathBuf::from(dir);
        if path.exists() {
            discover_from_dir(&path, &mut registry, true);
        }
    }

    let user_dir = default_user_plugins_dir();
    if user_dir.exists() {
        discover_from_dir(&user_dir, &mut registry, false);
    }

    // Discovery 从 `!builtin` 重新计算 `enabled`；
    // 在此重新应用用户已持久化的选择，以便之前的 enable/disable 实际生效 (#3918)。
    registry.apply_overrides();

    registry
}

fn discover_from_dir(dir: &Path, registry: &mut PluginRegistry, builtin: bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let manifest_path = path.join(PLUGIN_MANIFEST);
        if !manifest_path.exists() {
            continue;
        }

        match PluginManifest::from_path(&manifest_path) {
            Ok(manifest) => {
                if !manifest.check_when() {
                    continue;
                }

                let name = manifest.plugin.name.clone();
                let plugin = LoadedPlugin {
                    manifest,
                    base_path: path,
                    enabled: !builtin,
                };

                registry.register(name, plugin);
            }
            Err(e) => {
                tracing::warn!("Failed to load plugin from {}: {}", path.display(), e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_user_plugins_dir_uses_explicit_codewhale_home() {
        let _env_lock = crate::test_support::lock_test_env();
        let tmp = TempDir::new().expect("tempdir");
        let home = tmp.path().join("codewhale-home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());

        assert_eq!(default_user_plugins_dir(), home.join("plugins"));
        assert_eq!(
            default_overrides_path(),
            home.join("plugins").join(OVERRIDES_FILE)
        );
    }
}
