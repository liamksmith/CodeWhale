//! 模型可见的小米 MiMo 语音/TTS 生成工具。
//!
//! 这将 CLI `speech` / `tts` 命令镜像为一级 API 工具，
//! 以便 TUI 模型可以生成叙述音频，而无需通过嵌套的
//! CodeWhale 进程执行 shell 命令。

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose};
use serde_json::{Value, json};

use crate::client::{DeepSeekClient, SpeechSynthesisRequest};
use crate::config::{ApiProvider, normalize_model_name_for_provider};
use crate::network_policy::{Decision, host_from_url};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_bool, optional_str, required_str,
};

pub(crate) const DEFAULT_FORMAT: &str = "wav";
pub(crate) const DEFAULT_VOICE: &str = "mimo_default";
const VOICE_CLONE_BASE64_MAX_BYTES: usize = 10 * 1024 * 1024;
pub(crate) const SUPPORTED_SPEECH_FORMATS: &[&str] = &["wav", "mp3", "pcm16"];

pub const SUPPORTED_XIAOMI_MIMO_SPEECH_MODELS: &[&str] = &[
    "mimo-v2.5-tts-voiceclone",
    "mimo-v2.5-tts-voicedesign",
    "mimo-v2.5-tts",
    "mimo-v2-tts",
];

pub(crate) const SPEECH_MODEL_EXAMPLES: &[&str] = &[
    "mimo-v2.5-tts",
    "mimo-v2.5-tts-voicedesign",
    "mimo-v2.5-tts-voiceclone",
    "mimo-v2-tts",
];

pub struct SpeechTool {
    name: &'static str,
    client: Option<DeepSeekClient>,
    output_dir: Option<PathBuf>,
}

impl SpeechTool {
    #[must_use]
    pub fn new(
        name: &'static str,
        client: Option<DeepSeekClient>,
        output_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            name,
            client,
            output_dir,
        }
    }
}

