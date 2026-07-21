//! 工具输出溢出写入器 (#422)。
//!
//! 当工具产生的输出太大而无法放入模型的上下文预算时，
//! 我们希望同时实现两件事：
//!
//! 1. 转录/工具单元格渲染一个有限的预览，使 UI 保持可浏览性。
//! 2. 完整的原始输出保存在磁盘上，以便模型在之后需要省略的尾部时可以
//!    `read_file` 读取回来，并且用户可以在 `$EDITOR` 中打开它。
//!
//! 本模块负责磁盘端管理。文件存放在
//! `~/.codewhale/tool_outputs/<sanitised-id>.txt`。id 是引擎分配的工具
//! 调用 id；我们保守地对其进行清理（ASCII
//! 字母数字 + `-`/`_`），这样恶意的 id 无法通过 `..` 或绝对路径技巧
//! 逃逸出目录。
//!
//! 启动时修剪会删除 mtime 早于 [`SPILLOVER_MAX_AGE`]
//! （7 天）的文件。修剪失败会被记录日志，但绝不会致命——用户
//! 不应因为过期的工具输出文件而看到启动卡死。
//!
//! ## 实时调用者
//!
//! * [`apply_spillover`]——从引擎的工具执行路径（`turn_loop.rs`）调用，
//!   任何超过 [`SPILLOVER_THRESHOLD_BYTES`] 的成功工具结果都会
//!   溢出到磁盘，模型收到一个 [`SPILLOVER_HEAD_BYTES`] 的头部
//!   加上一个指针尾部。
//! * `main.rs` 中的启动修剪删除早于 [`SPILLOVER_MAX_AGE`] 的文件。
//!
//! UI 端渲染内联的 `full output: <path>` 注释
//! 由 `tui/history.rs::render_spillover_annotation` 负责。当用户
//! 在已溢出的工具单元格上按下工具详情快捷键时，
//! 工具详情分页器会打开溢出文件。

use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::tools::spec::ToolResult;

// `Path` 仅在仅限测试构建的辅助函数中被引用。
#[cfg(test)]
use std::path::Path;

/// CodeWhale 主目录下溢出目录的名称。
pub const SPILLOVER_DIR_NAME: &str = "tool_outputs";

/// 默认阈值，工具结果超过此值时将被视为溢出候选。
/// 镜像了我们在其他地方用于"太大而无法内联"的 `MAX_MEMORY_SIZE` 上限，
/// 使规则感觉一致。有线调用者可以传递不同的值，
/// 如果某个工具族有不同的经济考量。
pub const SPILLOVER_THRESHOLD_BYTES: usize = 100 * 1024; // 100 KiB

/// 默认启动修剪期限。启动时删除更早的溢出文件，
/// 防止 `~/.codewhale/tool_outputs/` 无限制增长。
/// 镜像工作区快照的 7 天默认值。
pub const SPILLOVER_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[cfg(test)]
static TEST_SPILLOVER_ROOT: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

#[cfg(test)]
pub(crate) static TEST_SPILLOVER_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 解析 `~/.codewhale/tool_outputs/`。如果无法确定主目录
/// 则返回 `None`（CI 容器偶尔会遇到这种情况）。
/// 调用者应将 `None` 视为"溢出不可用"，
/// 优雅降级而非使工具调用失败。
#[must_use]
pub fn spillover_root() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(root) = TEST_SPILLOVER_ROOT
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .clone()
    {
        return Some(root);
    }

    let home = dirs::home_dir()?;
    let primary = home.join(".codewhale").join(SPILLOVER_DIR_NAME);
    let legacy = home.join(".deepseek").join(SPILLOVER_DIR_NAME);
    if primary.exists() || !legacy.exists() {
        return Some(primary);
    }
    Some(legacy)
}

/// 在不改变 `$HOME` 的情况下为测试覆盖溢出根目录。
#[cfg(test)]
pub(crate) fn set_test_spillover_root(root: Option<PathBuf>) -> Option<PathBuf> {
    let mut guard = TEST_SPILLOVER_ROOT
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    std::mem::replace(&mut *guard, root)
}

