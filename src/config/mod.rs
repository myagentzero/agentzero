pub mod schema;
pub mod traits;

#[allow(unused_imports)]
pub use schema::{
    AckReactionChannelsConfig, AckReactionChatType, AckReactionConfig, AckReactionRuleAction,
    AckReactionRuleConfig, AckReactionStrategy, AgentConfig, AgentLoadBalanceStrategy,
    AgentSessionBackend, AgentSessionConfig, AgentSessionStrategy, AgentTeamsConfig,
    AgentsIpcConfig, AskUserConfig, AuditConfig, AutonomyConfig, BrowserComputerUseConfig,
    BrowserConfig, BuiltinHooksConfig, ChannelsConfig, ClassificationRule,
    CommandContextRuleAction, CommandContextRuleConfig, ComposioConfig, Config, CoordinationConfig,
    CostConfig, CronConfig, DEFAULT_MODEL_FALLBACK, DelegateAgentConfig, DiscordConfig,
    DockerRuntimeConfig, EconomicConfig, EconomicTokenPricing, EmbeddingRouteConfig, EstopConfig,
    GatewayConfig, GroupReplyConfig, GroupReplyMode, HardwareConfig, HardwareTransport,
    HeartbeatConfig, HooksConfig, HttpRequestConfig, HttpRequestCredentialProfile, IdentityConfig,
    MemoryConfig, ModelRouteConfig, MultimodalConfig,
    NonCliNaturalLanguageApprovalMode, NotionConfig, ObservabilityConfig, OtpChallengeDelivery,
    OtpConfig, OtpMethod, OutboundLeakGuardAction, OutboundLeakGuardConfig, PeripheralBoardConfig,
    PeripheralsConfig, PerplexityFilterConfig, PipelineConfig, PluginEntryConfig, PluginsConfig,
    ProgressMode, ProviderConfig, ProxyConfig, ProxyScope, QdrantConfig, QueryClassificationConfig,
    ReliabilityConfig, ResearchPhaseConfig, ResearchTrigger, ResourceLimitsConfig, RuntimeConfig,
    SandboxBackend, SandboxConfig, SchedulerConfig, SecretsConfig, SecurityConfig,
    SecurityRoleConfig, SkillCreationConfig, SkillImprovementConfig, SkillsConfig,
    SkillsPromptInjectionMode, SlackConfig, StorageConfig, StorageProviderConfig,
    StorageProviderSection, StreamMode, SubAgentsConfig, SyscallAnomalyConfig, TranscriptionConfig,
    TunnelConfig, UrlAccessConfig, WebFetchConfig, WebSearchConfig, WebhookConfig,
    apply_runtime_proxy_to_builder, build_runtime_proxy_client,
    build_runtime_proxy_client_with_timeouts, default_model_fallback_for_provider,
    resolve_default_model_id, runtime_proxy_config, set_runtime_proxy_config,
};

pub fn name_and_presence<T: traits::ChannelConfig>(channel: Option<&T>) -> (&'static str, bool) {
    (T::name(), channel.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reexported_config_default_is_constructible() {
        let config = Config::default();

        assert!(config.default_provider.is_some());
        assert!(config.default_model.is_some());
        assert!(config.default_temperature > 0.0);
    }

    #[test]
    fn reexported_channel_configs_are_constructible() {
        let discord = DiscordConfig {
            bot_token: "token".into(),
            guild_id: Some("123".into()),
            allowed_users: vec![],
            listen_to_bots: false,
            mention_only: false,
            group_reply: None,
        };

        assert_eq!(discord.guild_id.as_deref(), Some("123"));
    }

    #[test]
    fn reexported_http_request_config_is_constructible() {
        let cfg = HttpRequestConfig {
            enabled: true,
            allowed_domains: vec!["api.openai.com".into()],
            max_response_size: 256_000,
            timeout_secs: 10,
            user_agent: "zeroclaw-test".into(),
            credential_profiles: std::collections::HashMap::new(),
        };

        assert!(cfg.enabled);
        assert_eq!(cfg.allowed_domains, vec!["api.openai.com"]);
        assert_eq!(cfg.max_response_size, 256_000);
        assert_eq!(cfg.timeout_secs, 10);
        assert_eq!(cfg.user_agent, "zeroclaw-test");
    }
}
