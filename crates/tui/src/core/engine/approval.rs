//! 代理循环的审批 + 用户输入握手。
//!
//! 从 `core/engine.rs`（P1.3）中提取。每当工具需要显式审批（`await_tool_approval`）
//! 或工具请求实时用户输入（`await_user_input`）时，代理循环会阻塞在这两个 future 上。
//! 通道和引擎状态对父模块保持私有。

use std::time::Duration;

use crate::core::events::Event;
use crate::tools::spec::ToolError;
use crate::tools::user_input::{UserInputRequest, UserInputResponse};

const USER_INPUT_TIMEOUT: Duration = Duration::from_secs(300);

use super::Engine;

#[derive(Debug, Clone)]
pub(super) enum ApprovalDecision {
    Approved {
        id: String,
    },
    Denied {
        id: String,
    },
    /// 使用提升的沙箱策略重试一个工具。
    RetryWithPolicy {
        id: String,
        policy: crate::sandbox::SandboxPolicy,
    },
}

#[derive(Debug, Clone)]
pub(super) enum UserInputDecision {
    Submitted {
        id: String,
        response: UserInputResponse,
    },
    Cancelled {
        id: String,
    },
}

/// 等待用户工具审批的结果。
#[derive(Debug)]
pub(super) enum ApprovalResult {
    /// 用户批准了工具执行。
    Approved,
    /// 用户拒绝了工具执行。
    Denied,
    /// 用户请求使用提升的沙箱策略重试。
    RetryWithPolicy(crate::sandbox::SandboxPolicy),
}

impl Engine {
    /// 当引擎知道原因时格式化的取消后缀。
    /// #1541 打开期间，某些内部取消路径仍使用原始令牌；这些路径保持旧消息，不猜测原因。
    fn cancel_reason_suffix(&self) -> String {
        let reason = match self.cancel_reason.lock() {
            Ok(slot) => *slot,
            Err(poisoned) => *poisoned.into_inner(),
        };
        match reason {
            Some(reason) => format!(" (reason: {})", reason.describe()),
            None => String::new(),
        }
    }

    pub(super) async fn await_tool_approval(
        &mut self,
        tool_id: &str,
    ) -> Result<ApprovalResult, ToolError> {
        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    let suffix = self.cancel_reason_suffix();
                    return Err(ToolError::execution_failed(
                        format!("Request cancelled while awaiting approval{suffix}"),
                    ));
                }
                decision = self.rx_approval.recv() => {
                    let Some(decision) = decision else {
                        return Err(ToolError::execution_failed(
                            "Approval channel closed — engine is shutting down. \
                             The approval modal can no longer reach the engine; \
                             this is typically a teardown race, not a user action."
                                .to_string(),
                        ));
                    };
                    match decision {
                        ApprovalDecision::Approved { id } if id == tool_id => {
                            return Ok(ApprovalResult::Approved);
                        }
                        ApprovalDecision::Denied { id } if id == tool_id => {
                            return Ok(ApprovalResult::Denied);
                        }
                        ApprovalDecision::RetryWithPolicy { id, policy } if id == tool_id => {
                            return Ok(ApprovalResult::RetryWithPolicy(policy));
                        }
                        _ => continue,
                    }
                }
            }
        }
    }

    pub(super) async fn await_user_input(
        &mut self,
        tool_id: &str,
        request: UserInputRequest,
    ) -> Result<UserInputResponse, ToolError> {
        let _ = self
            .tx_event
            .send(Event::UserInputRequired {
                id: tool_id.to_string(),
                request,
            })
            .await;

        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    let suffix = self.cancel_reason_suffix();
                    return Err(ToolError::execution_failed(
                        format!("Request cancelled while awaiting user input{suffix}"),
                    ));
                }
                result = tokio::time::timeout(USER_INPUT_TIMEOUT, self.rx_user_input.recv()) => {
                    match result {
                        Ok(Some(decision)) => {
                            match decision {
                                UserInputDecision::Submitted { id, response } if id == tool_id => {
                                    return Ok(response);
                                }
                                UserInputDecision::Cancelled { id } if id == tool_id => {
                                    return Err(ToolError::execution_failed(
                                        "User input cancelled".to_string(),
                                    ));
                                }
                                _ => continue,
                            }
                        }
                        Ok(None) => {
                            return Err(ToolError::execution_failed(
                                "User input channel closed".to_string(),
                            ));
                        }
                        Err(_) => {
                            let _ = self
                                .tx_event
                                .send(Event::Status {
                                    message: format!(
                                        "User input timed out after {}s",
                                        USER_INPUT_TIMEOUT.as_secs()
                                    ),
                                })
                                .await;
                            return Err(ToolError::execution_failed(
                                format!(
                                    "User input timed out after {}s",
                                    USER_INPUT_TIMEOUT.as_secs()
                                ),
                            ));
                        }
                    }
                }
            }
        }
    }
}
