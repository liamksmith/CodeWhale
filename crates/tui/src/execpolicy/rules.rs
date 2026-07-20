//! 从 TOML 配置加载的执行策略规则。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use super::matcher::pattern_matches;
use crate::command_safety::prefix_allow_matches;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecPolicyDecision {
    Allow,
    Deny(String),
    AskUser(String),
}

#[derive(Debug, Deserialize, Default)]
pub struct ExecPolicyConfig {
    #[serde(default)]
    pub rules: BTreeMap<String, RuleSet>,
}

#[derive(Debug, Deserialize, Default)]
pub struct RuleSet {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

impl ExecPolicyConfig {
    pub fn from_str(contents: &str) -> Result<Self> {
        toml::from_str(contents).context("failed to parse execpolicy.toml")
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read execpolicy file {}", path.display()))?;
        Self::from_str(&contents)
    }

    pub fn evaluate(&self, command: &str) -> ExecPolicyDecision {
        for (group, rules) in &self.rules {
            for pattern in &rules.deny {
                if pattern_matches(pattern, command) {
                    return ExecPolicyDecision::Deny(format!(
                        "execpolicy denied by {group}: {pattern}"
                    ));
                }
            }
        }

        for (group, rules) in &self.rules {
            for pattern in &rules.allow {
                // 允许规则首先使用参数感知的前缀匹配，以便
                // `allow = ["git status"]` 匹配 `git status -s` 但**不**匹配
                // `git push origin main`。对于通配符模式（例如 `cargo *`），
                // 回退到正则表达式风格的 `pattern_matches`。
                if prefix_allow_matches(pattern, command) || pattern_matches(pattern, command) {
                    let _ = group;
                    return ExecPolicyDecision::Allow;
                }
            }
        }

        ExecPolicyDecision::AskUser("execpolicy: no matching allow rule".to_string())
    }
}

pub fn default_execpolicy_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".deepseek").join("execpolicy.toml"))
}

pub fn load_default_policy() -> Result<Option<ExecPolicyConfig>> {
    let Some(path) = default_execpolicy_path() else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    ExecPolicyConfig::from_path(&path).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execpolicy_evaluate() {
        let config = ExecPolicyConfig {
            rules: BTreeMap::from([
                (
                    "git".to_string(),
                    RuleSet {
                        allow: vec!["git status".to_string(), "git log *".to_string()],
                        deny: vec!["git push --force".to_string()],
                    },
                ),
                (
                    "danger".to_string(),
                    RuleSet {
                        allow: vec![],
                        deny: vec!["rm -rf /".to_string()],
                    },
                ),
            ]),
        };

        assert!(matches!(
            config.evaluate("git status"),
            ExecPolicyDecision::Allow
        ));
        assert!(matches!(
            config.evaluate("git log --oneline"),
            ExecPolicyDecision::Allow
        ));
        assert!(matches!(
            config.evaluate("git push --force"),
            ExecPolicyDecision::Deny(_)
        ));
        assert!(matches!(
            config.evaluate("unknown command"),
            ExecPolicyDecision::AskUser(_)
        ));
    }

    #[test]
    fn test_prefix_rule_allows_git_status_with_flags() {
        // 参数感知：`allow = ["git status"]` 必须匹配 `git status -s`。
        let config = ExecPolicyConfig {
            rules: BTreeMap::from([(
                "git".to_string(),
                RuleSet {
                    allow: vec!["git status".to_string()],
                    deny: vec![],
                },
            )]),
        };

        assert!(matches!(
            config.evaluate("git status -s"),
            ExecPolicyDecision::Allow
        ));
        assert!(matches!(
            config.evaluate("git status --porcelain"),
            ExecPolicyDecision::Allow
        ));
        // Push 绝对不能匹配 "git status" 的允许规则。
        assert!(matches!(
            config.evaluate("git push origin main"),
            ExecPolicyDecision::AskUser(_)
        ));
    }

    #[test]
    fn test_prefix_rule_allows_cargo_check_variants() {
        let config = ExecPolicyConfig {
            rules: BTreeMap::from([(
                "cargo".to_string(),
                RuleSet {
                    allow: vec!["cargo check".to_string()],
                    deny: vec![],
                },
            )]),
        };

        assert!(matches!(
            config.evaluate("cargo check"),
            ExecPolicyDecision::Allow
        ));
        assert!(matches!(
            config.evaluate("cargo check --workspace"),
            ExecPolicyDecision::Allow
        ));
        assert!(matches!(
            config.evaluate("cargo build --release"),
            ExecPolicyDecision::AskUser(_)
        ));
    }
}
