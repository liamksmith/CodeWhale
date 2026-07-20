//! 语音输入命令 — `/voice`、`/voice-send`、`/voice-control`。
//!
//! 从默认麦克风录制音频，发送到配置的提供商 API 进行转录，
//! 并将转录文本插入编辑器。交互模型镜像 MiMo Code 的语音 UX：
//!
//!   `/voice`         — 切换语音输入开/关（打开时录制）
//!   `/voice-send`    — 切换转录以 "send it" / "发送" 结尾时的自动发送
//!   `/voice-control` — 切换 AI 辅助听写，可看到当前编辑器文本
//!
//! 斜杠命令仅翻转状态并发出 [`AppAction::VoiceCapture`]；
//! 实际捕获在 UI 事件循环中运行，其中实时的 [`Config`]
//! 提供提供商凭据。这使处理程序保持无副作用
//!（注册表冒烟测试执行每个命令）并避免在 [`App`] 上缓存认证材料。
//!
//! ## 录制
//!
//! 使用平台特定的命令行工具（sox、rec、arecord）捕获
//! 16kHz 单声道 16 位 PCM 音频。录制直到检测到静音间隙或
//! 达到最大持续时间（默认 10 秒）。

use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

use crate::commands::CommandResult;
use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::config::Config;
use crate::localization::{MessageId, tr};
use crate::tui::app::{App, AppAction};

/// 从提供商聊天补全 API 请求的转录模型。
const ASR_MODEL: &str = "mimo-v2.5-asr";
/// 用于 AI 辅助语音控制管道的模型。
const VOICE_CONTROL_MODEL: &str = "mimo-v2.5";

pub(in crate::commands) const VOICE_INFO: CommandInfo = CommandInfo {
    name: "voice",
    aliases: &["yuyin", "语音"],
    usage: "/voice",
    description_id: MessageId::CmdVoiceDescription,
};

pub(in crate::commands) const VOICE_SEND_INFO: CommandInfo = CommandInfo {
    name: "voicesend",
    aliases: &["voice-send", "yuyinsend", "语音发送"],
    usage: "/voicesend",
    description_id: MessageId::CmdVoiceSendDescription,
};

pub(in crate::commands) const VOICE_CONTROL_INFO: CommandInfo = CommandInfo {
    name: "voicecontrol",
    aliases: &["voice-control", "yuyincontrol", "语音控制"],
    usage: "/voicecontrol",
    description_id: MessageId::CmdVoiceControlDescription,
};

pub(in crate::commands) struct VoiceCmd;
pub(in crate::commands) struct VoiceSendCmd;
pub(in crate::commands) struct VoiceControlCmd;

impl RegisterCommand for VoiceCmd {
    fn info() -> &'static CommandInfo {
        &VOICE_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        voice(app)
    }
}

impl RegisterCommand for VoiceSendCmd {
    fn info() -> &'static CommandInfo {
        &VOICE_SEND_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        voice_send(app)
    }
}

impl RegisterCommand for VoiceControlCmd {
    fn info() -> &'static CommandInfo {
        &VOICE_CONTROL_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        voice_control(app)
    }
}

// --- 录制器检测 ----------------------------------------------------

/// 平台特定的录制器定义。
#[derive(Debug, Clone)]
struct Recorder {
    cmd: &'static str,
    /// 将原始 16kHz 单声道 S16_LE PCM 通过管道输出到 stdout 的 CLI 参数。
    pipe_args: &'static [&'static str],
}

fn detect_recorder() -> Option<Recorder> {
    let candidates: &[Recorder] = if cfg!(target_os = "macos") {
        &[
            Recorder {
                cmd: "sox",
                pipe_args: &["-d", "-r", "16000", "-c", "1", "-b", "16", "-t", "raw", "-"],
            },
            Recorder {
                cmd: "rec",
                pipe_args: &["-r", "16000", "-c", "1", "-b", "16", "-t", "raw", "-"],
            },
        ]
    } else if cfg!(target_os = "linux") {
        &[
            Recorder {
                cmd: "arecord",
                pipe_args: &["-f", "S16_LE", "-r", "16000", "-c", "1", "-t", "raw"],
            },
            Recorder {
                cmd: "sox",
                pipe_args: &["-d", "-r", "16000", "-c", "1", "-b", "16", "-t", "raw", "-"],
            },
        ]
    } else if cfg!(target_os = "windows") {
        &[Recorder {
            cmd: "sox",
            pipe_args: &["-d", "-r", "16000", "-c", "1", "-b", "16", "-t", "raw", "-"],
        }]
    } else {
        &[]
    };

    candidates
        .iter()
        .find(|r| {
            Command::new(r.cmd)
                .arg("--version")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok()
        })
        .cloned()
}

