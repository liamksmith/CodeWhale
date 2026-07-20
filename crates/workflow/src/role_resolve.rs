//! Workflow 步骤的舰队角色解析 (#4177)。
//!
//! Workflow 拥有 **执行顺序**；Fleet 拥有 **执行者**。步骤声明一个舰队
//! `role`（以及可选的 task prompt）。运行时舰队名册将
//! `role → AgentProfile id` 进行映射。本模块是单元测试和调度器在生成前使用的
//! 纯解析路径——它从不导入 tmux 或会话管理。
//!
//! 优先级（与 #4111 / #4136 对齐）：
//! 1. 步骤上显式的 `profile`
//! 2. `role` 对应的舰队角色映射条目
//! 3. 当映射没有别名时，使用角色名称作为 profile id
//!
//! 内联的 provider/model **不是**身份字段。它们仍然是
//! [`crate::ModelPolicy`] 上的可选覆写；步骤的身份仅由 role/profile 决定。

use std::collections::BTreeMap;

use thiserror::Error;

/// 命名舰队名册：角色名 → AgentProfile id。
///
/// 角色和配置文件令牌在去除首尾空白并转为小写后进行不区分大小写的比较。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FleetRoleMap {
    /// 小写化后的角色 → 配置文件 id（按配置原样；不改变大小写）。
    roles: BTreeMap<String, String>,
}

impl FleetRoleMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// 插入一个角色 → 配置文件的绑定。空令牌将被拒绝。
    pub fn insert(
        &mut self,
        role: impl Into<String>,
        profile: impl Into<String>,
    ) -> Result<(), FleetRoleResolveError> {
        let role = normalize_token(&role.into()).ok_or(FleetRoleResolveError::EmptyRole)?;
        let profile =
            normalize_token(&profile.into()).ok_or(FleetRoleResolveError::EmptyProfile)?;
        self.roles.insert(role, profile);
        Ok(())
    }

    pub fn from_pairs<I, R, P>(pairs: I) -> Result<Self, FleetRoleResolveError>
    where
        I: IntoIterator<Item = (R, P)>,
        R: Into<String>,
        P: Into<String>,
    {
        let mut map = Self::new();
        for (role, profile) in pairs {
            map.insert(role, profile)?;
        }
        Ok(map)
    }

    /// 查找绑定到 `role` 的配置文件 id，如果有的话。
    pub fn get(&self, role: &str) -> Option<&str> {
        let key = normalize_token(role)?;
        self.roles.get(&key).map(String::as_str)
    }

    pub fn contains_role(&self, role: &str) -> bool {
        self.get(role).is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.roles.is_empty()
    }

    pub fn len(&self) -> usize {
        self.roles.len()
    }
}

/// 针对舰队名册解析工作流步骤的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWorkflowAgent {
    /// 步骤上声明的舰队角色，如果有的话。
    pub resolved_role: Option<String>,
    /// 要生成的 AgentProfile id。
    pub resolved_profile: String,
    /// 配置文件的选取方式：`explicit_profile`、`fleet_role` 或
    /// `role_as_profile`。
    pub route_source: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FleetRoleResolveError {
    #[error("fleet role name must be a non-empty token")]
    EmptyRole,
    #[error("fleet profile id must be a non-empty token")]
    EmptyProfile,
    #[error("unknown fleet role `{role}`: not present in fleet roster (known roles: {known})")]
    UnknownRole { role: String, known: String },
    #[error(
        "workflow step requires a fleet role or explicit profile; provider/model alone are not identity"
    )]
    MissingRoleOrProfile,
    #[error("role `{role}` must be a non-empty token without whitespace, quotes, or `=`")]
    InvalidRoleToken { role: String },
}

