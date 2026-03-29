//! Weather tool — fetches forecast data from the National Weather Service API.
//!
//! Uses the free NWS API (`https://api.weather.gov/`) which requires no API key.
//! Accepts latitude and longitude coordinates. Coverage is limited to US locations.
//! The tool performs a two-step fetch: first resolving the grid point, then
//! retrieving the forecast for that grid.

use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

const NWS_BASE_URL: &str = "https://api.weather.gov";
const NWS_TIMEOUT_SECS: u64 = 15;
const NWS_CONNECT_TIMEOUT_SECS: u64 = 10;

// ── NWS API response types ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct NwsPointsResponse {
    properties: PointsProperties,
}

#[derive(Debug, Deserialize)]
struct PointsProperties {
    forecast: String,
    #[serde(rename = "relativeLocation")]
    relative_location: Option<RelativeLocation>,
}

#[derive(Debug, Deserialize)]
struct RelativeLocation {
    properties: RelativeLocationProperties,
}

#[derive(Debug, Deserialize)]
struct RelativeLocationProperties {
    city: String,
    state: String,
}

#[derive(Debug, Deserialize)]
struct NwsForecastResponse {
    properties: ForecastProperties,
}

#[derive(Debug, Deserialize)]
struct ForecastProperties {
    periods: Vec<ForecastPeriod>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ForecastPeriod {
    pub name: String,
    pub temperature: i64,
    #[serde(rename = "temperatureUnit")]
    pub temperature_unit: String,
    #[serde(rename = "windSpeed")]
    pub wind_speed: String,
    #[serde(rename = "windDirection")]
    pub wind_direction: String,
    #[serde(rename = "shortForecast")]
    pub short_forecast: String,
    #[serde(rename = "detailedForecast")]
    pub detailed_forecast: String,
    #[serde(rename = "isDaytime")]
    pub is_daytime: bool,
    #[serde(rename = "probabilityOfPrecipitation")]
    pub probability_of_precipitation: Option<PrecipitationProbability>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PrecipitationProbability {
    pub value: Option<f64>,
}

// ── Intermediate result for formatting ──────────────────────────────────────

struct NwsWeatherData {
    city: String,
    state: String,
    periods: Vec<ForecastPeriod>,
}

// ── Tool struct ─────────────────────────────────────────────────────────────

/// Fetches weather data from the NWS API — no API key required, US coverage.
pub struct WeatherTool;

impl WeatherTool {
    pub fn new() -> Self {
        Self
    }

    fn build_client() -> anyhow::Result<reqwest::Client> {
        let builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(NWS_TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(NWS_CONNECT_TIMEOUT_SECS))
            .user_agent(crate::config::schema::DEFAULT_USER_AGENT);

        let builder = crate::config::apply_runtime_proxy_to_builder(builder, "tool.weather");
        Ok(builder.build()?)
    }

    /// Fetch the grid point metadata for the given coordinates.
    async fn fetch_points(
        client: &reqwest::Client,
        lat: f64,
        lon: f64,
    ) -> anyhow::Result<NwsPointsResponse> {
        let url = format!("{NWS_BASE_URL}/points/{lat},{lon}");
        let response = client
            .get(&url)
            .header("Accept", "application/geo+json")
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "NWS API returned HTTP {status} for coordinates ({lat}, {lon}). \
                 Ensure the coordinates are within the US. Response: {body}"
            );
        }

        let points: NwsPointsResponse = response.json().await?;
        Ok(points)
    }

    /// Fetch the forecast from the URL provided by the points endpoint.
    async fn fetch_forecast(
        client: &reqwest::Client,
        forecast_url: &str,
    ) -> anyhow::Result<NwsForecastResponse> {
        let response = client
            .get(forecast_url)
            .header("Accept", "application/geo+json")
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("NWS forecast endpoint returned HTTP {status}");
        }