/// 解析工具调用 id 的溢出文件路径。对 id 进行清理，
/// 防止恶意值逃逸存储目录。
/// 对空/完全无效的 id 返回 `None`；调用者应将其视为
/// "溢出不可用"并跳过写入。
#[must_use]
pub fn spillover_path(id: &str) -> Option<PathBuf> {
    let sanitised = sanitise_id(id)?;
    Some(spillover_root()?.join(format!("{sanitised}.txt")))
}

/// 解析 SHA256 内容哈希的溢出文件路径。使用独立的
/// 命名空间（`sha_<hex>.txt`）与工具调用 id 文件分离，使两个
/// 引用系统（引擎端溢出 + 线路端去重）可以在同一目录中共存
/// 而不发生冲突。`sha` 必须是原始的 64 字符小写十六进制摘要——
/// 不区分大小写的匹配由调用者处理。
#[must_use]
pub fn sha_spillover_path(sha: &str) -> Option<PathBuf> {
    let sha = sha.trim().to_ascii_lowercase();
    if !is_valid_sha256(&sha) {
        return None;
    }
    Some(spillover_root()?.join(format!("sha_{sha}.txt")))
}

/// 当 `s` 是 64 字符的小写 ASCII 十六进制字符串时为 true。用于
/// 检测模型可能传递给检索的裸 SHA 引用，以及
/// 验证 [`sha_spillover_path`] 的输入。
#[must_use]
pub fn is_valid_sha256(s: &str) -> bool {
    s.len() == 64
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// 将内容写入 SHA 地址的溢出文件。幂等——
/// 相同的哈希始终映射到相同的路径，文件的内容
/// 是哈希的函数。如果文件已存在则跳过写入
/// （这是线路去重的常见情况，因为
/// 第二次写入的内容与第一次相同）。
pub fn write_sha_spillover(sha: &str, content: &str) -> io::Result<PathBuf> {
    let path = sha_spillover_path(sha).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "sha must be a 64-char lowercase hex digest",
        )
    })?;
    if path.exists() {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::utils::write_atomic(&path, content.as_bytes())?;
    Ok(path)
}

/// 将 `content` 写入 `id` 的溢出文件。必要时创建
/// 父目录。成功时返回解析后的路径。
///
/// 通过底层操作系统的 `write` + 文件系统重命名保证实现原子性——
/// 文件首先以临时名称创建，然后重命名为目标位置。
/// 失败以 `io::Error` 形式向上传递，以便
/// 调用者决定是否将其展示给用户。
pub fn write_spillover(id: &str, content: &str) -> io::Result<PathBuf> {
    let path = spillover_path(id).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "could not resolve spillover path (empty/invalid id or missing home directory)",
        )
    })?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::utils::write_atomic(&path, content.as_bytes())?;
    Ok(path)
}

/// 删除早于 `max_age` 的溢出文件。返回已删除的
/// 文件数量。非致命：目录不存在返回 0；每个文件的
/// 错误会被记录并跳过。镜像
/// [`crate::session_manager::prune_workspace_snapshots`]。
pub fn prune_older_than(max_age: Duration) -> io::Result<usize> {
    let Some(root) = spillover_root() else {
        return Ok(0);
    };
    if !root.exists() {
        return Ok(0);
    }
    let cutoff = SystemTime::now()
        .checked_sub(max_age)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut pruned = 0usize;
    for entry in fs::read_dir(&root)? {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!(target: "spillover", ?err, "skipping unreadable dir entry");
                continue;
            }
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let modified = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(target: "spillover", ?err, ?path, "skipping unreadable mtime");
                continue;
            }
        };
        if modified < cutoff {
            if let Err(err) = fs::remove_file(&path) {
                tracing::warn!(target: "spillover", ?err, ?path, "spillover prune skipped a file");
                continue;
            }
            pruned += 1;
        }
    }
    Ok(pruned)
}

