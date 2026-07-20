//! @-提及的 frecency 跟踪 (#441)。
//!
//! 记录用户 @-提及的每个文件的时间戳和点击次数，分数随时间衰减，
//! 以使上周热门的文件排在 5 分钟前提及的文件之后，并根据结果分数
//! 重新排序提及弹出补全。持久化在 `~/.deepseek/file-frecency.jsonl`
//! 的单个 JSONL 文件中，因此 frecency 在重启后仍然有效。
//!
//! 只追加写入，内存中压缩：加载器将每一行重放到以仓库相对路径为键的
//! `HashMap<String, FrecencyEntry>` 中，将重复项折叠到最后一条记录。
//! 我们将内存映射上限设为 1000 条，溢出时驱逐得分最低的条目——
//! 与 OPENCODE 源代码使用相同的启发式策略。

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 我们跟踪的路径数量硬上限（#441 的验收标准）。
/// 映射超出此限制时，较旧/得分较低的条目将被驱逐。
const FRECENCY_CAP: usize = 1000;

/// frecency 分数的半衰期，以秒为单位。经过这段时间后，分数衰减到其峰值的 ½。
/// 7 天是 OPENCODE 的默认值——足够长以使常用编辑的文件在整个工作周保持热度，
/// 但又足够短以使昨天的深度探索不会永远困扰你。
const HALF_LIFE_SECS: f64 = 7.0 * 24.0 * 60.0 * 60.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FrecencyRecord {
    /// 工作区相对路径字符串。
    path: String,
    /// 条目生命周期内的总提及次数。
    count: u32,
    /// 最后一次提及的 Unix 时间戳（秒）。
    last_used: u64,
}

#[derive(Debug, Default)]
struct Store {
    by_path: HashMap<String, FrecencyRecord>,
    persisted_path: Option<PathBuf>,
    loaded: bool,
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Store::default()))
}

fn default_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codewhale").join("file-frecency.jsonl"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 记录的时间衰减 frecency 分数，以任意单位计。提及次数线性计数；
/// 总和乘以基于自 `last_used` 以来时间的指数衰减因子。
/// 早于约 5 个半衰期的记录得分基本为零。
fn decayed_score(record: &FrecencyRecord, now: u64) -> f64 {
    let age_secs = now.saturating_sub(record.last_used) as f64;
    let lambda = std::f64::consts::LN_2 / HALF_LIFE_SECS;
    (record.count as f64) * (-lambda * age_secs).exp()
}

fn ensure_loaded(store: &mut Store) {
    if store.loaded {
        return;
    }
    store.loaded = true;
    let Some(path) = default_path() else {
        return;
    };
    store.persisted_path = Some(path.clone());
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<FrecencyRecord>(line) else {
            continue;
        };
        store.by_path.insert(record.path.clone(), record);
    }
}

fn evict_to_cap(store: &mut Store, now: u64) {
    if store.by_path.len() <= FRECENCY_CAP {
        return;
    }
    let target = FRECENCY_CAP;
    let mut scored: Vec<(String, f64)> = store
        .by_path
        .iter()
        .map(|(k, v)| (k.clone(), decayed_score(v, now)))
        .collect();
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let drop_count = store.by_path.len().saturating_sub(target);
    for (key, _) in scored.iter().take(drop_count) {
        store.by_path.remove(key);
    }
}

