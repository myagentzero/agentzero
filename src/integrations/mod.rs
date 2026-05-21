pub mod registry;

use crate::config::Config;
use anyhow::Result;

/// Integration status
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum IntegrationStatus {
    /// Fully implemented and ready to use
    Available,
    /// Configured and active
    Active,
    /// Planned but not yet implemented
    ComingSoon,
}

/// Integration category
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum IntegrationCategory {
    Chat,
    AiModel,
    Productivity,
    ToolsAutomation,
    Platform,
}

impl IntegrationCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat Providers",
            Self::AiModel => "AI Models",
            Self::Productivity => "Productivity",
            Self::ToolsAutomation => "Tools & Automation",
            Self::Platform => "Platforms",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::Chat,
            Self::AiModel,
            Self::Productivity,
            Self::ToolsAutomation,
            Self::Platform,
        ]
    }
}

/// A registered integration
pub struct IntegrationEntry {
    pub name: &'static str,
    pub description: &'static str,
    pub category: IntegrationCategory,
    pub status_fn: fn(&Config) -> IntegrationStatus,
}

/// Handle the `integrations` CLI command
pub fn handle_command(command: crate::IntegrationCommands, config: &Config) -> Result<()> {
    match command {
        crate::IntegrationCommands::Info { name } => show_integration_info(config, &name),
    }
}

fn show_integration_info(config: &Config, name: &str) -> Result<()> {
    let entries = registry::all_integrations();
    let name_lower = name.to_lowercase();

    let Some(entry) = entries.iter().find(|e| e.name.to_lowercase() == name_lower) else {
        anyhow::bail!(
            "Unknown integration: {name}. Check README for supported integrations or run `zeroclaw onboard` to configure channels/providers."
        );
    };

    let status = (entry.status_fn)(config);
    let (icon, label) = match status {
        IntegrationStatus::Active => ("✅", "Active"),
        IntegrationStatus::Available => ("⚪", "Available"),
        IntegrationStatus::ComingSoon => ("🔜", "Coming Soon"),
    };

    println!();
    println!(
        "  {} {} — {}",
        icon,
        console::style(entry.name).white().bold(),
        entry.description
    );
    println!("  Category: {}", entry.category.label());
    println!("  Status:   {label}");
    println!();

    // Show setup hints based on integration
    match entry.name {
        "Discord" => {
            println!("  Setup:");
            println!("    1. Go to https://discord.com/developers/applications");
            println!("    2. Create app → Bot → Copy token");
            println!("    3. Enable MESSAGE CONTENT intent");
            println!("    4. Run: zeroclaw onboard --channels-only");
        }
        "Slack" => {
            println!("  Setup:");
            println!("    1. Go to https://api.slack.com/apps");
            println!("    2. Create app → Bot Token Scopes → Install");
            println!("    3. Run: zeroclaw onboard --channels-only");
        }
        "OpenRouter" => {
            println!("  Setup:");
            println!("    1. Get API key at https://openrouter.ai/keys");
            println!("    2. Run: zeroclaw onboard");
            println!("    Access 200+ models with one key.");
        }
        "GitHub" => {
            println!("  Setup:");
            println!("    1. Create a personal access token at https://github.com/settings/tokens");
            println!("    2. Add to config: [integrations.github] token = \"ghp_...\"");
        }
        "Webhooks" => {
            println!("  Built-in:");
            println!("    HTTP endpoint for external triggers.");
            println!("    Run: zeroclaw gateway");
        }
        _ => {
            if status == IntegrationStatus::ComingSoon {
                println!("  This integration is planned. Stay tuned!");
                println!("  Track progress: https://github.com/myagentzero/zeroclaw");
            }
        }
    }

    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integration_category_all_includes_every_variant_once() {
        let all = IntegrationCategory::all();
        assert_eq!(all.len(), 5);

        let labels: Vec<&str> = all.iter().map(|cat| cat.label()).collect();
        assert!(labels.contains(&"Chat Providers"));
        assert!(labels.contains(&"AI Models"));
        assert!(labels.contains(&"Productivity"));
        assert!(labels.contains(&"Tools & Automation"));
        assert!(labels.contains(&"Platforms"));
    }

    #[test]
    fn handle_command_info_is_case_insensitive_for_known_integrations() {
        let config = Config::default();
        let first_name = registry::all_integrations()
            .first()
            .expect("registry should define at least one integration")
            .name
            .to_lowercase();

        let result = handle_command(
            crate::IntegrationCommands::Info { name: first_name },
            &config,
        );

        assert!(result.is_ok());
    }

    #[test]
    fn handle_command_info_returns_error_for_unknown_integration() {
        let config = Config::default();
        let result = handle_command(
            crate::IntegrationCommands::Info {
                name: "definitely-not-a-real-integration".into(),
            },
            &config,
        );

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown integration"));
    }
}
