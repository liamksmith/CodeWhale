//! `/memory` 斜杠命令 — 检查并编辑用户记忆文件。
//!
//! 当用户记忆功能启用时（配置中的 `[memory] enabled = true` 或环境变量
//! `DEEPSEEK_MEMORY=on`），`/memory` 显示当前记忆文件路径和内容。
//! 子命令让用户清空或打开文件：
//!
//! - `/memory` — 显示路径 + 内容
//! - `/memory show` — 无参数形式的别名
//! - `/memory clear` — 将文件内容替换为空标记
//! - `/memory path` — 仅显示已解析的路径
//! - `/memory help` — 显示命令特定帮助和已解析的路径
//!
//! 编辑器集成（`/memory edit`）有意保持最小化：该命令打印一行可复制粘贴的
//! shell 命令来在用户的 `$VISUAL` / `$EDITOR` 中打开文件，
//! 因为进程内外部编辑器管道需要终端拆卸，而斜杠命令处理程序没有访问权限。

use std::fs;
use std::path::Path;

use crate::commands::CommandResult;
use crate::tui::app::App;

const MEMORY_USAGE: &str = "/memory [show|path|clear|edit|help]";

fn memory_help(path: &Path) -> String {
    format!(
        "Inspect or manage your persistent user-memory file.\n\n\
         Usage: {MEMORY_USAGE}\n\n\
         Current path: {}\n\n\
         Subcommands:\n\
           /memory          Show the resolved path and current contents\n\
           /memory show     Alias for the no-arg form\n\
           /memory path     Print just the resolved path\n\
           /memory clear    Replace the file contents with an empty marker\n\
           /memory edit     Print the editor command for this file\n\
           /memory help     Show this help\n\n\
         Quick capture: type `# foo` in the composer to append a timestamped\n\
         bullet without firing a turn.",
        path.display()
    )
}

fn memory(app: &mut App, arg: Option<&str>) -> CommandResult {
    if !app.use_memory {
        return CommandResult::error(
            "user memory is disabled. Enable with `[memory] enabled = true` in `~/.codewhale/config.toml` or `DEEPSEEK_MEMORY=on` in your environment, then restart the TUI.",
        );
    }

    let path = app.memory_path.clone();
    let sub = arg.unwrap_or("show").trim();

    match sub {
        "" | "show" => {
            let body = match fs::read_to_string(&path) {
                Ok(text) if text.trim().is_empty() => format!(
                    "{}\n(empty — add via `# foo` from the composer or have the model use the `remember` tool)",
                    path.display()
                ),
                Ok(text) => format!("{}\n\n{}", path.display(), text.trim_end()),
                Err(_) => format!(
                    "{}\n(file does not exist yet — add via `# foo` from the composer to create it)",
                    path.display()
                ),
            };
            CommandResult::message(body)
        }
        "path" => CommandResult::message(path.display().to_string()),
        "clear" => match fs::write(&path, "") {
            Ok(()) => CommandResult::message(format!("memory cleared: {}", path.display())),
            Err(err) => CommandResult::error(format!("failed to clear {}: {err}", path.display())),
        },
        "edit" => CommandResult::message(format!(
            "to edit your memory file, run:\n\n  ${{VISUAL:-${{EDITOR:-vi}}}} {}",
            path.display()
        )),
        "help" => CommandResult::message(memory_help(&path)),
        _ => CommandResult::error(format!(
            "unknown subcommand `{sub}`. Try `/memory help`.\n\n{}",
            memory_help(&path)
        )),
    }
}

pub(in crate::commands) const COMMAND_INFO: crate::commands::traits::CommandInfo =
    crate::commands::traits::CommandInfo {
        name: "memory",
        aliases: &[],
        usage: "/memory [show|path|clear|edit|help]",
        description_id: crate::localization::MessageId::CmdMemoryDescription,
    };

pub(in crate::commands) struct MemoryCmd;

impl crate::commands::traits::RegisterCommand for MemoryCmd {
    fn info() -> &'static crate::commands::traits::CommandInfo {
        &COMMAND_INFO
    }

    fn execute(
        app: &mut crate::tui::app::App,
        arg: Option<&str>,
    ) -> crate::commands::CommandResult {
        memory(app, arg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use tempfile::TempDir;

    fn create_test_app_with_memory(tmpdir: &TempDir, use_memory: bool) -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: tmpdir.path().to_path_buf(),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: tmpdir.path().join("skills"),
            memory_path: tmpdir.path().join("memory.md"),
            notes_path: tmpdir.path().join("notes.txt"),
            mcp_config_path: tmpdir.path().join("mcp.json"),
            use_memory,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &Config::default())
    }

    #[test]
    fn memory_help_lists_subcommands_and_resolved_path() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = create_test_app_with_memory(&tmpdir, true);
        let result = memory(&mut app, Some("help"));
        let msg = result.message.expect("help should return text");
        assert!(msg.contains("Usage: /memory [show|path|clear|edit|help]"));
        assert!(msg.contains("/memory edit"));
        assert!(msg.contains(app.memory_path.to_string_lossy().as_ref()));
    }

    #[test]
    fn memory_unknown_subcommand_points_to_help() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = create_test_app_with_memory(&tmpdir, true);
        let result = memory(&mut app, Some("wat"));
        let msg = result
            .message
            .expect("unknown subcommand should return text");
        assert!(msg.contains("Try `/memory help`"));
        assert!(msg.contains("/memory clear"));
    }

    #[test]
    fn memory_disabled_returns_enablement_hint() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = create_test_app_with_memory(&tmpdir, false);
        let result = memory(&mut app, None);
        let msg = result.message.expect("disabled memory should return text");
        assert!(msg.contains("user memory is disabled"));
        assert!(msg.contains("DEEPSEEK_MEMORY=on"));
    }
}
