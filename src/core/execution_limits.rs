//! Effective execution budgets. Sparse overrides replace, never clamp, ancestors.
use std::{collections::BTreeMap, time::Duration};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Bound {
    pub unlimited: bool,
    pub value: i64,
}

impl Bound {
    pub const fn limited(value: i64) -> Self {
        Self {
            unlimited: false,
            value,
        }
    }

    pub const fn unlimited() -> Self {
        Self {
            unlimited: true,
            value: 0,
        }
    }

    pub fn as_usize(self) -> Result<Option<usize>, String> {
        self.validate()?;
        if self.unlimited {
            return Ok(None);
        }
        usize::try_from(self.value)
            .map(Some)
            .map_err(|_| "execution limit is negative or exceeds this platform's capacity".into())
    }

    pub fn as_duration(self) -> Result<Option<Duration>, String> {
        self.validate()?;
        if self.unlimited {
            return Ok(None);
        }
        if self.value > i64::MAX / 1_000_000 {
            return Err("execution timeout exceeds the runtime's duration capacity".into());
        }
        u64::try_from(self.value)
            .map(|v| Some(Duration::from_millis(v)))
            .map_err(|_| "execution timeout must not be negative".into())
    }

    pub fn validate(self) -> Result<(), String> {
        if self.value < 0 {
            return Err("execution limit must not be negative".into());
        }
        if self.unlimited && self.value != 0 {
            return Err("an unlimited execution limit must not also specify a value".into());
        }
        Ok(())
    }
}

pub const DEFAULT_LIMITS: &[(&str, i64)] = &[
    ("http.timeout_ms", 60_000),
    ("http.connect_timeout_ms", 30_000),
    ("http.tls_handshake_timeout_ms", 10_000),
    ("http.request_bytes", 67_108_864),
    ("http.response_bytes", 67_108_864),
    ("http.envelope_bytes", 100_663_296),
    ("http.redirects", 10),
    ("http.header_count", 256),
    ("http.url_bytes", 16_384),
    ("websocket.handshake_timeout_ms", 60_000),
    ("websocket.opening_bytes", 1_048_576),
    ("websocket.message_bytes", 16_777_216),
    ("websocket.concurrent_sessions", 500),
    ("websocket.script_source_bytes", 1_048_576),
    ("websocket.script_modules", 64),
    ("script.timeout_ms", 30_000),
    ("script.memory_bytes", 33_554_432),
    ("script.stack_bytes", 262_144),
    ("script.source_bytes", 262_144),
    ("script.body_bytes", 5_242_880),
    ("script.result_bytes", 8_388_608),
    ("script.log_entries", 100),
    ("script.log_bytes", 65_536),
    ("chain.max_depth", 16),
    ("chain.max_requests", 64),
];

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct ExecutionLimits(pub BTreeMap<String, Bound>);

impl ExecutionLimits {
    /// Call only with a defined key; an unknown key is a programming error.
    pub fn get(&self, key: &str) -> Bound {
        self.0.get(key).copied().unwrap_or_else(|| {
            Bound::limited(
                DEFAULT_LIMITS
                    .iter()
                    .find(|(name, _)| *name == key)
                    .expect("unknown execution limit key")
                    .1,
            )
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        for (key, bound) in &self.0 {
            if key.ends_with("_ms") {
                bound
                    .as_duration()
                    .map_err(|error| format!("{key}: {error}"))?;
            } else {
                bound
                    .as_usize()
                    .map_err(|error| format!("{key}: {error}"))?;
            }
        }
        Ok(())
    }

    pub fn overlay(&mut self, overrides: &Self) {
        self.0
            .extend(overrides.0.iter().map(|(key, bound)| (key.clone(), *bound)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_unlimited_and_safe_conversions() {
        assert_eq!(Bound::limited(0).as_usize().unwrap(), Some(0));
        assert_eq!(
            Bound::limited(0).as_duration().unwrap(),
            Some(Duration::ZERO)
        );
        assert_eq!(Bound::unlimited().as_usize().unwrap(), None);
        assert!(Bound::limited(-1).as_usize().is_err());
        assert!(
            Bound {
                unlimited: true,
                value: -1
            }
            .as_usize()
            .is_err()
        );
        assert!(
            Bound {
                unlimited: true,
                value: 1
            }
            .as_duration()
            .is_err()
        );
        assert!(Bound::limited(i64::MAX).as_duration().is_err());
    }

    #[test]
    fn nearest_override_can_raise_or_remove_limit() {
        let mut limits = ExecutionLimits::default();
        assert_eq!(limits.get("http.response_bytes").value, 67_108_864);
        let child = serde_json::from_str(
            r#"{"http.response_bytes":{"unlimited":false,"value":100000000}}"#,
        )
        .unwrap();
        limits.overlay(&child);
        assert_eq!(limits.get("http.response_bytes").value, 100_000_000);
        limits.overlay(&ExecutionLimits(BTreeMap::from([(
            "http.response_bytes".into(),
            Bound::unlimited(),
        )])));
        assert_eq!(limits.get("http.response_bytes").as_usize().unwrap(), None);
    }
}
