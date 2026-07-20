//! 跨会话 composer 输入历史 (#366)。
//!
//! 将用户输入的提示词持久化到 `~/.codewhale/composer_history.txt`
//! （仅在旧版 `~/.deepseek/composer_history.txt` 已存在时回退到它，#3240），
//! 这样在 composer 中按上箭头可以回忆起之前会话的提交内容，
//! 而不仅仅是当前会话。每个条目一行，最早的在前，
//! 最多 [`MAX_HISTORY_ENTRIES`] 个条目（添加时修剪较早的条目）。
//!
//! 以 `/` 开头的条目（斜杠命令）不会被存储——它们会污染回忆流，
//! 且模糊斜杠菜单已覆盖了它们。空/仅空白的输入也会被跳过。
//!
//! ## 离线写入 (#1927)
//!
//! [`append_history`] 过去会阻塞调用方进行读取然后原子重写整个文件。
//! 这在 UI 线程的 `submit_input` 内部运行，导致按 Enter 后出现
//! 可感知的卡顿。现在公共入口通过 [`writer_sender`] 将工作交给
//! 专用的写入线程并立即返回。提交按到达顺序保持串行化，
//! 因此磁盘上的文件保持"最早优先"的不变性。

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

/// 持久化历史的硬上限。保持文件较小（典型条目 < 200 字符，
/// 所以 1000 个条目 ≈ 200 KB）并限制启动加载时间。
pub const MAX_HISTORY_ENTRIES: usize = 1000;

const HISTORY_FILE_NAME: &str = "composer_history.txt";

fn default_history_path() -> Option<PathBuf> {
    history_path_with_home(dirs::home_dir())
}

/// 解析 `home` 下的 composer 历史文件，优先使用 CodeWhale 根目录，
/// 仅在旧版文件已存在时回退到旧版 `.deepseek` 根目录。
///
/// 在新安装（两个文件都不存在）时，返回 `.codewhale` 路径，
/// 这样写入者永远不会在运行时重新创建 `~/.deepseek/` (#3240)，
/// 而尚未迁移的用户继续读取和追加到他们现有的旧版历史中。
/// 镜像了 `snapshot::paths` 和 `artifacts` 使用的主/旧版解析方式。
fn history_path_with_home(home: Option<PathBuf>) -> Option<PathBuf> {
    let home = home?;
    let primary = home.join(".codewhale").join(HISTORY_FILE_NAME);
    if primary.exists() {
        return Some(primary);
    }
    let legacy = home.join(".deepseek").join(HISTORY_FILE_NAME);
    if legacy.exists() {
        return Some(legacy);
    }
    Some(primary)
}

/// 将持久化的历史读取到内存。如果文件不存在或无法解析，返回空 vec——这是尽力而为的操作。
#[must_use]
pub fn load_history() -> Vec<String> {
    let Some(path) = default_history_path() else {
        return Vec::new();
    };
    load_history_from(&path)
}

fn load_history_from(path: &Path) -> Vec<String> {
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .collect()
}

/// 向持久化历史追加一个条目，修剪旧条目以
/// 保持在 [`MAX_HISTORY_ENTRIES`] 内。斜杠命令和空输入
/// 会被跳过——它们对回忆没有帮助。
///
/// 尽力而为且非阻塞——工作被转发到专用的写入线程，
/// 因此调用方（通常是 UI 提交处理器）立即返回。
/// 参见模块文档了解原理 (#1927)。写入线程上的失败
/// 通过 `tracing` 记录但不会传播。
pub fn append_history(entry: &str) {
    let Some(path) = default_history_path() else {
        return;
    };
    append_history_dispatched(&path, entry);
}

