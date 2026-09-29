use super::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListHostMetricsRequest {
    pub hostname: String,
    pub service_name: Option<String>,
    pub metric_name: String,
    /// Look back this many minutes (default 60, maximum 1440).
    pub minutes: Option<u32>,
    pub limit: Option<u32>,
    pub before_time_unix_nano: Option<i64>,
    pub before_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListHostMetricsResponse {
    pub points: Vec<db::HostMetricPoint>,
    pub next_cursor: Option<db::MetricCursor>,
    pub truncated: bool,
    pub queried_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListMetricHostsResponse {
    pub hosts: Vec<db::MetricHost>,
    pub queried_at: String,
}

impl ListHostMetricsResponse {
    pub(crate) fn from_page(page: db::HostMetricsPage, queried_at: String) -> Self {
        Self {
            points: page.points,
            next_cursor: page.next_cursor,
            truncated: page.truncated,
            queried_at,
        }
    }
}
