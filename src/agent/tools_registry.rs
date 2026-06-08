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
        include_mcp: false,
        apply_agent_tool_filters: true,
    };

    pub const CHANNEL: Self = Self {
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

async fn append_mcp_tools(config: &Config, tools_registry: &mut Vec<Box<dyn Tool>>) {
    tracing::info!(
        "Initializing MCP client — {} server(s) configured",
        config.mcp.servers.len()
    );
    match McpRegistry::connect_all(&config.mcp.servers).await {
        Ok(registry) => {
            let registry = Arc::new(registry);
            let names = registry.tool_names();
            let mut registered = 0usize;
            for name in names {
                if let Some(def) = registry.get_tool_def(&name).await {
                    let wrapper = McpToolWrapper::new(name, def, Arc::clone(&registry));
                    tools_registry.push(Box::new(wrapper));
                    registered += 1;
                }
            }
            tracing::info!(
                "MCP: {} tool(s) registered from {} server(s)",
                registered,
                registry.server_count()
            );
        }
        Err(e) => {
            tracing::error!("MCP registry failed to initialize: {e:#}");
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
        append_mcp_tools(config, &mut tools_registry).await;
    }

    if options.apply_agent_tool_filters {
        tools_registry = filter_primary_agent_tools_or_fail(config, tools_registry)?;
    }

    Ok(tools_registry)
}
