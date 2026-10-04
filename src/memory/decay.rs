use super::traits::{MemoryCategory, MemoryEntry};
use crate::config::MemoryConfig;
use chrono::{DateTime, Utc};

/// Half-life in days for custom categories, and for conversations when
/// retention is disabled or hygiene is off.
const DEFAULT_HALF_LIFE_DAYS: f64 = 7.0;

/// Half-life in days for `Daily` memories, which lose relevance quickly.
const DAILY_HALF_LIFE_DAYS: f64 = 3.0;

/// Half-life in days for `System` memories, which lose relevance quickly.
const SYSTEM_HALF_LIFE_DAYS: f64 = 2.0;

/// Per-category half-lives. Conversation uses half of its retention window
/// so a memory still holds 25% of its score when hygiene prunes it; Daily
/// and System use fixed short half-lives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecayHalfLives {
    conversation: f64,
    daily: f64,
    system: f64,
}

impl DecayHalfLives {
    pub fn from_config(config: &MemoryConfig) -> Self {
        let retention_days = config.conversation_retention_days;
        Self {
            conversation: if config.hygiene_enabled && retention_days > 0 {
                f64::from(retention_days) / 2.0
            } else {
                DEFAULT_HALF_LIFE_DAYS
            },
            daily: DAILY_HALF_LIFE_DAYS,
            system: SYSTEM_HALF_LIFE_DAYS,
        }
    }

    /// Half-life for `category`, or `None` for evergreen `Core` memories.
    fn for_category(&self, category: &MemoryCategory) -> Option<f64> {
        match category {
            MemoryCategory::Core => None,
            MemoryCategory::Conversation => Some(self.conversation),
            MemoryCategory::Daily => Some(self.daily),
            MemoryCategory::System => Some(self.system),
            MemoryCategory::Custom(_) => Some(DEFAULT_HALF_LIFE_DAYS),
        }
    }
}

impl Default for DecayHalfLives {
    fn default() -> Self {
        Self::from_config(&MemoryConfig::default())
    }
}

/// Days during which `Core` memories receive a ranking boost.
pub const CORE_BOOST_WINDOW_DAYS: f64 = 7.0;

/// Score added to `Core` memories that are still inside [`CORE_BOOST_WINDOW_DAYS`].
pub const CORE_CATEGORY_SCORE_BOOST: f64 = 0.3;

fn entry_age_days(entry: &MemoryEntry, now: DateTime<Utc>) -> Option<f64> {
    let ts = DateTime::parse_from_rfc3339(&entry.timestamp)
        .ok()?
        .with_timezone(&Utc);
    Some(now.signed_duration_since(ts).num_seconds().max(0) as f64 / 86_400.0)
}

/// Ranking boost for a `Core` memory, or `0.0` if it is ineligible.
///
/// Boost applies only while the entry's RFC3339 timestamp is at most
/// `window_days` old. Non-Core entries, unparseable timestamps, and older
/// Core entries get no boost. Core scores themselves are never time-decayed.
pub fn core_recency_boost(entry: &MemoryEntry, window_days: f64) -> f64 {
    if entry.category != MemoryCategory::Core {
        return 0.0;
    }
    let window = if window_days <= 0.0 {
        CORE_BOOST_WINDOW_DAYS
    } else {
        window_days
    };
    match entry_age_days(entry, Utc::now()) {
        Some(age_days) if age_days <= window => CORE_CATEGORY_SCORE_BOOST,
        _ => 0.0,
    }
}