#[async_trait]
impl ToolSpec for SpeechTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Generate speech/audio directly through the configured Xiaomi MiMo OpenAI-compatible API. Use this when the user asks for speech, TTS, narration, read-aloud, voice design, or voice cloning."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "要合成的文本。作为助手消息发送，是说的内容；MiMo TTS 风格/音频标签可以包含在此。"
                },
                "output": {
                    "type": "string",
                    "description": "要写入的音频文件路径，相对工作空间除非是绝对路径。默认：output_dir 中的 speech.<format>，配置的 [speech].output_dir 或工作空间。"
                },
                "output_dir": {
                    "type": "string",
                    "description": "当 output 省略时默认 speech.<format> 输出文件的目录。相对路径保持在工作空间内。"
                },
                "model": {
                    "type": "string",
                    "description": "TTS 模型。默认为 mimo-v2.5-tts，或根据 voice_prompt/clone_voice 推断 voice-design/voice-clone 模型。",
                    "enum": SPEECH_MODEL_EXAMPLES
                },
                "voice": {
                    "type": "string",
                    "description": "内置语音 ID（例如 mimo_default, 冰糖, 茉莉, 苏打, 白桦, Mia, Chloe, Milo, Dean）或语音克隆的 data:audio/...;base64,... URI。"
                },
                "instruction": {
                    "type": "string",
                    "description": "自然语言风格、情感、语速、场景或表现指令。它不是逐字说的内容。"
                },
                "voice_prompt": {
                    "type": "string",
                    "description": "语音设计提示。当 model 省略时，这使用 mimo-v2.5-tts-voicedesign。"
                },
                "clone_voice": {
                    "type": "string",
                    "description": "用于克隆的 .mp3 或 .wav 语音样本路径。当 model 省略时，这使用 mimo-v2.5-tts-voiceclone。"
                },
                "format": {
                    "type": "string",
                    "description": "请求的音频格式。默认：wav。MiMo-V2.5-TTS 文档示例使用 wav 和 pcm16；当 API 返回 mp3 时它也接受。",
                    "enum": SUPPORTED_SPEECH_FORMATS
                },
                "stream": {
                    "type": "boolean",
                    "description": "低延迟流式请求。直接工具当前只写入完整音频文件，因此保持为 false。"
                }
            },
            "required": ["text"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::WritesFiles,
            ToolCapability::Network,
            ToolCapability::Sandboxable,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        // 语音生成是显式的用户面向生成动作。
        // 路径解析仍然强制工作空间/可信根边界。
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let text = required_str(&input, "text")?.trim().to_string();
        if text.is_empty() {
            return Err(ToolError::invalid_input("语音文本不能为空"));
        }

        let client = self.client.clone().ok_or_else(|| {
            ToolError::not_available(
                "语音工具需要活跃的小米 MiMo API 客户端；先配置 provider = \"xiaomi-mimo\" 和 API 密钥",
            )
        })?;

        let requested_format_raw = optional_str(&input, "format")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_FORMAT);
        let requested_format = normalize_speech_format(requested_format_raw).ok_or_else(|| {
            ToolError::invalid_input(format!(
                "不支持的语音格式 '{requested_format_raw}'（允许：{}）",
                SUPPORTED_SPEECH_FORMATS.join(", ")
            ))
        })?;
        if optional_bool(&input, "stream", false) {
            return Err(ToolError::invalid_input(
                "stream=true 低延迟语音输出尚未在直接工具中实现；使用 stream=false 生成完整音频文件",
            ));
        }
        let output_raw = optional_str(&input, "output")
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let output_path = resolve_speech_output_path(
            &input,
            context,
            output_raw,
            &requested_format,
            self.output_dir.as_ref(),
        )?;
        let output_label = output_raw
            .map(str::to_string)
            .unwrap_or_else(|| output_path.display().to_string());

        let raw_voice = optional_str(&input, "voice")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let raw_instruction = optional_str(&input, "instruction")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let voice_prompt = optional_str(&input, "voice_prompt")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let clone_voice = optional_str(&input, "clone_voice")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        let voice_is_data_uri = raw_voice
            .as_deref()
            .is_some_and(|value| value.starts_with("data:audio/"));
        if clone_voice.is_some() && raw_voice.is_some() {
            return Err(ToolError::invalid_input(
                "同时使用 clone_voice 或 voice 用于克隆语音数据，不能同时使用",
            ));
        }
        let model = infer_speech_model(
            optional_str(&input, "model"),
            clone_voice.is_some() || voice_is_data_uri,
            voice_prompt.is_some(),
        );
        let model_lower = model.to_ascii_lowercase();
        if !model_lower.contains("tts") {
            return Err(ToolError::invalid_input(format!(
                "语音工具需要 TTS 模型（示例：{}），收到 '{model}'",
                SPEECH_MODEL_EXAMPLES.join(", ")
            )));
        }

        let is_voice_design = model_lower.contains("voicedesign");
        let is_voice_clone = model_lower.contains("voiceclone");
        let instruction = combine_speech_instructions(raw_instruction, voice_prompt);
        if is_voice_design
            && instruction
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err(ToolError::invalid_input(
                "mimo-v2.5-tts-voicedesign 需要 voice_prompt 或 instruction",
            ));
        }

        let voice = if let Some(clone_path) = clone_voice {
            let clone_path = context.resolve_path(&clone_path)?;
            Some(encode_voice_clone_data_uri(&clone_path).await?)
        } else if is_voice_design {
            None
        } else if let Some(value) = raw_voice {
            Some(value)
        } else if is_voice_clone {
            return Err(ToolError::invalid_input(
                "mimo-v2.5-tts-voiceclone 需要 clone_voice <mp3|wav> 或 voice <data-uri>",
            ));
        } else {
            Some(DEFAULT_VOICE.to_string())
        };

        check_network_policy(context, client.base_url())?;

        let response = client
            .synthesize_speech(SpeechSynthesisRequest {
                model: model.clone(),
                text,
                instruction,
                audio_format: requested_format,
                voice,
            })
            .await
            .map_err(|err| {
                ToolError::execution_failed(format!("语音合成失败: {err}"))
            })?;

        if let Some(parent) = output_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            tokio::fs::create_dir_all(parent).await.map_err(|err| {
                ToolError::execution_failed(format!(
                    "创建输出目录 {} 失败: {err}",
                    parent.display()
                ))
            })?;
        }
        tokio::fs::write(&output_path, &response.audio_bytes)
            .await
            .map_err(|err| {
                ToolError::execution_failed(format!(
                    "写入音频文件 {} 失败: {err}",
                    output_path.display()
                ))
            })?;

        let result = json!({
            "mode": "speech",
            "success": true,
            "api": "Xiaomi MiMo OpenAI-compatible chat/completions speech synthesis",
            "base_url": openai_compatible_base_url(client.base_url()),
            "model": response.model,
            "format": response.audio_format,
            "stream": false,
            "output": output_label,
            "absolute_output": output_path.display().to_string(),
            "bytes": response.audio_bytes.len(),
            "voice": response.voice.as_deref().map(describe_speech_voice),
            "transcript": response.transcript,
            "supported_formats": SUPPORTED_SPEECH_FORMATS,
            "supported_xiaomi_mimo_models": SUPPORTED_XIAOMI_MIMO_SPEECH_MODELS,
        });
        ToolResult::json(&result).map_err(|err| {
            ToolError::execution_failed(format!("序列化结果失败: {err}"))
        })
    }
}

