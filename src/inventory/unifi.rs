use futures_util::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use reqwest::header::HeaderMap;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

use crate::inventory::collectors::CollectorOutput;
use crate::inventory::http::{HttpProbe, api_key_header};

#[path = "unifi_normalize.rs"]
mod normalize;
use normalize::{normalize_devices, normalize_networks, normalize_sites};

const MAX_UNIFI_REQUEST_SECS: u64 = 10;
const MAX_UNIFI_CONCURRENT_REQUESTS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SiteRef {
    id: Option<String>,
    internal_reference: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestKind {
    Devices,
    ModernNetworks,
    LegacyNetworks,
    NetworkDetails,
}

#[derive(Debug, Clone)]
struct RequestSpec {
    kind: RequestKind,
    path: String,
    site_id: Option<String>,
}

type RequestFuture<'a> = BoxFuture<'a, (RequestSpec, Result<Value, String>)>;

fn request_site_key(spec: &RequestSpec) -> String {
    spec.site_id.clone().unwrap_or_else(|| spec.path.clone())
}

fn encode_path_segment(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[(byte >> 4) as usize]));
            encoded.push(char::from(HEX[(byte & 0x0f) as usize]));
        }
    }
    encoded
}

pub async fn collect(
    url: Option<&str>,
    api_key: Option<&str>,
    timeout: Duration,
) -> CollectorOutput {
    let mut out = CollectorOutput::new("unifi");
    let (Some(url), Some(api_key)) = (url, api_key) else {
        out.skip("CORTEX_UNIFI_URL/API_KEY not set; UniFi collection skipped");
        return out;
    };
    let request_timeout = timeout.min(Duration::from_secs(MAX_UNIFI_REQUEST_SECS));
    let Ok(http) = HttpProbe::new(request_timeout) else {
        out.warn("http", "failed to initialize UniFi HTTP client");
        return out;
    };
    let headers = match api_key_header("x-api-key", api_key) {
        Ok(headers) => headers,
        Err(error) => {
            out.warn(
                "config",
                format!("UniFi API key contains invalid header characters: {error}"),
            );
            return out;
        }
    };
    let deadline = tokio::time::Instant::now() + soft_collector_budget(timeout);

    let sites_path = "/proxy/network/integration/v1/sites?offset=0&limit=200";
    let sites = match tokio::time::timeout_at(
        deadline,
        get_json(&http, url, sites_path, headers.clone()),
    )
    .await
    {
        Ok(Ok(body)) => {
            let sites = normalize_sites(url, sites_path, &body, &mut out);
            if sites.is_empty() {
                vec![fallback_site()]
            } else {
                sites
            }
        }
        Ok(Err(message)) => {
            out.warn(sites_path, message);
            vec![fallback_site()]
        }
        Err(_) => {
            out.warn(
                "collection_deadline",
                "UniFi collection deadline reached while loading sites; preserved completed results",
            );
            return out;
        }
    };

    let request_limiter = Arc::new(Semaphore::new(MAX_UNIFI_CONCURRENT_REQUESTS));
    let mut requests = FuturesUnordered::new();
    for site in &sites {
        let site_reference = encode_path_segment(&site.internal_reference);
        let device_path = format!("/proxy/network/api/s/{site_reference}/stat/device");
        push_request(
            &mut requests,
            &http,
            url,
            headers.clone(),
            Arc::clone(&request_limiter),
            RequestSpec {
                kind: RequestKind::Devices,
                path: device_path,
                site_id: site.id.clone(),
            },
        );
        let legacy_path = format!("/proxy/network/api/s/{site_reference}/rest/networkconf");
        push_request(
            &mut requests,
            &http,
            url,
            headers.clone(),
            Arc::clone(&request_limiter),
            RequestSpec {
                kind: RequestKind::LegacyNetworks,
                path: legacy_path,
                site_id: site.id.clone(),
            },
        );
        if let Some(site_id) = &site.id {
            let encoded_site_id = encode_path_segment(site_id);
            let path = format!(
                "/proxy/network/integration/v1/sites/{encoded_site_id}/networks?offset=0&limit=200"
            );
            push_request(
                &mut requests,
                &http,
                url,
                headers.clone(),
                Arc::clone(&request_limiter),
                RequestSpec {
                    kind: RequestKind::ModernNetworks,
                    path,
                    site_id: Some(site_id.clone()),
                },
            );
        }
    }

    let mut detail_requests = Vec::new();
    let mut modern_network_sites = BTreeSet::new();
    let mut legacy_network_sites = BTreeSet::new();
    let mut network_errors = Vec::new();
    while !requests.is_empty() {
        let next = tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => {
                out.warn(
                    "collection_deadline",
                    format!(
                        "UniFi collection deadline reached; preserved completed results and skipped {} unfinished site requests",
                        requests.len()
                    ),
                );
                return out;
            }
            next = requests.next() => next,
        };
        let Some((spec, result)) = next else {
            break;
        };
        match result {
            Ok(body) => match spec.kind {
                RequestKind::Devices => normalize_devices(url, &spec.path, &body, &mut out),
                RequestKind::LegacyNetworks => {
                    legacy_network_sites.insert(request_site_key(&spec));
                    normalize_networks(url, &spec.path, spec.site_id.as_deref(), &body, &mut out);
                }
                RequestKind::ModernNetworks => {
                    modern_network_sites.insert(request_site_key(&spec));
                    let network_ids = normalize_networks(
                        url,
                        &spec.path,
                        spec.site_id.as_deref(),
                        &body,
                        &mut out,
                    );
                    if let Some(site_id) = spec.site_id {
                        let encoded_site_id = encode_path_segment(&site_id);
                        detail_requests.extend(network_ids.into_iter().map(|network_id| {
                            let encoded_network_id = encode_path_segment(&network_id);
                            RequestSpec {
                                kind: RequestKind::NetworkDetails,
                                path: format!(
                                    "/proxy/network/integration/v1/sites/{encoded_site_id}/networks/{encoded_network_id}"
                                ),
                                site_id: Some(site_id.clone()),
                            }
                        }));
                    }
                }
                RequestKind::NetworkDetails => {
                    normalize_networks(url, &spec.path, spec.site_id.as_deref(), &body, &mut out);
                }
            },
            Err(message) => match spec.kind {
                RequestKind::ModernNetworks | RequestKind::LegacyNetworks => {
                    network_errors.push((spec, message));
                }
                RequestKind::Devices | RequestKind::NetworkDetails => out.warn(&spec.path, message),
            },
        }
    }
    for (spec, message) in network_errors {
        let site_key = request_site_key(&spec);
        let fallback_succeeded = match spec.kind {
            RequestKind::ModernNetworks => legacy_network_sites.contains(&site_key),
            RequestKind::LegacyNetworks => modern_network_sites.contains(&site_key),
            RequestKind::Devices | RequestKind::NetworkDetails => false,
        };
        if !fallback_succeeded {
            out.warn(&spec.path, message);
        }
    }

    let mut details = FuturesUnordered::new();
    for spec in detail_requests {
        push_request(
            &mut details,
            &http,
            url,
            headers.clone(),
            Arc::clone(&request_limiter),
            spec,
        );
    }
    while !details.is_empty() {
        let next = tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => {
                out.warn(
                    "collection_deadline",
                    format!(
                        "UniFi collection deadline reached; preserved completed results and skipped {} unfinished network detail requests",
                        details.len()
                    ),
                );
                return out;
            }
            next = details.next() => next,
        };
        let Some((spec, result)) = next else {
            break;
        };
        match result {
            Ok(body) => {
                normalize_networks(url, &spec.path, spec.site_id.as_deref(), &body, &mut out);
            }
            Err(message) => out.warn(&spec.path, message),
        }
    }
    out
}