/// Apply exponential time decay to memory entry scores.
///
/// - `Core` memories are exempt ("evergreen") — their scores are never decayed.
/// - Entries without a parseable RFC3339 timestamp are left unchanged.
/// - Entries without a score (`None`) are left unchanged.
///
/// Decay formula: `score * 2^(-age_days / half_life_days)`, with the
/// half-life chosen per category by `half_lives`.
pub fn apply_time_decay(entries: &mut [MemoryEntry], half_lives: &DecayHalfLives) {
    let now = Utc::now();

    for entry in entries.iter_mut() {
        let Some(half_life) = half_lives.for_category(&entry.category) else {
            continue;
        };

        let score = match entry.score {
            Some(s) => s,
            None => continue,
        };

        let Some(age_days) = entry_age_days(entry, now) else {
            continue;
        };

        let decay_factor = (-age_days / half_life * std::f64::consts::LN_2).exp();
        entry.score = Some(score * decay_factor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(category: MemoryCategory, score: Option<f64>, timestamp: &str) -> MemoryEntry {
        MemoryEntry {
            id: "1".into(),
            key: "test".into(),
            content: "value".into(),
            category,
            timestamp: timestamp.into(),
            session_id: None,
            score,
        }
    }

    fn recent_rfc3339() -> String {
        Utc::now().to_rfc3339()
    }

    fn days_ago_rfc3339(days: i64) -> String {
        (Utc::now() - chrono::Duration::days(days)).to_rfc3339()
    }

    const WEEK: DecayHalfLives = DecayHalfLives {
        conversation: 7.0,
        daily: 7.0,
        system: 7.0,
    };

    #[test]
    fn half_lives_per_category() {
        let config = MemoryConfig {
            conversation_retention_days: 60,
            ..MemoryConfig::default()
        };
        let half_lives = DecayHalfLives::from_config(&config);
        assert_eq!(
            half_lives.for_category(&MemoryCategory::Conversation),
            Some(30.0)
        );
        assert_eq!(
            half_lives.for_category(&MemoryCategory::Daily),
            Some(DAILY_HALF_LIFE_DAYS)
        );
        assert_eq!(
            half_lives.for_category(&MemoryCategory::System),
            Some(SYSTEM_HALF_LIFE_DAYS)
        );
        assert_eq!(half_lives.for_category(&MemoryCategory::Core), None);
        assert_eq!(
            half_lives.for_category(&MemoryCategory::Custom("notes".into())),
            Some(DEFAULT_HALF_LIFE_DAYS)
        );
    }

    #[test]
    fn half_lives_fall_back_without_retention() {
        let unpruned = MemoryConfig {
            conversation_retention_days: 0,
            ..MemoryConfig::default()
        };
        assert_eq!(
            DecayHalfLives::from_config(&unpruned).for_category(&MemoryCategory::Conversation),
            Some(DEFAULT_HALF_LIFE_DAYS)
        );

        let hygiene_off = MemoryConfig {
            hygiene_enabled: false,
            ..MemoryConfig::default()
        };
        assert_eq!(
            DecayHalfLives::from_config(&hygiene_off),
            DecayHalfLives {
                conversation: DEFAULT_HALF_LIFE_DAYS,
                daily: DAILY_HALF_LIFE_DAYS,
                system: SYSTEM_HALF_LIFE_DAYS,
            }
        );
    }

    #[test]
    fn entry_at_retention_keeps_quarter_score() {
        let half_lives = DecayHalfLives::from_config(&MemoryConfig {
            conversation_retention_days: 30,
            ..MemoryConfig::default()
        });
        let mut entries = vec![make_entry(
            MemoryCategory::Conversation,
            Some(1.0),
            &days_ago_rfc3339(30),
        )];
        apply_time_decay(&mut entries, &half_lives);
        let decayed = entries[0].score.unwrap();
        assert!((decayed - 0.25).abs() < 0.01, "got {decayed}");
    }

    #[test]
    fn core_memories_are_never_decayed() {
        let mut entries = vec![make_entry(
            MemoryCategory::Core,
            Some(0.9),
            &days_ago_rfc3339(30),
        )];
        apply_time_decay(&mut entries, &WEEK);
        assert_eq!(entries[0].score, Some(0.9));
    }

    #[test]
    fn recent_entry_score_barely_changes() {
        let mut entries = vec![make_entry(
            MemoryCategory::Conversation,
            Some(0.8),
            &recent_rfc3339(),
        )];
        apply_time_decay(&mut entries, &WEEK);
        let decayed = entries[0].score.unwrap();
        assert!(
            (decayed - 0.8).abs() < 0.01,
            "recent entry should barely decay, got {decayed}"
        );
    }

    #[test]
    fn one_half_life_halves_score() {
        let mut entries = vec![make_entry(
            MemoryCategory::Conversation,
            Some(1.0),
            &days_ago_rfc3339(7),
        )];
        apply_time_decay(&mut entries, &WEEK);
        let decayed = entries[0].score.unwrap();
        assert!(
            (decayed - 0.5).abs() < 0.05,
            "score after one half-life should be ~0.5, got {decayed}"
        );
    }

    #[test]
    fn two_half_lives_quarters_score() {
        let mut entries = vec![make_entry(
            MemoryCategory::Conversation,
            Some(1.0),
            &days_ago_rfc3339(14),
        )];
        apply_time_decay(&mut entries, &WEEK);
        let decayed = entries[0].score.unwrap();
        assert!(
            (decayed - 0.25).abs() < 0.05,
            "score after two half-lives should be ~0.25, got {decayed}"
        );
    }

    #[test]
    fn no_score_entry_is_unchanged() {
        let mut entries = vec![make_entry(
            MemoryCategory::Conversation,
            None,
            &days_ago_rfc3339(30),
        )];
        apply_time_decay(&mut entries, &WEEK);
        assert_eq!(entries[0].score, None);
    }

    #[test]
    fn unparseable_timestamp_is_unchanged() {
        let mut entries = vec![make_entry(
            MemoryCategory::Conversation,
            Some(0.9),
            "not-a-date",
        )];
        apply_time_decay(&mut entries, &WEEK);
        assert_eq!(entries[0].score, Some(0.9));
    }

    #[test]
    fn recent_core_receives_recency_boost() {
        let entry = make_entry(MemoryCategory::Core, Some(0.5), &recent_rfc3339());
        assert!((core_recency_boost(&entry, 7.0) - CORE_CATEGORY_SCORE_BOOST).abs() < f64::EPSILON);
    }

    #[test]
    fn core_older_than_window_receives_no_boost() {
        let entry = make_entry(MemoryCategory::Core, Some(0.5), &days_ago_rfc3339(8));
        assert_eq!(core_recency_boost(&entry, 7.0), 0.0);
    }

    #[test]
    fn core_at_window_boundary_still_boosted() {
        let ts =
            (Utc::now() - chrono::Duration::days(7) + chrono::Duration::minutes(1)).to_rfc3339();
        let entry = make_entry(MemoryCategory::Core, Some(0.5), &ts);
        assert!((core_recency_boost(&entry, 7.0) - CORE_CATEGORY_SCORE_BOOST).abs() < f64::EPSILON);
    }

    #[test]
    fn non_core_and_unparseable_receive_no_boost() {
        let conv = make_entry(MemoryCategory::Conversation, Some(0.5), &recent_rfc3339());
        let bad = make_entry(MemoryCategory::Core, Some(0.5), "not-a-date");
        assert_eq!(core_recency_boost(&conv, 7.0), 0.0);
        assert_eq!(core_recency_boost(&bad, 7.0), 0.0);
    }
}
