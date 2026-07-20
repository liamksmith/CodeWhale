//! 事务性持久化、原子写入以及 v0.8.67 宪章优先设置通道（#3410）的密钥脱敏。
//!
//! 这是每个设置步骤下的安全层。一个设置会话可能涉及多个文件
//!（设置状态侧车文件、用户全局宪章，以及——通过现有的保留注释的
//! `ConfigStore`——`config.toml`）。此模块保证的契约：
//!
//! - **预览不写入任何内容。** [`SetupTransaction::preview`] 报告将会有哪些更改，
//!   而不触及文件系统。
//! - **取消后文件保持不变。** 一个已暂存但未调用 [`SetupTransaction::commit`] 就
//!   被丢弃的事务从不写入任何内容。
//! - **保存是原子的。** 每个文件都通过临时文件 + 重命名写入
//!   （[`atomic_write`]）；多文件提交要么完全应用，要么完全
//!   回滚，因此部分失败绝不会留下半写入的文件。
//! - **密钥从不泄漏。** [`redact_secrets`] 对可能回显配置文本的任何报告、
//!   日志行或诊断信息中的密钥承载值进行脱敏。
//!
//! 此模块刻意只拥有写入/回滚/密钥契约。
//! 每个设置步骤拥有*哪些*字段需要写入；参见 [`crate::setup_state`] 和
//! [`crate::user_constitution`]。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// 设置所属文件的限制性文件模式（仅所有者读写）。
#[cfg(unix)]
const SETUP_FILE_MODE: u32 = 0o600;

/// 通过同级临时文件 + 重命名原子地将 `bytes` 写入 `path`。
///
/// 临时文件在与 `path` 相同的目录中创建，以便最终的
/// `rename` 在同一文件系统上是原子的。在 Unix 上，文件以
/// `0o600` 权限创建，确保设置所属的状态永远不会被全局可读。
/// 父目录按需创建。
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建目录 {} 失败", parent.display()))?;
    }

    let dir = parent.unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("在 {} 中创建临时文件失败", dir.display()))?;

    use std::io::Write as _;
    tmp.write_all(bytes)
        .with_context(|| format!("写入 {} 的临时文件失败", path.display()))?;
    tmp.flush()
        .with_context(|| format("刷新 {} 的临时文件失败", path.display()))?;

    #[cfg(unix)]
    {
        let perms = fs::Permissions::from_mode(SETUP_FILE_MODE);
        tmp.as_file()
            .set_permissions(perms)
            .with_context(|| format!("设置 {} 的权限失败", path.display()))?;
    }

    tmp.persist(path)
        .map_err(|e| e.error)
        .with_context(|| format!("持久化 {} 失败", path.display()))?;
    Ok(())
}

/// 原子地将 `value` 作为美化打印的 JSON 写入 `path`。
///
/// 追加尾随换行符，以便文件对面向行的工具和差异操作格式良好。
pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut body = serde_json::to_string_pretty(value)
        .with_context(|| format!("序列化 {} 的 JSON 失败", path.display()))?;
    body.push('\n');
    atomic_write(path, body.as_bytes())
}

/// 一个暂存的多文件写入，要么完全应用，要么完全回滚。
///
/// 暂存设置步骤打算写入的每个文件，然后调用 [`commit`]。如果
/// 任何单个写入失败，事务中每个已应用的写入都将恢复到其提交前
/// 的内容（如果之前不存在则删除），并返回原始错误。一个未提交
/// 就被丢弃的事务不触及文件系统。
///
/// [`commit`]: SetupTransaction::commit
#[derive(Debug, Default)]
pub struct SetupTransaction {
    writes: Vec<StagedWrite>,
}

#[derive(Debug, Clone)]
struct StagedWrite {
    path: PathBuf,
    bytes: Vec<u8>,
}

/// 文件提交前状态的快照，以便 [`SetupTransaction`] 在回滚期间
/// 可以恢复它。
struct Snapshot {
    path: PathBuf,
    /// 原始字节，如果文件在提交前不存在则为 `None`。
    original: Option<Vec<u8>>,
}

