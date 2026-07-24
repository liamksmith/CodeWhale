//! 仓库感知的工作集跟踪与提示上下文打包模块。
//!
//! 本模块的目标是维护一个精简且高信息量的“活跃”路径列表，供助手优先关注。
//! 它观察用户消息和工具调用，提取可能的路径，并生成：
//! - 用于系统提示的紧凑工作集摘要块
//! - 压缩时应保留的固定消息索引
//! 
//! 这是在一个~/.codewhale/.session/目录下某个文件中关于本文件内容章节的示例
//! ## Repo Working Set
//! Workspace: D:\\tmp\\source\\CodeWhale
//! Key files: Cargo.toml, AGENTS.md, CLAUDE.md, package.json
//! Top-level dirs: assets, benchmark_results, crates, deploy, docs, extensions, fleets, integrations
//! When in doubt, use tools to verify and keep changes focused on the working set.
//! Git workspace: main | 1 modified

use crate::models::{ContentBlock, Message};
use crate::workspace_discovery::{
    DISCOVERY_ALWAYS_DIRS, path_is_excluded_from_discovery, should_skip_unignored_discovery_entry,
};
use ignore::WalkBuilder;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

/// 用于 `@` 提及和文件选择器的仓库感知解析器。
///
/// `cwd` 在构造时捕获；如果宿主当前目录在会话期间发生变化，则构建一个新的 `Workspace`。
/// 模糊查找由惰性文件名 → 路径索引支持，该索引在首次未命中时构建一次，并在会话剩余时间内复用——
/// 没有它，每一个拼写错误的提及都会触发一次完整的 `WalkBuilder` 遍历，直至配置的补全深度。
#[derive(Debug)]
pub struct Workspace {
    pub root: PathBuf,              // 工作区根目录
    cwd: Option<PathBuf>,              // 当前工作目录（可能不同于 root）
    file_index: OnceLock<HashMap<String, Vec<PathBuf>>>,   // 惰性文件名索引。保证只在第一次模糊匹配失败时才构建一次，然后在整个会话中复用。这是一个关键性能优化：没有它，每次拼写错误的提及都会触发一次完整的目录遍历。
    completion_walk_depth: Option<usize>,   // 自动补全遍历深度限制
    /// 在文件发现遍历过程中是否跟随符号链接。当为 `true` 时，
    /// 会遍历符号链接指向的目录，从而支持多项目工作空间，
    /// 即项目目录通过符号链接链接到一个中心目录中。
    follow_links: bool,   // 是否跟随符号链接
}

/// 在 `Workspace::completions()` 的两次遍历（CWD + workspace root）间共享的搜索上下文。
/// 通过 `prefix_hits`（前缀匹配，高优先级）和 `substring_hits`（子串匹配，低优先级）
/// 两个 bucket 收集候选路径，并借助 `seen` 集合按绝对路径去重。
struct SearchContext<'a> {
    needle: &'a str,                        // 搜索关键词（小写）
    limit: usize,                           // 结果数量上限
    prefix_hits: &'a mut Vec<String>,       // 前缀匹配结果
    substring_hits: &'a mut Vec<String>,    // 子串匹配结果
    seen: &'a mut HashSet<PathBuf>,         // 已见的绝对路径（用于去重）
}

impl SearchContext<'_> {
    /// 判断是否已达到结果上限
    fn is_full(&self) -> bool { 
        self.prefix_hits.len() + self.substring_hits.len() >= self.limit
    }

    /// 将路径标记为"已见过"，返回 true 表示首次见到
    fn remember(&mut self, path: PathBuf) -> bool {
        self.seen.insert(path)
    }

    /// 将候选路径分类加入前缀命中和子串命中列表：
    /// 如果 needle 为空或候选以小写 needle 开头 → 前缀命中（优先级高）
    /// 如果候选包含 needle → 子串命中（优先级低）
    fn push_match(&mut self, candidate: String) {
        let lower = candidate.to_lowercase();
        if self.needle.is_empty() || lower.starts_with(self.needle) {
            self.prefix_hits.push(candidate);
        } else if lower.contains(self.needle) {
            self.substring_hits.push(candidate);
        }
    }
}

impl Workspace {
    /// 构建一个锚定于 `root` 的工作空间，并将进程的当前工作目录（CWD）作为次级解析路径捕获。
    /// 这是一个便捷入口点，适用于调用者尚未持有 CWD 的情况；App 会通过 [`Workspace::with_cwd`]
    /// 并使用其自身捕获的启动目录进行路由。
    #[allow(dead_code)] // Keeps the surface stable for #97 (Ctrl+P picker).
    pub fn new(root: PathBuf) -> Self {
        Self::with_cwd(root, std::env::current_dir().ok())
    }

    /// 使用显式的 cwd 进行构造。用于需要针对已知目录进行确定性解析的测试，
    /// 而不依赖于（或修改）进程的实际工作目录。
    pub fn with_cwd(root: PathBuf, cwd: Option<PathBuf>) -> Self {
        Self::with_cwd_and_depth(root, cwd, DEFAULT_COMPLETIONS_WALK_DEPTH)
    }

    /// 使用显式的补全遍历深度进行构造。深度为 `0` 表示
    /// 为具有深层嵌套工作空间的用户禁用深度限制。
    pub fn with_cwd_and_depth(root: PathBuf, cwd: Option<PathBuf>, walk_depth: usize) -> Self {
        Self::with_cwd_depth_and_follow_links(root, cwd, walk_depth, false)
    }

    /// 使用显式的补全遍历深度和符号链接跟随偏好进行构造。
    /// 参见 [`Workspace::follow_links`]。
    pub fn with_cwd_depth_and_follow_links(
        root: PathBuf,
        cwd: Option<PathBuf>,
        walk_depth: usize,
        follow_links: bool,
    ) -> Self {
        Self {
            root,
            cwd,
            file_index: OnceLock::new(),
            completion_walk_depth: normalize_completion_walk_depth(walk_depth),
            follow_links,
        }
    }

    /// 解析路径。两遍解析：先工作空间，再当前工作目录，最后模糊匹配回退。
    pub fn resolve(&self, raw_path: &str) -> Result<PathBuf, PathBuf> {
        let path = expand_mention_home(raw_path);
        if path.is_absolute() {
            if path.exists() {
                return Ok(path);
            }
            return Err(path);
        }

        let ws_path = self.root.join(&path);
        if ws_path.exists() {
            return Ok(ws_path);
        }

        if let Some(cwd) = self.cwd.as_ref() {
            let cwd_path = cwd.join(&path);
            if cwd_path.exists() {
                return Ok(cwd_path);
            }
        }

        if let Some(fuzzy) = self.fuzzy_resolve(&path) {
            return Ok(fuzzy);
        }

        Err(ws_path)
    }

    /// 通过惰性构建的 basename → 路径列表索引进行模糊文件名匹配。
    /// 取 path 的 file_name，转小写后在索引中查找；返回第一个匹配的完整路径。
    /// 索引在首次调用时构建（`OnceLock`），后续复用
    fn fuzzy_resolve(&self, path: &Path) -> Option<PathBuf> {
        let needle = path.file_name()?.to_string_lossy().to_lowercase();
        if needle.is_empty() {
            return None;
        }

        let index = self.file_index.get_or_init(|| self.build_file_index());
        index.get(&needle).and_then(|paths| paths.first()).cloned()
    }

    /// 构建文件名索引。遍历工作区所有文件，建立 basename → [完整路径列表] 的映射。
    /// 这层索引使得模糊匹配（如 resolve("main.rs")）不需要重复遍历磁盘。
    fn build_file_index(&self) -> HashMap<String, Vec<PathBuf>> {
        let mut index: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let mut total: usize = 0;
        let builder =
            discovery_walk_builder(&self.root, self.completion_walk_depth, self.follow_links);

        for entry in builder.build().flatten() {
            if total >= FILE_INDEX_MAX_ENTRIES {
                tracing::warn!(
                    target: "working_set",
                    limit = FILE_INDEX_MAX_ENTRIES,
                    "file-index discovery hit the entry cap; truncating to keep first-turn latency bounded (#697)"
                );
                return index;
            }
            if entry
                .file_type()
                .is_some_and(|ft| ft.is_file() || ft.is_dir())
            {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                index
                    .entry(name)
                    .or_default()
                    .push(entry.path().to_path_buf());
                total += 1;
            }
        }

        // Also index AI-tool dot-directories with gitignore disabled.
        for dir_name in DISCOVERY_ALWAYS_DIRS {
            if total >= FILE_INDEX_MAX_ENTRIES {
                break;
            }
            let dot_dir = self.root.join(dir_name);
            if !dot_dir.is_dir() {
                continue;
            }
            let mut dot_builder = WalkBuilder::new(&dot_dir);
            dot_builder
                .hidden(true)
                .follow_links(self.follow_links)
                .git_ignore(false)
                .ignore(false);
            if let Some(depth) = child_completion_walk_depth(self.completion_walk_depth) {
                dot_builder.max_depth(Some(depth));
            }
            for entry in dot_builder.build().flatten() {
                if total >= FILE_INDEX_MAX_ENTRIES {
                    break;
                }
                // Exclude machine-generated bulk (e.g. .deepseek/snapshots/).
                if path_is_excluded_from_discovery(&self.root, entry.path()) {
                    continue;
                }
                if entry
                    .file_type()
                    .is_some_and(|ft| ft.is_file() || ft.is_dir())
                {
                    let name = entry.file_name().to_string_lossy().to_lowercase();
                    index
                        .entry(name)
                        .or_default()
                        .push(entry.path().to_path_buf());
                    total += 1;
                }
            }
        }

        // Beyond the curated dot-dir whitelist above, also index any explicit
        // hidden/ignored path the user might `@`-mention (e.g. a project's
        // own `.generated/specs/`). `local_reference_paths` walks with
        // gitignore disabled but still honors `.deepseekignore`.
        for path in local_reference_paths(
            &self.root,
            LOCAL_REFERENCE_SCAN_LIMIT,
            self.completion_walk_depth,
            self.follow_links,
        ) {
            if total >= FILE_INDEX_MAX_ENTRIES {
                break;
            }
            let Some(name) = path
                .file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
            else {
                continue;
            };
            index.entry(name).or_default().push(path);
            total += 1;
        }
        index
    }

    /// 文件/路径的自动补全方法。
    /// 遍历工作空间（以及当它偏离时记录下的 `cwd`），并返回表示形式与 `partial` 匹配的相对路径。
    ///
    /// 排序规则：当候选路径的不区分大小写的显示字符串以 `partial` 开头（前缀匹配）或包含它作为子串时，该候选路径匹配；
    /// 前缀匹配优先排序，因此 `docs/de` 会让 `docs/deepseek_v4.pdf` 排在任何仅包含这些字节的路径之前。
    ///
    /// 显示字符串在 `root` 下的文件以工作空间为相对路径，仅在记录的 `cwd` 下的文件以 cwd 为相对路径——
    /// 因此用户 Tab 补全的内容与他们 shell 中显示的内容一致。
    ///
    /// 遵循 `.gitignore`、`.git/info/exclude`、`.ignore` 和 `.deepseekignore`。
    /// 结果数量限制为 `limit` 个。
    #[must_use]
    pub fn completions(&self, partial: &str, limit: usize) -> Vec<String> {
        if limit == 0 {
            return Vec::new();
        }
        let needle = partial.to_lowercase();
        let mut prefix_hits: Vec<String> = Vec::new();
        let mut substring_hits: Vec<String> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();

        // Walk the recorded cwd first when it diverges from the workspace
        // root, so cwd-relative entries appear ahead of duplicates surfaced by
        // the workspace walk.
        {
            let mut ctx = SearchContext {
                needle: &needle,
                limit,
                prefix_hits: &mut prefix_hits,
                substring_hits: &mut substring_hits,
                seen: &mut seen,
            };

            let cwd_diverges = self
                .cwd
                .as_deref()
                .map(|c| c != self.root.as_path())
                .unwrap_or(false);
            if cwd_diverges && let Some(cwd) = self.cwd.as_deref() {
                // 第一步：遍历 CWD 目录
                walk_for_completions(
                    cwd,
                    cwd,
                    &mut ctx,
                    self.completion_walk_depth,
                    self.follow_links,
                );
                // 同时加载本地引用（./ ../ 开头的路径）
                add_local_reference_completions(
                    cwd,
                    cwd,
                    &mut ctx,
                    self.completion_walk_depth,
                    self.follow_links,
                );
            }

            // 第二步：遍历 workspace root
            walk_for_completions(
                &self.root,
                &self.root,
                &mut ctx,
                self.completion_walk_depth,
                self.follow_links,
            );
            add_local_reference_completions(
                &self.root,
                &self.root,
                &mut ctx,
                self.completion_walk_depth,
                self.follow_links,
            );
        }

        prefix_hits.sort();
        substring_hits.sort();
        prefix_hits.extend(substring_hits);
        prefix_hits.truncate(limit);
        prefix_hits
    }