/// 常见"太长？溢出它。"模式的便捷函数。如果
/// `content` 小于或等于 `threshold` 字节，返回 `None`，
/// 调用者保留内联内容。超过阈值时，将
/// 完整内容写入溢出文件并返回
/// `Some((head, path))`，其中 `head` 是调用者可以
/// 内联显示的头部切片。尾部不返回——`path` 是
/// 规范引用。
///
/// `head_bytes` 控制调用者希望保留多少内联内容。
/// 传入 `threshold` 表示"尽量保留适合内联的内容"，
/// 或使用较小的值（例如 `4 * 1024`）表示"显示预览"。
pub fn maybe_spillover(
    id: &str,
    content: &str,
    threshold: usize,
    head_bytes: usize,
) -> io::Result<Option<(String, PathBuf)>> {
    if content.len() <= threshold {
        return Ok(None);
    }
    let path = write_spillover(id, content)?;
    // 不要切在 UTF-8 中间：如果需要，回退到字符边界。
    let cut = head_bytes.min(content.len());
    let cut = (0..=cut)
        .rev()
        .find(|&i| content.is_char_boundary(i))
        .unwrap_or(0);
    Ok(Some((content[..cut].to_string(), path)))
}

/// 当 [`apply_spillover`] 截断工具结果时保留的内联头部。
/// 32 KiB 足够让模型保持有意义的上下文（长的堆栈跟踪、
/// `git diff` 的头部、典型深度的目录列表），
/// 而不会消耗单轮上下文预算的绝大部分。完整输出保存在
/// 磁盘上；模型如果之后需要尾部可以 `read_file` 读取回来。
pub const SPILLOVER_HEAD_BYTES: usize = 32 * 1024;

/// 对工具结果就地应用溢出。如果结果的
/// 内容超过 [`SPILLOVER_THRESHOLD_BYTES`]，将完整
/// 内容写入 `~/.codewhale/tool_outputs/` 下的同级文件，
/// 将 `result.content` 替换为 [`SPILLOVER_HEAD_BYTES`] 的头部
/// 加上指向溢出文件的尾部，并
/// 在 `metadata.spillover_path` 中打上标记，以便 UI 可以渲染其
/// "完整输出：…"注释。
///
/// 成功时返回溢出路径，如果未发生溢出则返回 `None`
/// （内容足够小、错误结果、写入失败）。
/// 失败会被记录但绝不会向上传递——产生了结果的
/// 工具不应因为溢出写入器无法写入磁盘而被标记为失败；
/// 我们降级为无操作，模型获得原始的（大的）内容。
///
/// 错误结果（`success == false`）会被跳过：错误消息
/// 通常很短，将其变成"查看文件"指针
/// 只会对模型的推理隐藏错误。
#[allow(dead_code)]
pub fn apply_spillover(result: &mut ToolResult, tool_id: &str) -> Option<PathBuf> {
    apply_spillover_inner(result, tool_id, None)
}

/// 应用溢出并发出会话范围的工件引用。
///
/// 主目录级别的 `tool_outputs/<tool-id>.txt` 文件仍然写入，
/// 以便 `retrieve_tool_result ref=<tool-id>` 在过渡期间保持有效。
/// 规范工件内容也会写入
/// `~/.codewhale/sessions/<session-id>/artifacts/`，内联工具结果
/// 变为固定格式的工件引用块。
pub fn apply_spillover_with_artifact(
    result: &mut ToolResult,
    tool_id: &str,
    tool_name: &str,
    session_id: &str,
) -> Option<PathBuf> {
    apply_spillover_inner(
        result,
        tool_id,
        Some(ArtifactSpilloverContext {
            tool_name,
            session_id,
        }),
    )
}

struct ArtifactSpilloverContext<'a> {
    tool_name: &'a str,
    session_id: &'a str,
}