impl SetupTransaction {
    /// 创建一个空事务。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 暂存 `bytes` 以在 [`commit`](Self::commit) 时写入 `path`。
    ///
    /// 暂存不触及磁盘。后面同一路径的暂存会替换前面的，
    /// 因此一个步骤可以在提交前修改其预期输出。
    pub fn stage(&mut self, path: impl Into<PathBuf>, bytes: impl Into<Vec<u8>>) -> &mut Self {
        let path = path.into();
        let bytes = bytes.into();
        if let Some(existing) = self.writes.iter_mut().find(|w| w.path == path) {
            existing.bytes = bytes;
        } else {
            self.writes.push(StagedWrite { path, bytes });
        }
        self
    }

    /// 暂存 `value` 序列化为美化 JSON（带尾随换行符）。
    pub fn stage_json<T: Serialize>(
        &mut self,
        path: impl Into<PathBuf>,
        value: &T,
    ) -> Result<&mut Self> {
        let path = path.into();
        let mut body = serde_json::to_string_pretty(value)
            .with_context(|| format!("序列化 {} 的 JSON 失败", path.display()))?;
        body.push('\n');
        Ok(self.stage(path, body.into_bytes()))
    }

    /// [`commit`](Self::commit) 会写入的路径，按暂存顺序排列。
    /// 不写入任何内容——这是预览表面。
    #[must_use]
    pub fn preview(&self) -> Vec<&Path> {
        self.writes.iter().map(|w| w.path.as_path()).collect()
    }

    /// 当没有暂存内容时为 true。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.writes.is_empty()
    }

    /// 原子地应用每个暂存的写入。
    ///
    /// 成功后所有文件都更新。在第一次失败时，每个已经落地的写入
    /// 都回滚到其捕获的提交前状态，并返回原始错误（回滚失败附加
    /// 为上下文信息）。
    pub fn commit(self) -> Result<()> {
        let mut snapshots: Vec<Snapshot> = Vec::with_capacity(self.writes.len());

        for write in &self.writes {
            // 在修改前捕获提交前状态，以便可以恢复。
            let original = match fs::read(&write.path) {
                Ok(bytes) => Some(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => {
                    rollback(&snapshots);
                    return Err(e).with_context(|| {
                        format!(
                            "在写入前读取现有 {} 失败；回滚了 {} 个先前的更改",
                            write.path.display(),
                            snapshots.len()
                        )
                    });
                }
            };

            match atomic_write(&write.path, &write.bytes) {
                Ok(()) => snapshots.push(Snapshot {
                    path: write.path.clone(),
                    original,
                }),
                Err(err) => {
                    // 此写入未落地（atomic_write 是全有或全无），
                    // 因此只回滚之前的写入。
                    rollback(&snapshots);
                    return Err(err).with_context(|| {
                        format!(
                            "设置事务写入 {} 失败；回滚了 {} 个先前的更改",
                            write.path.display(),
                            snapshots.len()
                        )
                    });
                }
            }
        }

        Ok(())
    }
}

/// 将每个快照恢复到其捕获的提交前状态。尽力而为：
/// 回滚错误会被记录但不会中止其余恢复，因为让尽可能多的文件
/// 保持其原始状态是目标。
fn rollback(snapshots: &[Snapshot]) {
    for snap in snapshots.iter().rev() {
        let result = match &snap.original {
            Some(bytes) => atomic_write(&snap.path, bytes),
            None => match fs::remove_file(&snap.path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            },
        };
        if let Err(e) = result {
            tracing::error!(
                target: "config::persistence",
                "设置事务期间回滚 {} 失败: {e:#}",
                snap.path.display()
            );
        }
    }
}

/// 标记配置/JSON/环境键承载密钥值的子串。
const SENSITIVE_KEY_HINTS: &[&str] = &[
    "api_key",
    "apikey",
    "api-key",
    "secret",
    "token",
    "password",
    "passwd",
    "authorization",
    "auth_token",
    "access_key",
    "client_secret",
    "private_key",
];

/// 即使以裸值形式出现（不是 `key = value` 形式）也值得屏蔽的已知不透明令牌前缀。
/// 有意保守：仅限已知的供应商/密钥形状。
const SECRET_TOKEN_PREFIXES: &[&str] = &["sk-", "sk_", "ghp_", "gho_", "xoxb-", "xoxp-", "pk-"];

/// 替换任何已脱敏的密钥值的占位符。
pub const REDACTED: &str = "[redacted]";

