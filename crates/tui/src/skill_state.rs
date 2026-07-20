//! 运行时 API 技能列表的持久化启用/禁用状态。
//!
//! 支撑 `GET /v1/skills`（每个技能的 `enabled` 字段）和
//! `POST /v1/skills/{name}`（切换）。这与基于文件系统发现的
//! `SkillRegistry` 是分离的：注册表告诉我们磁盘上有哪些技能，
//! 而这个存储告诉 API 客户端哪些被标记为启用。
//!
//! 存储格式（TOML，位于 `~/.codewhale/skills_state.toml`，旧版 `~/.deepseek/skills_state.toml`）：
//!
//! ```toml
//! disabled = ["skill-name-1", "skill-name-2"]
//! ```
//!
//! 默认状态（文件不存在时）：空列表（所有技能启用）。
//! 损坏的文件会被记录并视为默认状态，这样升级操作就不会
//! 意外隐藏所有技能。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const STATE_FILE_NAME: &str = "skills_state.toml";

#[derive(Debug, Clone, Default)]
pub struct SkillStateStore {
    path: Option<PathBuf>,
    disabled: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct OnDiskState {
    #[serde(default)]
    disabled: Vec<String>,
}

impl SkillStateStore {
    pub fn load_default() -> Result<Self> {
        let path = default_state_path()?;
        Self::load_from(path)
    }

    pub fn load_from(path: PathBuf) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                path: Some(path),
                disabled: BTreeSet::new(),
            });
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("读取 {} 处的技能状态", path.display()))?;
        let parsed: OnDiskState = match toml::from_str(&raw) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(
                    "{} 处的 skills_state.toml 格式错误（{}）；将所有技能视为启用",
                    path.display(),
                    err
                );
                OnDiskState::default()
            }
        };

        Ok(Self {
            path: Some(path),
            disabled: parsed.disabled.into_iter().collect(),
        })
    }

    pub fn is_enabled(&self, skill_name: &str) -> bool {
        !self.disabled.contains(skill_name)
    }

    pub fn set_enabled(&mut self, skill_name: &str, enabled: bool) -> Result<()> {
        let changed = if enabled {
            self.disabled.remove(skill_name)
        } else {
            self.disabled.insert(skill_name.to_string())
        };
        if !changed {
            return Ok(());
        }
        self.persist()
    }

    #[allow(dead_code)]
    pub fn disabled(&self) -> Vec<String> {
        self.disabled.iter().cloned().collect()
    }

    fn persist(&self) -> Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        let on_disk = OnDiskState {
            disabled: self.disabled.iter().cloned().collect(),
        };
        let body = toml::to_string_pretty(&on_disk).context("序列化技能状态")?;
        atomic_write(path, body.as_bytes())
    }
}

fn default_state_path() -> Result<PathBuf> {
    let dir = codewhale_config::ensure_state_dir(".")
        .context("无法解析或创建 CodeWhale 状态目录")?;
    Ok(dir.join(STATE_FILE_NAME))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("为 {} 创建父目录", path.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, bytes).with_context(|| format!("写入临时文件 {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("重命名临时文件为 {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh() -> (TempDir, SkillStateStore) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(STATE_FILE_NAME);
        let store = SkillStateStore::load_from(path).unwrap();
        (dir, store)
    }

    #[test]
    fn missing_file_defaults_to_everything_enabled() {
        let (_dir, store) = fresh();
        assert!(store.is_enabled("anything"));
        assert!(store.disabled().is_empty());
    }

    #[test]
    fn disable_then_reload_persists() {
        let (dir, mut store) = fresh();
        store.set_enabled("foo", false).unwrap();
        assert!(!store.is_enabled("foo"));

        let reloaded = SkillStateStore::load_from(dir.path().join(STATE_FILE_NAME)).unwrap();
        assert!(!reloaded.is_enabled("foo"));
        assert!(reloaded.is_enabled("bar"));
    }

    #[test]
    fn enable_removes_from_disabled_list() {
        let (_dir, mut store) = fresh();
        store.set_enabled("foo", false).unwrap();
        store.set_enabled("foo", true).unwrap();
        assert!(store.is_enabled("foo"));
        assert!(store.disabled().is_empty());
    }

    #[test]
    fn redundant_toggle_is_noop() {
        let (_dir, mut store) = fresh();
        store.set_enabled("foo", true).unwrap();
        assert!(store.disabled().is_empty());
    }

    #[test]
    fn malformed_file_falls_back_to_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(STATE_FILE_NAME);
        fs::write(&path, b"this is not toml = { broken").unwrap();
        let store = SkillStateStore::load_from(path).unwrap();
        assert!(store.is_enabled("anything"));
    }

    #[test]
    fn disabled_list_is_deterministic_order() {
        let (_dir, mut store) = fresh();
        store.set_enabled("zeta", false).unwrap();
        store.set_enabled("alpha", false).unwrap();
        store.set_enabled("mu", false).unwrap();
        assert_eq!(
            store.disabled(),
            vec!["alpha".to_string(), "mu".to_string(), "zeta".to_string()]
        );
    }
}
