#[allow(clippy::module_inception)]
pub mod agent;
pub mod classifier;
pub mod context_compressor;
pub mod dispatcher;
pub mod history_pruner;
pub mod loop_;
pub mod memory_loader;
pub mod prompt;
pub mod research;
pub mod session;
pub mod tools_registry;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub use agent::{Agent, AgentBuilder};
#[allow(unused_imports)]
pub use context_compressor::{
    CompressionResult, ContextCompressionConfig, ContextCompressor, estimate_tokens,
    parse_context_limit_from_error,
};
#[allow(unused_imports)]
pub use history_pruner::{
    PruneStats, PrunedOrphans, prune_history, remove_orphaned_tool_messages,
};
#[allow(unused_imports)]
pub use loop_::{process_message, process_message_with_session, run, run_tool_call_loop};