/// 从任意文本中脱敏密钥承载值，使其安全地放入设置报告、
/// 日志行、错误消息或测试快照中。
///
/// 两遍处理，均无依赖：
///
/// 1. **键值赋值。** 形如 `key = value`、`key: value` 或 `key=value` 的行，
///    其键（不区分大小写，忽略引号）包含 [`SENSITIVE_KEY_HINTS`] 子串的，
///    其值被替换为 [`REDACTED`]。
/// 2. **裸令牌。** 以已知 [`SECRET_TOKEN_PREFIXES`] 开头的空格分隔词被整体替换。
///
/// 目标是纵深防御：设置状态和报告从一开始就由不包含密钥的安全摘要构建，
/// 这是任何回显原始配置文本的后备防线。
#[must_use]
pub fn redact_secrets(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut first = true;
    for line in input.split_inclusive('\n') {
        if !first {
            // split_inclusive 将换行符保留在前一个块上，因此我们
            // 无需在此处重新添加分隔符。
        }
        first = false;
        out.push_str(&redact_line(line));
    }
    out
}

/// 脱敏单行（可能包含尾随换行符）。
fn redact_line(line: &str) -> String {
    // 保留任何尾随换行符，以便调用者保持其行结构。
    let (body, newline) = match line.strip_suffix('\n') {
        Some(rest) => (rest, "\n"),
        None => (line, ""),
    };

    if let Some(redacted) = redact_keyed_assignment(body) {
        return format!("{redacted}{newline}");
    }

    // 裸令牌传递：屏蔽任何以已知前缀开头的空格分隔词。
    let mut changed = false;
    let masked: Vec<String> = body
        .split(' ')
        .map(|word| {
            let trimmed = word.trim_matches(|c| matches!(c, '"' | '\'' | ',' | ';'));
            if !trimmed.is_empty() && looks_like_secret_token(trimmed) {
                changed = true;
                word.replace(trimmed, REDACTED)
            } else {
                word.to_string()
            }
        })
        .collect();

    if changed {
        format!("{}{newline}", masked.join(" "))
    } else {
        format!("{body}{newline}")
    }
}

/// 如果 `body` 是带有敏感键的 `key <sep> value` 赋值，则返回
/// 该行并脱敏值；否则返回 `None`。
fn redact_keyed_assignment(body: &str) -> Option<String> {
    // 找到第一个分隔键值的 `=` 或 `:`。
    let sep_idx = body.find(['=', ':'])?;
    let (raw_key, rest) = body.split_at(sep_idx);
    let sep = &rest[..1];
    let raw_value = &rest[1..];

    let key_norm = raw_key
        .trim()
        .trim_matches(|c| matches!(c, '"' | '\'' | '[' | ']'))
        .to_ascii_lowercase();
    if key_norm.is_empty() || !SENSITIVE_KEY_HINTS.iter().any(|h| key_norm.contains(h)) {
        return None;
    }

    // 保留键的前导空格和原始分隔符间距，使脱敏后的行读起来自然。
    let key_lead_ws: String = raw_key.chars().take_while(|c| c.is_whitespace()).collect();
    let value_lead_ws: String = raw_value
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    let value_rest = raw_value.trim_start();
    // 如果值为空，则无需隐藏。
    if value_rest.is_empty() {
        return None;
    }
    // 保留周围引号，使结构化文件看起来仍可解析。
    let quoted = value_rest.starts_with('"') || value_rest.starts_with('\'');
    let replacement = if quoted {
        format!("\"{REDACTED}\"")
    } else {
        REDACTED.to_string()
    };
    Some(format!(
        "{key_lead_ws}{}{sep}{value_lead_ws}{replacement}",
        raw_key.trim()
    ))
}

