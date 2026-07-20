//! UI 路径选择器和提及功能的共享工作区发现过滤器。

use std::path::Path;

/// 即使被 `.gitignore` 排除，`@` 提及补全和模糊文件解析也必须保持可发现的目录。
pub(crate) const DISCOVERY_ALWAYS_DIRS: &[&str] = &[".deepseek", ".cursor", ".claude", ".agents"];

/// 相对于根目录的目录，这些目录太大或是生成产物，在禁用 gitignore 时不应发现。用户指定的精确路径仍可解析。
const DISCOVERY_EXCLUDED_SUBDIRS: &[&str] =
    &[".deepseek/snapshots", ".worktrees", ".claude/worktrees"];

/// 那些故意禁用 gitignore 的后备发现遍历不应进入的目录基本名称。
const DISCOVERY_EXCLUDED_DIR_NAMES: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "env",
    "dist",
    "build",
    ".next",
    ".turbo",
    "coverage",
    "__pycache__",
    ".pytest_cache",
    ".ruff_cache",
];

/// 检查 `path` 是否位于相对于根目录的排除发现子树下。
pub(crate) fn path_is_excluded_from_discovery(walk_root: &Path, path: &Path) -> bool {
    DISCOVERY_EXCLUDED_SUBDIRS
        .iter()
        .any(|excluded| path.starts_with(walk_root.join(excluded)))
}

/// 用于关闭 gitignore 以暴露显式隐藏路径的遍历过滤器。
pub(crate) fn should_skip_unignored_discovery_entry(walk_root: &Path, path: &Path) -> bool {
    if path == walk_root {
        return false;
    }

    if path_is_excluded_from_discovery(walk_root, path) {
        return true;
    }

    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| DISCOVERY_EXCLUDED_DIR_NAMES.contains(&name))
}
