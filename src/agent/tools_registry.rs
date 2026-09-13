use crate::config::Config;
use crate::memory::Memory;
use crate::runtime::RuntimeAdapter;
use crate::security::SecurityPolicy;
use crate::tools::{self, McpRegistry, McpToolWrapper, Tool};
use anyhow::Result;
use std::sync::Arc;

/// Options controlling which extensions are layered onto the base tool registry.
#[derive(Debug, Clone, Copy)]
pub struct ToolsRegistryOptions {
    pub include_peripherals: bool,
    pub include_mcp: bool,
    pub apply_agent_tool_filters: bool,
}

impl ToolsRegistryOptions {
    pub const AGENT_LOOP: Self = Self {
        include_peripherals: true,
        include_mcp: true,
        apply_agent_tool_filters: true,
    };

    pub const CHANNEL: Self = Self {
        include_peripherals: false,
        include_mcp: true,
        apply_agent_tool_filters: false,
    };

    /// Gateway/webhook agent loop: MCP tools, no hardware peripherals.
    pub const GATEWAY: Self = Self {
        include_peripherals: false,
        include_mcp: true,
        apply_agent_tool_filters: false,
    };

    pub const MINIMAL: Self = Self {
        include_peripherals: false,
        include_mcp: false,
        apply_agent_tool_filters: false,
    };
}

/// Filter the primary-agent registry and fail when allow/deny settings remove all tools.
pub fn filter_primary_agent_tools_or_fail(
    config: &Config,
    tools_registry: Vec<Box<dyn Tool>>,
) -> Result<Vec<Box<dyn Tool>>> {
    let (filtered_tools, report) = tools::filter_primary_agent_tools(
        tools_registry,
        &config.agent.allowed_tools,
        &config.agent.denied_tools,
    );

    for unmatched in report.unmatched_allowed_tools {
        tracing::debug!(
            tool = %unmatched,
            "agent.allowed_tools entry did not match any registered tool"
        );
    }

    let has_agent_allowlist = config
        .agent
        .allowed_tools
        .iter()
        .any(|entry| !entry.trim().is_empty());
    let has_agent_denylist = config
        .agent
        .denied_tools
        .iter()
        .any(|entry| !entry.trim().is_empty());
    if has_agent_allowlist
        && has_agent_denylist
        && report.allowlist_match_count > 0
        && filtered_tools.is_empty()
    {
        anyhow::bail!(
            "agent.allowed_tools and agent.denied_tools removed all executable tools; update [agent] tool filters"
        );
    }

    Ok(filtered_tools)
}

/// Connect the configured MCP servers once for reuse across tool registries.
pub(crate) async fn connect_mcp_registry(config: &Config) -> Option<Arc<McpRegistry>> {
    if !config.mcp.enabled || config.mcp.servers.is_empty() {
        return None;
    }

    tracing::info!(
        "Initializing MCP client — {} server(s) configured",
        config.mcp.servers.len()
    );
    match McpRegistry::connect_all(&config.mcp.servers).await {
        Ok(registry) => {
            let registry = Arc::new(registry);
            tracing::info!(
                "MCP registry ready — {} tool(s) from {} server(s)",
                registry.tool_count(),
                registry.server_count()
            );
            Some(registry)
        }
        Err(e) => {
            tracing::error!("MCP registry failed to initialize: {e:#}");
            None
        }
    }
}

async fn append_mcp_tools(registry: Arc<McpRegistry>, tools_registry: &mut Vec<Box<dyn Tool>>) {
    for name in registry.tool_names() {
        if let Some(def) = registry.get_tool_def(&name).await {
            let wrapper = McpToolWrapper::new(name, def, Arc::clone(&registry));
            tools_registry.push(Box::new(wrapper));
        }
    }
}

/// Build the runtime tool registry shared by the agent loop, channels, and gateway.
pub async fn build_tools_registry(
    config: &Config,
    security: &Arc<SecurityPolicy>,
    runtime: Arc<dyn RuntimeAdapter>,
    memory: Arc<dyn Memory>,
    options: ToolsRegistryOptions,
) -> Result<Vec<Box<dyn Tool>>> {
    build_tools_registry_with_mcp(config, security, runtime, memory, options, None).await
}

