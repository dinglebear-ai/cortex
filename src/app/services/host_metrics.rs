use chrono::{Duration, Utc};

use super::super::models::{
    ListHostMetricsRequest, ListHostMetricsResponse, ListMetricHostsResponse,
};
use super::{CortexService, ServiceError, ServiceResult};
use crate::db::{self, HostMetricParams, MetricCursor};

impl CortexService {
    pub async fn list_metric_hosts(&self) -> ServiceResult<ListMetricHostsResponse> {
        let now = Utc::now();
        let since_unix_nano = (now - Duration::hours(24))
            .timestamp_nanos_opt()
            .ok_or_else(|| ServiceError::InvalidInput("time window is out of range".into()))?;
        let hosts = self
            .run_heavy_db("list_metric_hosts", move |pool| {
                db::list_metric_hosts(pool, since_unix_nano)
            })
            .await?;
        Ok(ListMetricHostsResponse {
            hosts,
            queried_at: super::super::time::rfc3339_z(now),
        })
    }

    pub async fn list_host_metrics(
        &self,
        req: ListHostMetricsRequest,
    ) -> ServiceResult<ListHostMetricsResponse> {
        let hostname = required_filter(req.hostname, "hostname", 255)?;
        let metric_name = required_filter(req.metric_name, "metric_name", 512)?;
        let service_name = req
            .service_name
            .map(|value| required_filter(value, "service_name", 512))
            .transpose()?;
        let minutes = req.minutes.unwrap_or(60);
        if !(1..=1440).contains(&minutes) {
            return Err(ServiceError::InvalidInput(
                "minutes must be between 1 and 1440".into(),
            ));
        }
        let before = match (req.before_time_unix_nano, req.before_id) {
            (None, None) => None,
            (Some(time_unix_nano), Some(id)) if time_unix_nano > 0 && id > 0 => {
                Some(MetricCursor { time_unix_nano, id })
            }
            _ => {
                return Err(ServiceError::InvalidInput(
                    "before_time_unix_nano and before_id must be positive and supplied together"
                        .into(),
                ));
            }
        };
        let now = Utc::now();
        let since_unix_nano = (now - Duration::minutes(i64::from(minutes)))
            .timestamp_nanos_opt()
            .ok_or_else(|| ServiceError::InvalidInput("time window is out of range".into()))?;
        let limit = req.limit.unwrap_or(120);
        if !(1..=500).contains(&limit) {
            return Err(ServiceError::InvalidInput(
                "limit must be between 1 and 500".into(),
            ));
        }
        let params = HostMetricParams {
            hostname,
            service_name,
            metric_name,
            since_unix_nano,
            before,
            limit: limit as usize,
        };
        let page = self
            .run_heavy_db("list_host_metrics", move |pool| {
                db::list_host_metrics(pool, &params)
            })
            .await?;
        Ok(ListHostMetricsResponse::from_page(
            page,
            super::super::time::rfc3339_z(now),
        ))
    }
}

// Match the character limits used by OTLP metric normalization so every
// accepted resource and metric identifier remains queryable.
fn required_filter(value: String, name: &str, max_chars: usize) -> ServiceResult<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max_chars {
        return Err(ServiceError::InvalidInput(format!(
            "{name} must contain 1 to {max_chars} characters"
        )));
    }
    Ok(value.to_owned())
}

#[cfg(test)]
#[path = "host_metrics_tests.rs"]
mod tests;
