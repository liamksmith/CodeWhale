use std::collections::HashMap;
use std::path::PathBuf;

use super::manifest::LoadedPlugin;

#[derive(Debug, Clone, Default)]
pub struct PluginRegistry {
    plugins: HashMap<String, LoadedPlugin>,
    user_overrides: HashMap<String, bool>,
    /// `user_overrides` 的持久化路径。Discovery 总是通过
    /// [`set_overrides_store`](Self::set_overrides_store) 设置此路径；
    /// 当注册表在没有持久化存储的情况下构建（例如单元测试中直接 `PluginRegistry::new()`）时
    /// 该值为 `None`，此时启用/禁用仅保留在内存中。
    overrides_path: Option<PathBuf>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注入已持久化的启用/禁用覆盖，并记住写入回存的位置。
    /// 由 discovery 在插件注册前调用；
    /// 之后通过 [`apply_overrides`](Self::apply_overrides) 应用这些覆盖。
    pub fn set_overrides_store(&mut self, path: PathBuf, overrides: HashMap<String, bool>) {
        self.overrides_path = Some(path);
        self.user_overrides = overrides;
    }

    /// 将所有已持久化的覆盖应用到当前已注册的插件上。
    /// Discovery 在每次启动时会从零重新计算 `enabled`（`!builtin`），
    /// 因此此方法使之前的 `/plugin enable|disable` 实际生效。
    pub fn apply_overrides(&mut self) {
        for (name, &enabled) in &self.user_overrides {
            if let Some(plugin) = self.plugins.get_mut(name) {
                plugin.enabled = enabled;
            }
        }
    }

    pub fn register(&mut self, name: String, plugin: LoadedPlugin) {
        self.plugins.insert(name, plugin);
    }

    pub fn enable(&mut self, name: &str) -> bool {
        if let Some(plugin) = self.plugins.get_mut(name) {
            plugin.enabled = true;
            self.user_overrides.insert(name.to_string(), true);
            self.persist_overrides();
            true
        } else {
            false
        }
    }

    pub fn disable(&mut self, name: &str) -> bool {
        if let Some(plugin) = self.plugins.get_mut(name) {
            plugin.enabled = false;
            self.user_overrides.insert(name.to_string(), false);
            self.persist_overrides();
            true
        } else {
            false
        }
    }

    /// 将当前覆盖映射写入磁盘（尽力而为）。失败时仅记录日志，
    /// 不会导致命令失败——内存中的开关在当前会话中仍然生效。
    fn persist_overrides(&self) {
        if let Some(path) = &self.overrides_path
            && let Err(e) = super::discovery::save_overrides(path, &self.user_overrides)
        {
            tracing::warn!(
                "failed to persist plugin overrides to {}: {e}",
                path.display()
            );
        }
    }

    pub fn list(&self) -> Vec<(&String, &LoadedPlugin)> {
        self.plugins.iter().collect()
    }

    pub fn get(&self, name: &str) -> Option<&LoadedPlugin> {
        self.plugins.get(name)
    }

    pub fn enabled_plugins(&self) -> Vec<(&String, &LoadedPlugin)> {
        self.list_enabled()
    }

    pub fn list_enabled(&self) -> Vec<(&String, &LoadedPlugin)> {
        self.plugins.iter().filter(|(_, p)| p.enabled).collect()
    }

    pub fn is_enabled(&self, name: &str) -> bool {
        self.plugins.get(name).is_some_and(|p| p.enabled)
    }

    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}