pub(crate) fn infer_speech_model(
    model: Option<&str>,
    has_clone_voice: bool,
    has_voice_prompt: bool,
) -> String {
    match model.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => normalize_model_name_for_provider(ApiProvider::XiaomiMimo, value)
            .unwrap_or_else(|| value.into()),
        None if has_clone_voice => "mimo-v2.5-tts-voiceclone".to_string(),
        None if has_voice_prompt => "mimo-v2.5-tts-voicedesign".to_string(),
        None => "mimo-v2.5-tts".to_string(),
    }
}

pub(crate) fn combine_speech_instructions(
    instruction: Option<String>,
    voice_prompt: Option<String>,
) -> Option<String> {
    match (instruction, voice_prompt) {
        (Some(instruction), Some(voice_prompt)) => {
            let instruction = instruction.trim();
            let voice_prompt = voice_prompt.trim();
            if instruction.is_empty() {
                Some(voice_prompt.to_string()).filter(|value| !value.is_empty())
            } else if voice_prompt.is_empty() {
                Some(instruction.to_string()).filter(|value| !value.is_empty())
            } else {
                Some(format!("{voice_prompt}\n\n{instruction}"))
            }
        }
        (Some(value), None) | (None, Some(value)) => {
            let value = value.trim().to_string();
            if value.is_empty() { None } else { Some(value) }
        }
        (None, None) => None,
    }
}

pub(crate) fn normalize_speech_format(format: &str) -> Option<String> {
    let normalized = format.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "wav" | "mp3" | "pcm16" => Some(normalized),
        "pcm" => Some("pcm16".to_string()),
        _ => None,
    }
}

pub(crate) fn default_speech_output_name(format: &str) -> String {
    format!(
        "speech.{}",
        normalize_speech_format(format)
            .as_deref()
            .unwrap_or(DEFAULT_FORMAT)
    )
}

fn resolve_speech_output_path(
    input: &Value,
    context: &ToolContext,
    output_raw: Option<&str>,
    format: &str,
    configured_output_dir: Option<&PathBuf>,
) -> Result<PathBuf, ToolError> {
    if let Some(output) = output_raw {
        return context.resolve_path(output);
    }

    let filename = default_speech_output_name(format);
    if let Some(output_dir) = optional_str(input, "output_dir")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(context.resolve_path(output_dir)?.join(filename));
    }

    if let Some(output_dir) = configured_output_dir {
        return Ok(output_dir.join(filename));
    }

    Ok(context.workspace.join(filename))
}

async fn encode_voice_clone_data_uri(path: &Path) -> Result<String, ToolError> {
    let bytes = tokio::fs::read(path).await.map_err(|err| {
        ToolError::execution_failed(format!(
            "读取语音克隆样本 {} 失败: {err}",
            path.display()
        ))
    })?;

    voice_clone_data_uri_from_bytes(path, &bytes)
        .map_err(|err| ToolError::invalid_input(err.to_string()))
}

pub(crate) fn encode_voice_clone_sample_data_uri(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("读取语音克隆样本 {} 失败", path.display()))?;

    voice_clone_data_uri_from_bytes(path, &bytes)
}

fn voice_clone_data_uri_from_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<String> {
    let base64_audio = general_purpose::STANDARD.encode(bytes);
    if base64_audio.len() > VOICE_CLONE_BASE64_MAX_BYTES {
        anyhow::bail!(
            "语音克隆样本 base64 编码后过大（{} 字节 > 10 MB）",
            base64_audio.len()
        );
    }

    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        other => {
            anyhow::bail!("不支持的语音克隆样本扩展名 '{other}'。请使用 .mp3 或 .wav。");
        }
    };

    Ok(format!("data:{mime};base64,{base64_audio}"))
}

pub(crate) fn describe_speech_voice(voice: &str) -> String {
    if voice.starts_with("data:") {
        "嵌入的语音克隆样本".to_string()
    } else {
        voice.to_string()
    }
}

