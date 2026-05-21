use async_trait::async_trait;

use crate::hooks::traits::HookHandler;

/// Built-in hook for startup prompt boot-script mutation.
///
/// Current implementation is a pass-through placeholder to keep behavior stable.
pub struct BootScriptHook;

#[async_trait]
impl HookHandler for BootScriptHook {
    fn name(&self) -> &str {
        "boot-script"
    }

    fn priority(&self) -> i32 {
        10
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_script_hook_has_priority() {
        let hook = BootScriptHook;
        assert_eq!(hook.priority(), 10);
    }
}
