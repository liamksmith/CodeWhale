#![allow(dead_code)]

//! 沙箱策略定义，用于命令执行限制。
//!
//! 该模块定义了控制沙箱进程可以访问哪些资源的策略。
//! 策略范围从完全无限制访问到严格控制的工作区只写访问。

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{CommandSpec, ExecEnv};
use crate::command_safety::SafetyLevel;

/// 确定 shell 命令的执行限制。
///
/// 沙箱策略控制已执行命令的文件系统访问、网络访问和其他
/// 系统资源。选择仍然允许命令运行的最具限制性策略。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SandboxPolicy {
    /// 没有任何限制。极谨慎使用。
    ///
    /// 此策略禁用所有沙箱化并允许完全系统访问。
    /// 仅在绝对必要且命令来源可信时使用。
    #[serde(rename = "danger-full-access")]
    DangerFullAccess,

    /// 对整个文件系统的只读访问。
    ///
    /// 进程可以读取任何文件，但无法写入任何位置。
    /// 适用于需要广泛读取访问的分析工具。
    #[serde(rename = "read-only")]
    ReadOnly,

    /// 表示进程已在外部沙箱中运行。
    ///
    /// 当 CodeWhale 本身在容器、VM 或其他沙箱化
    /// 环境中运行时使用此选项。这可以避免双层沙箱化，
    /// 双层沙箱化可能引发问题。
    #[serde(rename = "external-sandbox")]
    ExternalSandbox {
        /// 外部沙箱是否允许网络访问。
        #[serde(default)]
        network_access: bool,
    },

    /// 只读文件系统访问加上对指定目录的写入访问。
    ///
    /// 这是默认且推荐的策略。它允许：
    /// - 对整个文件系统的读取访问（用于工具、库等）
    /// - 仅对当前工作目录和指定根目录的写入访问
    /// - 可选的网络访问
    #[serde(rename = "workspace-write")]
    WorkspaceWrite {
        /// 允许写入的额外目录。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        writable_roots: Vec<PathBuf>,

        /// 是否允许出站网络连接。
        #[serde(default)]
        network_access: bool,

        /// 从可写路径中排除 TMPDIR。
        #[serde(default)]
        exclude_tmpdir: bool,

        /// 从可写路径中排除 /tmp。
        #[serde(default)]
        exclude_slash_tmp: bool,
    },
}

impl Default for SandboxPolicy {
    /// 返回默认策略：无额外根目录且无网络的 workspace-write。
    fn default() -> Self {
        SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![],
            network_access: false,
            exclude_tmpdir: false,
            exclude_slash_tmp: false,
        }
    }
}