fn openai_compatible_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/v1") || trimmed.ends_with("/beta") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

fn check_network_policy(context: &ToolContext, base_url: &str) -> Result<(), ToolError> {
    let Some(decider) = context.network_policy.as_ref() else {
        return Ok(());
    };
    let display_url = openai_compatible_base_url(base_url);
    let Some(host) = host_from_url(&display_url) else {
        return Ok(());
    };
    match decider.evaluate(&host, "speech") {
        Decision::Allow => Ok(()),
        Decision::Deny => Err(ToolError::permission_denied(format!(
            "语音网络调用到 '{host}' 被网络策略阻止"
        ))),
        Decision::Prompt => Err(ToolError::permission_denied(format!(
            "语音网络调用到 '{host}' 需要批准；在 `/network allow {host}` 后重新运行或设置 network.default = \"allow\""
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_speech_model_from_requested_mode() {
        assert_eq!(infer_speech_model(None, false, false), "mimo-v2.5-tts");
        assert_eq!(
            infer_speech_model(None, false, true),
            "mimo-v2.5-tts-voicedesign"
        );
        assert_eq!(
            infer_speech_model(None, true, false),
            "mimo-v2.5-tts-voiceclone"
        );
        assert_eq!(
            infer_speech_model(Some("mimo-tts"), false, false),
            "mimo-v2.5-tts"
        );
        assert_eq!(
            infer_speech_model(Some("mimo-v2-tts"), false, false),
            "mimo-v2-tts"
        );
    }

    #[test]
    fn combines_voice_prompt_before_instruction() {
        assert_eq!(
            combine_speech_instructions(
                Some("Speak warmly.".to_string()),
                Some("Young Chinese female voice".to_string())
            )
            .as_deref(),
            Some("Young Chinese female voice\n\nSpeak warmly.")
        );
        assert_eq!(
            combine_speech_instructions(Some("  calm  ".to_string()), None).as_deref(),
            Some("calm")
        );
    }

    #[test]
    fn normalizes_documented_speech_formats() {
        assert_eq!(normalize_speech_format("WAV").as_deref(), Some("wav"));
        assert_eq!(normalize_speech_format("pcm16").as_deref(), Some("pcm16"));
        assert_eq!(normalize_speech_format("pcm").as_deref(), Some("pcm16"));
        assert_eq!(normalize_speech_format("flac"), None);
    }

    #[test]
    fn supported_xiaomi_mimo_speech_models_are_tts_only() {
        assert!(
            SUPPORTED_XIAOMI_MIMO_SPEECH_MODELS
                .iter()
                .all(|model| model.to_ascii_lowercase().contains("tts")),
            "模型可见的语音列表不得包含仅聊天的 MiMo 模型"
        );
        assert!(SUPPORTED_XIAOMI_MIMO_SPEECH_MODELS.contains(&"mimo-v2.5-tts"));
        assert!(!SUPPORTED_XIAOMI_MIMO_SPEECH_MODELS.contains(&"mimo-v2.5-pro"));
        assert!(!SUPPORTED_XIAOMI_MIMO_SPEECH_MODELS.contains(&"mimo-v2.5"));
    }

    #[test]
    fn configured_output_dir_is_used_for_default_tool_output() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let context = ToolContext::new(tmp.path().to_path_buf());
        let configured = tmp.path().join("speech-artifacts");

        let output = resolve_speech_output_path(
            &json!({"text": "hello"}),
            &context,
            None,
            "pcm",
            Some(&configured),
        )
        .expect("output path");

        assert_eq!(output, configured.join("speech.pcm16"));
    }

    #[test]
    fn displays_openai_compatible_base_url() {
        assert_eq!(
            openai_compatible_base_url("https://api.xiaomimimo.com"),
            "https://api.xiaomimimo.com/v1"
        );
        assert_eq!(
            openai_compatible_base_url("https://api.xiaomimimo.com/v1"),
            "https://api.xiaomimimo.com/v1"
        );
    }

    #[test]
    fn speech_tool_is_auto_approved_but_not_read_only() {
        let tool = SpeechTool::new("speech", None, None);
        assert_eq!(tool.name(), "speech");
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Auto);
        assert!(!tool.is_read_only());
        let schema = tool.input_schema();
        assert!(schema.to_string().contains("mimo-v2.5-tts-voiceclone"));
        assert!(schema.to_string().contains("pcm16"));
        assert!(schema.to_string().contains("stream"));
    }
}
