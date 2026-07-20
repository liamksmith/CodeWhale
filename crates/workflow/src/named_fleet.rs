//! 命名舰队名册文件，用于日志车道（#4178）。
//!
//! 格式：`fleets/<name>.toml`（工作区）或
//! `$CODEWHALE_HOME/fleets/<name>.toml` 中的 TOML。
//!
//! Fleet 仅将角色解析为配置档案 ID。运行时拥有 tmux/工作树。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 已解析的命名舰队文件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedFleet {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// 角色名称 → AgentProfile id
    pub roles: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NamedFleetError {
    #[error("舰队文件未找到：{0}")]
    NotFound(String),
    #[error("读取舰队文件 {path} 失败：{message}")]
    Io { path: String, message: String },
    #[error("解析舰队文件 {path} 失败：{message}")]
    Parse { path: String, message: String },
    #[error("舰队 `{fleet}` 缺少必需的角色 `{role}`")]
    MissingRole { fleet: String, role: String },
    #[error("舰队名称不匹配：文件声明为 `{declared}`，期望 `{expected}`")]
    NameMismatch { declared: String, expected: String },
}

/// 停止运输日志舰队所需的角色（#4178）。
pub const STOPSHIP_REQUIRED_ROLES: &[&str] = &[
    "scout",
    "implementer",
    "reviewer",
    "verifier",
    "release_lead",
];

/// 解析舰队 TOML 文档。
pub fn parse_named_fleet(toml_text: &str) -> Result<NamedFleet, NamedFleetError> {
    // 最简 TOML 子集，无需向 workflow 添加 toml 依赖：
    // 对于测试也接受 JSON；对于 TOML 使用针对文档化形状的
    // 微型手写解析器，或通过 serde json 进行单元测试。
    // 如果文本看起来像 JSON 则优先使用 JSON；否则使用面向行的 TOML。
    let trimmed = toml_text.trim();
    if trimmed.starts_with('{') {
        return serde_json::from_str(trimmed).map_err(|e| NamedFleetError::Parse {
            path: "<memory>".into(),
            message: e.to_string(),
        });
    }
    parse_fleet_toml_minimal(trimmed)
}

fn parse_fleet_toml_minimal(text: &str) -> Result<NamedFleet, NamedFleetError> {
    let mut name = None;
    let mut description = None;
    let mut roles = BTreeMap::new();
    let mut section = "";
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1];
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"').to_string();
        match section {
            "" => match key {
                "name" => name = Some(value),
                "description" => description = Some(value),
                _ => {}
            },
            "roles" => {
                roles.insert(key.to_string(), value);
            }
            _ => {}
        }
    }
    let name = name.ok_or_else(|| NamedFleetError::Parse {
        path: "<memory>".into(),
        message: "缺少名称".into(),
    })?;
    Ok(NamedFleet {
        name,
        description,
        roles,
    })
}

/// 按名称从搜索路径加载舰队（首个命中获胜）。
pub fn load_named_fleet(
    name: &str,
    search_roots: &[PathBuf],
) -> Result<NamedFleet, NamedFleetError> {
    let file_name = format!("{name}.toml");
    for root in search_roots {
        let path = root.join("fleets").join(&file_name);
        if path.is_file() {
            return load_named_fleet_file(&path, Some(name));
        }
    }
    Err(NamedFleetError::NotFound(name.to_string()))
}

pub fn load_named_fleet_file(
    path: &Path,
    expect_name: Option<&str>,
) -> Result<NamedFleet, NamedFleetError> {
    let text = std::fs::read_to_string(path).map_err(|e| NamedFleetError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let fleet = parse_named_fleet(&text).map_err(|e| match e {
        NamedFleetError::Parse { message, .. } => NamedFleetError::Parse {
            path: path.display().to_string(),
            message,
        },
        other => other,
    })?;
    if let Some(expected) = expect_name
        && fleet.name != expected
    {
        return Err(NamedFleetError::NameMismatch {
            declared: fleet.name,
            expected: expected.to_string(),
        });
    }
    Ok(fleet)
}

impl NamedFleet {
    /// 将角色名称解析为配置档案 ID。
    pub fn resolve(&self, role: &str) -> Result<&str, NamedFleetError> {
        let key = role.trim().to_ascii_lowercase();
        self.roles
            .get(&key)
            .or_else(|| {
                self.roles
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(role))
                    .map(|(_, v)| v)
            })
            .map(String::as_str)
            .ok_or_else(|| NamedFleetError::MissingRole {
                fleet: self.name.clone(),
                role: role.to_string(),
            })
    }

    /// 确保所有必需的停止运输角色都存在。
    pub fn validate_stopship_roles(&self) -> Result<(), NamedFleetError> {
        for role in STOPSHIP_REQUIRED_ROLES {
            self.resolve(role)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STOPSHIP_TOML: &str = r#"
name = "v0868-stopship"
description = "停止运输日志舰队"

[roles]
scout = "scout"
implementer = "builder"
reviewer = "reviewer"
verifier = "verifier"
release_lead = "manager"
"#;

    #[test]
    fn stopship_fleet_resolves_all_five_roles() {
        let fleet = parse_named_fleet(STOPSHIP_TOML).expect("parse");
        assert_eq!(fleet.name, "v0868-stopship");
        fleet.validate_stopship_roles().expect("all roles");
        assert_eq!(fleet.resolve("scout").unwrap(), "scout");
        assert_eq!(fleet.resolve("implementer").unwrap(), "builder");
        assert_eq!(fleet.resolve("reviewer").unwrap(), "reviewer");
        assert_eq!(fleet.resolve("verifier").unwrap(), "verifier");
        assert_eq!(fleet.resolve("release_lead").unwrap(), "manager");
    }

    #[test]
    fn unknown_role_fails_clearly() {
        let fleet = parse_named_fleet(STOPSHIP_TOML).unwrap();
        let err = fleet.resolve("wizard").unwrap_err();
        assert!(matches!(err, NamedFleetError::MissingRole { .. }));
    }

    #[test]
    fn loads_workspace_fleet_file() {
        // 相对于 crate CARGO_MANIFEST_DIR → 仓库根目录 fleets/
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let fleet = load_named_fleet("v0868-stopship", &[root]).expect("load workspace fleet");
        fleet.validate_stopship_roles().unwrap();
    }
}