impl SandboxPolicy {
    /// 创建一个启用了网络访问的 workspace-write 策略。
    pub fn workspace_with_network() -> Self {
        SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![],
            network_access: true,
            exclude_tmpdir: false,
            exclude_slash_tmp: false,
        }
    }

    /// 创建一个带有额外可写目录的 workspace-write 策略。
    pub fn workspace_with_roots(roots: Vec<PathBuf>, network: bool) -> Self {
        SandboxPolicy::WorkspaceWrite {
            writable_roots: roots,
            network_access: network,
            exclude_tmpdir: false,
            exclude_slash_tmp: false,
        }
    }

    /// 如果策略允许读取文件系统上的任何文件，则返回 true。
    pub fn has_full_disk_read_access() -> bool {
        // 当前所有策略都允许完全磁盘读取访问
        true
    }

    /// 如果策略允许写入文件系统上的任何文件，则返回 true。
    pub fn has_full_disk_write_access(&self) -> bool {
        matches!(
            self,
            SandboxPolicy::DangerFullAccess | SandboxPolicy::ExternalSandbox { .. }
        )
    }

    /// 如果策略允许出站网络连接，则返回 true。
    pub fn has_network_access(&self) -> bool {
        match self {
            SandboxPolicy::DangerFullAccess => true,
            SandboxPolicy::ReadOnly => false,
            SandboxPolicy::ExternalSandbox { network_access }
            | SandboxPolicy::WorkspaceWrite { network_access, .. } => *network_access,
        }
    }

    /// 如果应该应用沙箱（而非绕过），则返回 true。
    pub fn should_sandbox(&self) -> bool {
        !matches!(
            self,
            SandboxPolicy::DangerFullAccess | SandboxPolicy::ExternalSandbox { .. }
        )
    }

    /// 获取此策略的可写根目录列表。
    ///
    /// 包括：
    /// - 当前工作目录
    /// - 任何显式指定的 `writable_roots`
    /// - /tmp（除非排除）
    /// - TMPDIR（除非排除）
    ///
    /// 对于具有完全写入访问权限的策略，返回空 vec，
    /// 因为无需枚举特定路径。
    pub fn get_writable_roots(&self, cwd: &Path) -> Vec<WritableRoot> {
        match self {
            // 完全写入访问或只读 - 无需枚举
            SandboxPolicy::DangerFullAccess
            | SandboxPolicy::ExternalSandbox { .. }
            | SandboxPolicy::ReadOnly => vec![],

            // 工作区写入 - 枚举所有可写路径
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                exclude_tmpdir,
                exclude_slash_tmp,
                ..
            } => {
                let mut roots: Vec<PathBuf> = writable_roots.clone();

                // 添加当前工作目录
                if let Ok(canonical_cwd) = cwd.canonicalize() {
                    roots.push(canonical_cwd);
                } else {
                    roots.push(cwd.to_path_buf());
                }

                // Git 工作树将可变元数据保留在工作树目录之外。
                // 仅允许从工作区 `.git` 指针派生的 gitdir 和 commondir，
                // 对所有其他外部路径保持工作区边界不变。
                for root in roots.clone() {
                    roots.extend(resolve_git_worktree_writable_roots(&root));
                }

                // 添加 /tmp，除非排除
                if !exclude_slash_tmp && let Ok(tmp) = Path::new("/tmp").canonicalize() {
                    roots.push(tmp);
                }

                // 添加 TMPDIR，除非排除
                if !exclude_tmpdir
                    && let Ok(tmpdir) = std::env::var("TMPDIR")
                    && let Ok(canonical) = Path::new(&tmpdir).canonicalize()
                {
                    roots.push(canonical);
                }

                // 转换为包含只读子路径的 WritableRoot
                roots
                    .into_iter()
                    .map(|root| {
                        let mut read_only_subpaths = Vec::new();

                        // 保护 .codewhale/ 和 .deepseek/ 目录免受修改
                        let codewhale_dir = root.join(".codewhale");
                        if codewhale_dir.is_dir() {
                            read_only_subpaths.push(codewhale_dir);
                        }
                        let deepseek_dir = root.join(".deepseek");
                        if deepseek_dir.is_dir() {
                            read_only_subpaths.push(deepseek_dir);
                        }

                        WritableRoot {
                            root,
                            read_only_subpaths,
                        }
                    })
                    .collect()
            }
        }
    }
}

fn resolve_git_worktree_writable_roots(root: &Path) -> Vec<PathBuf> {
    let Some(pointer) = resolve_gitdir_pointer(root) else {
        return Vec::new();
    };
    let git_dir = pointer.git_dir;
    let Some(common_dir) = resolve_git_common_dir(&git_dir) else {
        return Vec::new();
    };
    if !git_dir.starts_with(common_dir.join("worktrees")) {
        return Vec::new();
    }
    if !worktree_metadata_points_back_to_workspace(&git_dir, &pointer.git_file) {
        return Vec::new();
    }

    vec![git_dir, common_dir]
}

#[derive(Debug)]
struct GitDirPointer {
    git_dir: PathBuf,
    git_file: PathBuf,
}

fn resolve_gitdir_pointer(root: &Path) -> Option<GitDirPointer> {
    let search_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    for ancestor in search_root.ancestors() {
        let git_file = ancestor.join(".git");
        if !git_file.is_file() {
            continue;
        }

        let contents = fs::read_to_string(&git_file).ok()?;
        let value = contents
            .lines()
            .find_map(|line| line.strip_prefix("gitdir:"))?
            .trim();
        if value.is_empty() {
            return None;
        }

        let path = PathBuf::from(value);
        let resolved = if path.is_absolute() {
            path
        } else {
            ancestor.join(path)
        };

        return Some(GitDirPointer {
            git_dir: resolved.canonicalize().ok()?,
            git_file: git_file.canonicalize().ok()?,
        });
    }

    None
}

