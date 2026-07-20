//! 系统技能安装程序：打包第一方技能并在首次启动时自动安装。

use std::fs;
use std::path::Path;

const BUNDLED_SKILL_VERSION: &str = "4";
const SKILL_CREATOR_BODY: &str = include_str!("../../assets/skills/skill-creator/SKILL.md");
const DELEGATE_BODY: &str = include_str!("../../assets/skills/delegate/SKILL.md");
const V4_BEST_PRACTICES_BODY: &str = include_str!("../../assets/skills/v4-best-practices/SKILL.md");
const PLUGIN_CREATOR_BODY: &str = include_str!("../../assets/skills/plugin-creator/SKILL.md");
const SKILL_INSTALLER_BODY: &str = include_str!("../../assets/skills/skill-installer/SKILL.md");
const MCP_BUILDER_BODY: &str = include_str!("../../assets/skills/mcp-builder/SKILL.md");
const FLEET_MANAGER_BODY: &str = include_str!("../../assets/skills/fleet-manager/SKILL.md");
const DOCUMENTS_BODY: &str = include_str!("../../assets/skills/documents/SKILL.md");
const PRESENTATIONS_BODY: &str = include_str!("../../assets/skills/presentations/SKILL.md");
const SPREADSHEETS_BODY: &str = include_str!("../../assets/skills/spreadsheets/SKILL.md");
const PDF_BODY: &str = include_str!("../../assets/skills/pdf/SKILL.md");
const FEISHU_BODY: &str = include_str!("../../assets/skills/feishu/SKILL.md");

struct BundledSkill {
    name: &'static str,
    body: &'static str,
    introduced_in: u32,
}

const BUNDLED_SKILLS: &[BundledSkill] = &[
    BundledSkill {
        name: "skill-creator",
        body: SKILL_CREATOR_BODY,
        introduced_in: 1,
    },
    BundledSkill {
        name: "delegate",
        body: DELEGATE_BODY,
        introduced_in: 2,
    },
    BundledSkill {
        name: "v4-best-practices",
        body: V4_BEST_PRACTICES_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "plugin-creator",
        body: PLUGIN_CREATOR_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "skill-installer",
        body: SKILL_INSTALLER_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "mcp-builder",
        body: MCP_BUILDER_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "fleet-manager",
        body: FLEET_MANAGER_BODY,
        introduced_in: 4,
    },
    BundledSkill {
        name: "documents",
        body: DOCUMENTS_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "presentations",
        body: PRESENTATIONS_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "spreadsheets",
        body: SPREADSHEETS_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "pdf",
        body: PDF_BODY,
        introduced_in: 3,
    },
    BundledSkill {
        name: "feishu",
        body: FEISHU_BODY,
        introduced_in: 3,
    },
];

/// 技能名称是否匹配其中一个打包的第一方技能。
///
/// 由 `/skills` 用于区分用户创建的技能（应突出显示）
/// 与始终安装的包（当有许多技能时可以紧凑渲染）。
#[must_use]
pub fn is_bundled_skill_name(name: &str) -> bool {
    BUNDLED_SKILLS.iter().any(|s| s.name == name)
}

/// 尝试将单个打包技能安装到 `skills_dir`。
///
/// 如果发生安装（全新安装或版本升级），返回 `true`。
fn install_one(
    skills_dir: &Path,
    skill: &BundledSkill,
    installed_version: Option<&str>,
) -> std::io::Result<bool> {
    let target_dir = skills_dir.join(skill.name);
    let target_file = target_dir.join("SKILL.md");
    let dir_exists = target_dir.exists();
    let installed_number = installed_version.and_then(|value| value.parse::<u32>().ok());

    let should_install = match (installed_version, installed_number, dir_exists) {
        // 全新安装：既没有标记也没有目录。
        (None, _, false) => true,
        // 新打包的技能：为旧系统技能安装添加。
        (Some(_), Some(version), _) if version < skill.introduced_in => true,
        // 现有技能的版本升级：仅在用户未有意删除该技能目录时刷新。
        (Some(version), _, true) if version != BUNDLED_SKILL_VERSION => true,
        // 其他所有情况：当前安装、用户删除的目录或
        // 没有我们标记的预存在用户自有技能。
        _ => false,
    };

    if should_install {
        fs::create_dir_all(&target_dir)?;
        fs::write(&target_file, skill.body)?;
    }
    Ok(should_install)
}