fn looks_like_secret_token(word: &str) -> bool {
    SECRET_TOKEN_PREFIXES
        .iter()
        .any(|p| word.len() > p.len() + 6 && word.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn atomic_write_creates_parent_dirs_and_content() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/dir/state.json");
        atomic_write(&path, b"hello").unwrap();
        assert_eq!(read(&path), "hello");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_uses_owner_only_permissions() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");
        atomic_write(&path, b"x").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, SETUP_FILE_MODE);
    }

    #[test]
    fn atomic_write_replaces_existing_atomically() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");
        atomic_write(&path, b"old").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(read(&path), "new");
        // 不留残留的临时文件。
        let leftovers: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name() != "state.json")
            .collect();
        assert!(leftovers.is_empty(), "残留的临时文件: {leftovers:?}");
    }

    #[test]
    fn transaction_preview_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.json");
        let b = tmp.path().join("b.json");
        let mut tx = SetupTransaction::new();
        tx.stage(a.clone(), b"1".to_vec())
            .stage(b.clone(), b"2".to_vec());
        let preview = tx.preview();
        assert_eq!(preview, vec![a.as_path(), b.as_path()]);
        assert!(!a.exists());
        assert!(!b.exists());
    }

    #[test]
    fn dropped_transaction_leaves_files_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.json");
        {
            let mut tx = SetupTransaction::new();
            tx.stage(a.clone(), b"staged".to_vec());
            // tx 在此处被丢弃，未提交
        }
        assert!(!a.exists());
    }

    #[test]
    fn transaction_commit_applies_all() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.json");
        let b = tmp.path().join("sub/b.json");
        let mut tx = SetupTransaction::new();
        tx.stage(a.clone(), b"A".to_vec())
            .stage(b.clone(), b"B".to_vec());
        tx.commit().unwrap();
        assert_eq!(read(&a), "A");
        assert_eq!(read(&b), "B");
    }

    #[test]
    fn transaction_rolls_back_on_partial_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let good = tmp.path().join("good.json");
        fs::write(&good, "ORIGINAL").unwrap();

        // 第二个目标不可写：父路径是一个已存在的文件。
        let blocker = tmp.path().join("blocker");
        fs::write(&blocker, "i am a file").unwrap();
        let bad = blocker.join("child.json"); // 父路径是文件 → create_dir_all 失败

        let mut tx = SetupTransaction::new();
        tx.stage(good.clone(), b"UPDATED".to_vec())
            .stage(bad.clone(), b"NOPE".to_vec());
        let err = tx.commit().unwrap_err();
        assert!(format!("{err:#}").contains("rolled back"));

        // 第一个文件必须恢复到其原始内容。
        assert_eq!(read(&good), "ORIGINAL");
        assert!(!bad.exists());
    }

    #[test]
    fn transaction_rollback_removes_newly_created_file() {
        let tmp = tempfile::tempdir().unwrap();
        let fresh = tmp.path().join("fresh.json"); // 之前不存在
        let blocker = tmp.path().join("blocker");
        fs::write(&blocker, "file").unwrap();
        let bad = blocker.join("child.json");

        let mut tx = SetupTransaction::new();
        tx.stage(fresh.clone(), b"created".to_vec())
            .stage(bad, b"x".to_vec());
        assert!(tx.commit().is_err());
        // 新创建的文件必须在回滚时删除，而不是留下。
        assert!(!fresh.exists());
    }

    #[test]
    fn redact_masks_keyed_secrets_toml_and_json() {
        let input = "\
api_key = \"sk-supersecretvalue123\"
provider = \"openai\"
  \"token\": \"abc123def456ghi\",
model = \"mimo-ultraspeed\"
PASSWORD=hunter2hunter2";
        let out = redact_secrets(input);
        assert!(!out.contains("sk-supersecretvalue123"), "{out}");
        assert!(!out.contains("abc123def456ghi"), "{out}");
        assert!(!out.contains("hunter2hunter2"), "{out}");
        // 非密钥值保持不变。
        assert!(out.contains("provider = \"openai\""));
        assert!(out.contains("model = \"mimo-ultraspeed\""));
        assert!(out.matches(REDACTED).count() >= 3, "{out}");
    }

    #[test]
    fn redact_masks_bare_token_prefixes() {
        let out = redact_secrets("the leaked key sk-abcdef1234567890 appeared in a log");
        assert!(!out.contains("sk-abcdef1234567890"), "{out}");
        assert!(out.contains(REDACTED));
        assert!(out.contains("appeared in a log"));
    }

    #[test]
    fn redact_preserves_line_structure() {
        let input = "line1\nsecret = \"xyzsecretvalue\"\nline3";
        let out = redact_secrets(input);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "line1");
        assert_eq!(lines[2], "line3");
        assert!(lines[1].contains(REDACTED));
    }

    #[test]
    fn redact_leaves_plain_text_untouched() {
        let input = "the quick brown fox = jumps over";
        // `fox` 键没有敏感提示 → 不变。
        assert_eq!(redact_secrets(input), input);
    }
}