fn resolve_git_common_dir(git_dir: &Path) -> Option<PathBuf> {
    let contents = fs::read_to_string(git_dir.join("commondir")).ok()?;
    let value = contents.lines().next()?.trim();
    if value.is_empty() {
        return None;
    }

    let path = PathBuf::from(value);
    let resolved = if path.is_absolute() {
        path
    } else {
        git_dir.join(path)
    };

    resolved.canonicalize().ok()
}

fn worktree_metadata_points_back_to_workspace(git_dir: &Path, expected_git_file: &Path) -> bool {
    let Some(actual_git_file) = resolve_gitdir_back_pointer(git_dir) else {
        return false;
    };
    actual_git_file == expected_git_file
}

fn resolve_gitdir_back_pointer(git_dir: &Path) -> Option<PathBuf> {
    let contents = fs::read_to_string(git_dir.join("gitdir")).ok()?;
    let value = contents.lines().next()?.trim();
    if value.is_empty() {
        return None;
    }

    let path = PathBuf::from(value);
    let resolved = if path.is_absolute() {
        path
    } else {
        git_dir.join(path)
    };

    resolved.canonicalize().ok()
}

/// 允许写入的目录树，带有可选的只读子路径。
///
/// 这允许细粒度控制，例如"允许写入 /project，但 /project/.deepseek 除外"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritableRoot {
    /// 允许写入的根目录。
    pub root: PathBuf,

    /// 根目录内应保持只读的子目录。
    pub read_only_subpaths: Vec<PathBuf>,
}

impl WritableRoot {
    /// 创建没有只读例外的新可写根目录。
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            read_only_subpaths: vec![],
        }
    }

    /// 创建带有特定只读子路径的可写根目录。
    pub fn with_exceptions(root: PathBuf, read_only: Vec<PathBuf>) -> Self {
        Self {
            root,
            read_only_subpaths: read_only,
        }
    }

    /// 检查路径是否在此根目录下可写。
    ///
    /// 如果路径在根目录下且不在任何只读子路径下，则返回 true。
    pub fn is_path_writable(&self, path: &Path) -> bool {
        // 必须在根目录下
        if !path.starts_with(&self.root) {
            return false;
        }

        // 不得在任何只读子路径下
        for subpath in &self.read_only_subpaths {
            if path.starts_with(subpath) {
                return false;
            }
        }

        true
    }
}

/// 平台特定沙箱执行器的统一 trait（#2186）。
///
/// 每个平台模块（seatbelt、landlock、windows）都提供了
/// 此 trait 的实现。`SandboxManager` 通过此 trait 分发，
/// 而不是直接调用平台特定的函数。
pub trait SandboxExecutor {
    /// 从命令规范准备沙箱化执行环境。
    ///
    /// 返回启动进程所需的转换后命令、环境和沙箱元数据。
    fn prepare(&self, spec: &CommandSpec) -> io::Result<ExecEnv>;

    /// 检查命令失败是否由沙箱拒绝引起。
    fn was_denied(&self, exit_code: i32, stderr: &str) -> bool;

    /// 获取沙箱阻止命令的可读描述。
    fn denial_message(&self, stderr: &str) -> String;

    /// 返回此执行器提供的沙箱类型。
    fn sandbox_type(&self) -> super::SandboxType;
}

/// 根据命令安全分类映射到适当的沙箱策略（#2186）。
///
/// - `Safe` / `WorkspaceSafe` → 使用默认沙箱策略
/// - `RequiresApproval` → 用户必须批准后才能执行（由调用方处理）
/// - `Dangerous` → 除非以 YOLO 模式运行且受信任，否则被阻止
pub fn map_safety_level_to_behavior(
    level: SafetyLevel,
    default_policy: &SandboxPolicy,
) -> SandboxPolicyBehavior {
    match level {
        SafetyLevel::Safe | SafetyLevel::WorkspaceSafe => {
            SandboxPolicyBehavior::Sandboxed(default_policy.clone())
        }
        SafetyLevel::RequiresApproval => SandboxPolicyBehavior::RequiresApproval,
        SafetyLevel::Dangerous => SandboxPolicyBehavior::Blocked,
    }
}

