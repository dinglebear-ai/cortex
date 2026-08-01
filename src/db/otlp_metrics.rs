#![allow(dead_code)]

use super::agent_observatory::EnumParseError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetricInstrumentKind {
    Gauge,
    Sum,
    Histogram,
    ExponentialHistogram,
    Summary,
}

impl MetricInstrumentKind {
    pub const ALL: &'static [Self] = &[
        Self::Gauge,
        Self::Sum,
        Self::Histogram,
        Self::ExponentialHistogram,
        Self::Summary,
    ];
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gauge => "gauge",
            Self::Sum => "sum",
            Self::Histogram => "histogram",
            Self::ExponentialHistogram => "exponential_histogram",
            Self::Summary => "summary",
        }
    }
}
impl fmt::Display for MetricInstrumentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
impl FromStr for MetricInstrumentKind {
    type Err = EnumParseError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "gauge" => Ok(Self::Gauge),
            "sum" => Ok(Self::Sum),
            "histogram" => Ok(Self::Histogram),
            "exponential_histogram" => Ok(Self::ExponentialHistogram),
            "summary" => Ok(Self::Summary),
            _ => Err(EnumParseError::new("MetricInstrumentKind", value)),
        }
    }
}
impl Serialize for MetricInstrumentKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for MetricInstrumentKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OtelMetricPointRow {
    pub id: i64,
    pub point_key: String,
    pub metric_name: String,
    pub description: String,
    pub unit: String,
    pub instrument_kind: MetricInstrumentKind,
    pub aggregation_temporality: Option<i64>,
    pub monotonic: Option<bool>,
    pub start_time_unix_nano: Option<i64>,
    pub time_unix_nano: i64,
    pub hostname: String,
    pub service_name: Option<String>,
    pub service_version: Option<String>,
    pub scope_name: Option<String>,
    pub scope_version: Option<String>,
    pub ai_tool: Option<String>,
    pub ai_project: Option<String>,
    pub ai_session_id: Option<String>,
    pub run_id: Option<i64>,
    pub resource_json: String,
    pub attributes_json: String,
    pub value_json: String,
    pub exemplars_json: String,
    pub received_at: String,
    pub content_scrubbed: bool,
}