    /// 用于 UI 预加载完整路径列表，这样用户在键盘敲击时可以即时过滤而无需重新遍历文件系统。<br>
    /// 执行一次完整的补全遍历，不带匹配关键字（needle）：包含工作空间遍历中的所有可发现显示字符串，
    /// 加上偏离的 cwd 遍历（以及始终可发现的 AI 点目录）中的内容，
    /// 去重后按遍历顺序排列。
    /// 与 [`rank_completion_candidates`] 配对使用，以便编辑器（composer）可以在每次按键时进行过滤，
    /// 而无需重新遍历文件系统（#3757）。
    ///
    /// 注意：不包含受关键字门控的本地路径引用补全；
    /// 调用者必须回退到 [`Workspace::completions`] 来处理类似路径的关键字（以 `.` 开头或包含分隔符）。
    #[must_use]
    pub fn completion_candidates(&self) -> Vec<String> {
        let mut prefix_hits: Vec<String> = Vec::new();
        let mut substring_hits: Vec<String> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        {
            let mut ctx = SearchContext {
                needle: "",
                limit: usize::MAX,
                prefix_hits: &mut prefix_hits,
                substring_hits: &mut substring_hits,
                seen: &mut seen,
            };
            let cwd_diverges = self
                .cwd
                .as_deref()
                .map(|c| c != self.root.as_path())
                .unwrap_or(false);
            if cwd_diverges && let Some(cwd) = self.cwd.as_deref() {
                walk_for_completions(
                    cwd,
                    cwd,
                    &mut ctx,
                    self.completion_walk_depth,
                    self.follow_links,
                );
            }
            walk_for_completions(
                &self.root,
                &self.root,
                &mut ctx,
                self.completion_walk_depth,
                self.follow_links,
            );
        }
        // Empty needle routes everything into prefix_hits.
        prefix_hits
    }

    /// 专为文件浏览器模式设计，用于 `@` 提及的确定性目录浏览器补全。
    ///
    /// 与 [`Workspace::completions`] 不同，此模式不会在整个工作空间中进行模糊排名。
    /// 它会锁定 `partial` 中的目录部分，并仅以不区分大小写的字母顺序返回该目录的直接子项。
    /// 
    /// 拒绝路径逃逸：如果 needle 包含 ../ 或从当前目录向外逃逸，浏览器模式会拒绝列出工作区
    /// 以外的文件。这是安全措施，防止用户意外访问工作区外的敏感文件。
    #[must_use]
    pub fn browser_completions(&self, partial: &str, limit: usize) -> Vec<String> {
        if limit == 0 {
            return Vec::new();
        }

        let normalized = partial.replace('\\', "/");
        let trimmed = normalized.trim_start_matches('/');
        let (dir_part, name_part) = match trimmed.rsplit_once('/') {
            Some((dir, name)) => (dir.trim_end_matches('/'), name),
            None => ("", trimmed),
        };
        let Some(safe_dir_part) = browser_completion_dir_part(dir_part) else {
            return Vec::new();
        };
        let dir = if safe_dir_part.as_os_str().is_empty() {
            self.root.clone()
        } else {
            self.root.join(&safe_dir_part)
        };
        if !dir.is_dir() {
            return Vec::new();
        }
        let display_dir_part = safe_dir_part.to_string_lossy().replace('\\', "/");

        let show_hidden = name_part.starts_with('.');
        let needle = name_part.to_lowercase();
        let mut entries = Vec::new();

        let mut builder = WalkBuilder::new(&dir);
        builder
            .hidden(!show_hidden)
            .follow_links(self.follow_links)
            .max_depth(Some(1));
        let _ = builder.add_custom_ignore_filename(".deepseekignore");

        for entry in builder.build().flatten() {
            let path = entry.path();
            if path == dir || path_is_excluded_from_discovery(&self.root, path) {
                continue;
            }
            let Some(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_file() && !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy();
            if !needle.is_empty() && !name.to_lowercase().starts_with(&needle) {
                continue;
            }
            let mut candidate = if display_dir_part.is_empty() {
                name.to_string()
            } else {
                format!("{display_dir_part}/{name}")
            };
            if file_type.is_dir() {
                candidate.push('/');
            }
            entries.push(candidate);
        }

        entries.sort_by_key(|entry| entry.to_lowercase());
        entries.truncate(limit);
        entries
    }
}

/// 解析浏览器补全所需的目录部分，过滤掉不安全组件。
/// 拒绝 `ParentDir`（`..` 路径逃逸）、`RootDir` 和 `Prefix` 组件，
/// 仅保留 `CurDir`（`.`）和 `Normal` 组件作为安全的补全根目录。
fn browser_completion_dir_part(dir_part: &str) -> Option<PathBuf> {
    let mut safe = PathBuf::new();
    for component in Path::new(dir_part).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => safe.push(part),
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => return None,
        }
    }
    Some(safe)
}

/// 在呈现文件提及补全时遍历的默认目录深度。
/// 设置得足够高，以便常规嵌套的源代码树（Java/.NET/Web 项目通常达到 7-9 层）保持可访问，
/// 而覆盖为 `0` 则完全移除限制。
/// 通过感知 `.gitignore` 的遍历和每次按键的候选结果数量限制（#2488），
/// 在深度单仓库中保持 Tab 补全的快速响应。
pub const DEFAULT_COMPLETIONS_WALK_DEPTH: usize = 10;

/// 将用户配置的补全遍历深度转换为 `Option<usize>`。
/// `0` 表示无限制（对应 `None`），正值表示受限深度（对应 `Some(depth)`）
fn normalize_completion_walk_depth(depth: usize) -> Option<usize> {
    if depth == 0 { None } else { Some(depth) }
}

/// 计算子遍历的补全深度：父深度减 1。
/// 用于 `walk_always_discoverable_dirs`，因为已进入点目录内部，需要减去一级深度。
fn child_completion_walk_depth(depth: Option<usize>) -> Option<usize> {
    depth.map(|depth| depth.saturating_sub(1))
}

/// [`Workspace::build_file_index`] 索引的 `（文件或目录）` 条目的硬上限。
/// 模糊解析索引是 [`Workspace::fuzzy_resolve`] 的便捷辅助；缺失的条目会回退到字面路径解析。
/// 在此设置上限可确保首次 `fuzzy_resolve` 调用在大型工作空间中保持有界（#697 报告首次轮次约 10 秒的卡顿）。
/// 对于典型项目，50K 远高于实际条目数，此上限不会产生任何影响。
const FILE_INDEX_MAX_ENTRIES: usize = 50_000;

/// 为工作空间发现配置一个 `WalkBuilder`：
/// 包括隐藏文件、深度限制、遵循自定义的 `.deepseekignore`，
/// 以及针对 AI 工具点目录的 gitignore 覆盖，以便 `@` 补全即使在这些目录被 git 忽略时也能找到它们。
/// 符号链接跟随由 `follow_links` 控制。
fn discovery_walk_builder(
    root: &Path,
    max_depth: Option<usize>,
    follow_links: bool,
) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder.hidden(true).follow_links(follow_links);
    if let Some(depth) = max_depth {
        builder.max_depth(Some(depth));
    }
    let _ = builder.add_custom_ignore_filename(".deepseekignore");
    builder
}

/// 遍历 AI 工具点目录（`.deepseek/`、`.cursor/`、`.claude/`、`.agents/`），
/// 并禁用 gitignore 规则，以便即使项目的 `.gitignore` / `.ignore` 排除了这些目录，
/// 其内容依然可被发现。
fn walk_always_discoverable_dirs(
    walk_root: &Path,
    display_root: &Path,
    ctx: &mut SearchContext<'_>,
    max_depth: Option<usize>,
    follow_links: bool,
) {
    for dir_name in DISCOVERY_ALWAYS_DIRS {
        let dot_dir = walk_root.join(dir_name);
        if !dot_dir.is_dir() {
            continue;       // 目录不存在就跳过
        }
        let mut builder = WalkBuilder::new(&dot_dir);
        builder
            .hidden(true)   // 显示隐藏文件
            .follow_links(follow_links)
            .git_ignore(false)  // ⭐ 关键：忽略 .gitignore
            .ignore(false);                  // ⭐ 关键：忽略 .ignore
        if let Some(depth) = max_depth {
            builder.max_depth(Some(depth.saturating_sub(1)));   // 深度减1（因为已进入点目录）
        }
        for entry in builder.build().flatten() {
            if ctx.is_full() {
                break;
            }
            let path = entry.path();
            // 排除机器生成的大文件（如 .deepseek/snapshots/）
            // even though gitignore is disabled for this walk.
            if path_is_excluded_from_discovery(walk_root, path) {
                continue;
            }
            let Ok(rel) = path.strip_prefix(display_root) else {
                continue;
            };
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if rel_str.is_empty() {
                continue;
            }
            let abs = path.to_path_buf();
            if !ctx.remember(abs) {
                continue;       // 与主遍历去重
            }
            let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
            let candidate = if is_dir {
                format!("{rel_str}/")
            } else {
                rel_str.clone()
            };
            ctx.push_match(candidate);
        }
    }
}

// 它是一个感知 gitignore、有深度限制、支持提前终止、跨双重遍历去重、且确保 AI 工具目录始终可发现的智能目录遍历器。
fn walk_for_completions(
    walk_root: &Path,               // 实际遍历的根目录
    display_root: &Path,            // 显示路径时的根目录（用于计算相对路径）
    ctx: &mut SearchContext<'_>,    // 保存搜索状态（匹配结果、去重集合、上限）
    max_depth: Option<usize>,       // 可选的最大遍历深度
    follow_links: bool,             // 是否跟随符号链接
) {
    // 参数设计的关键洞察——为什么需要两个 root？
    // - walk_root：实际文件系统遍历的起点
    // - display_root：计算相对路径显示的根
    // 这是为了处理 CWD 与工作区 root 不同 的场景。假设：
    // - 工作区 root = C:\project
    // - 用户 CWD = C:\project\src\sub
    // 当遍历 src/sub 目录时，walk_root 是 src/sub，但 display_root 需要是共同的根 才能算出正确的相对路径。
    // 这个分离使得 completions() 可以先遍历 CWD（以用户看到的方式显示），再遍历 workspace root，而不会导致路径显示混乱

    // 创建遍历器并迭代
    let builder = discovery_walk_builder(walk_root, max_depth, follow_links);

    // builder.build() 返回 Walk 迭代器。flatten() 用于跳过 Result::Err
    // （如权限拒绝的目录），只保留 Result::Ok 的 DirEntry。
    // 
    // 这是 ignore crate 的惯用用法——Walk在内部通过多线程并行遍历目录树，但对使用者暴露的接口是同步迭代器。
    for entry in builder.build().flatten() {
        if ctx.is_full() {
            break;   // 不需要遍历完整的工作区，拿到足够的候选结果就停。
        }

        // 计算相对路径。strip_prefix 如果失败（路径不在 display_root 下）就跳过。
        // 这在 CWD 遍历和 workspace 遍历时都会发生——只有双方都能正确计算相对路径的文件才会被显示。
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(display_root) else {
            continue;
        };
        
        // 跨平台路径统一。在 Windows 上将反斜杠 \\ 替换为正斜杠 /
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if rel_str.is_empty() {
            continue;       // 跳过根路径本身（当 display_root == walk_root 时可能会发生）。
        }

        // 跨双重遍历去重。ctx.remember(abs) 返回 false
        // 表示这个文件的绝对路径已经见过（在之前的 CWD 遍历中已经处理过）。去重的逻辑是：
        // - 以绝对路径为键，而非相对路径
        // - 这样当 CWD 遍历和 workspace 遍历看到同一个文件时，只有第一次出现被保留（CWD 遍历优先）
        // - 当两者看到同一个文件时，优先用 CWD 相对路径的显示方式
        // 这是 completions() 函数调用 walk_for_completions 两次（CWD 一次、workspace 一次）时保持正确的关键。
        let abs = path.to_path_buf();
        if !ctx.remember(abs) {
            continue;
        }

        // 目录附加 `/` 后缀。如果条目是目录，路径末尾加斜杠（如 src/），方便用户区分文件和目录。
        let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
        let candidate = if is_dir {
            format!("{rel_str}/")
        } else {
            rel_str.clone()
        };

        // 将候选路径传递给 SearchContext 的 push_match 方法，该方法根据 needle前缀命中或子串命中进行分类。
        ctx.push_match(candidate);
    }

    // 补充遍历 AI 工具目录
    // 关键细节：标准遍历遵守 .gitignore，所以如果项目 .gitignore 中写了 .deepseek/ ，标准遍历就找不到
    // 里面的文件。这个补充遍历专门禁用 gitignore，确保 .deepseek/commands/、.cursor/rules/ 等始终可以被 @ 补全找到。
    // Also walk the AI-tool dot-directories with gitignore disabled so
    // `.deepseek/`, `.cursor/`, etc. are always discoverable.
    walk_always_discoverable_dirs(walk_root, display_root, ctx, max_depth, follow_links);
}