/// 根据安全级别对沙箱命令的行为决策。
#[derive(Debug, Clone)]
pub enum SandboxPolicyBehavior {
    /// 使用给定的沙箱策略执行。
    Sandboxed(SandboxPolicy),
    /// 执行前需要用户批准。
    RequiresApproval,
    /// 完全阻止执行（除非 YOLO+trust）。
    Blocked,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy() {
        let policy = SandboxPolicy::default();
        assert!(matches!(policy, SandboxPolicy::WorkspaceWrite { .. }));
        assert!(!policy.has_network_access());
        assert!(policy.should_sandbox());
    }

    #[test]
    fn test_full_access_policy() {
        let policy = SandboxPolicy::DangerFullAccess;
        assert!(policy.has_full_disk_write_access());
        assert!(policy.has_network_access());
        assert!(!policy.should_sandbox());
    }

    #[test]
    fn test_read_only_policy() {
        let policy = SandboxPolicy::ReadOnly;
        assert!(!policy.has_full_disk_write_access());
        assert!(!policy.has_network_access());
        assert!(policy.should_sandbox());
    }

    #[test]
    fn test_workspace_with_network() {
        let policy = SandboxPolicy::workspace_with_network();
        assert!(policy.has_network_access());
        assert!(policy.should_sandbox());
    }