/// 检查此系统上语音录制是否可用。
pub fn is_available() -> bool {
    detect_recorder().is_some()
}

// --- WAV 编码 ----------------------------------------------------------

/// 将原始 16kHz 单声道 S16_LE PCM 样本编码为 WAV 缓冲区。
fn encode_wav(samples: &[i16]) -> Vec<u8> {
    let data_size = (samples.len() * 2) as u32;
    let sample_rate: u32 = 16000;
    let mut buf = Vec::with_capacity(44 + data_size as usize);

    // RIFF 头
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_size).to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    // fmt 块
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // 块大小
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // 单声道
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // 字节率
    buf.extend_from_slice(&2u16.to_le_bytes()); // 块对齐
    buf.extend_from_slice(&16u16.to_le_bytes()); // 每样本位数

    // data 块
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());
    for &sample in samples {
        buf.extend_from_slice(&sample.to_le_bytes());
    }

    buf
}

// --- 录制 -------------------------------------------------------------

/// 自动停止前的最大录制时长（秒）。
const MAX_RECORD_SECS: u64 = 10;
/// 视为有效语音的最小时长（秒）。
const MIN_SEGMENT_SECS: f64 = 0.3;

/// 从默认麦克风录制音频。
///
/// 返回原始 16kHz 单声道 S16_LE PCM 样本。如果没有可用录制器、
/// 录制失败或未检测到语音，则返回 `None`。
fn record_audio() -> Option<(Vec<i16>, Duration)> {
    let recorder = detect_recorder()?;
    let start = std::time::Instant::now();

    let mut child = Command::new(recorder.cmd)
        .args(recorder.pipe_args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let stdout = child.stdout.take()?;
    let mut reader = std::io::BufReader::new(stdout);
    let mut all_samples: Vec<i16> = Vec::with_capacity(16000 * MAX_RECORD_SECS as usize);

    // 读取直到超时或静音
    let mut buf = [0u8; 320]; // 16kHz S16_LE 的 10ms
    let max_duration = Duration::from_secs(MAX_RECORD_SECS);
    let mut silence_samples = 0u32;
    let mut had_speech = false;
    let speech_threshold: i16 = 500; // 基于 RMS 的语音检测阈值
    let silence_duration_samples = 16000u32; // 停止前 1 秒静音

    loop {
        use std::io::Read;
        match reader.read_exact(&mut buf) {
            Ok(()) => {
                let chunk: Vec<i16> = buf
                    .chunks_exact(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]]))
                    .collect();

                // 简单的基于 RMS 的 VAD
                let rms = (chunk.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>()
                    / chunk.len() as f64)
                    .sqrt();
                let is_speech = rms > speech_threshold as f64;

                if is_speech {
                    had_speech = true;
                    silence_samples = 0;
                } else if had_speech {
                    silence_samples += chunk.len() as u32;
                }

                if had_speech {
                    all_samples.extend_from_slice(&chunk);
                }

                if start.elapsed() > max_duration {
                    let _ = child.kill();
                    break;
                }
                if had_speech && silence_samples >= silence_duration_samples {
                    let _ = child.kill();
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(_) => {
                let _ = child.kill();
                break;
            }
        }
    }

    let _ = child.wait();
    let elapsed = start.elapsed();

    let min_samples = (MIN_SEGMENT_SECS * 16000.0) as usize;
    if all_samples.len() < min_samples {
        return None;
    }

    Some((all_samples, elapsed))
}

// --- 自动发送后缀 ------------------------------------------------------

/// 匹配转录文本末尾的显式发送指令：
/// "send it"（任意空格/大小写）或 发送/發送，带尾随标点。
static SEND_SUFFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[\s,，.。!！?？]+)(?:send\s*it|发送|發送)[\s.。!！?？]*$").unwrap()
});

/// 将转录拆分为消息剩余部分和是否以显式发送指令结尾。
/// `"ship the fix, send it"` → `("ship the fix", true)`。
fn split_send_suffix(text: &str) -> (&str, bool) {
    match SEND_SUFFIX_RE.find(text) {
        Some(found) => (text[..found.start()].trim(), true),
        None => (text.trim(), false),
    }
}