fn append_record_line(path: &PathBuf, record: &FrecencyRecord) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(record).map_err(std::io::Error::other)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// 记录一次对 `path`（工作区相对路径字符串）的提及。更新内存存储，
/// 持久化一条 JSONL 行，如果刚超出上限则驱逐最低得分条目。
/// 尽力而为：I/O 失败会被记录并忽略——丢失一个 frecency 数据点
/// 永远不值得让用户的 `@` 自动补全失败。
pub fn record_mention(path: &str) {
    if path.is_empty() {
        return;
    }
    let store = store();
    let Ok(mut store) = store.lock() else {
        return;
    };
    ensure_loaded(&mut store);
    let now = now_secs();
    let entry = store
        .by_path
        .entry(path.to_string())
        .or_insert_with(|| FrecencyRecord {
            path: path.to_string(),
            count: 0,
            last_used: now,
        });
    entry.count = entry.count.saturating_add(1);
    entry.last_used = now;
    let snapshot = entry.clone();
    if let Some(persisted_path) = store.persisted_path.clone()
        && let Err(err) = append_record_line(&persisted_path, &snapshot)
    {
        tracing::debug!(target: "frecency", "persist failed: {err}");
    }
    evict_to_cap(&mut store, now);
}

/// 按 frecency 分数重新排序候选列表（高分优先），
/// 平局时保留原始顺序，以便底层排序器的选择不会被颠覆。
/// 存储中从未见过的候选者得分为零——它们会排在末尾，
/// 这意味着一次性提及在首次使用后将开始浮到顶部。
#[must_use]
pub fn rerank_by_frecency(candidates: Vec<String>) -> Vec<String> {
    if candidates.len() <= 1 {
        return candidates;
    }
    let store = store();
    let Ok(mut store) = store.lock() else {
        return candidates;
    };
    ensure_loaded(&mut store);
    let now = now_secs();
    let mut scored: Vec<(usize, String, f64)> = candidates
        .into_iter()
        .enumerate()
        .map(|(idx, path)| {
            let score = store
                .by_path
                .get(&path)
                .map(|r| decayed_score(r, now))
                .unwrap_or(0.0);
            (idx, path, score)
        })
        .collect();
    // 按（-分数，原始索引）稳定排序：平局时保持底层排序器的顺序。
    scored.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    scored.into_iter().map(|(_, path, _)| path).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最近提及的路径优于从未提及的路径；从未提及的路径保持其原始排序器顺序。
    #[test]
    fn rerank_floats_recent_paths_to_the_top() {
        // 使用全局存储；重置其状态以避免跨测试泄漏。
        let store = super::store();
        let mut s = store.lock().unwrap();
        s.by_path.clear();
        s.loaded = true; // skip on-disk replay
        s.persisted_path = None; // skip persistence
        let now = super::now_secs();
        s.by_path.insert(
            "src/popular.rs".into(),
            FrecencyRecord {
                path: "src/popular.rs".into(),
                count: 8,
                last_used: now,
            },
        );
        drop(s);

        let order = super::rerank_by_frecency(vec![
            "README.md".to_string(),
            "src/popular.rs".to_string(),
            "Cargo.toml".to_string(),
        ]);
        assert_eq!(order[0], "src/popular.rs");
        // README.md 原始顺序第一，Cargo.toml 第二。两者得分均为 0，
        // 因此原始相对顺序得以保留。
        assert_eq!(order[1], "README.md");
        assert_eq!(order[2], "Cargo.toml");
    }

    /// 经过足够的半衰期后，衰减分数降至低于新使用条目的分数，
    /// 仅凭计数无法让较旧的条目保持领先。以 7 天半衰期计算，
    /// 8 周即 8 个半衰期 → 约 256 倍衰减；今天被提及 2 次的条目
    /// 轻松超过两个月前被提及 50 次的条目。
    #[test]
    fn old_entries_decay_below_recent_ones() {
        let now: u64 = 7 * 24 * 60 * 60 * 8; // 8 weeks (8 half-lives)
        let stale = FrecencyRecord {
            path: "x".into(),
            count: 50,
            last_used: 0,
        };
        let fresh = FrecencyRecord {
            path: "y".into(),
            count: 2,
            last_used: now,
        };
        assert!(
            super::decayed_score(&fresh, now) > super::decayed_score(&stale, now),
            "fresh={}, stale={}",
            super::decayed_score(&fresh, now),
            super::decayed_score(&stale, now)
        );
    }
}
