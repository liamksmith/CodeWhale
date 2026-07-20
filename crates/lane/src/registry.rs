//! `$CODEWHALE_HOME/lanes/` 下的持久化 lane 注册表。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::runtime::RuntimeBackendKind;

const LANES_SUBDIR: &str = "lanes";
const LOGS_SUBDIR: &str = "logs";

/// 运行中工作流实例的生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneStatus {
    Pending,
    Running,
    Stopped,
    Failed,
    Completed,
}

impl LaneStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Completed => "completed",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }
}

/// 一条 lane 记录：一个运行中（或已完成）的工作流实例。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneRecord {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fleet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    pub runtime: RuntimeBackendKind,
    pub status: LaneStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// 当 `runtime == tmux` 时的 tmux 会话名称。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_session: Option<String>,
    /// 此 lane 的流式 JSON / NDJSON 日志的绝对路径。
    pub log_path: PathBuf,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<String>,
    /// 可选的人类可读附加目标（例如 `tmux attach -t …`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach_target: Option<String>,
    /// 工作目录清理 TTL，以秒为单位（None = 不自动清理）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_ttl_secs: Option<u64>,
}

impl LaneRecord {
    pub fn new_id() -> String {
        let short = uuid::Uuid::new_v4().to_string();
        format!("lane-{}", &short[..8])
    }

    pub fn now_rfc3339() -> String {
        Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
    }
}

/// 注册表根目录：`$CODEWHALE_HOME/lanes`。
pub fn lanes_dir() -> Result<PathBuf> {
    codewhale_config::ensure_state_dir(LANES_SUBDIR)
}

/// 持久化和加载 lane 记录。
#[derive(Debug, Clone)]
pub struct LaneRegistry {
    root: PathBuf,
}

impl LaneRegistry {
    /// 打开 `$CODEWHALE_HOME/lanes` 下的默认注册表。
    pub fn open_default() -> Result<Self> {
        Self::open(lanes_dir()?)
    }

    /// 在显式根目录下打开注册表（测试/自定义 home）。
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)
            .with_context(|| format!("create lane registry {}", root.display()))?;
        fs::create_dir_all(root.join(LOGS_SUBDIR))
            .with_context(|| format!("create lane logs under {}", root.display()))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join(LOGS_SUBDIR)
    }

    pub fn record_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }

    pub fn log_path_for(&self, id: &str) -> PathBuf {
        self.logs_dir().join(format!("{id}.ndjson"))
    }

    pub fn save(&self, record: &LaneRecord) -> Result<()> {
        let path = self.record_path(&record.id);
        let json = serde_json::to_string_pretty(record).context("serialize lane record")?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json).with_context(|| format!("write {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("rename {}", path.display()))?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Result<LaneRecord> {
        let path = self.record_path(id);
        if !path.is_file() {
            bail!("lane `{id}` not found under {}", self.root.display());
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("read lane record {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parse lane record {}", path.display()))
    }

    pub fn list(&self) -> Result<Vec<LaneRecord>> {
        let mut records = Vec::new();
        for entry in fs::read_dir(&self.root)
            .with_context(|| format!("read lane registry {}", self.root.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let text =
                fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
            match serde_json::from_str::<LaneRecord>(&text) {
                Ok(record) => records.push(record),
                Err(err) => {
                    // 跳过损坏的记录，而不是使整个列表失败。
                    eprintln!(
                        "warning: skip corrupt lane record {}: {err}",
                        path.display()
                    );
                }
            }
        }
        records.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(records)
    }

    /// 创建待处理的 lane 并保留日志文件。
    pub fn create_pending(
        &self,
        workflow: Option<String>,
        fleet: Option<String>,
        issue: Option<String>,
        goal: Option<String>,
        runtime: RuntimeBackendKind,
        worktree_ttl_secs: Option<u64>,
    ) -> Result<LaneRecord> {
        let id = LaneRecord::new_id();
        let log_path = self.log_path_for(&id);
        // 创建日志文件，以便 `lane logs` 立即可用。
        fs::write(&log_path, "").with_context(|| format!("create log {}", log_path.display()))?;
        let record = LaneRecord {
            id,
            workflow,
            fleet,
            issue,
            goal,
            runtime,
            status: LaneStatus::Pending,
            worktree_path: None,
            branch: None,
            tmux_session: None,
            log_path,
            started_at: LaneRecord::now_rfc3339(),
            stopped_at: None,
            attach_target: None,
            worktree_ttl_secs,
        };
        self.save(&record)?;
        Ok(record)
    }

    pub fn mark_running(&self, record: &mut LaneRecord) -> Result<()> {
        record.status = LaneStatus::Running;
        self.save(record)
    }

    pub fn mark_stopped(&self, record: &mut LaneRecord, status: LaneStatus) -> Result<()> {
        record.status = status;
        record.stopped_at = Some(LaneRecord::now_rfc3339());
        self.save(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn registry_persists_across_open() {
        let dir = tempdir().unwrap();
        let reg = LaneRegistry::open(dir.path()).unwrap();
        let record = reg
            .create_pending(
                Some("stopship".into()),
                Some("v0868-stopship".into()),
                Some("4090".into()),
                None,
                RuntimeBackendKind::Tmux,
                Some(3600),
            )
            .unwrap();
        let id = record.id.clone();

        let reg2 = LaneRegistry::open(dir.path()).unwrap();
        let loaded = reg2.load(&id).unwrap();
        assert_eq!(loaded.workflow.as_deref(), Some("stopship"));
        assert_eq!(loaded.fleet.as_deref(), Some("v0868-stopship"));
        assert_eq!(loaded.issue.as_deref(), Some("4090"));
        assert_eq!(loaded.runtime, RuntimeBackendKind::Tmux);
        assert_eq!(loaded.status, LaneStatus::Pending);
        assert!(loaded.log_path.is_file() || loaded.log_path.exists());

        let listed = reg2.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
    }
}
