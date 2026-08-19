use chrono::Utc;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

use crate::inventory::collectors::CollectorOutput;
use crate::inventory::config::MediaServiceConfig;
use crate::inventory::http::HttpProbe;
use crate::inventory::schema::{MediaService, Provenance};

pub async fn collect(services: &[MediaServiceConfig], timeout: Duration) -> CollectorOutput {
    let mut out = CollectorOutput::new("media_stack");
    if services.is_empty() {
        out.skip("no CORTEX_<MEDIA>_URL values set; media stack collection skipped");
        return out;
    }
    let Ok(http) = HttpProbe::new(timeout) else {
        out.warn("http", "failed to initialize media HTTP client");
        return out;
    };
    let responses = futures_util::future::join_all(services.iter().map(|service| {
        let endpoint = endpoint_for(service);
        let request_endpoint = request_endpoint_for(service, &endpoint);
        let headers = headers_for(service);
        let http = &http;
        async move { http.get_json(&request_endpoint, headers).await }
    }))
    .await;
    for (service, response) in services.iter().zip(responses) {
        match response {
            Ok(response) if response.status < 400 => {
                normalize_service(service, &response.body, "ok", &mut out)
            }
            Ok(response) => out.warn(
                &service.kind,
                format!("{} returned HTTP {}", service.kind, response.status),
            ),
            Err(error) => out.warn(
                &service.kind,
                format!("{} unavailable: {error}", service.kind),
            ),
        }
    }
    out
}

fn endpoint_for(service: &MediaServiceConfig) -> String {
    let base = service.base_url.trim_end_matches('/');
    match service.kind.as_str() {
        "sonarr" | "radarr" | "prowlarr" => format!("{base}/api/v3/system/status"),
        "sabnzbd" => format!("{base}/api?mode=version&output=json"),
        "qbittorrent" => format!("{base}/api/v2/app/version"),
        "plex" => format!("{base}/identity"),
        "tautulli" => format!("{base}/api/v2?cmd=get_server_info"),
        "overseerr" => format!("{base}/api/v1/status"),
        _ => base.to_string(),
    }
}

fn request_endpoint_for(service: &MediaServiceConfig, endpoint: &str) -> String {
    if service.kind != "tautulli" {
        return endpoint.to_string();
    }
    let Some(api_key) = &service.api_key else {
        return endpoint.to_string();
    };
    let Ok(mut url) = reqwest::Url::parse(endpoint) else {
        return endpoint.to_string();
    };
    url.query_pairs_mut().append_pair("apikey", api_key);
    url.to_string()
}

fn headers_for(service: &MediaServiceConfig) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(api_key) = &service.api_key {
        let header = match service.kind.as_str() {
            "plex" => "X-Plex-Token",
            _ => "X-Api-Key",
        };
        if let Ok(value) = HeaderValue::from_str(api_key) {
            headers.insert(header, value);
        }
    }
    headers
}

fn normalize_service(
    service: &MediaServiceConfig,
    body: &Value,
    status: &str,
    out: &mut CollectorOutput,
) {
    let mut topology = BTreeMap::new();
    let version = version_from_body(body);
    if let Some(version) = &version {
        topology.insert("version".to_string(), Value::String(version.clone()));
    }
    out.media_services.push(MediaService {
        service: service.kind.clone(),
        base_url: service.base_url.clone(),
        status: status.to_string(),
        version,
        topology,
        provenance: Provenance::new(
            endpoint_for(service),
            "source_inventory",
            Utc::now().to_rfc3339(),
        ),
    });
}

fn version_from_body(body: &Value) -> Option<String> {
    if let Some(version) = body.as_str() {
        return Some(version.to_string());
    }
    body.get("version")
        .or_else(|| body.get("Version"))
        .or_else(|| body.pointer("/response/data/version"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

#[cfg(test)]
#[path = "media_stack_tests.rs"]
mod tests;
