//! 通过 TUI 请求用户输入的工具和类型。

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInputOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInputQuestion {
    pub header: String,
    pub id: String,
    pub question: String,
    pub options: Vec<UserInputOption>,
    /// 当为 `true` 时，模态框除了固定选项外还提供一个自由文本的"其他"响应。
    /// 默认为 `false` 以向后兼容（省略此字段的旧负载获得之前的行为）。
    #[serde(default)]
    pub allow_free_text: bool,
    /// 当为 `true` 时，用户可以在确认前选择多个选项。
    #[serde(default)]
    pub multi_select: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInputRequest {
    pub questions: Vec<UserInputQuestion>,
}

impl UserInputRequest {
    pub fn from_value(value: &Value) -> Result<Self, ToolError> {
        let request: UserInputRequest = serde_json::from_value(value.clone()).map_err(|e| {
            ToolError::invalid_input(format!("Invalid request_user_input payload: {e}"))
        })?;
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), ToolError> {
        if self.questions.is_empty() {
            return Err(ToolError::invalid_input(
                "request_user_input.questions must be non-empty",
            ));
        }
        if self.questions.len() > 3 {
            return Err(ToolError::invalid_input(
                "request_user_input.questions must contain 1 to 3 items",
            ));
        }
        for q in &self.questions {
            if q.header.trim().is_empty() {
                return Err(ToolError::invalid_input(
                    "request_user_input.questions.header cannot be empty",
                ));
            }
            if q.id.trim().is_empty() {
                return Err(ToolError::invalid_input(
                    "request_user_input.questions.id cannot be empty",
                ));
            }
            if q.question.trim().is_empty() {
                return Err(ToolError::invalid_input(
                    "request_user_input.questions.question cannot be empty",
                ));
            }
            if q.options.len() < 2 || q.options.len() > 4 {
                return Err(ToolError::invalid_input(
                    "request_user_input.questions.options must contain 2 to 4 items",
                ));
            }
            for opt in &q.options {
                if opt.label.trim().is_empty() {
                    return Err(ToolError::invalid_input(
                        "request_user_input option label cannot be empty",
                    ));
                }
                if opt.description.trim().is_empty() {
                    return Err(ToolError::invalid_input(
                        "request_user_input option description cannot be empty",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInputAnswer {
    pub id: String,
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInputResponse {
    pub answers: Vec<UserInputAnswer>,
}

pub struct RequestUserInputTool;

#[async_trait]
impl ToolSpec for RequestUserInputTool {
    fn name(&self) -> &'static str {
        "request_user_input"
    }

    fn description(&self) -> &'static str {
        "Ask the user 1-3 short questions and return their selections."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "header": { "type": "string" },
                            "id": { "type": "string" },
                            "question": { "type": "string" },
                            "options": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": { "type": "string" },
                                        "description": { "type": "string" }
                                    },
                                    "required": ["label", "description"]
                                },
                                "minItems": 2,
                                "maxItems": 4
                            },
                            "allow_free_text": {
                                "type": "boolean",
                                "description": "When true, also offer a free-text 'Other' response. Defaults to false.",
                                "default": false
                            },
                            "multi_select": {
                                "type": "boolean",
                                "description": "When true, allow selecting more than one option. Defaults to false.",
                                "default": false
                            }
                        },
                        "required": ["header", "id", "question", "options"]
                    },
                    "minItems": 1,
                    "maxItems": 3
                }
            },
            "required": ["questions"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(
        &self,
        _input: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::execution_failed(
            "request_user_input must be handled by the engine",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_request_shape() {
        let request = UserInputRequest {
            questions: vec![UserInputQuestion {
                header: "Pick".to_string(),
                id: "choice".to_string(),
                question: "Which option?".to_string(),
                options: vec![
                    UserInputOption {
                        label: "A".to_string(),
                        description: "Option A".to_string(),
                    },
                    UserInputOption {
                        label: "B".to_string(),
                        description: "Option B".to_string(),
                    },
                ],
                allow_free_text: false,
                multi_select: false,
            }],
        };
        assert!(request.validate().is_ok());
    }

    #[test]
    fn from_value_accepts_four_options_and_flags() {
        // 与 tools/subagent/tests.rs 中使用的 json! 字面量风格一致，
        // 并测试来自 issue #3102 的模式放宽：4 个选项（之前上限为 3）加上新的 allow_free_text / multi_select 标志。
        let input = json!({
            "questions": [{
                "header": "Scope",
                "id": "scope",
                "question": "Which surfaces should this change affect?",
                "options": [
                    { "label": "TUI", "description": "Visible modal flow only" },
                    { "label": "Headless", "description": "Protocol event only" },
                    { "label": "All surfaces", "description": "TUI and headless" },
                    { "label": "CLI", "description": "Command-line surface" }
                ],
                "allow_free_text": true,
                "multi_select": true
            }]
        });
        let request = UserInputRequest::from_value(&input).expect("4 options + flags parse");
        assert_eq!(request.questions.len(), 1);
        assert_eq!(request.questions[0].options.len(), 4);
        assert!(request.questions[0].allow_free_text);
        assert!(request.questions[0].multi_select);
    }

    #[test]
    fn from_value_defaults_flags_when_omitted() {
        // 向后兼容：省略了新布尔字段的旧负载仍必须能解析，两者默认为 false。
        let input = json!({
            "questions": [{
                "header": "Pick",
                "id": "choice",
                "question": "Which?",
                "options": [
                    { "label": "A", "description": "a" },
                    { "label": "B", "description": "b" }
                ]
            }]
        });
        let request = UserInputRequest::from_value(&input).expect("legacy payload parses");
        assert!(!request.questions[0].allow_free_text);
        assert!(!request.questions[0].multi_select);
    }

    #[test]
    fn rejects_five_options() {
        let input = json!({
            "questions": [{
                "header": "Pick",
                "id": "choice",
                "question": "Which?",
                "options": [
                    { "label": "A", "description": "a" },
                    { "label": "B", "description": "b" },
                    { "label": "C", "description": "c" },
                    { "label": "D", "description": "d" },
                    { "label": "E", "description": "e" }
                ]
            }]
        });
        let err = UserInputRequest::from_value(&input).expect_err("5 options must fail");
        assert!(err.to_string().contains("2 to 4 items"));
    }

    fn yes_no_question(header: &str, id: &str) -> UserInputQuestion {
        UserInputQuestion {
            header: header.to_string(),
            id: id.to_string(),
            question: "?".to_string(),
            options: vec![
                UserInputOption {
                    label: "A".to_string(),
                    description: "A".to_string(),
                },
                UserInputOption {
                    label: "B".to_string(),
                    description: "B".to_string(),
                },
            ],
            allow_free_text: false,
            multi_select: false,
        }
    }

    #[test]
    fn rejects_too_many_questions() {
        let request = UserInputRequest {
            questions: vec![
                yes_no_question("Q1", "q1"),
                yes_no_question("Q2", "q2"),
                yes_no_question("Q3", "q3"),
                yes_no_question("Q4", "q4"),
            ],
        };
        assert!(request.validate().is_err());
    }
}