// --- 转录 -------------------------------------------------------------

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn chat_completions_url(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
}

async fn post_chat_completions(
    api_key: &str,
    base_url: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let client = crate::tls::reqwest_client();
    let resp = client
        .post(chat_completions_url(base_url))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {api_key}"))
        .timeout(Duration::from_secs(30))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("API 返回状态 {}", resp.status()));
    }

    resp.json()
        .await
        .map_err(|e| format!("解析响应失败: {e}"))
}

/// 将音频发送到提供商 API 进行纯转录。
///
/// 使用带有 `input_audio` 内容块的聊天补全端点。
async fn transcribe(
    api_key: &str,
    base_url: &str,
    audio_samples: &[i16],
) -> Result<String, String> {
    let wav = encode_wav(audio_samples);
    let data_url = format!("data:audio/wav;base64,{}", base64_encode(&wav));

    let body = serde_json::json!({
        "model": ASR_MODEL,
        "messages": [
            {
                "role": "user",
                "content": [
                    {
                        "type": "input_audio",
                        "input_audio": {
                            "data": data_url
                        }
                    }
                ]
            }
        ],
        "asr_options": {
            "language": "auto"
        }
    });

    let data = post_chat_completions(api_key, base_url, body).await?;
    data["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| "响应中无转录内容".to_string())
}

/// 通过语音控制管道处理音频：AI 辅助听写，
/// 可看到当前编辑器文本，镜像 MiMo Code 的
/// `processVoiceControl`。当 `/voice-control` 启用时使用。
async fn process_voice_control(
    api_key: &str,
    base_url: &str,
    audio_samples: &[i16],
    current_text: &str,
) -> Result<String, String> {
    let wav = encode_wav(audio_samples);
    let data_url = format!("data:audio/wav;base64,{}", base64_encode(&wav));

    let user_context = serde_json::json!({
        "current_text": current_text,
        "cursor": "end",
    });

    let body = serde_json::json!({
        "model": VOICE_CONTROL_MODEL,
        "messages": [
            {
                "role": "system",
                "content": "You are a voice input assistant. Transcribe the user's speech. Output JSON: {\"text\": \"transcribed text\"}."
            },
            {
                "role": "user",
                "content": [
                    { "type": "text", "text": user_context.to_string() },
                    { "type": "input_audio", "input_audio": { "data": data_url } }
                ]
            }
        ],
        "response_format": { "type": "json_object" }
    });

    let data = post_chat_completions(api_key, base_url, body).await?;
    let content = data["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| "无响应内容".to_string())?;

    let parsed: serde_json::Value = serde_json::from_str(content)
        .map_err(|e| format!("解析语音控制 JSON 失败: {e}"))?;

    parsed["text"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "语音控制响应中无 text 字段".to_string())
}

// --- 捕获编排（UI 事件循环）--------------------------------

/// UI 应对完成捕获后的操作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceCaptureOutcome {
    /// 将转录文本插入编辑器光标的当前位置。
    Insert(String),
    /// 将此文本作为消息提交（自动发送）。
    Send(String),
}

/// 执行完整的录制 + 转录周期。
///
/// 在 UI 事件循环中运行（参见 [`AppAction::VoiceCapture`]），因此提供商
/// 凭据来自实时的 [`Config`] 而不是 [`App`] 上缓存的状态。
/// 录制在阻塞线程上发生；转录使用共享异步 HTTP 客户端。
/// 每个失败路径返回本地化消息，以便调用者可以将其显示为状态行。
pub async fn capture_and_transcribe(
    app: &mut App,
    config: &Config,
) -> Result<VoiceCaptureOutcome, String> {
    let locale = app.ui_locale;

    if !is_available() {
        return Err(tr(locale, MessageId::VoiceErrNoRecorder).to_string());
    }
    let api_key = config
        .deepseek_api_key()
        .map_err(|_| tr(locale, MessageId::VoiceErrNoAuth).to_string())?;
    let base_url = config.deepseek_base_url();

    app.status_message = Some(tr(locale, MessageId::VoiceRecording).to_string());
    let (samples, _duration) = tokio::task::spawn_blocking(record_audio)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| tr(locale, MessageId::VoiceErrTooShort).to_string())?;

    app.status_message = Some(tr(locale, MessageId::VoiceProcessing).to_string());
    let text = if app.voice_control_enabled {
        process_voice_control(&api_key, &base_url, &samples, &app.composer.input).await
    } else {
        transcribe(&api_key, &base_url, &samples).await
    }
    .map_err(|e| format!("{}: {e}", tr(locale, MessageId::VoiceErrNetwork)))?;

    let clean = text.trim();
    if app.voice_send_enabled {
        let (remainder, wants_send) = split_send_suffix(clean);
        if wants_send {
            // 裸的 "send it" 提交编辑器中已有的任何内容。
            let outgoing = if remainder.is_empty() {
                let existing = app.composer.input.trim().to_string();
                if !existing.is_empty() {
                    app.clear_input();
                }
                existing
            } else {
                remainder.to_string()
            };
            if outgoing.is_empty() {
                return Err(tr(locale, MessageId::VoiceErrEmptySend).to_string());
            }
            return Ok(VoiceCaptureOutcome::Send(outgoing));
        }
    }
    if clean.is_empty() {
        return Err(tr(locale, MessageId::VoiceErrEmptySend).to_string());
    }
    Ok(VoiceCaptureOutcome::Insert(clean.to_string()))
}

