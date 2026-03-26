use parking_lot::{Mutex, const_mutex};

// Serialize tests that mutate process-global plugin runtime state.
pub(crate) static PLUGIN_RUNTIME_LOCK: Mutex<()> = const_mutex(());