/// 本地引用路径扫描的硬上限（4096 个路径）。
/// 防止 `add_local_reference_completions` 在巨型仓库或无 .gitignore 的项目中遍历过多文件。
/// 详见 #1921（WSL2 上 `/mnt/c/` 工作区的 UI 线程卡顿）
const LOCAL_REFERENCE_SCAN_LIMIT: usize = 4096;

/// 参数签名与 `walk_for_completions` 完全相同，但意图不同：
/// - walk_for_completions：遍历工作区内所有文件
/// - add_local_reference_completions：专门处理以 `.` 或路径分隔符开头的本地引用路径（./ 、../、src/ 等形态）
fn add_local_reference_completions(
    root: &Path,
    display_root: &Path,
    ctx: &mut SearchContext<'_>,
    max_depth: Option<usize>,
    follow_links: bool,
) {
    if !should_try_local_reference_completion(ctx.needle) {
        return;
    }

    // 从 local_reference_paths 获取所有路径。注意上限 LOCAL_REFERENCE_SCAN_LIMIT = 4096
    // 最多扫描 4096 个路径，防止巨型仓库导致卡顿。
    for path in local_reference_paths(root, LOCAL_REFERENCE_SCAN_LIMIT, max_depth, follow_links) {
        if ctx.is_full() {
            break;  // 一旦 SearchContext 中收集的候选结果已满（prefix_hits + substring_hits >= limit），立即停止迭代。
        }

        // 计算显示用相对路径。如果路径不在 display_root 下（不可能发生，但防御性编程），跳过。
        let Ok(rel) = path.strip_prefix(display_root) else {
            continue;
        };

        // 跨平台路径统一。Windows 反斜杠 \\ 转正斜杠 /。
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if rel_str.is_empty() || !ctx.remember(path.clone()) {
            continue;   // 路径就是 root 本身（跳过根目录）或在之前的 CWD 或 workspace 遍历中已经出现过
        }
        ctx.push_match(rel_str);
    }
}

/// Rank pre-collected completion candidates for `partial` the same way
/// [`Workspace::completions`] ranks live walk hits: case-insensitive prefix
/// matches first, then substring matches, each bucket alphabetical, truncated
/// to `limit` (#3757).
#[must_use]
pub fn rank_completion_candidates(
    candidates: &[String],
    partial: &str,
    limit: usize,
) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let needle = partial.to_lowercase();
    let mut prefix_hits: Vec<String> = Vec::new();
    let mut substring_hits: Vec<String> = Vec::new();
    for candidate in candidates {
        let lower = candidate.to_lowercase();
        if needle.is_empty() || lower.starts_with(&needle) {
            prefix_hits.push(candidate.clone());
        } else if lower.contains(&needle) {
            substring_hits.push(candidate.clone());
        }
    }
    prefix_hits.sort();
    substring_hits.sort();
    prefix_hits.extend(substring_hits);
    prefix_hits.truncate(limit);
    prefix_hits
}

fn should_try_local_reference_completion(needle: &str) -> bool {
    if needle.is_empty() {
        return false;   // 空 needle → 不需要本地引用
    }
    //  #1921 裸分隔符或裸点不是可操作的路径。没有这个保护，
    // 一个 @/ 按键就会触发 LOCAL_REFERENCE_SCAN_LIMIT (4096 个路径)
    // 的 UI 线程遍历（#1921）——在 WSL2 中 /mnt/c/... 工作区
    // 的每个条目都穿透 Windows 宿主机 I/O，编辑器会卡顿数秒到数分钟。
    if matches!(needle, "/" | "\\" | "." | "..") {
        return false;
    }
    needle.starts_with('.') || needle.contains('/') || needle.contains('\\')
}

/// 遍历工作区获取本地引用路径列表（禁用 gitignore，上限 4096）。
/// 使用 `.deepseekignore` 和 `should_skip_unignored_discovery_entry` 双重过滤。
/// 配合 `should_try_local_reference_completion` 使用，避免不必要的性能开销。
fn local_reference_paths(
    root: &Path,
    limit: usize,       // 扫描上限（传进来的是 4096）
    max_depth: Option<usize>,
    follow_links: bool,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)  // 不显示隐藏文件; 本地引用补全的目的是文件名补全，用户在路径中输入的 .是为了导航（./、../），而不是为了查找隐藏文件。隐藏文件（如 .env、.git/config ）在常规文件列表中没有意义，而且 .git/ 内的文件特别多，隐藏它们避免了大量无用遍历。
        .follow_links(follow_links)
        .git_ignore(false)  // 不遵守 .gitignore
        .git_global(false)  // 不遵守全局 .gitignore
        .git_exclude(false);  // 不遵守 .git/info/exclude
    if let Some(depth) = max_depth {
        builder.max_depth(Some(depth));
    }
    let _ = builder.add_custom_ignore_filename(".deepseekignore");
    let root_for_filter = root.to_path_buf();
    builder.filter_entry(move |entry| {
        // 用于过滤掉虽未被 gitignore 但也不该出现在补全中的路径（如 .deepseek/snapshots/ 中的快照文件）。
        !should_skip_unignored_discovery_entry(&root_for_filter, entry.path())
    });

    for entry in builder.build().flatten() {
        if out.len() >= limit {
            break;
        }
        let path = entry.path();
        if path == root {
            continue;
        }
        if entry
            .file_type()
            .is_some_and(|ft| ft.is_file() || ft.is_dir())
        {
            out.push(path.to_path_buf());
        }
    }
    out
}

impl Clone for Workspace {
    fn clone(&self) -> Self {
        // Don't carry the cached file_index — clones get a fresh OnceLock so
        // they don't pin a stale snapshot of the previous owner's tree.
        Self {
            root: self.root.clone(),
            cwd: self.cwd.clone(),
            file_index: OnceLock::new(),
            completion_walk_depth: self.completion_walk_depth,
            follow_links: self.follow_links,
        }
    }
}

/// 展开 `@~` 和 `@~/...` 提及为用户的 home 目录路径。
/// 读取 `$HOME` 环境变量，仅用于 `Workspace::resolve()` 的路径解析入口。
fn expand_mention_home(path: &str) -> PathBuf {
    if path == "~"
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home);
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

/// 将 `s` 截断至最多 `max_bytes`，并向下对齐到 UTF-8 字符边界，
/// 以确保结果始终有效。返回截断后的切片以及是否发生了截断。
fn truncate_on_char_boundary(s: &str, max_bytes: usize) -> (&str, bool) {
    if s.len() <= max_bytes {
        return (s, false);
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (&s[..end], true)
}

/// 工作集跟踪的配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingSetConfig {
    /// 保留的最大条目数。
    pub max_entries: usize,
    /// 压缩期间固定保留的最大路径数。
    pub max_pinned_paths: usize,
    /// 固定消息时，每个文本块扫描的最大字符数。
    pub max_scan_chars: usize,
    /// 系统提示块中显示的最大条目数。
    pub max_prompt_entries: usize,
    /// 缓存最大化上下文模式（#528）：启用时，工作集块会将当前顶部活跃文件的完整内容
    /// 具体化到系统提示中（确定性顺序，大小受限），而不仅仅是路径列表。
    /// 当文件未更改时，这些内容保持字节稳定，因此 DeepSeek 的 KV 前缀缓存可以持续命中；
    /// 编辑文件会导致从该文件块开始之后的缓存失效。默认关闭 —— 现有行为仅为路径列表。
    #[serde(default)]
    pub cache_maximal: bool,
    /// 缓存最大化模式下，具体化内容的单文件字节数上限。
    #[serde(default = "default_max_resident_file_bytes")]
    pub max_resident_file_bytes: usize,
    /// 缓存最大化模式下，所有具体化文件的总字节数上限。
    #[serde(default = "default_max_total_resident_bytes")]
    pub max_total_resident_bytes: usize,
}

/// `WorkingSetConfig::max_resident_file_bytes` 的默认值：24,000 字节（约 23.4 KB）。
/// 被 serde 的 `#[serde(default = "...")]` 属性引用。
fn default_max_resident_file_bytes() -> usize {
    24_000
}

/// `WorkingSetConfig::max_total_resident_bytes` 的默认值：96,000 字节（约 93.75 KB）。
/// 被 serde 的 `#[serde(default = "...")]` 属性引用。
fn default_max_total_resident_bytes() -> usize {
    96_000
}

/// `WorkingSetConfig` 的默认值。
/// - `max_entries`: 16 —— 工作集最大条目数（保守值，避免提示词膨胀）
/// - `max_pinned_paths`: 8 —— 压缩时保留的路径数
/// - `max_scan_chars`: 2000 —— 钉住消息时扫描的字符上限
/// - `max_prompt_entries`: 8 —— 提示词摘要块中显示的条目数
/// - `cache_maximal`: false —— 默认不启用缓存最大化模式
/// - max_resident_file_bytes: 24_000 —— 单文件缓存上限
/// - max_total_resident_bytes: 96_000 —— 总缓存上限
impl Default for WorkingSetConfig {
    fn default() -> Self {
        Self {
            max_entries: 16,
            max_pinned_paths: 8,
            max_scan_chars: 2_000,
            max_prompt_entries: 8,
            cache_maximal: false,
            max_resident_file_bytes: default_max_resident_file_bytes(),
            max_total_resident_bytes: default_max_total_resident_bytes(),
        }
    }
}

/// 标记路径最后一次被更新的来源。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorkingSetSource {
    UserMessage,    // 用户消息中提到
    ToolInput,      // 工具调用的输入参数中引用
    ToolOutput,     // 工具的输出结果中提到
    Rebuild,        // 从已有消息重新构建时恢复
}

/// A single working-set entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingSetEntry {
    /// 工作区相对路径
    pub path: String,
    /// 是否是目录(best-effort).如果文件后来被删除，这个值可能过时，所以叫"best-effort"
    pub is_dir: bool,
    /// 磁盘上是否存在(best-effort: 文件可能在被记录后被删除).
    pub exists: bool,
    /// 被观察到的次数(这是评分的主要因子（`touches * 4`）)
    pub touches: u32,
    /// 最后被引用的轮次编号.新鲜度字段段。`WorkingSet.turn` 的当前值。用于计算recency_bonus（最近性加分）
    pub last_turn: u64,
    /// 最后更新的来源(仅用于调试/追踪，不参与评分逻辑)
    pub last_source: WorkingSetSource,
}