// --- 命令处理程序 ------------------------------------------------------

/// 处理 `/voice` 命令：切换语音输入。打开时通过
/// [`AppAction::VoiceCapture`] 请求一次性录制 + 转录。
pub fn voice(app: &mut App) -> CommandResult {
    let locale = app.ui_locale;

    if app.voice_enabled {
        app.voice_enabled = false;
        return CommandResult::message(tr(locale, MessageId::VoiceDisabled));
    }
    if !is_available() {
        return CommandResult::error(tr(locale, MessageId::VoiceErrNoRecorder));
    }
    app.voice_enabled = true;
    CommandResult::with_message_and_action(
        tr(locale, MessageId::VoiceEnabled),
        AppAction::VoiceCapture,
    )
}

/// 处理 `/voice-send` 命令：切换转录后自动发送。
pub fn voice_send(app: &mut App) -> CommandResult {
    let locale = app.ui_locale;
    app.voice_send_enabled = !app.voice_send_enabled;

    let msg = if app.voice_send_enabled {
        tr(locale, MessageId::VoiceSendEnabled)
    } else {
        tr(locale, MessageId::VoiceSendDisabled)
    };
    CommandResult::message(msg)
}

/// 处理 `/voice-control` 命令：切换 AI 辅助听写。
pub fn voice_control(app: &mut App) -> CommandResult {
    let locale = app.ui_locale;
    app.voice_control_enabled = !app.voice_control_enabled;

    let msg = if app.voice_control_enabled {
        tr(locale, MessageId::VoiceControlEnabled)
    } else {
        tr(locale, MessageId::VoiceControlDisabled)
    };
    CommandResult::message(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_encoding_produces_valid_header() {
        let samples = vec![0i16; 16000]; // 1 秒静音
        let wav = encode_wav(&samples);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        // 数据大小 = 16000 * 2 = 32000
        assert_eq!(&wav[4..8], &(36 + 32000u32).to_le_bytes());
    }

    #[test]
    fn wav_encoding_empty_is_minimal() {
        let wav = encode_wav(&[]);
        assert_eq!(wav.len(), 44);
        assert_eq!(&wav[4..8], &36u32.to_le_bytes());
    }

    #[test]
    fn send_suffix_detected_and_stripped() {
        assert_eq!(split_send_suffix("send it"), ("", true));
        assert_eq!(split_send_suffix("Send It!"), ("", true));
        assert_eq!(split_send_suffix("发送"), ("", true));
        assert_eq!(split_send_suffix("發送。"), ("", true));
        assert_eq!(
            split_send_suffix("ship the fix, send it"),
            ("ship the fix", true)
        );
        assert_eq!(
            split_send_suffix("修复这个问题，发送"),
            ("修复这个问题", true)
        );
    }

    #[test]
    fn send_suffix_leaves_plain_text_alone() {
        assert_eq!(split_send_suffix("send it now"), ("send it now", false));
        assert_eq!(
            split_send_suffix("帮我发送一封邮件"),
            ("帮我发送一封邮件", false)
        );
        assert_eq!(split_send_suffix("发送邮件"), ("发送邮件", false));
        assert_eq!(
            split_send_suffix("resend it to the queue"),
            ("resend it to the queue", false)
        );
    }

    #[test]
    fn recorder_detection_does_not_crash() {
        // 仅验证函数运行而不 panic
        let _ = is_available();
    }
}