fn apply_spillover_inner(
    result: &mut ToolResult,
    tool_id: &str,
    artifact_context: Option<ArtifactSpilloverContext<'_>>,
) -> Option<PathBuf> {
    if !result.success {
        return None;
    }
    if result.content.len() <= SPILLOVER_THRESHOLD_BYTES {
        return None;
    }
    let original_content = result.content.clone();
    let total = original_content.len();
    let outcome = match maybe_spillover(
        tool_id,
        &original_content,
        SPILLOVER_THRESHOLD_BYTES,
        SPILLOVER_HEAD_BYTES,
    ) {
        Ok(Some(pair)) => pair,
        Ok(None) => return None,
        Err(err) => {
            tracing::warn!(
                target: "spillover",
                ?err,
                tool_id,
                "spillover write failed; passing original content through"
            );
            return None;
        }
    };
    let (head, path) = outcome;
    let path_str = path.display().to_string();

    let mut artifact_path = None;
    if let Some(context) = artifact_context {
        let artifact_id = crate::artifacts::artifact_id_for_tool_call(tool_id);
        match crate::artifacts::write_session_artifact(
            context.session_id,
            &artifact_id,
            &original_content,
        ) {
            Ok((absolute_path, relative_path)) => {
                let record = crate::artifacts::record_tool_output_artifact(
                    context.session_id,
                    tool_id,
                    context.tool_name,
                    relative_path.clone(),
                    &original_content,
                );
                let transcript_ref = crate::artifacts::TranscriptArtifactRef::from(&record);
                result.content = crate::artifacts::render_transcript_artifact_ref(&transcript_ref);
                artifact_path = Some((absolute_path, relative_path, record));
            }
            Err(err) => {
                tracing::warn!(
                    target: "spillover",
                    ?err,
                    tool_id,
                    "session artifact write failed; falling back to legacy spillover footer"
                );
            }
        }
    }

    if artifact_path.is_none() {
        let footer = format!(
            "\n\n[Output truncated: {head_kib} KiB of {total_kib} KiB shown. \
             Full output saved to {path_str}. Use \
             `retrieve_tool_result ref={tool_id} mode=tail` or \
             `retrieve_tool_result ref={tool_id} mode=query query=<text>` \
             if you need the elided output.]",
            head_kib = head.len() / 1024,
            total_kib = total / 1024,
        );
        result.content = format!("{head}{footer}");
    }

    let metadata = result.metadata.get_or_insert_with(|| serde_json::json!({}));
    if let Some(obj) = metadata.as_object_mut() {
        if let Some((absolute_path, relative_path, record)) = artifact_path.as_ref() {
            obj.insert(
                "spillover_path".into(),
                serde_json::Value::String(absolute_path.display().to_string()),
            );
            obj.insert(
                "legacy_spillover_path".into(),
                serde_json::Value::String(path_str),
            );
            obj.insert(
                "artifact_id".into(),
                serde_json::Value::String(record.id.clone()),
            );
            obj.insert(
                "artifact_session_id".into(),
                serde_json::Value::String(record.session_id.clone()),
            );
            obj.insert(
                "artifact_relative_path".into(),
                serde_json::Value::String(crate::artifacts::format_artifact_relative_path(
                    relative_path,
                )),
            );
            obj.insert(
                "artifact_path".into(),
                serde_json::Value::String(absolute_path.display().to_string()),
            );
            obj.insert(
                "artifact_byte_size".into(),
                serde_json::Value::Number(serde_json::Number::from(record.byte_size)),
            );
            obj.insert(
                "artifact_preview".into(),
                serde_json::Value::String(record.preview.clone()),
            );
        } else {
            obj.insert("spillover_path".into(), serde_json::Value::String(path_str));
        }
    } else {
        // 预先存在的元数据不是 JSON 对象（罕见，
        // 可能是数组）。替换为对象，以便我们能够
        // 附加键而不丢失先前的数据——将其包装在
        // `_prior` 字段下，以便内省的调用者可以恢复。
        let prior = std::mem::replace(metadata, serde_json::json!({}));
        if let Some(obj) = metadata.as_object_mut() {
            obj.insert("_prior".into(), prior);
            if let Some((absolute_path, relative_path, record)) = artifact_path.as_ref() {
                obj.insert(
                    "spillover_path".into(),
                    serde_json::Value::String(absolute_path.display().to_string()),
                );
                obj.insert(
                    "legacy_spillover_path".into(),
                    serde_json::Value::String(path.display().to_string()),
                );
                obj.insert(
                    "artifact_id".into(),
                    serde_json::Value::String(record.id.clone()),
                );
                obj.insert(
                    "artifact_session_id".into(),
                    serde_json::Value::String(record.session_id.clone()),
                );
                obj.insert(
                    "artifact_relative_path".into(),
                    serde_json::Value::String(crate::artifacts::format_artifact_relative_path(
                        relative_path,
                    )),
                );
                obj.insert(
                    "artifact_path".into(),
                    serde_json::Value::String(absolute_path.display().to_string()),
                );
                obj.insert(
                    "artifact_byte_size".into(),
                    serde_json::Value::Number(serde_json::Number::from(record.byte_size)),
                );
                obj.insert(
                    "artifact_preview".into(),
                    serde_json::Value::String(record.preview.clone()),
                );
            } else {
                obj.insert(
                    "spillover_path".into(),
                    serde_json::Value::String(path.display().to_string()),
                );
            }
        }
    }
    artifact_path
        .map(|(absolute_path, _, _)| absolute_path)
        .or(Some(path))
}