/// [`append_history`] 的可注入路径变体，由测试使用。将工作转发
/// 到专用的写入线程（如果通道发送失败则回退到同步写入），
/// 因此调用方永远不会阻塞在磁盘 I/O 上。
fn append_history_dispatched(path: &Path, entry: &str) {
    let entry = entry.to_string();
    if let Err(err) = writer_sender().send(HistoryWrite::Append(path.to_path_buf(), entry)) {
        match err.0 {
            HistoryWrite::Append(path, entry) => append_history_to(&path, &entry),
            #[cfg(test)]
            HistoryWrite::Flush(_) => unreachable!("flush messages are only sent by tests"),
        }
    }
}

enum HistoryWrite {
    Append(PathBuf, String),
    #[cfg(test)]
    Flush(Sender<()>),
}

/// 专用的 composer 历史写入线程的惰性单例发送者。
/// 首次使用时初始化；线程在进程生命周期内运行，
/// 按到达顺序排空排队的写入。
fn writer_sender() -> &'static Sender<HistoryWrite> {
    static SENDER: OnceLock<Sender<HistoryWrite>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = channel::<HistoryWrite>();
        let spawn_result = std::thread::Builder::new()
            .name("composer-history-writer".to_string())
            .spawn(move || {
                // 当所有发送者都已释放时 recv() 返回 Err，
                // 这只在进程关闭时发生，因为单例发送者
                // 在进程的整个生命周期内都存在于 static 中。
                while let Ok(message) = rx.recv() {
                    match message {
                        HistoryWrite::Append(path, entry) => {
                            append_history_batch(&rx, (path, entry));
                        }
                        #[cfg(test)]
                        HistoryWrite::Flush(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            });
        if let Err(err) = spawn_result {
            tracing::warn!("Failed to spawn composer-history-writer: {err}");
        }
        tx
    })
}