/// Build a tool registry, reusing an existing MCP connection when supplied.
pub(crate) async fn build_tools_registry_with_mcp(
    config: &Config,
    security: &Arc<SecurityPolicy>,
    runtime: Arc<dyn RuntimeAdapter>,
    memory: Arc<dyn Memory>,
    options: ToolsRegistryOptions,
    shared_mcp_registry: Option<Arc<McpRegistry>>,
) -> Result<Vec<Box<dyn Tool>>> {
    let (composio_key, composio_entity_id) = if config.composio.enabled {
        (
            config.composio.api_key.as_deref(),
            Some(config.composio.entity_id.as_str()),
        )
    } else {
        (None, None)
    };

    let mut tools_registry = tools::all_tools_with_runtime(
        Arc::new(config.clone()),
        security,
        runtime,
        memory,
        composio_key,
        composio_entity_id,
        &config.browser,
        &config.http_request,
        &config.web_fetch,
        &config.workspace_dir,
        &config.agents,
        config.api_key.as_deref(),
        config,
    );

    if options.include_peripherals {
        let peripheral_tools =
            crate::peripherals::create_peripheral_tools(&config.peripherals).await?;
        if !peripheral_tools.is_empty() {
            tracing::info!(count = peripheral_tools.len(), "Peripheral tools added");
            tools_registry.extend(peripheral_tools);
        }
    }

    if options.include_mcp && config.mcp.enabled && !config.mcp.servers.is_empty() {
        let registry = match shared_mcp_registry {
            Some(registry) => Some(registry),
            None => connect_mcp_registry(config).await,
        };
        if let Some(registry) = registry {
            append_mcp_tools(registry, &mut tools_registry).await;
        }
    }

    if options.apply_agent_tool_filters {
        tools_registry = filter_primary_agent_tools_or_fail(config, tools_registry)?;
    }

    Ok(tools_registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{McpServerConfig, McpTransport};
    use crate::memory::Memory;
    use std::path::Path;
    use tempfile::TempDir;

    fn test_config(tmp: &TempDir) -> Config {
        let mut config = Config::default();
        config.workspace_dir = tmp.path().join("workspace");
        config.config_path = tmp.path().join("config.toml");
        config.memory.backend = "none".to_string();
        std::fs::create_dir_all(&config.workspace_dir).unwrap();
        config
    }

    async fn build_for(config: &Config, options: ToolsRegistryOptions) -> Vec<Box<dyn Tool>> {
        build_for_with_mcp(config, options, None).await
    }

    async fn build_for_with_mcp(
        config: &Config,
        options: ToolsRegistryOptions,
        shared_mcp_registry: Option<Arc<McpRegistry>>,
    ) -> Vec<Box<dyn Tool>> {
        let runtime: Arc<dyn RuntimeAdapter> =
            Arc::from(crate::runtime::create_runtime(&config.runtime).unwrap());
        let security = Arc::new(SecurityPolicy::from_config(
            &config.autonomy,
            &config.workspace_dir,
        ));
        let mem: Arc<dyn Memory> = Arc::from(
            crate::memory::create_memory_with_storage(
                &config.memory,
                Some(&config.storage.provider.config),
                &config.workspace_dir,
                None,
            )
            .unwrap(),
        );
        build_tools_registry_with_mcp(
            config,
            &security,
            runtime,
            mem,
            options,
            shared_mcp_registry,
        )
        .await
        .unwrap()
    }

    fn mock_mcp_server_script(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("mock_mcp_server.sh");
        std::fs::write(
            &path,
            r#"#!/usr/bin/env bash
set -euo pipefail
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  method=$(printf '%s' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
  case "$method" in
    initialize)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"mock"}}}\n' "$id"
      ;;
    tools/list)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"ping","description":"Ping","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id"
      ;;
    tools/call)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id"
      ;;
  esac
