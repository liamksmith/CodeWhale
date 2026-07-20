//! 专门的持久化 Actor，用于会话保存/检查点 I/O。
//!
//! ## 动机
//!
//! 在本模块之前，`persist_checkpoint` 和 `persist_session_snapshot`
//! 在驱动 TUI 事件循环的 tokio 工作线程上同步运行。
//! 每次调用都将所有 API 消息序列化为 JSON、写入临时文件、然后
//! 原子性地重命名——在此期间阻塞键盘输入。`save_session` 还额外调用
//! `cleanup_old_sessions`，它会列出所有会话文件、解析每个文件的元数据、
//! 排序并删除最旧的——每个轮次的开销为 O(会话字节 + 文件数)。
//!
//! ## 设计
//!
//! - 在 TUI 启动时**生成一个专用的 tokio 任务**。所有磁盘 I/O 都移到
//!   这个任务中。UI 仅通过 `try_send` 发送请求（非阻塞、有界通道丢弃）
//!   并立即返回——击键永远不会等待写入完成。
//! - **最新胜出合并**：当多个 `Checkpoint`、`SessionSnapshot` 或离线队列
//!   请求在 Actor 的下一个写入周期之前堆积时，只写入最新的一个。
//!   `ClearCheckpoint` 请求正常累积（它们廉价且可交换）。
//! - **无界通道**确保 `try_send` 始终成功；Actor 通过生成池自然地进行
//!   背压。通道中少量未完成的 `SavedSession` 值（< 1 MB）压力可忽略。

use std::sync::OnceLock;

use tokio::sync::mpsc;

use crate::session_manager::{OfflineQueueState, SavedSession, SessionManager};
use crate::utils::spawn_supervised;

// ---------------------------------------------------------------------------
// 请求类型
// ---------------------------------------------------------------------------

/// 发送给 Actor 的持久化工作项。
#[derive(Debug)]
pub enum PersistRequest {
    /// 写入崩溃恢复检查点（进行中的轮次状态）。
    Checkpoint(SavedSession),
    /// 写入完整的会话快照（完成的轮次，持久保存）。
    SessionSnapshot(SavedSession),
    /// 写入排队的/草稿离线输入，用于崩溃恢复。
    OfflineQueue {
        state: OfflineQueueState,
        session_id: Option<String>,
    },
    /// 移除排队的/草稿离线输入文件。
    ClearOfflineQueue,
    /// 移除崩溃恢复检查点文件。
    ClearCheckpoint,
    /// 优雅关闭——刷新挂起的写入，然后退出 Actor 循环。
    Shutdown,
}

#[derive(Debug)]
enum PendingOfflineQueue {
    Save {
        state: OfflineQueueState,
        session_id: Option<String>,
    },
    Clear,
}

// ---------------------------------------------------------------------------
// 句柄（由 TUI 持有）
// ---------------------------------------------------------------------------

/// UI 持有的轻量级句柄，用于排队持久化工作。
#[derive(Debug, Clone)]
pub struct PersistActorHandle {
    tx: mpsc::UnboundedSender<PersistRequest>,
}

impl PersistActorHandle {
    /// 排队一个持久化请求而不阻塞。如果 Actor 的通道已关闭
    ///（已关闭完毕），则静默丢弃请求。
    pub fn try_send(&self, request: PersistRequest) {
        let _ = self.tx.send(request);
    }
}

// ---------------------------------------------------------------------------
// 全局单例（避免通过 App 传递）
// ---------------------------------------------------------------------------

static ACTOR_TX: OnceLock<PersistActorHandle> = OnceLock::new();

/// 初始化全局持久化 Actor 句柄。必须在启动时、事件循环开始前
/// 调用一次。
pub fn init_actor(handle: PersistActorHandle) {
    let _ = ACTOR_TX.set(handle);
}

/// 通过全局句柄排队持久化请求。当 Actor 尚未初始化时是空操作
///（静默忽略）——这可能在测试或早期启动时发生。
pub fn persist(request: PersistRequest) {
    if let Some(handle) = ACTOR_TX.get() {
        handle.try_send(request);
    }
}

// ---------------------------------------------------------------------------
// Actor 生成
// ---------------------------------------------------------------------------