fn push_request<'a>(
    requests: &mut FuturesUnordered<RequestFuture<'a>>,
    http: &'a HttpProbe,
    url: &'a str,
    headers: HeaderMap,
    limiter: Arc<Semaphore>,
    spec: RequestSpec,
) {
    requests.push(Box::pin(async move {
        let result = match limiter.acquire_owned().await {
            Ok(_permit) => get_json(http, url, &spec.path, headers).await,
            Err(_) => Err("UniFi request limiter closed".to_string()),
        };
        (spec, result)
    }));
}

async fn get_json(
    http: &HttpProbe,
    url: &str,
    path: &str,
    headers: HeaderMap,
) -> Result<Value, String> {
    let endpoint = format!("{}{}", url.trim_end_matches('/'), path);
    match http.get_json(&endpoint, headers).await {
        Ok(response) if response.status < 400 => Ok(response.body),
        Ok(response) => Err(format!(
            "UniFi endpoint {path} returned HTTP {}",
            response.status
        )),
        Err(error) => Err(format!("UniFi endpoint {path} failed: {error}")),
    }
}

fn fallback_site() -> SiteRef {
    SiteRef {
        id: None,
        internal_reference: "default".to_string(),
    }
}

fn soft_collector_budget(timeout: Duration) -> Duration {
    if timeout.is_zero() {
        return timeout;
    }
    let margin_ms = (timeout.as_millis() / 20).clamp(1, 250) as u64;
    timeout.saturating_sub(Duration::from_millis(margin_ms))
}

#[cfg(test)]
#[path = "unifi_tests.rs"]
mod tests;