        let forecast: NwsForecastResponse = response.json().await?;
        Ok(forecast)
    }

    /// Two-step fetch: resolve grid point, then retrieve forecast.
    async fn fetch(lat: f64, lon: f64) -> anyhow::Result<NwsWeatherData> {
        let client = Self::build_client()?;

        let points = Self::fetch_points(&client, lat, lon).await?;
        let forecast = Self::fetch_forecast(&client, &points.properties.forecast).await?;

        let (city, state) = match points.properties.relative_location {
            Some(loc) => (loc.properties.city, loc.properties.state),
            None => ("Unknown".to_string(), "US".to_string()),
        };

        Ok(NwsWeatherData {
            city,
            state,
            periods: forecast.properties.periods,
        })
    }

    /// Format a single forecast period.
    fn format_period(period: &ForecastPeriod) -> String {
        let precip = period
            .probability_of_precipitation
            .as_ref()
            .and_then(|p| p.value)
            .map(|v| format!(" | Precip: {v:.0}%"))
            .unwrap_or_default();

        format!(
            "  {name}: {temp}°{unit} | Wind: {wind_speed} {wind_dir}{precip}\n\
             \x20   {short}\n\
             \x20   {detailed}",
            name = period.name,
            temp = period.temperature,
            unit = period.temperature_unit,
            wind_speed = period.wind_speed,
            wind_dir = period.wind_direction,
            short = period.short_forecast,
            detailed = period.detailed_forecast,
        )
    }

    /// Build the final human-readable output string.
    fn format_output(data: &NwsWeatherData, days: u8) -> String {
        if data.periods.is_empty() {
            return "No forecast data available for this location.".to_string();
        }

        let mut out = format!(
            "**Weather forecast for {city}, {state}**\n\
             ─────────────────────────────────────────",
            city = data.city,
            state = data.state,
        );

        // NWS returns ~14 periods (2 per day: daytime + nighttime).
        // Limit to requested days × 2 periods.
        let max_periods = (days as usize) * 2;
        let periods: Vec<&ForecastPeriod> = data.periods.iter().take(max_periods).collect();

        for period in &periods {
            out.push('\n');
            out.push_str(&Self::format_period(period));
        }

        out
    }
}

impl Default for WeatherTool {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tool trait ──────────────────────────────────────────────────────────────

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &str {
        "weather"
    }

    fn description(&self) -> &str {
        "Get weather forecast for US locations using the National Weather Service API. \
         Requires latitude and longitude coordinates. Returns up to 7 days of forecast \
         data including temperature, wind, and precipitation probability. \
         No API key required. Coverage is limited to US locations only."
    }

    fn prompt_hint(&self) -> Option<&str> {
        Some("Get weather forecast for US locations. Use when: user asks about weather. Requires lat/lon coordinates; US-only coverage.")
    }

