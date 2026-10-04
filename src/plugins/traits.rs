use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginCapability {
    Hooks,
    Tools,
    Providers,
    /// Permission to modify tool results via the `tool_result_persist` hook.
    ModifyToolResults,
}