impl WorkingSetEntry {
    /// 创建一个新的工作集条目。
    /// 
    /// `touches` 初始化为 **1**，使路径首次出现即计为一次有效引用，
    /// 避免因 `touches = 0` 而立即被 `prune()` 淘汰。
    fn new(path: String, exists: bool, is_dir: bool, turn: u64, source: WorkingSetSource) -> Self {
        Self {
            path,
            is_dir,
            exists,
            touches: 1,     // 首次记录就计为一次引用
            last_turn: turn,
            last_source: source,
        }
    }
}

/// Repo-aware working-set state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkingSet {
    /// Tracking 配置参数.
    pub config: WorkingSetConfig,
    /// 单调递增的轮次计数器 (increments on user messages).
    /// `turn` 只在 observe_user_message() 中递增（通过 next_turn()），而observe_tool_call() 不会递增。这意味着
    /// 一轮用户消息（可能包含多次工具调用）共享同一个 turn 编号,即一个用户问题 + N次工具调用都在同一轮。
    pub turn: u64,
    /// map_pair: {路径, 条目}, Path entries keyed by workspace-relative path.
    pub entries: HashMap<String, WorkingSetEntry>,
}

impl WorkingSet {
    /// Advance to the next turn.
    pub fn next_turn(&mut self) {
        self.turn = self.turn.saturating_add(1);
    }

    /// 观察用户消息 and update the working set.
    pub fn observe_user_message(&mut self, text: &str, workspace: &Path) {
        // 递增轮次计数器
        self.next_turn();
        // 从文本中提取文件路径（用正则匹配从消息文本中提取疑似路径的字符串）
        let paths = extract_paths_from_text(text); 
        // 将这些路径录入工作集，标记来源为WorkingSetSource::UserMessage。
        // 后续生成 summary_block时，会把这些近期被提及的文件路径包含进 <turn_meta> 中，让 LLM 知道用户的兴趣点。
        self.record_candidates(paths, workspace, WorkingSetSource::UserMessage);
    }

    /// 观察工具调用 (input and optional output).
    /// 和observe_user_message的差异：不调用 `next_turn()`一轮用户消息可以触发多次工具调用，所有这些调用共享同一个 self.turn 值。
    pub fn observe_tool_call(
        &mut self,
        tool_name: &str,
        input: &Value,
        output: Option<&str>,
        workspace: &Path,
    ) {
        // 输入：递归遍历JSON，寻找path-like的key和value
        let input_candidates = extract_paths_from_value(input, Some(tool_name));
        self.record_candidates(input_candidates, workspace, WorkingSetSource::ToolInput);

        if let Some(text) = output {
            //  输出：使用正则匹配从输出文本中提取
            let output_candidates = extract_paths_from_text(text);
            self.record_candidates(output_candidates, workspace, WorkingSetSource::ToolOutput);
        }
    }

    /// 从已有消息重建(best effort).
    ///
    /// This is used when syncing a resumed session.
    pub fn rebuild_from_messages(&mut self, messages: &[Message], workspace: &Path) {
        self.entries.clear();       // 清空现有条目
        self.turn = 0;              // 重置轮次

        for message in messages {
            if message.role == "user" {
                self.next_turn();   // 每遇到一条用户消息就递增轮次
            }
            let candidates = extract_paths_from_message(message);
            if candidates.is_empty() {
                continue;
            }
            self.record_candidates(candidates, workspace, WorkingSetSource::Rebuild);
        }
    }

    /// 生成最终提示词摘要块,为系统提示渲染一个紧凑的工作集块。
    ///
    /// 当没有观察到新路径时，在连续的 `next_turn()` 调用间保持字节稳定（#280）：
    /// 渲染的行会省略与轮次相关的 `touches` 和 `last seen N turn(s) ago` 字段，
    /// 且顺序取自 `sorted_for_prompt`（与轮次无关）而非 `sorted_entries`。
    /// 该块位于系统提示中历史对话之前；此处任何字节的变化都会导致 DeepSeek KV 前缀缓存中其后的所有内容发生缓存未命中。
    pub fn summary_block(&self, workspace: &Path) -> Option<String> {
        let prompt_entries: Vec<&WorkingSetEntry> = self
            .sorted_for_prompt()        // 按 touches 降序
            .into_iter()
            .take(self.config.max_prompt_entries)   // 取前 N 个(默认8)
            .collect();

        // Key files(etc. "Cargo.toml","README.md" ... 固定的集合)
        // Top-dirs
        let repo_summary = summarize_repo_root(workspace);

        if repo_summary.is_none() && prompt_entries.is_empty() {
            return None;  // 无可展示内容
        }

        // summary_block的示例：
        // ## Repo Working Set
        // Workspace: D:\\tmp\\source\\CodeWhale
        // Key files: Cargo.toml, AGENTS.md, CLAUDE.md, package.json
        // Top-level dirs: assets, benchmark_results, crates, deploy, docs, extensions, fleets, integrations
        // Active paths (prioritize these):
        // - crates/tui/src/working_set.rs (file)
        // - build.md (file)
        // ...
        // When in doubt, use tools to verify and keep changes focused on the working set.
        // Git workspace: main | 1 modified
        let mut lines: Vec<String> = Vec::new();
        lines.push("## Repo Working Set".to_string());
        lines.push(format!("Workspace: {}", workspace.display()));

        if let Some(summary) = repo_summary {
            lines.push(summary);
        }

        if !prompt_entries.is_empty() {
            lines.push("Active paths (prioritize these):".to_string());
            for entry in &prompt_entries {
                let kind = if entry.is_dir { "dir" } else { "file" };
                lines.push(format!("- {} ({kind})", entry.path));
            }
        }

        lines.push(
            // “如有疑问，请使用工具进行验证，并将更改集中在工作集范围内。”
            "When in doubt, use tools to verify and keep changes focused on the working set."
                .to_string(),
        );

        // 缓存最大化模式（#528）：追加当前顶部活跃文件的完整内容，
        // 使模型每轮都能读取实时源码，而无需通过工具重新获取。
        // 放置在路径列表之后，并受限于单文件和总字节数上限；
        // 顺序遵循 `sorted_for_prompt`，以便在文件未发生变化时该块保持字节稳定。
        if self.cache_maximal_enabled() && !prompt_entries.is_empty() {
            self.append_resident_file_contents(&mut lines, workspace, &prompt_entries);
        }

        Some(lines.join("\n"))
    }

    /// 缓存最大化上下文模式是否激活：通过显式配置启用，或通过 `CODEWHALE_CACHE_MAXIMAL` 环境变量切换（`1`/`true`/`on`/`yes`）。
    /// 环境变量值在进程生命周期内保持不变，因此渲染的块在轮次之间保持字节稳定。
    fn cache_maximal_enabled(&self) -> bool {
        if self.config.cache_maximal {
            return true;
        }
        match std::env::var("CODEWHALE_CACHE_MAXIMAL") {
            Ok(v) => matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "on" | "yes"
            ),
            Err(_) => false,
        }
    }

    /// 为常驻文件渲染 `### Active file contents` 块，并遵循单文件和总字节数上限。
    /// 对于无法读取或非 UTF-8 编码的文件，会予以注明而非静默跳过，以便模型能够看到这些遗漏。
    fn append_resident_file_contents(
        &self,
        lines: &mut Vec<String>,
        workspace: &Path,
        prompt_entries: &[&WorkingSetEntry],
    ) {
        let mut header_pushed = false;
        let mut total_bytes: usize = 0;
        let mut omitted: usize = 0;

        for entry in prompt_entries {
            if entry.is_dir || !entry.exists {
                continue;
            }
            if total_bytes >= self.config.max_total_resident_bytes {
                omitted += 1;
                continue;
            }

            let abs = workspace.join(&entry.path);
            let body = match std::fs::read_to_string(&abs) {
                Ok(text) => text,
                Err(_) => {
                    if !header_pushed {
                        lines.push("### Active file contents (cache-resident)".to_string());
                        header_pushed = true;
                    }
                    lines.push(format!(
                        "<!-- file: {} (unreadable, skipped) -->",
                        entry.path
                    ));
                    continue;
                }
            };

            if !header_pushed {
                lines.push("### Active file contents (cache-resident)".to_string());
                header_pushed = true;
            }

            let remaining_total = self
                .config
                .max_total_resident_bytes
                .saturating_sub(total_bytes);
            let cap = self.config.max_resident_file_bytes.min(remaining_total);
            let (shown, truncated) = truncate_on_char_boundary(&body, cap);
            total_bytes += shown.len();

            lines.push(format!("<!-- file: {} -->", entry.path));
            lines.push("```".to_string());
            lines.push(shown.to_string());
            if truncated {
                lines.push(format!(
                    "<!-- ...{} more bytes truncated for prompt budget -->",
                    body.len().saturating_sub(shown.len())
                ));
            }
            lines.push("```".to_string());
        }

        if omitted > 0 {
            lines.push(format!(
                "<!-- {omitted} additional active file(s) omitted from the cache-resident budget -->"
            ));
        }
    }

    /// Return the most relevant paths in score order.
    pub fn top_paths(&self, limit: usize) -> Vec<String> {
        self.sorted_entries()
            .into_iter()
            .take(limit)
            .map(|entry| entry.path.clone())
            .collect()
    }

    /// Identify message indices that should be pinned during compaction.
    pub fn pinned_message_indices(&self, messages: &[Message], workspace: &Path) -> Vec<usize> {
        if messages.is_empty() || self.entries.is_empty() {
            return Vec::new();
        }

        let pinned_paths: Vec<&WorkingSetEntry> = self
            .sorted_entries()
            .into_iter()
            .take(self.config.max_pinned_paths)
            .collect();
        if pinned_paths.is_empty() {
            return Vec::new();
        }

        let needles = build_search_needles(&pinned_paths, workspace);
        if needles.is_empty() {
            return Vec::new();
        }

        let mut pinned: Vec<usize> = Vec::new();
        for (idx, message) in messages.iter().enumerate() {
            if message_mentions_any_path(message, &needles, self.config.max_scan_chars) {
                pinned.push(idx);
            }
        }
        pinned
    }

    /// 候选路径记录的三阶段管道：
    /// 1. `normalize_candidate()` — 去除两端空白和引号
    /// 2. `relativize_candidate()` — 安全检查 + 转为工作区相对路径 + 磁盘存在性检测
    /// 3. `record_path()` — 更新或插入 WorkingSetEntry
    /// 最后调用 `prune()` 检查是否超出 max_entries 上限。
    fn record_candidates(
        &mut self,
        candidates: Vec<String>,
        workspace: &Path,
        source: WorkingSetSource,
    ) {
        if candidates.is_empty() {
            return;
        }

        let workspace_canon = workspace.canonicalize().ok();

        for raw in candidates {
            let Some(normalized) = normalize_candidate(&raw) else {
                continue;
            };
            let Some((rel, exists, is_dir)) =
                relativize_candidate(&normalized, workspace, workspace_canon.as_deref())
            else {
                continue;
            };
            self.record_path(rel, exists, is_dir, source);
        }

        self.prune();
    }
    
    /// 记录或更新一条工作集路径。
    /// - 已存在：`exists` 和 `is_dir` 用按位或合并（一旦为 true 就保持 true），
    ///   `touches` 会进一，`last_turn` 和 `last_source` 更新为当前值。
    /// - 不存在：调用 `WorkingSetEntry::new` 创建新条目（touches 初始化为 1）
    fn record_path(&mut self, rel: String, exists: bool, is_dir: bool, source: WorkingSetSource) {
        match self.entries.get_mut(&rel) {
            Some(entry) => {
                entry.exists |= exists;
                entry.is_dir |= is_dir;
                entry.touches = entry.touches.saturating_add(1);
                entry.last_turn = self.turn;
                entry.last_source = source;
            }
            None => {
                let entry = WorkingSetEntry::new(rel.clone(), exists, is_dir, self.turn, source);
                let _ = self.entries.insert(rel, entry);
            }
        }
    }

    /// 检查工作集条目数是否超过 `config.max_entries` 上限。
    /// 若超出，按 `score_entry()` 得分升序排列，移除分数最低的超额条目。
    /// 每次 `record_candidates()` 完成后自动调用。
    fn prune(&mut self) {
        let max_entries = self.config.max_entries;
        if self.entries.len() <= max_entries {
            return;
        }

        // Rank by score ascending and drop the lowest until within bounds.
        let mut ranked: Vec<(String, i64)> = self
            .entries
            .values()
            .map(|entry| (entry.path.clone(), score_entry(entry, self.turn)))
            .collect();
        ranked.sort_by_key(|a| a.1);

        let to_remove = self.entries.len().saturating_sub(max_entries);
        for (path, _) in ranked.into_iter().take(to_remove) {
            let _ = self.entries.remove(&path);
        }
    }

    fn sorted_entries(&self) -> Vec<&WorkingSetEntry> {
        let mut entries: Vec<&WorkingSetEntry> = self.entries.values().collect();
        entries.sort_by(|a, b| {
            let sb = score_entry(b, self.turn);
            let sa = score_entry(a, self.turn);
            sb.cmp(&sa).then_with(|| a.path.cmp(&b.path))
        });
        entries
    }

    /// 根据WorkingSetEntry::touches对self.entries排序后输出Vec.
    /// 
    /// 渲染提示摘要块时使用的与轮次无关的排序。
    /// `sorted_entries` 混合了来自 `self.turn` 的新近度加成，因此即使没有触及新路径，
    /// 其输出也会随着轮次推进而重新排序——这种变动会跨越 `max_prompt_entries` 边界，
    /// 从而破坏 KV 前缀缓存（#280）。压缩固定策略仍使用感知新近度的 `sorted_entries`；
    /// 此处仅对面向提示的表面层进行稳定处理。
    fn sorted_for_prompt(&self) -> Vec<&WorkingSetEntry> {
        let mut entries: Vec<&WorkingSetEntry> = self.entries.values().collect();
        entries.sort_by(|a, b| b.touches.cmp(&a.touches).then_with(|| a.path.cmp(&b.path)));
        entries
    }
}