    fn prompt_hint_compact(&self) -> &str {
        "Get weather forecast for US locations."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "latitude": {
                    "type": "number",
                    "description": "Latitude coordinate (e.g. 39.7456). Must be within US territory."
                },
                "longitude": {
                    "type": "number",
                    "description": "Longitude coordinate (e.g. -97.0892). Must be within US territory."
                },
                "days": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 7,
                    "description": "Number of forecast days to include (1–7). Default: 3."
                }
            },
            "required": ["latitude", "longitude"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let latitude = match args.get("latitude").and_then(|v| v.as_f64()) {
            Some(lat) if (-90.0..=90.0).contains(&lat) => lat,
            Some(lat) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "Latitude {lat} is out of range. Must be between -90 and 90."
                    )),
                });
            }
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing required parameter 'latitude'".into()),
                });
            }
        };

        let longitude = match args.get("longitude").and_then(|v| v.as_f64()) {
            Some(lon) if (-180.0..=180.0).contains(&lon) => lon,
            Some(lon) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "Longitude {lon} is out of range. Must be between -180 and 180."
                    )),
                });
            }
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing required parameter 'longitude'".into()),
                });
            }
        };

        let days: u8 = args
            .get("days")
            .and_then(|v| v.as_u64())
            .map(|d| d.clamp(1, 7) as u8)
            .unwrap_or(3);

        match Self::fetch(latitude, longitude).await {
            Ok(data) => {
                let output = Self::format_output(&data, days);
                tracing::info!(
                    lat = latitude,
                    lon = longitude,
                    days = days,
                    city = %data.city,
                    state = %data.state,
                    periods = data.periods.len(),
                    "🌤️ weather forecast fetched successfully"
                );
                Ok(ToolResult {
                    success: true,
                    output,
                    error: None,
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "{e}. \
                     Ensure the coordinates are within the US. \
                     Try again later or use a different tool."
                )),
            }),
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool() -> WeatherTool {
        WeatherTool::new()
    }

    fn make_period(name: &str, temp: i64, daytime: bool) -> ForecastPeriod {
        ForecastPeriod {
            name: name.into(),
            temperature: temp,
            temperature_unit: "F".into(),
            wind_speed: "10 mph".into(),
            wind_direction: "NW".into(),
            short_forecast: "Partly Cloudy".into(),
            detailed_forecast: "Partly cloudy with a high near 75.".into(),
            is_daytime: daytime,
            probability_of_precipitation: Some(PrecipitationProbability { value: Some(20.0) }),
        }
    }

    fn make_weather_data() -> NwsWeatherData {
        NwsWeatherData {
            city: "Topeka".into(),
            state: "KS".into(),
            periods: vec![
                make_period("Today", 75, true),
                make_period("Tonight", 55, false),
                make_period("Tuesday", 80, true),
                make_period("Tuesday Night", 60, false),
                make_period("Wednesday", 72, true),
                make_period("Wednesday Night", 50, false),
            ],
        }
    }

    // ── Metadata ────────────────────────────────────────────────────────────

    #[test]
    fn name_is_weather() {
        assert_eq!(make_tool().name(), "weather");
    }

    #[test]
    fn description_is_non_empty() {
        assert!(!make_tool().description().is_empty());
    }

    #[test]
    fn parameters_schema_is_valid_object() {
        let schema = make_tool().parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"].is_object());
    }

    #[test]
    fn schema_requires_latitude_and_longitude() {
        let schema = make_tool().parameters_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&Value::String("latitude".into())));
        assert!(required.contains(&Value::String("longitude".into())));
    }

    #[test]
    fn schema_latitude_property_exists() {
        let schema = make_tool().parameters_schema();
        assert!(schema["properties"]["latitude"].is_object());
        assert_eq!(schema["properties"]["latitude"]["type"], "number");
    }

    #[test]
    fn schema_longitude_property_exists() {
        let schema = make_tool().parameters_schema();
        assert!(schema["properties"]["longitude"].is_object());
        assert_eq!(schema["properties"]["longitude"]["type"], "number");
    }

    #[test]
    fn schema_days_has_bounds() {
        let schema = make_tool().parameters_schema();
        let days = &schema["properties"]["days"];
        assert_eq!(days["minimum"], 1);
        assert_eq!(days["maximum"], 7);
    }

    // ── execute: parameter validation ───────────────────────────────────────

    #[tokio::test]
    async fn execute_missing_latitude_returns_error() {
        let result = make_tool()
            .execute(json!({"longitude": -97.0}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("latitude"));
    }

    #[tokio::test]
    async fn execute_missing_longitude_returns_error() {
        let result = make_tool()
            .execute(json!({"latitude": 39.7}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("longitude"));
    }

    #[tokio::test]
    async fn execute_missing_both_returns_error() {
        let result = make_tool().execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("latitude"));
    }

    #[tokio::test]
    async fn execute_latitude_out_of_range_returns_error() {
        let result = make_tool()
            .execute(json!({"latitude": 91.0, "longitude": -97.0}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("out of range"));
    }

    #[tokio::test]
    async fn execute_longitude_out_of_range_returns_error() {
        let result = make_tool()
            .execute(json!({"latitude": 39.7, "longitude": 181.0}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("out of range"));
    }

    #[tokio::test]
    async fn execute_null_latitude_returns_error() {
        let result = make_tool()
            .execute(json!({"latitude": null, "longitude": -97.0}))
            .await
            .unwrap();
        assert!(!result.success);
    }

    // ── format_period ───────────────────────────────────────────────────────

    #[test]
    fn format_period_includes_all_fields() {
        let period = make_period("Today", 75, true);
        let formatted = WeatherTool::format_period(&period);
        assert!(formatted.contains("Today"));
        assert!(formatted.contains("75°F"));
        assert!(formatted.contains("10 mph"));
        assert!(formatted.contains("NW"));
        assert!(formatted.contains("Partly Cloudy"));
        assert!(formatted.contains("Precip: 20%"));
    }

    #[test]
    fn format_period_no_precip_when_none() {
        let mut period = make_period("Tonight", 55, false);
        period.probability_of_precipitation = None;
        let formatted = WeatherTool::format_period(&period);
        assert!(!formatted.contains("Precip"));
    }

    #[test]
    fn format_period_no_precip_when_value_is_null() {
        let mut period = make_period("Tonight", 55, false);
        period.probability_of_precipitation = Some(PrecipitationProbability { value: None });
        let formatted = WeatherTool::format_period(&period);
        assert!(!formatted.contains("Precip"));
    }

    // ── format_output ───────────────────────────────────────────────────────

    #[test]
    fn format_output_includes_location() {
        let data = make_weather_data();
        let out = WeatherTool::format_output(&data, 1);
        assert!(out.contains("Topeka"));
        assert!(out.contains("KS"));
    }

    #[test]
    fn format_output_respects_days_limit() {
        let data = make_weather_data();
        // 1 day = 2 periods
        let out = WeatherTool::format_output(&data, 1);
        assert!(out.contains("Today"));
        assert!(out.contains("Tonight"));
        assert!(!out.contains("Tuesday"));
    }

    #[test]
    fn format_output_multiple_days() {
        let data = make_weather_data();
        let out = WeatherTool::format_output(&data, 3);
        assert!(out.contains("Today"));
        assert!(out.contains("Tonight"));
        assert!(out.contains("Tuesday"));
        assert!(out.contains("Wednesday"));
    }

    #[test]
    fn format_output_empty_periods() {
        let data = NwsWeatherData {
            city: "Nowhere".into(),
            state: "XX".into(),
            periods: vec![],
        };
        let out = WeatherTool::format_output(&data, 1);
        assert!(out.contains("No forecast data available"));
    }

    // ── JSON deserialization ────────────────────────────────────────────────

    #[test]
    fn deserialize_points_response() {
        let json_str = r#"{
            "properties": {
                "forecast": "https://api.weather.gov/gridpoints/TOP/32,81/forecast",
                "relativeLocation": {
                    "properties": {
                        "city": "Topeka",
                        "state": "KS"
                    }
                }
            }
        }"#;
        let parsed: NwsPointsResponse = serde_json::from_str(json_str).unwrap();
        assert_eq!(
            parsed.properties.forecast,
            "https://api.weather.gov/gridpoints/TOP/32,81/forecast"
        );
        let loc = parsed.properties.relative_location.unwrap();
        assert_eq!(loc.properties.city, "Topeka");
        assert_eq!(loc.properties.state, "KS");
    }

    #[test]
    fn deserialize_forecast_response() {
        let json_str = r#"{
            "properties": {
                "periods": [
                    {
                        "name": "Today",
                        "temperature": 75,
                        "temperatureUnit": "F",
                        "windSpeed": "10 mph",
                        "windDirection": "NW",
                        "shortForecast": "Sunny",
                        "detailedForecast": "Sunny with a high near 75.",
                        "isDaytime": true,
                        "probabilityOfPrecipitation": {
                            "value": 10
                        }
                    }
                ]
            }
        }"#;
        let parsed: NwsForecastResponse = serde_json::from_str(json_str).unwrap();
        assert_eq!(parsed.properties.periods.len(), 1);
        let period = &parsed.properties.periods[0];
        assert_eq!(period.name, "Today");
        assert_eq!(period.temperature, 75);
        assert!(period.is_daytime);
    }

    #[test]
    fn deserialize_forecast_null_precip_value() {
        let json_str = r#"{
            "properties": {
                "periods": [
                    {
                        "name": "Tonight",
                        "temperature": 55,
                        "temperatureUnit": "F",
                        "windSpeed": "5 mph",
                        "windDirection": "S",
                        "shortForecast": "Clear",
                        "detailedForecast": "Clear with a low around 55.",
                        "isDaytime": false,
                        "probabilityOfPrecipitation": {
                            "value": null
                        }
                    }
                ]
            }
        }"#;
        let parsed: NwsForecastResponse = serde_json::from_str(json_str).unwrap();
        let period = &parsed.properties.periods[0];
        assert!(
            period
                .probability_of_precipitation
                .as_ref()
                .unwrap()
                .value
                .is_none()
        );
    }

    // ── spec ────────────────────────────────────────────────────────────────

    #[test]
    fn spec_reflects_tool_metadata() {
        let tool = make_tool();
        let spec = tool.spec();
        assert_eq!(spec.name, "weather");
        assert_eq!(spec.description, tool.description());
        assert!(spec.parameters.is_object());
    }
}
