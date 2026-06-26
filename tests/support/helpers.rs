//! Shared builder helpers for constructing test agents.

use agentzero::agent::agent::Agent;
use agentzero::agent::dispatcher::{NativeToolDispatcher, XmlToolDispatcher};
use agentzero::agent::memory_loader::MemoryLoader;
use agentzero::config::MemoryConfig;
use agentzero::memory;
use agentzero::memory::Memory;
use agentzero::observability::{NoopObserver, Observer};
use agentzero::providers::{ChatResponse, Provider, ToolCall};
use agentzero::tools::Tool;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Create an in-memory "none" backend for tests.
pub fn make_memory() -> Arc<dyn Memory> {
    let cfg = MemoryConfig {
        backend: "none".into(),
        ..MemoryConfig::default()
    };
    Arc::from(memory::create_memory(&cfg, &std::env::temp_dir(), None).unwrap())
}

/// Create a `NoopObserver` for tests.
pub fn make_observer() -> Arc<dyn Observer> {
    Arc::from(NoopObserver {})
}

/// Create a text-only `ChatResponse`.
pub fn text_response(text: &str) -> ChatResponse {
    ChatResponse {
        text: Some(text.into()),
        tool_calls: vec![],
        usage: None,
        reasoning_content: None,
        quota_metadata: None,
        stop_reason: None,
        raw_stop_reason: None,
    }
}

/// Create a `ChatResponse` with tool calls.
pub fn tool_response(calls: Vec<ToolCall>) -> ChatResponse {
    ChatResponse {
        text: Some(String::new()),
        tool_calls: calls,
        usage: None,
        reasoning_content: None,
        quota_metadata: None,
        stop_reason: None,
        raw_stop_reason: None,
    }
}

/// Build an agent with `NativeToolDispatcher`.
pub fn build_agent(provider: Box<dyn Provider>, tools: Vec<Box<dyn Tool>>) -> Agent {
    Agent::builder()
        .provider(provider)
        .tools(tools)
        .memory(make_memory())
        .observer(make_observer())
        .tool_dispatcher(Box::new(NativeToolDispatcher))
        .workspace_dir(std::env::temp_dir())
        .build()
        .unwrap()
}

/// Build an agent with `XmlToolDispatcher`.
pub fn build_agent_xml(provider: Box<dyn Provider>, tools: Vec<Box<dyn Tool>>) -> Agent {
    Agent::builder()
        .provider(provider)
        .tools(tools)
        .memory(make_memory())
        .observer(make_observer())
        .tool_dispatcher(Box::new(XmlToolDispatcher))
        .workspace_dir(std::env::temp_dir())
        .build()
        .unwrap()
}

/// Build an agent with optional custom `MemoryLoader`.
pub fn build_recording_agent(
    provider: Box<dyn Provider>,
    tools: Vec<Box<dyn Tool>>,
    memory_loader: Option<Box<dyn MemoryLoader>>,
) -> Agent {
    let mut builder = Agent::builder()
        .provider(provider)
        .tools(tools)
        .memory(make_memory())
        .observer(make_observer())
        .tool_dispatcher(Box::new(NativeToolDispatcher))
        .workspace_dir(std::env::temp_dir());

    if let Some(loader) = memory_loader {
        builder = builder.memory_loader(loader);
    }

    builder.build().unwrap()
}

/// Build an agent with real `SqliteMemory` in a temporary directory.
pub fn build_agent_with_sqlite_memory(
    provider: Box<dyn Provider>,
    tools: Vec<Box<dyn Tool>>,
    temp_dir: &std::path::Path,
) -> Agent {
    let cfg = MemoryConfig {
        backend: "sqlite".into(),
        ..MemoryConfig::default()
    };
    let mem = Arc::from(memory::create_memory(&cfg, temp_dir, None).unwrap());
    Agent::builder()
        .provider(provider)
        .tools(tools)
        .memory(mem)
        .observer(make_observer())
        .tool_dispatcher(Box::new(NativeToolDispatcher))
        .workspace_dir(std::env::temp_dir())
        .build()
        .unwrap()
}

/// Mock memory loader that returns a static context string.
pub struct StaticMemoryLoader {
    context: String,
}

impl StaticMemoryLoader {
    pub fn new(context: &str) -> Self {
        Self {
            context: context.to_string(),
        }
    }
}

#[async_trait]
impl MemoryLoader for StaticMemoryLoader {
    async fn load_context(&self, _memory: &dyn Memory, _user_message: &str) -> Result<String> {
        Ok(self.context.clone())
    }
}