/// 工作集条目评分函数。
/// 公式：`touches * 4 + recency_bonus(age)`
/// 新鲜度加分：age=0→+6、1→+4、2→+3、3-5→+2、6-10→+1、>10→0
/// touches 的权重（×4）确保高频引用路径的基线分数远高于偶发路径。
fn score_entry(entry: &WorkingSetEntry, current_turn: u64) -> i64 {
    let age = current_turn.saturating_sub(entry.last_turn);
    let recency_bonus = match age {
        0 => 6,
        1 => 4,
        2 => 3,
        3..=5 => 2,
        6..=10 => 1,
        _ => 0,
    };
    i64::from(entry.touches) * 4 + recency_bonus
}

/// 清理候选路径字符串：去除两端空白，去除可能附着在路径周围的标点符号
/// （引号、逗号、分号、冒号、括号等）。空结果返回 None。
fn normalize_candidate(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | '`' | ',' | ';' | ':' | '(' | ')' | '[' | ']'
        )
    });
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

/// 将候选路径转换为工作区相对路径并检测磁盘存在性。
/// - 拒绝包含 `://` 的 URL
/// - 绝对路径必须在工作区内
/// - 相对路径拒绝 `../` 父目录逃逸
/// - 调用 `clean_relative` 解析 `./` 和 `../` 中间组件
/// 返回 `(工作区相对路径字符串, 是否存在, 是否是目录)` 三元组。
fn relativize_candidate(
    candidate: &str,
    workspace: &Path,
    workspace_canon: Option<&Path>,
) -> Option<(String, bool, bool)> {
    let candidate_path = Path::new(candidate);

    // Reject obvious URLs and non-paths early.
    if candidate.contains("://") {
        return None;
    }

    let (rel_path, abs_path) = if candidate_path.is_absolute() {
        let within_workspace = workspace_canon
            .map(|ws| candidate_path.starts_with(ws))
            .unwrap_or_else(|| candidate_path.starts_with(workspace));
        if !within_workspace {
            return None;
        }
        let rel = candidate_path.strip_prefix(workspace).ok()?.to_path_buf();
        (rel, candidate_path.to_path_buf())
    } else {
        if starts_with_parent_dir(candidate_path) {
            return None;
        }
        let rel = clean_relative(candidate_path);
        let abs = workspace.join(&rel);
        (rel, abs)
    };

    let metadata = fs::metadata(&abs_path).ok();
    let exists = metadata.is_some();
    let is_dir = metadata
        .as_ref()
        .map(fs::Metadata::is_dir)
        .unwrap_or_else(|| candidate.ends_with('/'));

    let rel_string = path_to_string(&rel_path)?;
    Some((rel_string, exists, is_dir))
}

/// 检查路径是否以 `../`（父目录）开头。
/// 用于 `relativize_candidate()` 中拒绝工作区外的路径逃逸。
fn starts_with_parent_dir(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(std::path::Component::ParentDir)
    )
}

/// 规范化相对路径：解析 `./`（当前目录）和 `../`（父目录）组件。
/// 不处理绝对路径和跨平台前缀。
/// 例如：`src/./lib/../main.rs` → `src/main.rs`
fn clean_relative(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut parts: Vec<PathBuf> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = parts.pop();
            }
            Component::Normal(p) => parts.push(PathBuf::from(p)),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    let mut out = PathBuf::new();
    for part in parts {
        out.push(part);
    }
    out
}

/// 将 `Path` 转换为正斜杠分隔的字符串。
/// 在 Windows 上将 `\\` 替换为 `/`，确保跨平台路径格式一致。
/// 路径包含非 UTF-8 字符时返回 None。
fn path_to_string(path: &Path) -> Option<String> {
    path.as_os_str().to_str().map(|s| s.replace('\\', "/"))
}

/// 从消息的所有 ContentBlock 中提取疑似文件路径。
/// 处理 `Text`（文本）、`ToolUse`（工具输入 JSON）、`ToolResult`（工具输出文本）三种块类型。
/// 用于会话重建时从历史消息恢复工作集。
fn extract_paths_from_message(message: &Message) -> Vec<String> {
    let mut paths = Vec::new();
    for block in &message.content {
        match block {
            ContentBlock::Text { text, .. } => {
                paths.extend(extract_paths_from_text(text));
            }
            ContentBlock::ToolUse { input, .. } => {
                paths.extend(extract_paths_from_value(input, None));
            }
            ContentBlock::ToolResult { content, .. } => {
                paths.extend(extract_paths_from_text(content));
            }
            ContentBlock::Thinking { .. }
            | ContentBlock::ServerToolUse { .. }
            | ContentBlock::ToolSearchToolResult { .. }
            | ContentBlock::CodeExecutionToolResult { .. }
            | ContentBlock::ImageUrl { .. } => {}
        }
    }
    paths
}

/// 从 `serde_json::Value` 中递归提取疑似文件路径。
/// 使用 `tool_hint`（工具名称）辅助判断：对于 `exec_shell` 工具，即使字符串不含分隔符也尝试提取。
/// 入口函数，委托给 `extract_paths_from_value_inner`。
fn extract_paths_from_value(value: &Value, tool_hint: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    extract_paths_from_value_inner(value, tool_hint, None, &mut out);
    out
}


/// `extract_paths_from_value` 的递归内部实现。
/// - `Value::String`：如果 key 名 path-like 或值 looks_like_path 则提取；
///   对 `exec_shell` 工具，短字符串（<400 字符）也尝试提取。
/// - `Value::Array`：递归每个元素
/// - `Value::Object`：递归每个值，以 key 名作为 `key_hint`
fn extract_paths_from_value_inner(
    value: &Value,
    tool_hint: Option<&str>,
    key_hint: Option<&str>,
    out: &mut Vec<String>,
) {
    match value {
        Value::String(s) => {
            let key_suggests_path = key_hint.map(key_is_path_like).unwrap_or(false);
            if key_suggests_path || looks_like_path(s) {
                out.extend(extract_paths_from_text(s));
                if key_suggests_path && !s.contains('/') && !s.contains('\\') {
                    out.push(s.to_string());
                }
            } else if tool_hint == Some("exec_shell") && s.len() < 400 {
                out.extend(extract_paths_from_text(s));
            }
        }
        Value::Array(arr) => {
            for item in arr {
                extract_paths_from_value_inner(item, tool_hint, key_hint, out);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                extract_paths_from_value_inner(v, tool_hint, Some(k.as_str()), out);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// 判断 JSON 对象的 key 名是否暗示对应的 value 可能是文件路径。
/// 匹配以下词汇（不区分大小写）："path"、"file"、"dir"、"cwd"、"workspace"、"root",
/// 以及精确匹配 "target"。
fn key_is_path_like(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.contains("path")
        || lower.contains("file")
        || lower.contains("dir")
        || lower.contains("cwd")
        || lower.contains("workspace")
        || lower.contains("root")
        || lower == "target"
}

/// 启发式判断字符串是否可能是文件路径。
/// 条件之一满足即可：
/// 1. 包含路径分隔符（`/` 或 `\`）
/// 2. 扩展名（如 `.rs`、`.py`、`.toml`、`.md` 等）在 `COMMON_EXTENSIONS` 列表中
fn looks_like_path(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return true;
    }
    match Path::new(trimmed).extension().and_then(OsStr::to_str) {
        Some(ext) => COMMON_EXTENSIONS.contains(&ext),
        None => false,
    }
}

/// 常见源文件和配置文件的扩展名列表。
/// 用于 `looks_like_path()` 的启发式判断：当字符串不包含路径分隔符时，
/// 如果其扩展名在此列表中，仍被视为"可能是文件路径"。
const COMMON_EXTENSIONS: &[&str] = &[
    "rs", "toml", "md", "txt", "json", "yaml", "yml", "ts", "tsx", "js", "jsx", "py", "go", "java",
    "c", "cc", "cpp", "h", "hpp", "sh", "bash", "zsh", "sql", "html", "css", "scss",
];

/// 使用正则表达式从用户文本中解析出看起来像文件路径的 token（包含 / 或 \ 分隔符的 token，或者带有常见扩展名如
/// .rs、.py、.toml 等的 token）。
fn extract_paths_from_text(text: &str) -> Vec<String> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    let re = path_regex();
    re.find_iter(text)
        .map(|m| m.as_str().to_string())
        .filter(|s| looks_like_path(s))
        .collect()
}

/// 使用 `OnceLock` 编译并缓存的正则表达式，用于从文本中匹配疑似文件路径。
/// 匹配两种格式：
/// 1. 带路径分隔符的（`a/b/c`、`C:\a\b\c`）
/// 2. 纯 `filename.ext` 格式
/// `OnceLock` 确保正则只编译一次，后续复用。
fn path_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Path-ish tokens with separators or file extensions.
        Regex::new(
            r#"(?x)
            (?:
                (?:[A-Za-z]:\\)?                # 可选的 Windows 盘符
                (?:\./|\../|/)?                 # 可选的前导路径
                [A-Za-z0-9._-]+
                (?:[/\\][A-Za-z0-9._-]+)+
                (?:\.[A-Za-z0-9]{1,8})?         # 可选的拓展名
            )
            |
            (?:
                [A-Za-z0-9._-]+\.[A-Za-z0-9]{1,8}
            )
            "#,
        )
        .expect("path regex should compile")
    })
}

/// 按字符数（而非字节数）截断字符串，返回前 `max_chars` 个字符。
/// 与 `truncate_on_char_boundary`（按字节截断）不同，此函数在字符边界上截断。
/// 用于 `message_mentions_any_path` 中限制消息扫描范围。
fn truncate_chars(text: &str, max_chars: usize) -> &str {
    if max_chars == 0 {
        return "";
    }
    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => &text[..idx],
        None => text,
    }
}

/// 从工作集条目构建搜索关键词（needles）集合。
/// 每个条目生成两个 needle：工作区相对路径和绝对路径，
/// 确保在消息的文本和 JSON 中都能匹配到路径引用。
/// 用于 `pinned_message_indices()` 的路径匹配
fn build_search_needles(entries: &[&WorkingSetEntry], workspace: &Path) -> Vec<String> {
    let mut needles: HashSet<String> = HashSet::new();
    for entry in entries {
        let rel = entry.path.clone();
        if rel.is_empty() {
            continue;
        }
        let abs = workspace.join(&rel);
        let abs_str = abs.as_os_str().to_str().map(ToOwned::to_owned);

        let _ = needles.insert(rel.clone());
        if let Some(abs_str) = abs_str {
            let _ = needles.insert(abs_str);
        }
    }
    needles.into_iter().collect()
}

