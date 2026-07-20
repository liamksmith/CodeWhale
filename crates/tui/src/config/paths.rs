//! 用于配置/缓存/工作区位置的文件系统路径解析辅助函数。
//!
//! 纯路径构建辅助函数，从 `config.rs` 原样提取。它们仅依赖于
//! `std`、`dirs` 和 `shellexpand` 以及彼此，因此构成了一个
//! 干净的叶子模块。`config.rs` 通过 `use paths::{...}` 将其拉回，
//! 用于仍保留在那里的工作区信任和配置加载逻辑，并重新导出
//! 两个 `pub(crate)` 入口点（`effective_home_dir`、`expand_path`），
//! 以便外部的 `crate::config::` 调用者保持不变（#3311）。
//!
//! 可见性说明：在 `config.rs` 中为文件私有 `fn` 的辅助函数
//! 在此处为 `pub(crate)`，纯粹为了让父模块能够引用它们；
//! 没有一个被公开重新导出，因此 crate 的外部表面不变。

use std::path::{Path, PathBuf};

pub(crate) fn default_config_path() -> Option<PathBuf> {
    env_config_path().or_else(home_config_path)
}

pub(crate) fn codewhale_home_dir() -> Option<PathBuf> {
    std::env::var_os("CODEWHALE_HOME").and_then(|path| {
        let path = PathBuf::from(path);
        (!path.as_os_str().is_empty()).then_some(path)
    })
}

pub(crate) fn effective_home_dir() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("HOME") {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }

    if let Some(path) = std::env::var_os("USERPROFILE") {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }

    #[cfg(windows)]
    {
        if let (Some(drive), Some(homepath)) =
            (std::env::var_os("HOMEDRIVE"), std::env::var_os("HOMEPATH"))
        {
            let mut path = PathBuf::from(drive);
            path.push(homepath);
            if !path.as_os_str().is_empty() {
                return Some(path);
            }
        }
    }

    dirs::home_dir()
}

pub(crate) fn home_config_path() -> Option<PathBuf> {
    if let Some(home) = codewhale_home_dir() {
        return Some(home.join("config.toml"));
    }

    effective_home_dir().map(|home| {
        let primary = home.join(".codewhale").join("config.toml");
        if primary.exists() {
            return primary;
        }
        let legacy = home.join(".deepseek").join("config.toml");
        if legacy.exists() {
            return legacy;
        }
        primary
    })
}

pub(crate) fn workspace_config_key(workspace: &Path) -> String {
    canonicalize_or_keep(workspace)
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn canonicalize_or_keep(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn env_config_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("CODEWHALE_CONFIG_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(expand_path(trimmed));
        }
    }
    if let Ok(path) = std::env::var("DEEPSEEK_CONFIG_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(expand_path(trimmed));
        }
    }
    None
}

pub(crate) fn expand_pathbuf(path: PathBuf) -> PathBuf {
    if let Some(raw) = path.to_str() {
        return expand_path(raw);
    }
    path
}

pub(crate) fn default_managed_config_path() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        Some(PathBuf::from("/etc/deepseek/managed_config.toml"))
    }
    #[cfg(not(unix))]
    {
        effective_home_dir().map(|home| {
            let primary = home.join(".codewhale").join("managed_config.toml");
            if primary.exists() {
                return primary;
            }
            home.join(".deepseek").join("managed_config.toml")
        })
    }
}

pub(crate) fn default_requirements_path() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        Some(PathBuf::from("/etc/deepseek/requirements.toml"))
    }
    #[cfg(not(unix))]
    {
        effective_home_dir().map(|home| {
            let primary = home.join(".codewhale").join("requirements.toml");
            if primary.exists() {
                return primary;
            }
            home.join(".deepseek").join("requirements.toml")
        })
    }
}

pub(crate) fn expand_path(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix('~')
        && (stripped.is_empty() || stripped.starts_with('/') || stripped.starts_with('\\'))
        && let Some(mut home) = effective_home_dir()
    {
        let suffix = stripped.trim_start_matches(['/', '\\']);
        if !suffix.is_empty() {
            home.push(suffix);
        }
        return home;
    }

    let expanded = shellexpand::tilde(path);
    PathBuf::from(expanded.as_ref())
}

pub(crate) fn default_skills_dir() -> Option<PathBuf> {
    effective_home_dir().map(|home| home.join(".codewhale").join("skills"))
}

pub(crate) fn default_mcp_config_path() -> Option<PathBuf> {
    effective_home_dir().map(|home| {
        let primary = home.join(".codewhale").join("mcp.json");
        if primary.exists() {
            return primary;
        }
        let legacy = home.join(".deepseek").join("mcp.json");
        if legacy.exists() {
            return legacy;
        }
        primary
    })
}

pub(crate) fn default_notes_path() -> Option<PathBuf> {
    effective_home_dir().map(|home| {
        let primary = home.join(".codewhale").join("notes.txt");
        if primary.exists() {
            return primary;
        }
        let legacy = home.join(".deepseek").join("notes.txt");
        if legacy.exists() {
            return legacy;
        }
        primary
    })
}

pub(crate) fn default_memory_path() -> Option<PathBuf> {
    effective_home_dir().map(|home| {
        let primary = home.join(".codewhale").join("memory.md");
        if primary.exists() {
            return primary;
        }
        let legacy = home.join(".deepseek").join("memory.md");
        if legacy.exists() {
            return legacy;
        }
        primary
    })
}