/// 生成持久化 Actor 任务并返回调用者存储和初始化的句柄。
///
/// 返回的句柄应传递给 [`init_actor`]，以便 `persist()` 自由函数
/// 可以从 TUI 的任何位置访问它。
pub fn spawn_persistence_actor(manager: SessionManager) -> PersistActorHandle {
    let (tx, mut rx) = mpsc::unbounded_channel::<PersistRequest>();
    let handle = PersistActorHandle { tx };

    spawn_supervised(
        "persistence-actor",
        std::panic::Location::caller(),
        async move {
            let mut latest_checkpoint: Option<SavedSession> = None;
            let mut latest_session: Option<SavedSession> = None;
            let mut latest_offline_queue: Option<PendingOfflineQueue> = None;
            let mut should_clear: bool = false;

            loop {
                // 排空所有等待中的请求，每类只保留最新的。
                while let Ok(req) = rx.try_recv() {
                    match req {
                        PersistRequest::Checkpoint(session) => {
                            // 最后写入者胜出：新的检查点取代挂起的清除，
                            // 因此两者不会在一次排空中同时生效（之前会导致先清除
                            // 然后重写过时检查点，从而撤销清除）。
                            latest_checkpoint = Some(session);
                            should_clear = false;
                        }
                        PersistRequest::SessionSnapshot(session) => {
                            latest_session = Some(session);
                        }
                        PersistRequest::OfflineQueue { state, session_id } => {
                            latest_offline_queue =
                                Some(PendingOfflineQueue::Save { state, session_id });
                        }
                        PersistRequest::ClearOfflineQueue => {
                            latest_offline_queue = Some(PendingOfflineQueue::Clear);
                        }
                        PersistRequest::ClearCheckpoint => {
                            // 清除取代挂起的检查点写入。
                            should_clear = true;
                            latest_checkpoint = None;
                        }
                        PersistRequest::Shutdown => {
                            flush_inner(
                                &manager,
                                latest_checkpoint.as_ref(),
                                latest_session.as_ref(),
                                latest_offline_queue.as_ref(),
                                should_clear,
                            );
                            return;
                        }
                    }
                }

                // 写入合并后的工作。
                if should_clear {
                    let _ = manager.clear_checkpoint();
                    should_clear = false;
                }
                if let Some(ref session) = latest_checkpoint.take() {
                    let _ = manager.save_checkpoint(session);
                }
                if let Some(ref session) = latest_session.take() {
                    let _ = manager.save_session(session);
                }
                if let Some(ref request) = latest_offline_queue.take() {
                    apply_offline_queue_request(&manager, request);
                }

                // 阻塞直到下一个请求到达。
                match rx.recv().await {
                    Some(PersistRequest::Checkpoint(session)) => {
                        latest_checkpoint = Some(session);
                        should_clear = false;
                    }
                    Some(PersistRequest::SessionSnapshot(session)) => {
                        latest_session = Some(session);
                    }
                    Some(PersistRequest::OfflineQueue { state, session_id }) => {
                        latest_offline_queue =
                            Some(PendingOfflineQueue::Save { state, session_id });
                    }
                    Some(PersistRequest::ClearOfflineQueue) => {
                        latest_offline_queue = Some(PendingOfflineQueue::Clear);
                    }
                    Some(PersistRequest::ClearCheckpoint) => {
                        should_clear = true;
                        latest_checkpoint = None;
                    }
                    Some(PersistRequest::Shutdown) => {
                        flush_inner(
                            &manager,
                            latest_checkpoint.as_ref(),
                            latest_session.as_ref(),
                            latest_offline_queue.as_ref(),
                            should_clear,
                        );
                        return;
                    }
                    None => {
                        // 通道关闭——最终刷新并退出。
                        flush_inner(
                            &manager,
                            latest_checkpoint.as_ref(),
                            latest_session.as_ref(),
                            latest_offline_queue.as_ref(),
                            should_clear,
                        );
                        return;
                    }
                }
            }
        },
    );

    handle
}

/// 将所有挂起的工作写入磁盘（在关闭时使用）。
fn flush_inner(
    manager: &SessionManager,
    checkpoint: Option<&SavedSession>,
    session: Option<&SavedSession>,
    offline_queue: Option<&PendingOfflineQueue>,
    should_clear: bool,
) {
    if should_clear {
        let _ = manager.clear_checkpoint();
    }
    if let Some(s) = checkpoint {
        let _ = manager.save_checkpoint(s);
    }
    if let Some(s) = session {
        let _ = manager.save_session(s);
    }
    if let Some(request) = offline_queue {
        apply_offline_queue_request(manager, request);
    }
}

fn apply_offline_queue_request(manager: &SessionManager, request: &PendingOfflineQueue) {
    match request {
        PendingOfflineQueue::Save { state, session_id } => {
            let _ = manager.save_offline_queue_state(state, session_id.as_deref());
        }
        PendingOfflineQueue::Clear => {
            let _ = manager.clear_offline_queue_state();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::session_manager::{OfflineQueueState, QueuedSessionMessage};

    async fn wait_until(mut predicate: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if predicate() {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "等待持久化 Actor 超时"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn actor_persists_and_clears_offline_queue_requests() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("manager");
        let queue_path = sessions_dir.join("checkpoints").join("offline_queue.json");
        let handle = spawn_persistence_actor(manager);

        let state = OfflineQueueState {
            messages: vec![QueuedSessionMessage {
                display: "queued from enter".to_string(),
                skill_instruction: None,
            }],
            ..OfflineQueueState::default()
        };

        handle.try_send(PersistRequest::OfflineQueue {
            state,
            session_id: Some("session-A".to_string()),
        });
        wait_until(|| {
            std::fs::read_to_string(&queue_path)
                .is_ok_and(|body| body.contains("queued from enter"))
        })
        .await;

        handle.try_send(PersistRequest::ClearOfflineQueue);
        wait_until(|| !queue_path.exists()).await;
        handle.try_send(PersistRequest::Shutdown);
    }
}