/// 清理工具调用 id 以用作文件名。保留 ASCII
/// 字母数字、`-` 和 `_`；拒绝 `.` 以防止 `..` 遍历，
/// 拒绝空结果。如果输入中不包含
/// 任何可接受字符则返回 `None`。
fn sanitise_id(id: &str) -> Option<String> {
    let cleaned: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// 覆盖测试的存储根目录，使其不会污染
/// 用户的真实 `~/.codewhale/` 目录。这使用显式的测试钩子而非
/// `$HOME`，因为 Windows 主目录解析可能会忽略环境
/// 覆盖而返回运行器配置文件目录。
#[cfg(test)]
fn with_test_home<F, R>(home: &Path, f: F) -> R
where
    F: FnOnce() -> R,
{
    let _artifact_guard = crate::artifacts::TEST_ARTIFACT_SESSIONS_GUARD
        .lock()
        .unwrap_or_else(|err| err.into_inner());

    struct StorageRootOverride {
        prior_spillover: Option<PathBuf>,
        prior_artifacts: Option<PathBuf>,
    }

    impl Drop for StorageRootOverride {
        fn drop(&mut self) {
            set_test_spillover_root(self.prior_spillover.take());
            crate::artifacts::set_test_artifact_sessions_root(self.prior_artifacts.take());
        }
    }

    // 本模块中的测试通过 `TEST_GUARD` 序列化溢出；上面的
    // 工件保护锁保护与 artifacts.rs 测试共享的会话工件根。
    let prior_spillover =
        set_test_spillover_root(Some(home.join(".codewhale").join(SPILLOVER_DIR_NAME)));
    let prior_artifacts = crate::artifacts::set_test_artifact_sessions_root(Some(
        home.join(".codewhale").join("sessions"),
    ));
    let _restore = StorageRootOverride {
        prior_spillover,
        prior_artifacts,
    };
    f()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// 本模块中的测试通过此保护锁序列化，因为它们会改变
    /// 进程全局的测试存储根目录。没有它，cargo 的并行运行器
    /// 会观察到交错的覆盖。
    fn setup() -> std::sync::MutexGuard<'static, ()> {
        super::TEST_SPILLOVER_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn with_test_home_overrides_storage_roots_without_home_resolution() {
        let _g = setup();
        let tmp = tempdir().unwrap();

        with_test_home(tmp.path(), || {
            assert_eq!(
                spillover_root().as_deref(),
                Some(tmp.path().join(".codewhale").join("tool_outputs").as_path())
            );
            assert_eq!(
                crate::artifacts::session_artifact_absolute_path(
                    "session-123",
                    &PathBuf::from("artifacts").join("art_call-big.txt")
                )
                .as_deref(),
                Some(
                    tmp.path()
                        .join(".codewhale")
                        .join("sessions")
                        .join("session-123")
                        .join("artifacts")
                        .join("art_call-big.txt")
                        .as_path()
                )
            );
        });
    }

    #[test]
    fn sanitise_id_keeps_safe_chars_and_drops_dangerous() {
        assert_eq!(super::sanitise_id("abc-123_x"), Some("abc-123_x".into()));
        // 删除 `.` 以防止 `..` 进入路径。
        assert_eq!(super::sanitise_id("../etc"), Some("etc".into()));
        assert_eq!(super::sanitise_id("/etc/passwd"), Some("etcpasswd".into()));
        // 清理后为空 → None。
        assert!(super::sanitise_id("...").is_none());
        assert!(super::sanitise_id("").is_none());
    }

    #[test]
    fn write_spillover_creates_directory_and_writes_file() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let path = write_spillover("call-abc", "hello world").expect("write");
            assert!(path.exists(), "{path:?} missing");
            let body = fs::read_to_string(&path).unwrap();
            assert_eq!(body, "hello world");
            // 目录位于 `<HOME>/.codewhale/tool_outputs/` 下。
            // 比较路径组件而不是在 `to_string_lossy` 上进行子串匹配
            // ——Windows 使用 `\` 作为分隔符，因此 `/` 子串匹配
            // 会在那里错误地失败。
            let components: Vec<&str> = path
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect();
            assert!(
                components.contains(&".codewhale") && components.contains(&"tool_outputs"),
                "spillover path missing expected `.codewhale/tool_outputs/...` segments: {path:?}"
            );
        });
    }

    #[test]
    fn write_spillover_rejects_empty_id() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let err = write_spillover("...", "x").unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        });
    }

    #[test]
    fn maybe_spillover_returns_none_below_threshold() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let out = maybe_spillover("call-1", "tiny content", 100 * 1024, 4 * 1024).expect("ok");
            assert!(out.is_none());
        });
    }

    #[test]
    fn maybe_spillover_writes_and_returns_head_above_threshold() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            // 内容大于阈值。
            let big = "A".repeat(2_000);
            let (head, path) = maybe_spillover("call-2", &big, 1_000, 256)
                .expect("ok")
                .expect("should have spilled");
            // 头部有界。
            assert_eq!(head.len(), 256);
            // 磁盘上的完整内容。
            let body = fs::read_to_string(&path).unwrap();
            assert_eq!(body.len(), 2_000);
        });
    }

    #[test]
    fn maybe_spillover_does_not_split_inside_a_codepoint() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            // 4 字节字符；请求 3 字节头部 → 回退到
            // 前一个字符边界 (0)。
            let s = "🐳🐳🐳🐳"; // 4 × 4-byte codepoints
            assert_eq!(s.len(), 16);
            let (head, _) = maybe_spillover("call-3", s, 1, 3)
                .expect("ok")
                .expect("spilled");
            // 3 不是此字符串中的字符边界；回退 → 0。
            assert_eq!(head, "");
            // 请求 4 字节落在第一个字符边界上。
            let (head, _) = maybe_spillover("call-3b", s, 1, 4)
                .expect("ok")
                .expect("spilled");
            assert_eq!(head, "🐳");
        });
    }

    #[test]
    fn prune_older_than_handles_missing_root() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            // 从未写入过；根目录不存在；没问题。
            let count = prune_older_than(SPILLOVER_MAX_AGE).expect("ok");
            assert_eq!(count, 0);
        });
    }

    // mtime 回退使用 utimensat（仅 Unix）。在 Windows 上，
    // filetime_set_modified 辅助函数是空操作，因此修剪不会看到
    // 任何过期文件。将整个测试限制在 `cfg(unix)` 上，而不是
    // 测试一个无法有意义地失败的空操作路径。
    #[test]
    #[cfg(unix)]
    fn prune_older_than_keeps_fresh_files_drops_stale_ones() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let fresh = write_spillover("fresh", "x").unwrap();
            let stale = write_spillover("stale", "y").unwrap();

            // 将 `stale` 的 mtime 回退到 30 天前。
            let thirty_days = SystemTime::now() - Duration::from_secs(30 * 24 * 60 * 60);
            filetime_set_modified(&stale, thirty_days);

            let pruned = prune_older_than(SPILLOVER_MAX_AGE).unwrap();
            assert_eq!(pruned, 1);
            assert!(fresh.exists());
            assert!(!stale.exists());
        });
    }

    /// 设置文件的 mtime。工作区不引入 `filetime` crate，
    /// 因此我们在 Unix 上直接使用 `utimensat`。
    /// Windows 上是空操作——修剪语义相同，
    /// 每周期压力测试位于 Unix 路径上。
    #[cfg(unix)]
    fn filetime_set_modified(path: &Path, when: SystemTime) {
        let secs = when
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as libc::time_t;
        let times = [
            libc::timespec {
                tv_sec: secs,
                tv_nsec: 0,
            },
            libc::timespec {
                tv_sec: secs,
                tv_nsec: 0,
            },
        ];
        let path_c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: path_c 是有效的 CString；times 是与 utimensat 签名匹配的 2 元素数组。
        let rc = unsafe { libc::utimensat(libc::AT_FDCWD, path_c.as_ptr(), times.as_ptr(), 0) };
        assert_eq!(
            rc,
            0,
            "utimensat failed: {}",
            std::io::Error::last_os_error()
        );
    }

    // Windows 存根已在 v0.8.8 中移除——`filetime_set_modified` 的唯一调用者是
    // `prune_older_than_keeps_fresh_files_drops_stale_ones`，
    // 它现在是 `#[cfg(unix)]`，因为 mtime 回退需要
    // `utimensat`，而且 Windows 的空操作存根无论如何也无法让断言通过。
    // 保留存根会在 Windows 构建上触发 `-D dead-code`
    // （修剪测试是唯一的调用者）并破坏 `Test (windows-latest)`。

    #[test]
    fn apply_spillover_is_noop_below_threshold() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let mut result = ToolResult::success("small payload");
            let path = apply_spillover(&mut result, "call-small");
            assert!(path.is_none());
            assert_eq!(result.content, "small payload");
            assert!(result.metadata.is_none());
        });
    }

    #[test]
    fn apply_spillover_is_noop_for_error_results() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            // 即使非常大的错误消息也会被传递——
            // 截断错误会对其模型隐藏错误。
            let big_err = "boom\n".repeat(50_000);
            let mut result = ToolResult::error(big_err.clone());
            let path = apply_spillover(&mut result, "call-err");
            assert!(path.is_none());
            assert_eq!(result.content, big_err);
        });
    }

    #[test]
    fn apply_spillover_truncates_and_stamps_metadata_above_threshold() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            // 200 KiB 内容——远高于 100 KiB 阈值。
            let big = "X".repeat(200 * 1024);
            let mut result = ToolResult::success(big.clone());
            let path = apply_spillover(&mut result, "call-big").expect("should spill");

            // 内联内容缩小为头部 + 尾部。
            assert!(result.content.len() < big.len());
            assert!(
                result.content.contains("Output truncated:"),
                "footer missing: {}",
                &result.content[result.content.len().saturating_sub(200)..]
            );
            assert!(result.content.contains("retrieve_tool_result ref=call-big"));

            // 完整字节保存在返回路径的磁盘上。
            assert!(path.exists(), "spillover file missing: {path:?}");
            let body = fs::read_to_string(&path).unwrap();
            assert_eq!(body.len(), 200 * 1024);

            // metadata.spillover_path 已打上标记供 UI 查找。
            let metadata = result.metadata.expect("metadata stamped");
            let stamped = metadata
                .get("spillover_path")
                .and_then(serde_json::Value::as_str)
                .expect("spillover_path key present");
            assert_eq!(stamped, path.display().to_string());
        });
    }

    #[test]
    fn apply_spillover_with_artifact_writes_session_file_and_ref_block() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let big = "checking crate ... error[E0425]: cannot find value\n".repeat(4_000);
            let mut result = ToolResult::success(big.clone());
            let path =
                apply_spillover_with_artifact(&mut result, "call-big", "exec_shell", "session-123")
                    .expect("should spill");

            let session_artifact = tmp
                .path()
                .join(".codewhale")
                .join("sessions")
                .join("session-123")
                .join("artifacts")
                .join("art_call-big.txt");
            assert_eq!(path, session_artifact);
            assert_eq!(fs::read_to_string(&session_artifact).unwrap(), big);
            assert!(
                tmp.path()
                    .join(".codewhale/tool_outputs/call-big.txt")
                    .exists(),
                "home-level spillover file should remain during transition"
            );

            assert!(result.content.starts_with("[artifact: exec_shell]"));
            assert!(result.content.contains("id:           art_call-big"));
            assert!(result.content.contains("tool_call_id: call-big"));
            assert!(
                result
                    .content
                    .contains("path:         artifacts/art_call-big.txt")
            );
            assert!(!result.content.contains("Output truncated:"));

            let metadata = result.metadata.expect("metadata stamped");
            assert_eq!(
                metadata
                    .get("artifact_id")
                    .and_then(serde_json::Value::as_str),
                Some("art_call-big")
            );
            assert_eq!(
                metadata
                    .get("artifact_relative_path")
                    .and_then(serde_json::Value::as_str),
                Some("artifacts/art_call-big.txt")
            );
            assert_eq!(
                metadata
                    .get("artifact_session_id")
                    .and_then(serde_json::Value::as_str),
                Some("session-123")
            );
        });
    }

    #[test]
    fn apply_spillover_preserves_existing_metadata() {
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let big = "Y".repeat(200 * 1024);
            let mut result = ToolResult::success(big)
                .with_metadata(serde_json::json!({"prior_key": "prior_value"}));
            let path = apply_spillover(&mut result, "call-meta").expect("should spill");

            let metadata = result.metadata.expect("metadata present");
            // 先前的键保留。
            assert_eq!(
                metadata
                    .get("prior_key")
                    .and_then(serde_json::Value::as_str),
                Some("prior_value")
            );
            // 新键同时添加。
            assert_eq!(
                metadata
                    .get("spillover_path")
                    .and_then(serde_json::Value::as_str),
                Some(path.display().to_string().as_str())
            );
        });
    }

    #[test]
    fn apply_spillover_wraps_non_object_metadata_under_prior_key() {
        // 防止工具的 `metadata` 是 JSON 对象以外的类型
        // （罕见——大多数使用 `json!({})` 模式——但根据 `serde_json::Value`
        // 是合法的）。溢出写入器必须添加 `spillover_path`
        // 而不丢失先前的载荷。
        let _g = setup();
        let tmp = tempdir().unwrap();
        with_test_home(tmp.path(), || {
            let big = "Z".repeat(200 * 1024);
            let mut result = ToolResult::success(big).with_metadata(serde_json::json!([
                "unexpected",
                "array",
                "payload"
            ]));
            let path = apply_spillover(&mut result, "call-arr").expect("should spill");

            let metadata = result.metadata.expect("metadata stamped");
            // 先前的载荷迁移到 `_prior` 下。
            let prior = metadata.get("_prior").expect("_prior wrap key present");
            assert_eq!(
                prior,
                &serde_json::json!(["unexpected", "array", "payload"]),
                "prior array should round-trip under _prior"
            );
            // 新键同时添加。
            assert_eq!(
                metadata
                    .get("spillover_path")
                    .and_then(serde_json::Value::as_str),
                Some(path.display().to_string().as_str())
            );
        });
    }
}