/// 规范化的角色/配置文件令牌：去除首尾空白、转小写。如果为空
/// 或令牌包含空白/引号/反引号/`=`，返回 `None`。
pub fn normalize_token(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed
        .chars()
        .any(|ch| ch.is_whitespace() || matches!(ch, '"' | '\'' | '`' | '='))
    {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

/// 以与叶子配置文件相同的方式验证角色令牌。
pub fn validate_role_token(role: &str) -> Result<String, FleetRoleResolveError> {
    normalize_token(role).ok_or_else(|| FleetRoleResolveError::InvalidRoleToken {
        role: role.to_string(),
    })
}

/// 根据可选的 `role` + 可选的显式 `profile` 针对舰队角色映射解析步骤身份。
///
/// 当 `require_known_role` 为 true 且设置了 `role` 但没有显式 profile 时，
/// 该角色必须存在于 `fleet` 中（未知角色会清晰报错）。
/// 当为 false 时，未知角色会回退到 `role_as_profile`（当调度器稍后将针对完整名册验证成员时有用）。
pub fn resolve_workflow_agent(
    role: Option<&str>,
    profile: Option<&str>,
    fleet: &FleetRoleMap,
    require_known_role: bool,
) -> Result<ResolvedWorkflowAgent, FleetRoleResolveError> {
    let role_norm = match role {
        Some(raw) => Some(validate_role_token(raw)?),
        None => None,
    };
    let profile_norm =
        match profile {
            Some(raw) => Some(validate_role_token(raw).map_err(|_| {
                FleetRoleResolveError::InvalidRoleToken {
                    role: raw.to_string(),
                }
            })?),
            None => None,
        };

    // 显式 profile 始终优先（任务字段优先级）。
    if let Some(resolved_profile) = profile_norm {
        return Ok(ResolvedWorkflowAgent {
            resolved_role: role_norm,
            resolved_profile,
            route_source: "explicit_profile",
        });
    }

    let Some(role_name) = role_norm else {
        return Err(FleetRoleResolveError::MissingRoleOrProfile);
    };

    if let Some(mapped) = fleet.get(&role_name) {
        return Ok(ResolvedWorkflowAgent {
            resolved_role: Some(role_name),
            resolved_profile: mapped.to_string(),
            route_source: "fleet_role",
        });
    }

    if require_known_role {
        let known = if fleet.is_empty() {
            "(none)".to_string()
        } else {
            fleet.roles.keys().cloned().collect::<Vec<_>>().join(", ")
        };
        return Err(FleetRoleResolveError::UnknownRole {
            role: role_name,
            known,
        });
    }

    Ok(ResolvedWorkflowAgent {
        resolved_role: Some(role_name.clone()),
        resolved_profile: role_name,
        route_source: "role_as_profile",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stopship_fleet() -> FleetRoleMap {
        FleetRoleMap::from_pairs([
            ("scout", "scout"),
            ("implementer", "builder"),
            ("reviewer", "reviewer"),
            ("verifier", "verifier"),
            ("release_lead", "manager"),
        ])
        .expect("valid fleet pairs")
    }

    #[test]
    fn known_role_resolves_to_configured_profile() {
        let fleet = stopship_fleet();
        let resolved =
            resolve_workflow_agent(Some("implementer"), None, &fleet, true).expect("resolve");
        assert_eq!(resolved.resolved_role.as_deref(), Some("implementer"));
        assert_eq!(resolved.resolved_profile, "builder");
        assert_eq!(resolved.route_source, "fleet_role");
    }

    #[test]
    fn unknown_role_fails_clearly() {
        let fleet = stopship_fleet();
        let err = resolve_workflow_agent(Some("wizard"), None, &fleet, true)
            .expect_err("unknown role must fail");
        match err {
            FleetRoleResolveError::UnknownRole { role, known } => {
                assert_eq!(role, "wizard");
                assert!(known.contains("scout"), "known={known}");
                assert!(known.contains("implementer"), "known={known}");
            }
            other => panic!("expected UnknownRole, got {other:?}"),
        }
    }

    #[test]
    fn explicit_profile_wins_over_role_map() {
        let fleet = stopship_fleet();
        let resolved = resolve_workflow_agent(Some("scout"), Some("custom-scout"), &fleet, true)
            .expect("resolve");
        assert_eq!(resolved.resolved_role.as_deref(), Some("scout"));
        assert_eq!(resolved.resolved_profile, "custom-scout");
        assert_eq!(resolved.route_source, "explicit_profile");
    }

    #[test]
    fn missing_role_and_profile_fails() {
        let fleet = stopship_fleet();
        let err = resolve_workflow_agent(None, None, &fleet, true).expect_err("identity required");
        assert!(matches!(err, FleetRoleResolveError::MissingRoleOrProfile));
    }

    #[test]
    fn role_token_rejects_whitespace_and_equals() {
        for bad in ["", "has space", "role=x", "quote\"y"] {
            assert!(
                validate_role_token(bad).is_err(),
                "token {bad:?} should be rejected"
            );
        }
        assert_eq!(validate_role_token("  Scout  ").unwrap(), "scout");
    }

    #[test]
    fn require_known_role_false_falls_back_to_role_as_profile() {
        let fleet = FleetRoleMap::new();
        let resolved = resolve_workflow_agent(Some("scout"), None, &fleet, false).expect("resolve");
        assert_eq!(resolved.resolved_profile, "scout");
        assert_eq!(resolved.route_source, "role_as_profile");
    }
}