    #[test]
    fn workspace_write_includes_git_worktree_metadata_roots() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let common_git_dir = tmp.path().join("main-repo").join(".git");
        let worktree_git_dir = common_git_dir.join("worktrees").join("feature");
        let worktree = tmp.path().join("feature-worktree");
        std::fs::create_dir_all(&worktree_git_dir).expect("mkdir gitdir");
        std::fs::create_dir_all(&worktree).expect("mkdir worktree");
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", worktree_git_dir.display()),
        )
        .expect("write git pointer");
        std::fs::write(worktree_git_dir.join("commondir"), "../..").expect("write commondir");
        std::fs::write(
            worktree_git_dir.join("gitdir"),
            worktree.join(".git").display().to_string(),
        )
        .expect("write gitdir back pointer");

        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![worktree.clone()],
            network_access: true,
            exclude_tmpdir: true,
            exclude_slash_tmp: true,
        };

        let root_paths: Vec<PathBuf> = policy
            .get_writable_roots(&worktree)
            .into_iter()
            .map(|root| root.root)
            .collect();

        assert!(root_paths.contains(&worktree.canonicalize().expect("canonical worktree")));
        assert!(root_paths.contains(&worktree_git_dir.canonicalize().expect("canonical gitdir")));
        assert!(root_paths.contains(&common_git_dir.canonicalize().expect("canonical common git")));
    }

    #[test]
    fn workspace_write_resolves_git_worktree_metadata_from_subdirectory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let common_git_dir = tmp.path().join("main-repo").join(".git");
        let worktree_git_dir = common_git_dir.join("worktrees").join("feature");
        let worktree = tmp.path().join("feature-worktree");
        let nested = worktree.join("crates").join("cli");
        std::fs::create_dir_all(&worktree_git_dir).expect("mkdir gitdir");
        std::fs::create_dir_all(&nested).expect("mkdir nested worktree path");
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", worktree_git_dir.display()),
        )
        .expect("write git pointer");
        std::fs::write(worktree_git_dir.join("commondir"), "../..").expect("write commondir");
        std::fs::write(
            worktree_git_dir.join("gitdir"),
            worktree.join(".git").display().to_string(),
        )
        .expect("write gitdir back pointer");

        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![],
            network_access: true,
            exclude_tmpdir: true,
            exclude_slash_tmp: true,
        };

        let root_paths: Vec<PathBuf> = policy
            .get_writable_roots(&nested)
            .into_iter()
            .map(|root| root.root)
            .collect();

        assert!(root_paths.contains(&nested.canonicalize().expect("canonical nested cwd")));
        assert!(root_paths.contains(&worktree_git_dir.canonicalize().expect("canonical gitdir")));
        assert!(root_paths.contains(&common_git_dir.canonicalize().expect("canonical common git")));
    }

    #[test]
    fn workspace_write_rejects_non_reciprocal_git_worktree_metadata() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let common_git_dir = tmp.path().join("main-repo").join(".git");
        let worktree_git_dir = common_git_dir.join("worktrees").join("feature");
        let worktree = tmp.path().join("feature-worktree");
        let other_worktree = tmp.path().join("other-worktree");
        std::fs::create_dir_all(&worktree_git_dir).expect("mkdir gitdir");
        std::fs::create_dir_all(&worktree).expect("mkdir worktree");
        std::fs::create_dir_all(&other_worktree).expect("mkdir other worktree");
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", worktree_git_dir.display()),
        )
        .expect("write git pointer");
        std::fs::write(worktree_git_dir.join("commondir"), "../..").expect("write commondir");
        std::fs::write(
            worktree_git_dir.join("gitdir"),
            other_worktree.join(".git").display().to_string(),
        )
        .expect("write mismatched gitdir back pointer");
        std::fs::write(
            other_worktree.join(".git"),
            "gitdir: /tmp/not-this-worktree\n",
        )
        .expect("write other git pointer");

        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![worktree.clone()],
            network_access: true,
            exclude_tmpdir: true,
            exclude_slash_tmp: true,
        };

        let root_paths: Vec<PathBuf> = policy
            .get_writable_roots(&worktree)
            .into_iter()
            .map(|root| root.root)
            .collect();

        assert!(root_paths.contains(&worktree.canonicalize().expect("canonical worktree")));
        assert!(!root_paths.contains(&worktree_git_dir.canonicalize().expect("canonical gitdir")));
        assert!(
            !root_paths.contains(&common_git_dir.canonicalize().expect("canonical common git"))
        );
    }

    #[test]
    fn test_writable_root_basic() {
        let root = WritableRoot::new(PathBuf::from("/project"));
        assert!(root.is_path_writable(Path::new("/project/src/main.rs")));
        assert!(!root.is_path_writable(Path::new("/other/file.txt")));
    }

    #[test]
    fn test_writable_root_with_exceptions() {
        let root = WritableRoot::with_exceptions(
            PathBuf::from("/project"),
            vec![PathBuf::from("/project/.deepseek")],
        );
        assert!(root.is_path_writable(Path::new("/project/src/main.rs")));
        assert!(!root.is_path_writable(Path::new("/project/.deepseek/config")));
    }

    #[test]
    fn test_safety_level_mapping() {
        let default = SandboxPolicy::default();

        // 安全命令被沙箱化
        assert!(matches!(
            map_safety_level_to_behavior(SafetyLevel::Safe, &default),
            SandboxPolicyBehavior::Sandboxed(_)
        ));
        assert!(matches!(
            map_safety_level_to_behavior(SafetyLevel::WorkspaceSafe, &default),
            SandboxPolicyBehavior::Sandboxed(_)
        ));

        // RequiresApproval 获得 RequiresApproval
        assert!(matches!(
            map_safety_level_to_behavior(SafetyLevel::RequiresApproval, &default),
            SandboxPolicyBehavior::RequiresApproval
        ));

        // Dangerous 获得 Blocked
        assert!(matches!(
            map_safety_level_to_behavior(SafetyLevel::Dangerous, &default),
            SandboxPolicyBehavior::Blocked
        ));
    }

    #[test]
    fn test_policy_serialization() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![PathBuf::from("/extra")],
            network_access: true,
            exclude_tmpdir: false,
            exclude_slash_tmp: false,
        };

        let json = serde_json::to_string(&policy).unwrap();
        assert!(json.contains("workspace-write"));

        let parsed: SandboxPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, parsed);
    }
}