done
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    fn mock_server_config(script: &Path) -> McpServerConfig {
        McpServerConfig {
            name: "mock".to_string(),
            transport: McpTransport::Stdio,
            command: script.to_string_lossy().into_owned(),
            args: vec![],
            env: std::collections::HashMap::new(),
            tool_timeout_secs: Some(10),
            url: None,
            headers: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn agent_loop_options_include_mcp() {
        assert!(ToolsRegistryOptions::AGENT_LOOP.include_mcp);
        assert!(ToolsRegistryOptions::GATEWAY.include_mcp);
        assert!(!ToolsRegistryOptions::MINIMAL.include_mcp);
    }

    #[tokio::test]
    async fn skips_mcp_when_disabled() {
        let tmp = TempDir::new().unwrap();
        let script = mock_mcp_server_script(tmp.path());
        let mut config = test_config(&tmp);
        config.mcp.enabled = false;
        config.mcp.servers = vec![mock_server_config(&script)];

        let tools = build_for(&config, ToolsRegistryOptions::AGENT_LOOP).await;
        assert!(!tools.iter().any(|tool| tool.name() == "mock__ping"));
    }

    #[tokio::test]
    async fn skips_mcp_when_server_list_empty() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.mcp.enabled = true;
        config.mcp.servers.clear();

        let tools = build_for(&config, ToolsRegistryOptions::AGENT_LOOP).await;
        assert!(!tools.iter().any(|tool| tool.name() == "mock__ping"));
    }

    #[tokio::test]
    async fn agent_loop_registers_mcp_tools_from_stdio_server() {
        let tmp = TempDir::new().unwrap();
        let script = mock_mcp_server_script(tmp.path());
        let mut config = test_config(&tmp);
        config.mcp.enabled = true;
        config.mcp.servers = vec![mock_server_config(&script)];

        let tools = build_for(&config, ToolsRegistryOptions::AGENT_LOOP).await;
        assert!(
            tools.iter().any(|tool| tool.name() == "mock__ping"),
            "expected mock__ping in registry, got {:?}",
            tools
                .iter()
                .map(|t| t.name().to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            tools
                .iter()
                .find(|tool| tool.name() == "mock__ping")
                .expect("MCP tool")
                .category(),
            crate::tools::ToolCategory::McpTools
        );
    }

    #[tokio::test]
    async fn shared_mcp_registry_is_reused_by_multiple_tool_registries() {
        let tmp = TempDir::new().unwrap();
        let script = mock_mcp_server_script(tmp.path());
        let mut config = test_config(&tmp);
        config.mcp.enabled = true;
        config.mcp.servers = vec![mock_server_config(&script)];

        let shared = connect_mcp_registry(&config)
            .await
            .expect("MCP registry should connect");
        let initial_refs = Arc::strong_count(&shared);

        let gateway_tools = build_for_with_mcp(
            &config,
            ToolsRegistryOptions::GATEWAY,
            Some(Arc::clone(&shared)),
        )
        .await;
        let gateway_refs = Arc::strong_count(&shared);
        let channel_tools = build_for_with_mcp(
            &config,
            ToolsRegistryOptions::CHANNEL,
            Some(Arc::clone(&shared)),
        )
        .await;

        assert!(gateway_tools.iter().any(|tool| tool.name() == "mock__ping"));
        assert!(channel_tools.iter().any(|tool| tool.name() == "mock__ping"));
        assert!(gateway_refs > initial_refs);
        assert!(Arc::strong_count(&shared) > gateway_refs);
    }

    #[tokio::test]
    async fn minimal_registry_skips_mcp_even_when_configured() {
        let tmp = TempDir::new().unwrap();
        let script = mock_mcp_server_script(tmp.path());
        let mut config = test_config(&tmp);
        config.mcp.enabled = true;
        config.mcp.servers = vec![mock_server_config(&script)];

        let tools = build_for(&config, ToolsRegistryOptions::MINIMAL).await;
        assert!(!tools.iter().any(|tool| tool.name() == "mock__ping"));
    }
}
