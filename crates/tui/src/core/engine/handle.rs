//! 公开的 `EngineHandle` 方法。
//!
//! 结构体本身位于相邻的 `engine.rs` 中，因为两个构造点（`Engine::new` 和仅测试用的
//! `mock_engine_handle`）需要访问其私有的 mpsc 通道。
//! 方法接口 — `send`、`cancel*`、`is_cancelled`、
//! `approve_tool_call` / `deny_tool_call` / `retry_tool_with_policy`、
//! `submit_user_input` / `cancel_user_input` 和 `steer` — 移至此处，
//! 以便代理循环的邮箱 API 可独立审查。

use anyhow::Result;

use super::approval::{ApprovalDecision, UserInputDecision};
use super::{CancelReason, EngineHandle, Op, UserInputResponse};

impl EngineHandle {
    /// 向引擎发送一个操作
    pub async fn send(&self, op: Op) -> Result<()> {
        self.tx_op.send(op).await?;
        Ok(())
    }

    /// 尝试非阻塞地发送一个操作。
    ///
    /// 如果通道已满或已关闭，则返回 `Err`。将此用于非关键性的刷新类型操作
    /// （例如 `Op::ListSubAgents`），这些操作可以被安全地丢弃并在下一个排空周期重新请求。
    pub fn try_send(&self, op: Op) -> Result<()> {
        self.tx_op.try_send(op)?;
        Ok(())
    }

    /// 取消当前请求（用户发起路径 — 保持公开的 `cancel()` 签名稳定）。
    /// 等同于 `cancel_with_reason(CancelReason::User)`。
    pub fn cancel(&self) {
        self.cancel_with_reason(CancelReason::User);
    }

    /// 取消当前请求并锁存原因，以便下游的"请求已取消"错误消息能够指明原因。
    pub fn cancel_with_reason(&self, reason: CancelReason) {
        match self.cancel_reason.lock() {
            Ok(mut slot) => *slot = Some(reason),
            Err(poisoned) => *poisoned.into_inner() = Some(reason),
        }
        match self.cancel_token.lock() {
            Ok(token) => token.cancel(),
            Err(poisoned) => poisoned.into_inner().cancel(),
        }
        crate::retry_status::clear();
    }

    /// 检查请求当前是否已取消
    #[must_use]
    #[allow(dead_code)]
    pub fn is_cancelled(&self) -> bool {
        match self.cancel_token.lock() {
            Ok(token) => token.is_cancelled(),
            Err(poisoned) => poisoned.into_inner().is_cancelled(),
        }
    }

    /// 暂停或恢复当前可暂停的命令。
    pub fn set_paused(&self, paused: bool) {
        match self.shared_paused.lock() {
            Ok(mut slot) => *slot = paused,
            Err(poisoned) => *poisoned.into_inner() = paused,
        }
    }

    /// 检查引擎暂停门是否已设置。
    #[cfg(test)]
    #[must_use]
    pub fn is_paused(&self) -> bool {
        match self.shared_paused.lock() {
            Ok(slot) => *slot,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    /// 批准一个待处理的工具调用
    pub async fn approve_tool_call(&self, id: impl Into<String>) -> Result<()> {
        self.tx_approval
            .send(ApprovalDecision::Approved { id: id.into() })
            .await?;
        Ok(())
    }

    /// 拒绝一个待处理的工具调用
    pub async fn deny_tool_call(&self, id: impl Into<String>) -> Result<()> {
        self.tx_approval
            .send(ApprovalDecision::Denied { id: id.into() })
            .await?;
        Ok(())
    }

    /// 使用提升的沙箱策略重试一个工具调用。
    pub async fn retry_tool_with_policy(
        &self,
        id: impl Into<String>,
        policy: crate::sandbox::SandboxPolicy,
    ) -> Result<()> {
        self.tx_approval
            .send(ApprovalDecision::RetryWithPolicy {
                id: id.into(),
                policy,
            })
            .await?;
        Ok(())
    }

    /// 为 request_user_input 提交响应。
    pub async fn submit_user_input(
        &self,
        id: impl Into<String>,
        response: UserInputResponse,
    ) -> Result<()> {
        self.tx_user_input
            .send(UserInputDecision::Submitted {
                id: id.into(),
                response,
            })
            .await?;
        Ok(())
    }

    /// 取消一个 request_user_input 提示。
    pub async fn cancel_user_input(&self, id: impl Into<String>) -> Result<()> {
        self.tx_user_input
            .send(UserInputDecision::Cancelled { id: id.into() })
            .await?;
        Ok(())
    }

    /// 使用额外的用户输入引导正在进行中的回合。
    pub async fn steer(&self, content: impl Into<String>) -> Result<()> {
        self.tx_steer.send(content.into()).await?;
        Ok(())
    }

    /// 请求当前会话状态的快照。
    /// 通过 oneshot 通道直接返回快照，避免与 mpsc 接收器上的 SSE 事件流竞争。
    pub async fn get_session_snapshot(&self) -> Result<crate::core::ops::SessionSnapshot> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
        self.send(Op::GetSessionSnapshot { tx }).await?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Engine dropped session snapshot oneshot"))
    }

    /// 请求活跃的提供者请求并发状态。
    pub async fn get_provider_runtime_status(
        &self,
    ) -> Result<crate::core::ops::ProviderRuntimeStatus> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
        self.send(Op::GetProviderRuntimeStatus { tx }).await?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Engine dropped provider runtime status oneshot"))
    }
}