/// 将打包的系统技能安装到 `skills_dir`。
///
/// 行为：
/// - 全新安装（无标记，无目录）：安装每个打包技能，然后写入版本标记。
/// - 版本升级（存在具有旧版本的标记）：重新安装任何现有的打包技能
///   并安装新引入的打包技能。
/// - 用户删除了一个技能目录而标记仍以相同版本存在：保持删除状态。
/// - 幂等性：无更改的情况下调用两次是无操作。
///
/// 错误是来自文件系统的 I/O 错误；调用者应记录它们但不中止启动。
pub fn install_system_skills(skills_dir: &Path) -> std::io::Result<()> {
    let marker = skills_dir.join(".system-installed-version");

    let installed_version = fs::read_to_string(&marker)
        .ok()
        .map(|s| s.trim().to_string());

    let mut changed = false;
    for skill in BUNDLED_SKILLS {
        changed |= install_one(skills_dir, skill, installed_version.as_deref())?;
    }

    if changed {
        fs::create_dir_all(skills_dir)?;
        fs::write(&marker, BUNDLED_SKILL_VERSION)?;
    }
    Ok(())
}

/// 删除所有系统技能和版本标记。
///
/// 用于测试和 `deepseek setup --clean`。忽略缺失文件。
#[allow(dead_code)]
pub fn uninstall_system_skills(skills_dir: &Path) -> std::io::Result<()> {
    let marker = skills_dir.join(".system-installed-version");

    for skill in BUNDLED_SKILLS {
        let dir = skills_dir.join(skill.name);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
    }
    if marker.exists() {
        fs::remove_file(&marker)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn skill_file(tmp: &TempDir, name: &str) -> std::path::PathBuf {
        tmp.path().join(name).join("SKILL.md")
    }

    fn skill_dir(tmp: &TempDir, name: &str) -> std::path::PathBuf {
        tmp.path().join(name)
    }

    fn marker_file(tmp: &TempDir) -> std::path::PathBuf {
        tmp.path().join(".system-installed-version")
    }

    // ── 全新安装 ─────────────────────────────────────────────────────────

    #[test]
    fn fresh_install_creates_bundled_skills_and_marker() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        for skill in BUNDLED_SKILLS {
            assert!(
                skill_file(&tmp, skill.name).exists(),
                "{} SKILL.md should be created",
                skill.name
            );
        }
        assert!(marker_file(&tmp).exists(), "marker should be created");

        let ver = fs::read_to_string(marker_file(&tmp)).unwrap();
        assert_eq!(ver.trim(), BUNDLED_SKILL_VERSION);
    }

    #[test]
    fn fresh_install_skills_parse_for_discovery() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        let registry = crate::skills::SkillRegistry::discover(tmp.path());
        assert!(
            registry.warnings().is_empty(),
            "bundled skills should parse cleanly: {:?}",
            registry.warnings()
        );

        for skill in BUNDLED_SKILLS {
            let parsed = registry
                .get(skill.name)
                .unwrap_or_else(|| panic!("{} should be discoverable", skill.name));
            assert!(
                !parsed.description.is_empty(),
                "{} should include model-visible description",
                skill.name
            );
        }
    }

    // ── 幂等性 ───────────────────────────────────────────────────────────

    #[test]
    fn calling_twice_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        for skill in BUNDLED_SKILLS {
            fs::write(
                skill_file(&tmp, skill.name),
                format!("{}-sentinel", skill.name),
            )
            .unwrap();
        }

        install_system_skills(tmp.path()).unwrap();

        for skill in BUNDLED_SKILLS {
            let body = fs::read_to_string(skill_file(&tmp, skill.name)).unwrap();
            assert_eq!(
                body,
                format!("{}-sentinel", skill.name),
                "second install should not overwrite {}",
                skill.name
            );
        }
    }

    // ── 用户删除了一个目录 ──────────────────────────────────────────────

    #[test]
    fn user_deleted_dir_is_not_recreated() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        // 模拟用户有意删除一个技能目录。
        fs::remove_dir_all(skill_dir(&tmp, "delegate")).unwrap();

        // 重新启动不得重新创建已删除的目录。
        install_system_skills(tmp.path()).unwrap();

        assert!(
            !skill_file(&tmp, "delegate").exists(),
            "delegate must not be recreated after user deleted it"
        );
        assert!(
            skill_file(&tmp, "skill-creator").exists(),
            "skill-creator should still be present (not deleted by user)"
        );
    }

    #[test]
    fn user_deleted_all_dirs_are_not_recreated() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();

        for skill in BUNDLED_SKILLS {
            fs::remove_dir_all(skill_dir(&tmp, skill.name)).unwrap();
        }

        install_system_skills(tmp.path()).unwrap();

        for skill in BUNDLED_SKILLS {
            assert!(
                !skill_file(&tmp, skill.name).exists(),
                "{} must not be recreated after user deletion",
                skill.name
            );
        }
    }

    // ── 版本升级重新安装 ────────────────────────────────────────────────

    #[test]
    fn outdated_marker_triggers_reinstall_of_existing_skills() {
        let tmp = TempDir::new().unwrap();

        // 模拟所有技能存在且版本较低的先前安装。
        for skill in BUNDLED_SKILLS {
            fs::create_dir_all(skill_dir(&tmp, skill.name)).unwrap();
            fs::write(skill_file(&tmp, skill.name), format!("old-{}", skill.name)).unwrap();
        }
        fs::write(marker_file(&tmp), "0").unwrap(); // 早于 BUNDLED_SKILL_VERSION

        install_system_skills(tmp.path()).unwrap();

        for skill in BUNDLED_SKILLS {
            let body = fs::read_to_string(skill_file(&tmp, skill.name)).unwrap();
            assert_ne!(
                body,
                format!("old-{}", skill.name),
                "outdated {} should be overwritten",
                skill.name
            );
            assert_eq!(body, skill.body);
        }

        let ver = fs::read_to_string(marker_file(&tmp)).unwrap();
        assert_eq!(ver.trim(), BUNDLED_SKILL_VERSION);
    }

    // ── 部分先前的安装 ─────────────────────────────────────────────────

    #[test]
    fn version_bump_adds_skills_introduced_after_marker() {
        let tmp = TempDir::new().unwrap();

        // 模拟 v2 的状态：v1/v2 技能存在，v3 技能不存在。
        for skill in BUNDLED_SKILLS
            .iter()
            .filter(|skill| skill.introduced_in <= 2)
        {
            fs::create_dir_all(skill_dir(&tmp, skill.name)).unwrap();
            fs::write(skill_file(&tmp, skill.name), format!("old-{}", skill.name)).unwrap();
        }
        fs::write(marker_file(&tmp), "2").unwrap();

        install_system_skills(tmp.path()).unwrap();

        for skill in BUNDLED_SKILLS {
            assert_eq!(
                fs::read_to_string(skill_file(&tmp, skill.name)).unwrap(),
                skill.body,
                "{} should be installed or refreshed",
                skill.name
            );
        }
    }

    #[test]
    fn version_bump_respects_deleted_existing_skill_while_adding_new_skill() {
        let tmp = TempDir::new().unwrap();

        // 模拟 v2，其中较早的打包技能在后续版本引入更多系统技能之前被有意删除。
        fs::write(marker_file(&tmp), "2").unwrap();

        install_system_skills(tmp.path()).unwrap();

        assert!(
            !skill_file(&tmp, "skill-creator").exists(),
            "version bump should not recreate deleted skill-creator"
        );
        assert!(
            !skill_file(&tmp, "delegate").exists(),
            "version bump should not recreate deleted delegate"
        );
        for skill in BUNDLED_SKILLS
            .iter()
            .filter(|skill| skill.introduced_in > 2)
        {
            assert!(
                skill_file(&tmp, skill.name).exists(),
                "version bump should install newly introduced {}",
                skill.name
            );
        }
        let ver = fs::read_to_string(marker_file(&tmp)).unwrap();
        assert_eq!(ver.trim(), BUNDLED_SKILL_VERSION);
    }

    // ── 卸载 ─────────────────────────────────────────────────────────────

    #[test]
    fn uninstall_removes_bundled_skills_and_marker() {
        let tmp = TempDir::new().unwrap();
        install_system_skills(tmp.path()).unwrap();
        uninstall_system_skills(tmp.path()).unwrap();

        for skill in BUNDLED_SKILLS {
            assert!(
                !skill_file(&tmp, skill.name).exists(),
                "{} should be removed",
                skill.name
            );
        }
        assert!(!marker_file(&tmp).exists(), "marker should be removed");
    }

    #[test]
    fn uninstall_on_clean_dir_is_a_noop() {
        let tmp = TempDir::new().unwrap();
        // 不得 panic 或报错。
        uninstall_system_skills(tmp.path()).unwrap();
    }
}