/// 检查消息是否包含对任何工作集路径的引用。
/// 扫描 Text、ToolUse（JSON 序列化后）、ToolResult 三种内容块，
/// 受 `max_scan_chars` 字符上限保护（防止消息体过大导致性能问题）。
/// 用于 `pinned_message_indices()` 确定哪些消息应在压缩时保留。
fn message_mentions_any_path(message: &Message, needles: &[String], max_scan_chars: usize) -> bool {
    if needles.is_empty() {
        return false;
    }
    for block in &message.content {
        match block {
            ContentBlock::Text { text, .. } => {
                let snippet = truncate_chars(text, max_scan_chars);
                if contains_any(snippet, needles) {
                    return true;
                }
            }
            ContentBlock::ToolUse { input, .. } => {
                if let Ok(json) = serde_json::to_string(input)
                    && contains_any(&json, needles)
                {
                    return true;
                }
            }
            ContentBlock::ToolResult { content, .. } => {
                let snippet = truncate_chars(content, max_scan_chars);
                if contains_any(snippet, needles) {
                    return true;
                }
            }
            ContentBlock::Thinking { .. }
            | ContentBlock::ServerToolUse { .. }
            | ContentBlock::ToolSearchToolResult { .. }
            | ContentBlock::CodeExecutionToolResult { .. }
            | ContentBlock::ImageUrl { .. } => {}
        }
    }
    false
}

/// 检查 `text` 是否包含 `needles` 中的任一字符串。
/// 自动跳过空 needle。不区分大小写？否——保持原始匹配。
/// 用于 `message_mentions_any_path` 中的朴素子串匹配。
fn contains_any(text: &str, needles: &[String]) -> bool {
    needles
        .iter()
        .any(|needle| !needle.is_empty() && text.contains(needle))
}

/// 生成仓库根目录摘要文本，用于 `summary_block()`。
/// 组合 `detect_key_files()` 和 `list_top_level_dirs()` 的结果。
/// 两个列表都为空时返回 `None`。
/// 输出示例：
///   Key files: Cargo.toml, AGENTS.md
///   Top-level dirs: src, docs
fn summarize_repo_root(workspace: &Path) -> Option<String> {
    let key_files = detect_key_files(workspace);
    let top_dirs = list_top_level_dirs(workspace, 8);

    if key_files.is_empty() && top_dirs.is_empty() {
        return None;
    }

    let mut parts: Vec<String> = Vec::new();
    if !key_files.is_empty() {
        parts.push(format!("Key files: {}", key_files.join(", ")));
    }
    if !top_dirs.is_empty() {
        parts.push(format!("Top-level dirs: {}", top_dirs.join(", ")));
    }
    Some(parts.join("\n"))
}

/// 检测工作区根目录下是否存在预定义的关键项目文件。
/// 候选列表：`Cargo.toml`、`README.md`、`AGENTS.md`、`CLAUDE.md`、
/// `package.json`、`pyproject.toml`、`go.mod`、`Makefile`。
/// 返回存在的文件名列表，用于提示词中向模型展示项目概况。
fn detect_key_files(workspace: &Path) -> Vec<String> {
    const CANDIDATES: &[&str] = &[
        "Cargo.toml",
        "README.md",
        "AGENTS.md",
        "CLAUDE.md",
        "package.json",
        "pyproject.toml",
        "go.mod",
        "Makefile",
    ];

    CANDIDATES
        .iter()
        .filter_map(|name| {
            let path = workspace.join(name);
            if path.exists() {
                Some((*name).to_string())
            } else {
                None
            }
        })
        .collect()
}

/// 列出工作区根目录的一级子目录。
/// 过滤隐藏目录（`.` 开头）和已知忽略目录（`target`、`node_modules`、`dist`、`build`、`.git`）。
/// 返回排序去重后的目录名列表，上限由 `limit` 控制。
fn list_top_level_dirs(workspace: &Path, limit: usize) -> Vec<String> {
    let mut dirs = Vec::new();
    let entries = match fs::read_dir(workspace) {
        Ok(entries) => entries,
        Err(_) => return dirs,
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };

        if name.starts_with('.') || IGNORED_ROOT_DIRS.contains(&name) {
            continue;
        }

        if let Ok(meta) = entry.metadata()
            && meta.is_dir()
        {
            dirs.push(name.to_string());
        }

        if dirs.len() >= limit {
            break;
        }
    }

    dirs.sort();
    dirs
}

/// `list_top_level_dirs()` 中跳过的不展示根级目录名称。
/// 这些是构建产物、依赖管理或版本控制目录，对项目结构展示无帮助
const IGNORED_ROOT_DIRS: &[&str] = &["target", "node_modules", "dist", "build", ".git"];

#[cfg(test)]
mod tests {
    // 测试模块覆盖：WorkingSet（路径追踪、钉住消息、摘要块、缓存最大化）+
    // Workspace（路径解析、自动补全、浏览器模式、文件索引）+ 路径提取函数
    use super::*;
    use tempfile::TempDir;

    /// 测试辅助函数：快速构造一条仅包含 Text 块的 Message。
    fn make_message(role: &str, text: &str) -> Message {
        Message {
            role: role.to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    #[test]
    fn observe_user_message_tracks_paths() {
        let tmp = TempDir::new().expect("tempdir");
        let src = tmp.path().join("src");
        let file = src.join("lib.rs");
        fs::create_dir_all(&src).expect("mkdir");
        fs::write(&file, "pub fn x() {}").expect("write");

        let mut ws = WorkingSet::default();
        ws.observe_user_message("Please check src/lib.rs", tmp.path());

        assert!(ws.entries.contains_key("src/lib.rs"));
        let entry = ws.entries.get("src/lib.rs").expect("entry");
        assert!(entry.exists);
        assert!(!entry.is_dir);
    }

    #[test]
    fn observe_tool_call_extracts_paths_from_input() {
        let tmp = TempDir::new().expect("tempdir");
        let file = tmp.path().join("Cargo.toml");
        fs::write(&file, "[package]\nname = \"x\"").expect("write");

        let mut ws = WorkingSet::default();
        let input = serde_json::json!({ "path": "Cargo.toml" });
        ws.observe_tool_call("read_file", &input, None, tmp.path());

        assert!(ws.entries.contains_key("Cargo.toml"));
    }

    #[test]
    fn pinned_message_indices_respects_working_set() {
        let tmp = TempDir::new().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        let file = src.join("main.rs");
        fs::write(&file, "fn main() {}").expect("write");

        let mut ws = WorkingSet::default();
        ws.observe_user_message("Edit src/main.rs", tmp.path());

        let messages = vec![
            make_message("user", "Unrelated text"),
            make_message("assistant", "I will read src/main.rs next."),
            make_message("user", "More unrelated text"),
        ];

        let pinned = ws.pinned_message_indices(&messages, tmp.path());
        assert_eq!(pinned, vec![1]);
    }

    #[test]
    fn summary_block_includes_repo_and_working_set() {
        let tmp = TempDir::new().expect("tempdir");
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"").expect("write");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        fs::write(src.join("lib.rs"), "pub fn x() {}").expect("write");

        let mut ws = WorkingSet::default();
        ws.observe_user_message("src/lib.rs", tmp.path());
        let block = ws.summary_block(tmp.path()).expect("block");

        assert!(block.contains("Repo Working Set"));
        assert!(block.contains("Cargo.toml"));
        assert!(block.contains("src"));
        assert!(block.contains("src/lib.rs"));
    }

    /// #280 regression: `summary_block` must produce byte-identical output
    /// across `next_turn()` advances when no new paths are touched. Prior to
    /// the fix, the rendered lines interpolated `entry.touches` and
    /// `self.turn - entry.last_turn`, both of which drift turn-over-turn even
    /// when the path set is unchanged. The drift busted DeepSeek's KV prefix
    /// cache on every user message because the working-set block lands in the
    /// system prompt before the historical conversation.
    #[test]
    fn summary_block_is_byte_stable_across_next_turn_when_no_new_paths_observed() {
        use crate::test_support::assert_byte_identical;

        let tmp = TempDir::new().expect("tempdir");
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"").expect("write");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        fs::write(src.join("a.rs"), "a").expect("write");
        fs::write(src.join("b.rs"), "b").expect("write");

        let mut ws = WorkingSet::default();
        ws.observe_user_message("Edit src/a.rs and src/b.rs", tmp.path());

        let before = ws.summary_block(tmp.path()).expect("block before");
        ws.next_turn();
        let after = ws.summary_block(tmp.path()).expect("block after");

        assert_byte_identical(
            "summary_block must be stable across next_turn when no new paths touched",
            &before,
            &after,
        );
    }

    /// Companion to the byte-stability test: a fresh path *should* invalidate
    /// the block (the KV cache is allowed to miss when there's genuinely new
    /// signal), so the model still sees newly touched paths after the block
    /// stabilises across no-op turns.
    #[test]
    fn summary_block_changes_when_a_new_path_is_observed() {
        let tmp = TempDir::new().expect("tempdir");
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"").expect("write");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        fs::write(src.join("a.rs"), "a").expect("write");
        fs::write(src.join("c.rs"), "c").expect("write");

        let mut ws = WorkingSet::default();
        ws.observe_user_message("src/a.rs", tmp.path());
        let before = ws.summary_block(tmp.path()).expect("block before");

        ws.observe_user_message("src/c.rs", tmp.path());
        let after = ws.summary_block(tmp.path()).expect("block after");