fn append_history_batch(rx: &Receiver<HistoryWrite>, first: (PathBuf, String)) {
    let mut pending = vec![first];
    #[cfg(test)]
    let mut flush = None;

    loop {
        match rx.recv_timeout(Duration::from_millis(2)) {
            Ok(HistoryWrite::Append(path, entry)) => pending.push((path, entry)),
            #[cfg(test)]
            Ok(HistoryWrite::Flush(done)) => {
                flush = Some(done);
                break;
            }
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    for (path, entries) in group_history_writes_by_path(pending) {
        append_history_entries_to(&path, entries.iter().map(String::as_str));
    }

    #[cfg(test)]
    if let Some(done) = flush {
        let _ = done.send(());
    }
}

fn group_history_writes_by_path(writes: Vec<(PathBuf, String)>) -> Vec<(PathBuf, Vec<String>)> {
    let mut grouped: Vec<(PathBuf, Vec<String>)> = Vec::new();

    for (path, entry) in writes {
        if let Some((_, entries)) = grouped
            .iter_mut()
            .find(|(existing_path, _)| existing_path == &path)
        {
            entries.push(entry);
        } else {
            grouped.push((path, vec![entry]));
        }
    }

    grouped
}

fn append_history_to(path: &Path, entry: &str) {
    append_history_entries_to(path, std::iter::once(entry));
}

fn append_history_entries_to<'a>(
    path: &Path,
    entries_to_append: impl IntoIterator<Item = &'a str>,
) {
    if let Some(parent) = path.parent()
        && let Err(err) = fs::create_dir_all(parent)
    {
        tracing::warn!(
            "Failed to create composer history dir {}: {err}",
            parent.display()
        );
        return;
    }

    // 读取现有条目，追加新条目，从前面修剪
    // 直到低于上限，然后原子重写。
    let mut entries = load_history_from(path);
    let mut changed = false;
    for entry in entries_to_append {
        let trimmed = entry.trim();
        if trimmed.is_empty() || trimmed.starts_with('/') {
            continue;
        }
        if entries.last().map(String::as_str) == Some(trimmed) {
            // 去重连续重复——相同提示词的重复提交不应使文件膨胀。
            continue;
        }
        entries.push(trimmed.to_string());
        changed = true;
    }

    if !changed {
        return;
    }

    if entries.len() > MAX_HISTORY_ENTRIES {
        let excess = entries.len() - MAX_HISTORY_ENTRIES;
        entries.drain(0..excess);
    }

    let payload = entries.join("\n") + "\n";
    if let Err(err) = write_history_atomic(path, payload.as_bytes()) {
        tracing::warn!(
            "Failed to persist composer history at {}: {err}",
            path.display()
        );
    }
}

fn write_history_atomic(path: &Path, payload: &[u8]) -> std::io::Result<()> {
    const RETRY_DELAYS: &[Duration] = &[
        Duration::from_millis(5),
        Duration::from_millis(10),
        Duration::from_millis(25),
        Duration::from_millis(50),
        Duration::from_millis(100),
        Duration::from_millis(200),
        Duration::from_millis(400),
    ];

    for (attempt, delay) in RETRY_DELAYS
        .iter()
        .map(Some)
        .chain(std::iter::once(None))
        .enumerate()
    {
        match crate::utils::write_atomic(path, payload) {
            Ok(()) => return Ok(()),
            Err(err) if delay.is_some() => {
                tracing::debug!(
                    "Retrying composer history write to {} after attempt {} failed: {err}",
                    path.display(),
                    attempt + 1
                );
                std::thread::sleep(*delay.expect("delay checked"));
            }
            Err(err) => return Err(err),
        }
    }

    unreachable!("retry iterator always ends with a final write attempt")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// 测试使用注入路径的 `*_from` / `*_to` 辅助函数，这样
    /// 就不需要修改 `HOME`（在 Windows 上 `dirs::home_dir()` 不识别它
    /// ——它读取 `USERPROFILE` / `SHGetKnownFolderPath`）。
    /// 这使得测试套件在所有三个 CI 运行器上都可移植，
    /// 无需按平台进行环境调整。
    fn temp_history_path() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join(HISTORY_FILE_NAME);
        (tmp, path)
    }

    fn flush_history_writer_for_tests(timeout: Duration) {
        let (done_tx, done_rx) = channel();
        writer_sender()
            .send(HistoryWrite::Flush(done_tx))
            .expect("history writer accepts flush");
        done_rx
            .recv_timeout(timeout)
            .expect("history writer flush timed out");
    }

    // #3240：新安装必须在 `.codewhale` 下解析历史文件，
    // 绝不会在旧版 `.deepseek` 目录下，这样正常使用不会重新创建它。
    #[test]
    fn fresh_install_uses_codewhale_not_legacy() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = history_path_with_home(Some(tmp.path().to_path_buf()))
            .expect("path resolves with a home dir");
        assert_eq!(path, tmp.path().join(".codewhale").join(HISTORY_FILE_NAME));
        assert!(
            !path.starts_with(tmp.path().join(".deepseek")),
            "fresh install must not target the legacy .deepseek dir: {path:?}"
        );
    }

    // 迁移注意：现有的旧版历史仍然被读取/追加。
    #[test]
    fn existing_legacy_history_is_still_used() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join(".deepseek").join(HISTORY_FILE_NAME);
        fs::create_dir_all(legacy.parent().expect("legacy parent")).expect("mkdir legacy");
        fs::write(&legacy, "old entry\n").expect("seed legacy history");
        let path = history_path_with_home(Some(tmp.path().to_path_buf())).expect("path resolves");
        assert_eq!(path, legacy);
    }

    // 一旦 `.codewhale` 历史存在，它将覆盖任何旧版文件。
    #[test]
    fn codewhale_history_preferred_over_legacy() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let primary = tmp.path().join(".codewhale").join(HISTORY_FILE_NAME);
        let legacy = tmp.path().join(".deepseek").join(HISTORY_FILE_NAME);
        for p in [&primary, &legacy] {
            fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            fs::write(p, "x\n").expect("seed");
        }
        let path = history_path_with_home(Some(tmp.path().to_path_buf())).expect("path resolves");
        assert_eq!(path, primary);
    }

    #[test]
    fn append_and_load_round_trip() {
        let (_tmp, path) = temp_history_path();
        append_history_to(&path, "first");
        append_history_to(&path, "second");
        append_history_to(&path, "third");
        assert_eq!(load_history_from(&path), vec!["first", "second", "third"]);
    }

    #[test]
    fn slash_commands_skipped() {
        let (_tmp, path) = temp_history_path();
        append_history_to(&path, "/help");
        append_history_to(&path, "real prompt");
        append_history_to(&path, "/cost");
        assert_eq!(load_history_from(&path), vec!["real prompt"]);
    }

    #[test]
    fn empty_and_whitespace_skipped() {
        let (_tmp, path) = temp_history_path();
        append_history_to(&path, "");
        append_history_to(&path, "   ");
        append_history_to(&path, "\n\t");
        append_history_to(&path, "real");
        assert_eq!(load_history_from(&path), vec!["real"]);
    }

    #[test]
    fn consecutive_duplicates_deduped() {
        let (_tmp, path) = temp_history_path();
        append_history_to(&path, "same");
        append_history_to(&path, "same");
        append_history_to(&path, "same");
        append_history_to(&path, "different");
        append_history_to(&path, "same");
        assert_eq!(load_history_from(&path), vec!["same", "different", "same"]);
    }

    #[test]
    fn pruned_to_cap_at_append_time() {
        let (_tmp, path) = temp_history_path();
        for i in 0..(MAX_HISTORY_ENTRIES + 50) {
            append_history_to(&path, &format!("entry {i}"));
        }
        let history = load_history_from(&path);
        assert_eq!(history.len(), MAX_HISTORY_ENTRIES);
        // 最新的条目保留；最旧的 50 个被修剪。
        assert_eq!(history.first().map(String::as_str), Some("entry 50"));
        assert_eq!(
            history.last().map(String::as_str),
            Some(format!("entry {}", MAX_HISTORY_ENTRIES + 49)).as_deref()
        );
    }

    #[test]
    fn missing_file_loads_empty() {
        let (_tmp, path) = temp_history_path();
        assert!(load_history_from(&path).is_empty());
    }

    /// #1927 的回归测试——即使同步写入种子文件很慢，
    /// 分派的追加路径也必须迅速返回。我们预填充文件
    /// 约 1000 个条目（上限），这样同步读取-修改-写入
    /// 在任何平台上都会花费实际的磁盘时间，然后多次调用
    /// `append_history_dispatched` 并断言累计挂钟时间
    /// 远低于用户报告的卡顿。
    #[test]
    fn append_history_dispatched_does_not_block_the_caller() {
        let (_tmp, path) = temp_history_path();
        // 预填充接近上限，使同步重写非平凡。
        let seed = (0..(MAX_HISTORY_ENTRIES - 50))
            .map(|i| format!("seed entry {i}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(&path, seed).expect("seed history");

        let start = Instant::now();
        for i in 0..50 {
            append_history_dispatched(&path, &format!("new entry {i}"));
        }
        let dispatch_elapsed = start.elapsed();

        // 在约 200KB 文件上进行 50 次同步读取-修改-写入循环应该是
        // 可测量的（即使在快速 SSD 上也要几十毫秒）。分派路径
        // 将工作交给写入线程并返回；整个循环
        // 应该在个位数毫秒内完成。选择一个宽松的 CI 安全
        // 边界，仍能捕获回退到旧同步路径的回归。
        assert!(
            dispatch_elapsed < Duration::from_millis(150),
            "append_history dispatch was too slow: {dispatch_elapsed:?} \
             (likely re-introduced #1927: caller blocked on disk write)"
        );

        flush_history_writer_for_tests(Duration::from_secs(if cfg!(windows) { 10 } else { 5 }));

        let loaded = load_history_from(&path);
        assert!(
            loaded.iter().any(|line| line == "new entry 49"),
            "writer thread did not persist the dispatched entries; \
             loaded {} entries, last = {:?}",
            loaded.len(),
            loaded.last()
        );
        assert!(loaded.iter().any(|line| line == "new entry 0"));
    }
}
