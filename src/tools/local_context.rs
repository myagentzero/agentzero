//! Local context tool — provides date, time, timezone, and location to the LLM.
//!
//! LLMs struggle with day-of-week calculations and timezone conversions. This
//! tool returns the current system time, explicit day of week, timezone with
//! UTC offset, and optional user location so the LLM can reason accurately
//! about scheduling, localized greetings, and time-sensitive queries.

use super::traits::{Tool, ToolResult};
use crate::config::schema::LocalContextConfig;
use async_trait::async_trait;
use chrono::{Local, Utc};
use serde_json::{Value, json};

pub struct LocalContextTool {
    city: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    timezone_override: Option<String>,
}

impl LocalContextTool {
    pub fn new(config: &LocalContextConfig) -> Self {
        Self {
            city: config.city.clone(),
            latitude: config.latitude,
            longitude: config.longitude,
            timezone_override: config.timezone.clone(),
        }
    }
}

#[async_trait]
impl Tool for LocalContextTool {
    fn name(&self) -> &str {
        "local_context"
    }

    fn description(&self) -> &str {
        "Get current date/time, day of week, timezone, and location (lat, lon, city)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
        let mut lines = Vec::with_capacity(6);

        if let Some(ref tz_name) = self.timezone_override {
            match tz_name.parse::<chrono_tz::Tz>() {
                Ok(tz) => {
                    let now = Utc::now().with_timezone(&tz);
                    lines.push(format!("Current Date: {}", now.format("%Y-%m-%d")));
                    lines.push(format!("Day of Week: {}", now.format("%A")));
                    lines.push(format!("Current Time: {}", now.format("%H:%M:%S")));
                    lines.push(format!("Timezone: {} (UTC{})", tz_name, now.format("%:z")));
                }
                Err(_) => {
                    let now = Local::now();
                    lines.push(format!("Current Date: {}", now.format("%Y-%m-%d")));
                    lines.push(format!("Day of Week: {}", now.format("%A")));
                    lines.push(format!("Current Time: {}", now.format("%H:%M:%S")));
                    lines.push(format!(
                        "Timezone: {} (UTC{})",
                        now.format("%Z"),
                        now.format("%:z")
                    ));
                    lines.push(format!(
                        "Note: configured timezone '{}' is not a valid IANA timezone; using system timezone",
                        tz_name
                    ));
                }
            }
        } else {
            let now = Local::now();
            lines.push(format!("Current Date: {}", now.format("%Y-%m-%d")));
            lines.push(format!("Day of Week: {}", now.format("%A")));
            lines.push(format!("Current Time: {}", now.format("%H:%M:%S")));
            lines.push(format!(
                "Timezone: {} (UTC{})",
                now.format("%Z"),
                now.format("%:z")
            ));
        }

        if let Some(ref city) = self.city {
            lines.push(format!("City: {city}"));
        }

        if let (Some(lat), Some(lon)) = (self.latitude, self.longitude) {
            lines.push(format!("Coordinates: {lat}, {lon}"));
        }

        let output = lines.join("\n");
        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> LocalContextConfig {
        LocalContextConfig::default()
    }

    fn config_with_location() -> LocalContextConfig {
        LocalContextConfig {
            enabled: true,
            city: Some("Denver".into()),
            latitude: Some(39.7392),
            longitude: Some(-104.9903),
            timezone: None,
        }
    }

    // ── Metadata ────────────────────────────────────────────────────────

    #[test]
    fn name_is_local_context() {
        let tool = LocalContextTool::new(&default_config());
        assert_eq!(tool.name(), "local_context");
    }

    #[test]
    fn description_is_non_empty() {
        let tool = LocalContextTool::new(&default_config());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn parameters_schema_is_valid_object() {
        let tool = LocalContextTool::new(&default_config());
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"].is_object());
    }

    #[test]
    fn spec_reflects_metadata() {
        let tool = LocalContextTool::new(&default_config());
        let spec = tool.spec();
        assert_eq!(spec.name, "local_context");
        assert_eq!(spec.description, tool.description());
        assert!(spec.parameters.is_object());
    }

    // ── Execute ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn execute_default_contains_date_and_day() {
        let tool = LocalContextTool::new(&default_config());
        let result = tool.execute(json!({})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("Current Date:"));
        assert!(result.output.contains("Day of Week:"));
        assert!(result.output.contains("Current Time:"));
        assert!(result.output.contains("Timezone:"));
    }

    #[tokio::test]
    async fn execute_default_does_not_contain_city() {
        let tool = LocalContextTool::new(&default_config());
        let result = tool.execute(json!({})).await.unwrap();
        assert!(result.success);
        assert!(!result.output.contains("City:"));
        assert!(!result.output.contains("Coordinates:"));
    }

    #[tokio::test]
    async fn execute_with_location_contains_city_and_coords() {
        let tool = LocalContextTool::new(&config_with_location());
        let result = tool.execute(json!({})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("City: Denver"));
        assert!(result.output.contains("Coordinates: 39.7392, -104.9903"));
    }

    #[tokio::test]
    async fn execute_with_valid_timezone_override() {
        let config = LocalContextConfig {
            timezone: Some("America/Denver".into()),
            ..default_config()
        };
        let tool = LocalContextTool::new(&config);
        let result = tool.execute(json!({})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("America/Denver"));
        assert!(!result.output.contains("Note:"));
    }

    #[tokio::test]
    async fn execute_with_invalid_timezone_falls_back() {
        let config = LocalContextConfig {
            timezone: Some("Invalid/Timezone".into()),
            ..default_config()
        };
        let tool = LocalContextTool::new(&config);
        let result = tool.execute(json!({})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("Note:"));
        assert!(result.output.contains("Invalid/Timezone"));
    }

    #[tokio::test]
    async fn execute_with_only_latitude_omits_coordinates() {
        let config = LocalContextConfig {
            latitude: Some(39.7392),
            ..default_config()
        };
        let tool = LocalContextTool::new(&config);
        let result = tool.execute(json!({})).await.unwrap();
        assert!(result.success);
        assert!(!result.output.contains("Coordinates:"));
    }

    #[tokio::test]
    async fn execute_day_of_week_is_valid_name() {
        let tool = LocalContextTool::new(&default_config());
        let result = tool.execute(json!({})).await.unwrap();
        let valid_days = [
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
            "Sunday",
        ];
        let has_valid_day = valid_days.iter().any(|day| result.output.contains(day));
        assert!(has_valid_day, "Output should contain a valid day name");
    }
}
