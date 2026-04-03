use super::{IntegrationCategory, IntegrationEntry, IntegrationStatus};
use crate::providers::{
    is_glm_alias, is_minimax_alias, is_moonshot_alias, is_qianfan_alias, is_qwen_alias,
    is_zai_alias,
};

/// Returns the full catalog of integrations
#[allow(clippy::too_many_lines)]
pub fn all_integrations() -> Vec<IntegrationEntry> {
    vec![
        // ── Chat Providers ──────────────────────────────────────
        IntegrationEntry {
            name: "Discord",
            description: "Servers, channels & DMs",
            category: IntegrationCategory::Chat,
            status_fn: |c| {
                if c.channels_config.discord.is_some() {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Slack",
            description: "Workspace apps via Web API",
            category: IntegrationCategory::Chat,
            status_fn: |c| {
                if c.channels_config.slack.is_some() {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Webhooks",
            description: "HTTP endpoint for triggers",
            category: IntegrationCategory::Chat,
            status_fn: |c| {
                if c.channels_config.webhook.is_some() {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Email",
            description: "IMAP/SMTP email channel",
            category: IntegrationCategory::Chat,
            status_fn: |c| {
                if c.channels_config.email.is_some() {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "IRC",
            description: "IRC over TLS",
            category: IntegrationCategory::Chat,
            status_fn: |c| {
                if c.channels_config.irc.is_some() {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "ACP Server",
            description: "JSON-RPC 2.0 over stdio for IDEs",
            category: IntegrationCategory::Chat,
            status_fn: |_| IntegrationStatus::Available,
        },
        // ── AI Models ───────────────────────────────────────────
        IntegrationEntry {
            name: "OpenRouter",
            description: "200+ models, 1 API key",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref() == Some("openrouter") && c.api_key.is_some() {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Anthropic",
            description: "Claude Sonnet 4.6, Claude Opus 4.6",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref() == Some("anthropic") {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "OpenAI",
            description: "GPT-5.2, GPT-5.2-Codex",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref() == Some("openai") {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Google",
            description: "Gemini 3.1 Pro, Gemini 3 Flash",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_model
                    .as_deref()
                    .is_some_and(|m| m.starts_with("google/"))
                {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "DeepSeek",
            description: "DeepSeek-Reasoner, DeepSeek-Chat",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_model
                    .as_deref()
                    .is_some_and(|m| m.starts_with("deepseek/"))
                {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "xAI",
            description: "Grok 4, Grok 3",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_model
                    .as_deref()
                    .is_some_and(|m| m.starts_with("x-ai/"))
                {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Mistral",
            description: "Mistral Large Latest, Codestral",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_model
                    .as_deref()
                    .is_some_and(|m| m.starts_with("mistral"))
                {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Ollama",
            description: "Local models (Llama, etc.)",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref() == Some("ollama") {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Perplexity",
            description: "Sonar Pro, Sonar Reasoning Pro",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref() == Some("perplexity") {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "LM Studio",
            description: "Local model server",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref().is_some_and(|provider| {
                    provider.eq_ignore_ascii_case("lmstudio")
                        || provider.eq_ignore_ascii_case("lm-studio")
                }) {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Venice",
            description: "Privacy-first inference (Llama, Opus)",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref() == Some("venice") {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Vercel AI",
            description: "Vercel AI Gateway",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref() == Some("vercel") {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Cloudflare AI",
            description: "Cloudflare AI Gateway",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref() == Some("cloudflare") {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Moonshot",
            description: "Kimi & Kimi Coding",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref().is_some_and(is_moonshot_alias) {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Synthetic",
            description: "Synthetic AI models",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref() == Some("synthetic") {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "OpenCode Zen",
            description: "Code-focused AI models",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref() == Some("opencode") {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Z.AI",
            description: "Z.AI inference",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref().is_some_and(is_zai_alias) {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "GLM",
            description: "ChatGLM / Zhipu models",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref().is_some_and(is_glm_alias) {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "MiniMax",
            description: "MiniMax AI models",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref().is_some_and(is_minimax_alias) {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Qwen",
            description: "Alibaba DashScope Qwen models",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref().is_some_and(is_qwen_alias) {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Amazon Bedrock",
            description: "AWS managed model access",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref() == Some("bedrock") {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Qianfan",
            description: "Baidu AI models",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref().is_some_and(is_qianfan_alias) {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Groq",
            description: "Ultra-fast LPU inference",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref() == Some("groq") {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Together AI",
            description: "Open-source model hosting",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref() == Some("together") {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Fireworks AI",
            description: "Fast open-source inference",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref() == Some("fireworks") {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Cohere",
            description: "Command R+ & embeddings",
            category: IntegrationCategory::AiModel,
            status_fn: |c| {
                if c.default_provider.as_deref() == Some("cohere") {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        // ── Productivity ────────────────────────────────────────
        IntegrationEntry {
            name: "GitHub",
            description: "Code, issues, PRs",
            category: IntegrationCategory::Productivity,
            status_fn: |c| {
                if c.channels_config.github.is_some() {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Notion",
            description: "Workspace & databases",
            category: IntegrationCategory::Productivity,
            status_fn: |c| {
                if c.notion.enabled && !c.notion.api_key.is_empty() {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        // ── Tools & Automation ──────────────────────────────────
        IntegrationEntry {
            name: "Browser",
            description: "Chrome/Chromium control",
            category: IntegrationCategory::ToolsAutomation,
            status_fn: |c| {
                if c.browser.enabled {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Shell",
            description: "Terminal command execution",
            category: IntegrationCategory::ToolsAutomation,
            status_fn: |_| IntegrationStatus::Active,
        },
        IntegrationEntry {
            name: "File System",
            description: "Read/write files",
            category: IntegrationCategory::ToolsAutomation,
            status_fn: |_| IntegrationStatus::Active,
        },
        IntegrationEntry {
            name: "Cron",
            description: "Scheduled tasks",
            category: IntegrationCategory::ToolsAutomation,
            status_fn: |c| {
                if c.cron.enabled {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Weather",
            description: "Forecasts & conditions",
            category: IntegrationCategory::ToolsAutomation,
            status_fn: |_| IntegrationStatus::Active,
        },
        // ── Platforms ───────────────────────────────────────────
        IntegrationEntry {
            name: "macOS",
            description: "Native support + AppleScript",
            category: IntegrationCategory::Platform,
            status_fn: |_| {
                if cfg!(target_os = "macos") {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Linux",
            description: "Native support",
            category: IntegrationCategory::Platform,
            status_fn: |_| {
                if cfg!(target_os = "linux") {
                    IntegrationStatus::Active
                } else {
                    IntegrationStatus::Available
                }
            },
        },
        IntegrationEntry {
            name: "Windows",
            description: "WSL2 recommended",
            category: IntegrationCategory::Platform,
            status_fn: |_| IntegrationStatus::Available,
        },
        IntegrationEntry {
            name: "iOS",
            description: "Chat via Telegram/Discord",
            category: IntegrationCategory::Platform,
            status_fn: |_| IntegrationStatus::Available,
        },
        IntegrationEntry {
            name: "Android",
            description: "Chat via Telegram/Discord",
            category: IntegrationCategory::Platform,
            status_fn: |_| IntegrationStatus::Available,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    #[test]
    fn registry_has_entries() {
        let entries = all_integrations();
        assert!(
            entries.len() >= 42,
            "Expected 42+ integrations, got {}",
            entries.len()
        );
    }

    #[test]
    fn all_categories_represented() {
        let entries = all_integrations();
        // Only assert categories that currently have registry entries.
        let expected = [
            IntegrationCategory::Chat,
            IntegrationCategory::AiModel,
            IntegrationCategory::Productivity,
            IntegrationCategory::ToolsAutomation,
            IntegrationCategory::Platform,
        ];
        for cat in &expected {
            let count = entries.iter().filter(|e| e.category == *cat).count();
            assert!(count > 0, "Category {cat:?} has no entries");
        }
    }

    #[test]
    fn status_functions_dont_panic() {
        let config = Config::default();
        let entries = all_integrations();
        for entry in &entries {
            let _ = (entry.status_fn)(&config);
        }
    }

    #[test]
    fn no_duplicate_names() {
        let entries = all_integrations();
        let mut seen = std::collections::HashSet::new();
        for entry in &entries {
            assert!(
                seen.insert(entry.name),
                "Duplicate integration name: {}",
                entry.name
            );
        }
    }

    #[test]
    fn no_empty_names_or_descriptions() {
        let entries = all_integrations();
        for entry in &entries {
            assert!(!entry.name.is_empty(), "Found integration with empty name");
            assert!(
                !entry.description.is_empty(),
                "Integration '{}' has empty description",
                entry.name
            );
        }
    }

    #[test]
    fn discord_active_when_configured() {
        let mut config = Config::default();
        config.channels_config.discord = Some(crate::config::DiscordConfig {
            bot_token: "discord-tok".into(),
            guild_id: None,
            allowed_users: vec!["user".into()],
            listen_to_bots: false,
            mention_only: false,
            group_reply: None,
        });
        let entries = all_integrations();
        let dc = entries.iter().find(|e| e.name == "Discord").unwrap();
        assert!(matches!((dc.status_fn)(&config), IntegrationStatus::Active));
    }

    #[test]
    fn discord_available_when_not_configured() {
        let config = Config::default();
        let entries = all_integrations();
        let dc = entries.iter().find(|e| e.name == "Discord").unwrap();
        assert!(matches!(
            (dc.status_fn)(&config),
            IntegrationStatus::Available
        ));
    }

    #[test]
    fn lm_studio_available_when_not_selected_as_default_provider() {
        let config = Config::default();
        let entries = all_integrations();
        let lm_studio = entries.iter().find(|e| e.name == "LM Studio").unwrap();
        assert!(matches!(
            (lm_studio.status_fn)(&config),
            IntegrationStatus::Available
        ));
    }

    #[test]
    fn lm_studio_active_for_lmstudio_default_provider_aliases() {
        let entries = all_integrations();
        let lm_studio = entries.iter().find(|e| e.name == "LM Studio").unwrap();

        let mut config = Config {
            default_provider: Some("lmstudio".to_string()),
            ..Config::default()
        };
        assert!(matches!(
            (lm_studio.status_fn)(&config),
            IntegrationStatus::Active
        ));

        config.default_provider = Some("lm-studio".to_string());
        assert!(matches!(
            (lm_studio.status_fn)(&config),
            IntegrationStatus::Active
        ));
    }

    #[test]
    fn email_available_when_not_configured() {
        let config = Config::default();
        let entries = all_integrations();
        let email = entries.iter().find(|e| e.name == "Email").unwrap();
        assert!(matches!(
            (email.status_fn)(&config),
            IntegrationStatus::Available
        ));
    }

    #[test]
    fn shell_and_filesystem_always_active() {
        let config = Config::default();
        let entries = all_integrations();
        for name in ["Shell", "File System"] {
            let entry = entries.iter().find(|e| e.name == name).unwrap();
            assert!(
                matches!((entry.status_fn)(&config), IntegrationStatus::Active),
                "{name} should always be Active"
            );
        }
    }

    #[test]
    fn macos_active_on_macos() {
        let config = Config::default();
        let entries = all_integrations();
        let macos = entries.iter().find(|e| e.name == "macOS").unwrap();
        let status = (macos.status_fn)(&config);
        if cfg!(target_os = "macos") {
            assert!(matches!(status, IntegrationStatus::Active));
        } else {
            assert!(matches!(status, IntegrationStatus::Available));
        }
    }

    #[test]
    fn category_counts_reasonable() {
        let entries = all_integrations();
        let chat_count = entries
            .iter()
            .filter(|e| e.category == IntegrationCategory::Chat)
            .count();
        let ai_count = entries
            .iter()
            .filter(|e| e.category == IntegrationCategory::AiModel)
            .count();
        assert!(
            chat_count >= 5,
            "Expected 5+ chat integrations, got {chat_count}"
        );
        assert!(
            ai_count >= 5,
            "Expected 5+ AI model integrations, got {ai_count}"
        );
    }

    #[test]
    fn regional_provider_aliases_activate_expected_ai_integrations() {
        let entries = all_integrations();
        let mut config = Config {
            default_provider: Some("minimax-cn".to_string()),
            ..Config::default()
        };

        let minimax = entries.iter().find(|e| e.name == "MiniMax").unwrap();
        assert!(matches!(
            (minimax.status_fn)(&config),
            IntegrationStatus::Active
        ));

        config.default_provider = Some("glm-cn".to_string());
        let glm = entries.iter().find(|e| e.name == "GLM").unwrap();
        assert!(matches!(
            (glm.status_fn)(&config),
            IntegrationStatus::Active
        ));

        config.default_provider = Some("moonshot-intl".to_string());
        let moonshot = entries.iter().find(|e| e.name == "Moonshot").unwrap();
        assert!(matches!(
            (moonshot.status_fn)(&config),
            IntegrationStatus::Active
        ));

        config.default_provider = Some("qwen-intl".to_string());
        let qwen = entries.iter().find(|e| e.name == "Qwen").unwrap();
        assert!(matches!(
            (qwen.status_fn)(&config),
            IntegrationStatus::Active
        ));

        config.default_provider = Some("zai-cn".to_string());
        let zai = entries.iter().find(|e| e.name == "Z.AI").unwrap();
        assert!(matches!(
            (zai.status_fn)(&config),
            IntegrationStatus::Active
        ));

        config.default_provider = Some("baidu".to_string());
        let qianfan = entries.iter().find(|e| e.name == "Qianfan").unwrap();
        assert!(matches!(
            (qianfan.status_fn)(&config),
            IntegrationStatus::Active
        ));
    }
}
