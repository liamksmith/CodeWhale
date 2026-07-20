//! 余额：查询当前提供商的账户余额或信用状态。
//!
//! 提供商特定的网络分发仍在待办中。在功能落地之前，明确此命令为占位脚手架，
//! 以免用户误认为它是实时余额查询。

use crate::config::ApiProvider;
use crate::tui::app::App;

use super::CommandResult;

/// 查询提供商账户余额/信用额度。
pub fn balance(app: &mut App) -> CommandResult {
    let provider = app.api_provider;
    match provider {
        ApiProvider::Deepseek
        | ApiProvider::DeepseekCN
        | ApiProvider::Openrouter
        | ApiProvider::Novita => CommandResult::message(format!(
            "Balance check for {} is planned, but provider balance network dispatch is not wired in this build yet.",
            provider.display_name()
        )),
        _ => CommandResult::message(format!(
            "Balance check is not supported for {} yet. Check the provider dashboard for account balance details.",
            provider.display_name()
        )),
    }
}