        assert_ne!(before, after, "new path must update the rendered summary");
        assert!(after.contains("src/c.rs"));
    }

    // ── Cache-maximal context mode (#528) ──
    // Tests drive the flag through `config.cache_maximal` directly so they
    // don't touch the process-wide `CODEWHALE_CACHE_MAXIMAL` env var (which
    // would race with parallel tests).

    fn cache_maximal_ws() -> WorkingSet {
        let mut ws = WorkingSet::default();
        ws.config.cache_maximal = true;
        ws
    }

    #[test]
    fn cache_maximal_off_keeps_path_list_only() {
        let tmp = TempDir::new().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        fs::write(src.join("lib.rs"), "pub fn hello() {}").expect("write");

        let mut ws = WorkingSet::default(); // cache_maximal defaults to false
        ws.observe_user_message("src/lib.rs", tmp.path());
        let block = ws.summary_block(tmp.path()).expect("block");

        assert!(block.contains("src/lib.rs"), "path list still present");
        assert!(
            !block.contains("Active file contents"),
            "no materialized contents when the flag is off"
        );
        assert!(!block.contains("pub fn hello"));
    }

    #[test]
    fn cache_maximal_on_materializes_file_contents() {
        let tmp = TempDir::new().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        fs::write(src.join("lib.rs"), "pub fn hello() {}").expect("write");

        let mut ws = cache_maximal_ws();
        ws.observe_user_message("src/lib.rs", tmp.path());
        let block = ws.summary_block(tmp.path()).expect("block");

        assert!(block.contains("Active file contents (cache-resident)"));
        assert!(block.contains("<!-- file: src/lib.rs -->"));
        assert!(block.contains("pub fn hello() {}"));
    }

    #[test]
    fn cache_maximal_directories_are_not_materialized() {
        let tmp = TempDir::new().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");

        let mut ws = cache_maximal_ws();
        ws.observe_user_message("look in src/", tmp.path());
        let block = ws.summary_block(tmp.path()).expect("block");

        // `src` is a dir; it appears in the path list but has no content block.
        assert!(!block.contains("<!-- file: src -->"));
    }

    #[test]
    fn cache_maximal_respects_per_file_byte_cap() {
        let tmp = TempDir::new().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        let big = "x".repeat(10_000);
        fs::write(src.join("big.rs"), &big).expect("write");

        let mut ws = cache_maximal_ws();
        ws.config.max_resident_file_bytes = 100;
        ws.config.max_total_resident_bytes = 10_000;
        ws.observe_user_message("src/big.rs", tmp.path());
        let block = ws.summary_block(tmp.path()).expect("block");

        assert!(block.contains("truncated for prompt budget"));
        // The full 10k body must not be inlined.
        assert!(!block.contains(&big));
    }

    #[test]
    fn cache_maximal_total_cap_omits_extra_files() {
        let tmp = TempDir::new().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        fs::write(src.join("a.rs"), "a".repeat(200)).expect("write");
        fs::write(src.join("b.rs"), "b".repeat(200)).expect("write");

        let mut ws = cache_maximal_ws();
        ws.config.max_resident_file_bytes = 200;
        ws.config.max_total_resident_bytes = 200; // only one file fits
        ws.observe_user_message("Edit src/a.rs and src/b.rs", tmp.path());
        let block = ws.summary_block(tmp.path()).expect("block");

        assert!(
            block.contains("omitted from the cache-resident budget"),
            "second file should be reported as omitted:\n{block}"
        );
    }

    #[test]
    fn cache_maximal_is_byte_stable_when_files_unchanged() {
        use crate::test_support::assert_byte_identical;

        let tmp = TempDir::new().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        fs::write(src.join("a.rs"), "fn a() {}").expect("write");

        let mut ws = cache_maximal_ws();
        ws.observe_user_message("src/a.rs", tmp.path());
        let before = ws.summary_block(tmp.path()).expect("before");
        ws.next_turn();
        let after = ws.summary_block(tmp.path()).expect("after");

        assert_byte_identical(
            "cache-maximal block must be stable while files are unchanged (KV cache hit)",
            &before,
            &after,
        );
    }

    #[test]
    fn cache_maximal_changes_when_file_edited() {
        let tmp = TempDir::new().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        let file = src.join("a.rs");
        fs::write(&file, "fn a() {}").expect("write");

        let mut ws = cache_maximal_ws();
        ws.observe_user_message("src/a.rs", tmp.path());
        let before = ws.summary_block(tmp.path()).expect("before");

        fs::write(&file, "fn a() { todo!() }").expect("rewrite");
        let after = ws.summary_block(tmp.path()).expect("after");

        assert_ne!(before, after, "editing the file must change the block");
        assert!(after.contains("todo!()"));
    }

    #[test]
    fn extract_paths_from_message_picks_up_tool_results() {
        let msg = Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool_1".to_string(),
                content: "Changed src/compaction.rs".to_string(),
                is_error: None,
                content_blocks: None,
            }],
        };

        let paths = extract_paths_from_message(&msg);
        assert!(paths.iter().any(|p| p.contains("src/compaction.rs")));
    }

    #[test]
    fn pinning_prefers_high_signal_paths() {
        let tmp = TempDir::new().expect("tempdir");
        fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
        fs::write(tmp.path().join("src/a.rs"), "a").expect("write");
        fs::write(tmp.path().join("src/b.rs"), "b").expect("write");

        let mut ws = WorkingSet::default();
        ws.observe_user_message("src/a.rs", tmp.path());
        ws.observe_tool_call(
            "read_file",
            &serde_json::json!({ "path": "src/a.rs" }),
            Some("src/a.rs"),
            tmp.path(),
        );
        ws.observe_user_message("src/b.rs", tmp.path());

        let a_score = score_entry(ws.entries.get("src/a.rs").expect("a"), ws.turn);
        let b_score = score_entry(ws.entries.get("src/b.rs").expect("b"), ws.turn);
        assert!(a_score >= b_score);
    }

    #[test]
    fn estimate_tokens_is_available_for_future_budgeting() {
        use crate::compaction::estimate_tokens;
        let messages = vec![make_message("user", "src/main.rs")];
        assert!(estimate_tokens(&messages) > 0);
    }

    #[test]
    fn workspace_resolve_respects_cwd_and_workspace() {
        let tmp = TempDir::new().unwrap();

        let sub = tmp.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let bar = sub.join("bar.txt");
        std::fs::write(&bar, "bar").unwrap();

        let nested = tmp.path().join("nested/deep");
        std::fs::create_dir_all(&nested).unwrap();
        let file_md = nested.join("file.md");
        std::fs::write(&file_md, "md").unwrap();

        // Construct with an explicit cwd so the test doesn't race with other
        // tests that mutate the real process cwd.
        let ws = Workspace::with_cwd(tmp.path().to_path_buf(), Some(sub.clone()));

        // #101 repro #1: @bar.txt with cwd=sub MUST resolve via the cwd pass,
        // never to the bogus workspace path tmp/bar.txt (which doesn't exist).
        let res1 = ws.resolve("bar.txt").unwrap();
        assert_eq!(
            res1.canonicalize().unwrap_or(res1.clone()),
            bar.canonicalize().unwrap_or(bar.clone())
        );
        let wrong = tmp.path().join("bar.txt");
        assert_ne!(res1, wrong, "must not have routed to workspace fallback");

        // #101 repro #2: @nested/deep/file.md falls through to workspace root.
        let res2 = ws.resolve("nested/deep/file.md").unwrap();
        assert_eq!(
            res2.canonicalize().unwrap_or(res2),
            file_md.canonicalize().unwrap_or(file_md)
        );
    }

    /// Negative test (#101): a truly missing path returns `Err` with a path
    /// that callers can show to the user as a signal of failure.
    #[test]
    fn workspace_resolve_returns_err_for_truly_missing_path() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::with_cwd(tmp.path().to_path_buf(), Some(tmp.path().to_path_buf()));

        let res = ws.resolve("does/not/exist.txt");
        assert!(res.is_err(), "expected Err for missing path, got: {res:?}");
    }

    /// `Workspace::completions` returns workspace-relative entries for files
    /// under the root, and cwd-relative entries when the cwd-only file lives
    /// outside the workspace tree. Honors `.gitignore`.
    #[test]
    fn workspace_completions_walk_surfaces_workspace_and_cwd() {
        let tmp = TempDir::new().unwrap();
        // Two trees: a workspace under `ws/` and a cwd under `cwd/` that is
        // NOT inside the workspace, so the two walks are disjoint and we can
        // assert each branch contributed.
        let ws_root = tmp.path().join("ws");
        let cwd_root = tmp.path().join("cwd");
        std::fs::create_dir_all(&ws_root).unwrap();
        std::fs::create_dir_all(&cwd_root).unwrap();
        std::fs::write(ws_root.join("alpha.txt"), "a").unwrap();
        std::fs::write(cwd_root.join("alphabeta.txt"), "b").unwrap();

        let ws = Workspace::with_cwd(ws_root.clone(), Some(cwd_root.clone()));
        let entries = ws.completions("alpha", 16);
        assert!(
            entries.iter().any(|e| e == "alpha.txt"),
            "expected workspace entry alpha.txt; got: {entries:?}",
        );
        assert!(
            entries.iter().any(|e| e == "alphabeta.txt"),
            "expected cwd entry alphabeta.txt; got: {entries:?}",
        );
    }

    #[test]
    fn workspace_completions_honor_configured_walk_depth() {
        let tmp = TempDir::new().unwrap();
        // Sits at component depth 12, past the default walk depth (10) but
        // within the explicit deeper walk (16) below.
        let deep_dir = tmp.path().join("a/b/c/d/e/f/g/h/i/j/k");
        std::fs::create_dir_all(&deep_dir).unwrap();
        std::fs::write(deep_dir.join("target.txt"), "target").unwrap();

        let default_ws = Workspace::with_cwd(tmp.path().to_path_buf(), None);
        let default_entries = default_ws.completions("target", 16);
        assert!(
            !default_entries
                .iter()
                .any(|entry| entry.ends_with("target.txt")),
            "default depth should keep very deep entries out of the hot completion path: {default_entries:?}",
        );

        let deep_ws = Workspace::with_cwd_and_depth(tmp.path().to_path_buf(), None, 16);
        let deep_entries = deep_ws.completions("target", 16);
        assert!(
            deep_entries
                .iter()
                .any(|entry| entry.ends_with("target.txt")),
            "configured deeper walk should surface the nested file: {deep_entries:?}",
        );

        let unlimited_ws = Workspace::with_cwd_and_depth(tmp.path().to_path_buf(), None, 0);
        let unlimited_entries = unlimited_ws.completions("target", 16);
        assert!(
            unlimited_entries
                .iter()
                .any(|entry| entry.ends_with("target.txt")),
            "depth 0 should disable the completion walk depth limit: {unlimited_entries:?}",
        );
    }

    #[test]
    fn browser_completions_show_only_immediate_children() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("src/nested")).unwrap();
        std::fs::write(tmp.path().join("src/lib.rs"), "lib").unwrap();
        std::fs::write(tmp.path().join("src/nested/deep.rs"), "deep").unwrap();
        std::fs::write(tmp.path().join("README.md"), "readme").unwrap();

        let ws = Workspace::with_cwd(tmp.path().to_path_buf(), None);

        let root_entries = ws.browser_completions("", 16);
        assert_eq!(root_entries, vec!["README.md", "src/"]);

        let src_entries = ws.browser_completions("src/", 16);
        assert_eq!(src_entries, vec!["src/lib.rs", "src/nested/"]);
        assert!(
            !src_entries.iter().any(|entry| entry.ends_with("deep.rs")),
            "browser mode must not walk past immediate children: {src_entries:?}",
        );
    }

    #[test]
    fn browser_completions_hide_dot_entries_until_dot_query() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".agents")).unwrap();
        std::fs::write(tmp.path().join(".env"), "secret-ish fixture").unwrap();
        std::fs::write(tmp.path().join("app.rs"), "app").unwrap();

        let ws = Workspace::with_cwd(tmp.path().to_path_buf(), None);

        let default_entries = ws.browser_completions("", 16);
        assert_eq!(default_entries, vec!["app.rs"]);

        let dot_entries = ws.browser_completions(".", 16);
        assert_eq!(dot_entries, vec![".agents/", ".env"]);
    }

    #[test]
    fn browser_completions_reject_path_escape_segments() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let sibling = tmp.path().join("outside");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(workspace.join("inside.rs"), "inside").unwrap();
        std::fs::write(sibling.join("secret.rs"), "outside").unwrap();

        let ws = Workspace::with_cwd(workspace, None);

        assert_eq!(ws.browser_completions("", 16), vec!["inside.rs"]);
        assert!(
            ws.browser_completions("../", 16).is_empty(),
            "browser mode must not list workspace siblings",
        );
        assert!(
            ws.browser_completions("../outside", 16).is_empty(),
            "browser mode must not complete names from outside the workspace",
        );
    }

    #[test]
    fn workspace_completions_surface_explicit_hidden_and_ignored_paths() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".gitignore"), ".deepseek/\n.generated/\n").unwrap();
        std::fs::write(
            tmp.path().join(".deepseekignore"),
            ".generated/specs/secrets.env\n",
        )
        .unwrap();
        let deepseek_commands = tmp.path().join(".deepseek").join("commands");
        let generated_specs = tmp.path().join(".generated").join("specs");
        std::fs::create_dir_all(&deepseek_commands).unwrap();
        std::fs::create_dir_all(&generated_specs).unwrap();
        std::fs::write(deepseek_commands.join("start-task.md"), "start").unwrap();
        std::fs::write(generated_specs.join("device-layout.md"), "layout").unwrap();
        std::fs::write(generated_specs.join("secrets.env"), "secret").unwrap();

        let ws = Workspace::with_cwd(tmp.path().to_path_buf(), Some(tmp.path().to_path_buf()));

        let start_entries = ws.completions(".deepseek/commands", 16);
        assert!(
            start_entries
                .iter()
                .any(|e| e == ".deepseek/commands/start-task.md"),
            "expected explicitly addressed hidden command file in completions: {start_entries:?}",
        );

        let generated_entries = ws.completions(".generated/specs", 16);
        assert!(
            generated_entries
                .iter()
                .any(|e| e == ".generated/specs/device-layout.md"),
            "expected explicitly addressed ignored user folder in completions: {generated_entries:?}",
        );
        assert!(
            !generated_entries
                .iter()
                .any(|e| e == ".generated/specs/secrets.env"),
            ".deepseekignore entries must not be reintroduced by local fallback: {generated_entries:?}",
        );
    }

    #[test]
    fn workspace_completions_skip_hidden_worktrees_and_build_bulk() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join(".gitignore"), ".worktrees/\n.generated/\n").unwrap();

        std::fs::create_dir_all(root.join(".worktrees/release/src")).unwrap();
        std::fs::write(
            root.join(".worktrees/release/src/worktree-only.rs"),
            "fn main() {}",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".worktrees/release/target/debug")).unwrap();
        std::fs::write(
            root.join(".worktrees/release/target/debug/generated.o"),
            "object",
        )
        .unwrap();

        std::fs::create_dir_all(root.join(".claude/worktrees/agent/src")).unwrap();
        std::fs::write(
            root.join(".claude/worktrees/agent/src/agent-only.md"),
            "agent note",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".claude/commands")).unwrap();
        std::fs::write(root.join(".claude/commands/keep.md"), "command").unwrap();

        std::fs::create_dir_all(root.join(".generated/specs")).unwrap();
        std::fs::write(root.join(".generated/specs/device-layout.md"), "layout").unwrap();

        let ws = Workspace::with_cwd(root.to_path_buf(), Some(root.to_path_buf()));

        let worktree_entries = ws.completions(".worktrees", 32);
        assert!(
            worktree_entries
                .iter()
                .all(|entry| !entry.starts_with(".worktrees/")),
            "hidden release worktrees must stay out of completions: {worktree_entries:?}",
        );

        let claude_worktree_entries = ws.completions(".claude/worktrees", 32);
        assert!(
            claude_worktree_entries
                .iter()
                .all(|entry| !entry.starts_with(".claude/worktrees/")),
            ".claude/worktrees must stay out of completions: {claude_worktree_entries:?}",
        );

        let generated_entries = ws.completions(".generated/specs", 32);
        assert!(
            generated_entries
                .iter()
                .any(|entry| entry == ".generated/specs/device-layout.md"),
            "explicit user-generated hidden folders should still complete: {generated_entries:?}",
        );

        let command_entries = ws.completions(".claude/commands", 32);
        assert!(
            command_entries
                .iter()
                .any(|entry| entry == ".claude/commands/keep.md"),
            "normal .claude command files should still complete: {command_entries:?}",
        );

        assert!(
            ws.resolve("worktree-only.rs").is_err(),
            "fuzzy resolution must not index files from hidden release worktrees"
        );
        assert!(
            ws.resolve("agent-only.md").is_err(),
            "fuzzy resolution must not index files from .claude/worktrees"
        );
        assert!(ws.resolve("keep.md").is_ok());
    }

    #[test]
    fn fuzzy_index_resolves_hidden_and_ignored_files_except_deepseekignored() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".gitignore"), ".generated/\n").unwrap();
        std::fs::write(
            tmp.path().join(".deepseekignore"),
            ".generated/specs/secrets.env\n",
        )
        .unwrap();
        let generated_specs = tmp.path().join(".generated").join("specs");
        std::fs::create_dir_all(&generated_specs).unwrap();
        std::fs::write(generated_specs.join("device-layout.md"), "layout").unwrap();
        std::fs::write(generated_specs.join("secrets.env"), "secret").unwrap();

        let ws = Workspace::with_cwd(tmp.path().to_path_buf(), None);
        let resolved = ws.resolve("device-layout.md").unwrap();

        assert!(resolved.ends_with(".generated/specs/device-layout.md"));
        assert!(
            ws.resolve("secrets.env").is_err(),
            "basename fuzzy resolution must honor .deepseekignore"
        );
        assert!(
            ws.resolve(".generated/specs/secrets.env").is_ok(),
            "exact user-specified paths should still resolve"
        );
    }

    #[test]
    fn fuzzy_index_finds_files_and_directories() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("a/b/target_dir")).unwrap();
        std::fs::write(tmp.path().join("a/b/needle.rs"), "fn main(){}").unwrap();

        let ws = Workspace::with_cwd(tmp.path().to_path_buf(), None);

        // Basename-only mention triggers fuzzy fallback for both files and dirs.
        let f = ws.resolve("needle.rs").unwrap();
        assert!(f.ends_with("a/b/needle.rs"));
        let d = ws.resolve("target_dir").unwrap();
        assert!(d.ends_with("a/b/target_dir"));

        // Index was populated exactly once (subsequent lookups reuse it).
        assert!(ws.file_index.get().is_some());
    }

    /// Regression: `@`-mention completion must discover files inside
    /// `.deepseek/`, `.cursor/`, `.claude/`, `.agents/` even when
    /// those directories are excluded by `.gitignore` (or `.ignore`).
    /// The `discovery_walk_builder` override un-ignores them.
    #[test]
    fn completions_discovers_files_inside_gitignored_dot_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // `.ignore` works even outside a git repo; use it to simulate
        // a project that gitignores its AI-tool dot-directories.
        std::fs::write(
            root.join(".ignore"),
            ".deepseek/\n.cursor/\n.claude/\n.agents/\n",
        )
        .unwrap();

        // Create files inside each dot-dir.
        std::fs::create_dir_all(root.join(".deepseek/commands")).unwrap();
        std::fs::write(root.join(".deepseek/commands/build.md"), "build cmd").unwrap();
        std::fs::create_dir_all(root.join(".cursor/commands")).unwrap();
        std::fs::write(root.join(".cursor/commands/run.md"), "run cmd").unwrap();
        std::fs::create_dir_all(root.join(".claude/commands")).unwrap();
        std::fs::write(root.join(".claude/commands/test.md"), "test cmd").unwrap();
        std::fs::create_dir_all(root.join(".agents/skills/example")).unwrap();
        std::fs::write(
            root.join(".agents/skills/example/SKILL.md"),
            "name: example\n",
        )
        .unwrap();

        let ws = Workspace::with_cwd(root.to_path_buf(), None);

        // Completions should find entries inside the dot-dirs.
        {
            let entries = ws.completions("build", 16);
            assert!(
                entries.iter().any(|e| e.contains("build.md")),
                "expected build.md in completions although .deepseek/ is ignored; got: {entries:?}"
            );
        }
        {
            let entries = ws.completions("run", 16);
            assert!(
                entries.iter().any(|e| e.contains("run.md")),
                "expected run.md from .cursor/; got: {entries:?}"
            );
        }
        {
            let entries = ws.completions("test", 16);
            assert!(
                entries.iter().any(|e| e.contains("test.md")),
                "expected test.md from .claude/; got: {entries:?}"
            );
        }

        // Fuzzy resolution should also work.
        let f = ws.resolve("build.md").unwrap();
        assert!(f.ends_with("build.md"));
        let f2 = ws.resolve("SKILL.md").unwrap();
        assert!(f2.ends_with("SKILL.md"));
    }

    /// Regression: the dot-dir walk must NOT index `.deepseek/snapshots/`,
    /// which is the snapshot side repo that can grow to hundreds of GB.
    /// Indexing it would re-create the same OOM/hang that #1112 was built
    /// to prevent.
    #[test]
    fn dot_dir_walk_excludes_snapshot_side_repo() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // Create a snapshot-like directory tree.
        std::fs::create_dir_all(root.join(".deepseek/snapshots/deadbeef/deadbeef/.git/objects"))
            .unwrap();
        std::fs::write(
            root.join(".deepseek/snapshots/deadbeef/deadbeef/.git/objects/snapshot.pack"),
            b"fake pack data",
        )
        .unwrap();
        // Also create a legitimate file in .deepseek/ that should be found.
        std::fs::create_dir_all(root.join(".deepseek/commands")).unwrap();
        std::fs::write(root.join(".deepseek/commands/build.md"), "build cmd").unwrap();

        let ws = Workspace::with_cwd(root.to_path_buf(), None);

        // Searching for "build" must find build.md.
        let entries = ws.completions("build", 16);
        assert!(
            entries.iter().any(|e| e.contains("build.md")),
            "build.md must still be found; got: {entries:?}"
        );
        // Searching for "snapshot" must NOT return snapshot files.
        let snap_entries = ws.completions("snapshot", 16);
        assert!(
            !snap_entries.iter().any(|e| e.contains("snapshot")),
            "snapshot files must NOT appear in completions; got: {snap_entries:?}"
        );

        // Fuzzy index must also exclude snapshots.
        let f = ws.resolve("build.md").unwrap();
        assert!(f.ends_with("build.md"));
        // snapshot.pack should NOT resolve.
        let result = ws.resolve("snapshot.pack");
        assert!(
            result.is_err(),
            "snapshot.pack must not resolve via fuzzy index"
        );
    }

    /// Regression for #1921 — typing `@/` (or `@.`) must NOT trigger the
    /// `local_reference_paths` walk, which scans up to
    /// `LOCAL_REFERENCE_SCAN_LIMIT` paths on the UI thread. On WSL2 with a
    /// `/mnt/c/...` workspace this hangs the composer for seconds to minutes.
    #[test]
    fn should_try_local_reference_completion_skips_bare_separators_and_dots() {
        // The trigger gate must reject bare separators/dots.
        assert!(!should_try_local_reference_completion("/"));
        assert!(!should_try_local_reference_completion("\\"));
        assert!(!should_try_local_reference_completion("."));
        assert!(!should_try_local_reference_completion(".."));
        // Empty string was already rejected; keep that.
        assert!(!should_try_local_reference_completion(""));

        // Actionable references must still trigger.
        assert!(should_try_local_reference_completion("./foo"));
        assert!(should_try_local_reference_completion("../bar"));
        assert!(should_try_local_reference_completion(".env"));
        assert!(should_try_local_reference_completion("path/"));
        assert!(should_try_local_reference_completion("path/to/file"));
        assert!(should_try_local_reference_completion("/usr"));
    }

    #[test]
    fn cached_candidates_rank_like_live_completions() {
        // #3757: the composer caches one full candidate walk and ranks per
        // keystroke in memory; the ranked result must match what the live
        // walk would return for non-path-like needles.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("src/mention.rs"), "// m").unwrap();
        std::fs::write(root.join("README.md"), "# readme").unwrap();
        std::fs::write(root.join("Makefile"), "all:").unwrap();

        let ws = Workspace::with_cwd(root.to_path_buf(), None);
        let candidates = ws.completion_candidates();
        assert!(
            candidates.iter().any(|c| c == "src/main.rs"),
            "{candidates:?}"
        );

        for needle in ["ma", "readme", "men", ""] {
            let live = ws.completions(needle, 16);
            let ranked = rank_completion_candidates(&candidates, needle, 16);
            assert_eq!(ranked, live, "needle {needle:?}");
        }

        // Limit truncation applies after prefix/substring bucketing.
        let ranked = rank_completion_candidates(&candidates, "ma", 1);
        assert_eq!(ranked.len(), 1);
        assert!(ranked[0].to_lowercase().starts_with("ma"), "{ranked:?}");
    }

    /// Regression for #1921 — `completions("/", N)` must return without
    /// invoking `local_reference_paths`, even on a workspace large enough
    /// to expose the original 4096-path walk. We can't assert "doesn't
    /// touch the disk", but we can assert the call completes promptly and
    /// stays within the requested limit.
    #[test]
    fn completions_for_bare_slash_does_not_trigger_local_reference_walk() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // Lay out enough files that a runaway walk would be visibly slow,
        // but the bounded path returns near-instantly. Depth-1 entries are
        // enough; we don't need to stress the filesystem.
        for i in 0..40 {
            std::fs::write(root.join(format!("file_{i}.txt")), "x").unwrap();
        }
        let ws = Workspace::with_cwd(root.to_path_buf(), None);

        let start = std::time::Instant::now();
        let entries = ws.completions("/", 64);
        let elapsed = start.elapsed();

        // Behavioral assertions:
        // 1. The call returns within a generous bound. Real freezes on
        //    WSL2 were tens of seconds; a 2s budget is comfortable for a
        //    40-file tmp dir on any CI host.
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "completions(\"/\") took too long: {elapsed:?} (likely re-introduced #1921)"
        );
        // 2. Results stay within the requested cap.
        assert!(entries.len() <= 64);
    }
}
