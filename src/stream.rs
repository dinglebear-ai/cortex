use std::{collections::VecDeque, convert::Infallible, sync::OnceLock, time::Duration};

use axum::response::sse::{Event, KeepAlive, Sse};
use chrono::Utc;
use lab_auth::AuthContext;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{app::CortexService, db};

pub const MAX_BATCH_ITEMS: u32 = 100;
pub const MAX_BATCH_BYTES: usize = 128 * 1024;
const MAX_EVENT_BYTES: usize = 64 * 1024;
const CURSOR_TTL_SECS: i64 = 900;
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const MAX_CLIENTS: usize = 64;
static CLIENTS: OnceLock<std::sync::Arc<Semaphore>> = OnceLock::new();

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogStreamRequest {
    pub cursor: Option<String>,
    pub host: Option<String>,
    pub app: Option<String>,
    pub severity: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionStreamRequest {
    pub project: String,
    pub tool: String,
    pub session_id: String,
    pub host: String,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StreamCursor {
    version: u8,
    position: i64,
    principal: String,
    filters: String,
    issued_at: i64,
}

struct StreamState {
    service: CortexService,
    params: db::DurableStreamParams,
    principal: String,
    filters: String,
    position: i64,
    snapshot_high: i64,
    pending: VecDeque<db::DurableStreamRow>,
    pending_bytes: usize,
    issued_at: i64,
    _client_permit: OwnedSemaphorePermit,
}

pub async fn log_stream(
    service: CortexService,
    auth: AuthContext,
    request: LogStreamRequest,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, StreamError> {
    let mut filter_request = request.clone();
    filter_request.cursor = None;
    let filters = fingerprint(&filter_request)?;
    let params = db::DurableStreamParams {
        hostname: request.host,
        app_name: request.app,
        severity: request.severity,
        limit: MAX_BATCH_ITEMS + 1,
        ..Default::default()
    };
    build_stream(service, auth, request.cursor, filters, params, "log").await
}

pub async fn session_stream(
    service: CortexService,
    auth: AuthContext,
    request: SessionStreamRequest,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, StreamError> {
    for value in [
        &request.project,
        &request.tool,
        &request.session_id,
        &request.host,
    ] {
        if value.trim().is_empty() {
            return Err(StreamError::Invalid("session identity must not be empty"));
        }
    }
    let mut filter_request = request.clone();
    filter_request.cursor = None;
    let filters = fingerprint(&filter_request)?;
    let params = db::DurableStreamParams {
        hostname: Some(request.host),
        ai_project: Some(request.project),
        ai_tool: Some(request.tool),
        ai_session_id: Some(request.session_id),
        limit: MAX_BATCH_ITEMS + 1,
        ..Default::default()
    };
    build_stream(service, auth, request.cursor, filters, params, "session").await
}

async fn build_stream(
    service: CortexService,
    auth: AuthContext,
    cursor: Option<String>,
    filters: String,
    mut params: db::DurableStreamParams,
    event_name: &'static str,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, StreamError> {
    require_read_scope(&auth)?;
    let client_permit = CLIENTS
        .get_or_init(|| std::sync::Arc::new(Semaphore::new(MAX_CLIENTS)))
        .clone()
        .try_acquire_owned()
        .map_err(|_| StreamError::Overloaded)?;
    let principal = principal_key(&auth);
    let decoded = cursor.as_deref().map(decode_cursor).transpose()?;
    if let Some(cursor) = &decoded {
        if cursor.principal != principal {
            return Err(StreamError::Forbidden(
                "cursor belongs to another principal",
            ));
        }
        if cursor.filters != filters {
            return Err(StreamError::Invalid("cursor does not match stream filters"));
        }
        if Utc::now().timestamp() - cursor.issued_at > CURSOR_TTL_SECS {
            return Err(StreamError::Expired);
        }
    }
    params.after_id = decoded.as_ref().map_or(0, |cursor| cursor.position);
    let initial = service
        .durable_stream_page(params.clone())
        .await
        .map_err(StreamError::Service)?;
    if decoded.is_some()
        && initial
            .minimum_watermark
            .is_some_and(|minimum| params.after_id < minimum.saturating_sub(1))
    {
        return Err(StreamError::Gap {
            minimum: initial.minimum_watermark.unwrap(),
            requested: params.after_id,
        });
    }
    let position = decoded
        .as_ref()
        .map_or(initial.high_watermark, |cursor| cursor.position);
    let issued_at = decoded
        .as_ref()
        .map_or_else(|| Utc::now().timestamp(), |cursor| cursor.issued_at);
    let state = StreamState {
        service,
        params,
        principal,
        filters,
        position,
        snapshot_high: initial.high_watermark,
        pending: VecDeque::new(),
        pending_bytes: 0,
        issued_at,
        _client_permit: client_permit,
    };
    let stream = async_stream::stream! {
        let mut state = state;
        let snapshot = serde_json::json!({"kind":"snapshot","highWatermark":state.snapshot_high,
            "cursor": encode_cursor(state.position, &state.principal, &state.filters, state.issued_at)});
        yield Ok(Event::default().event("snapshot").data(snapshot.to_string()));
        loop {
            if Utc::now().timestamp() - state.issued_at > CURSOR_TTL_SECS {
                yield Ok(control_event("token_expired", serde_json::json!({"resync":true})));
                break;
            }
            if let Some(row) = state.pending.pop_front() {
                let data = row_json(&row, event_name);
                let size = data.len();
                state.pending_bytes = state.pending_bytes.saturating_sub(size);
                state.position = row.id;
                let cursor = encode_cursor(state.position, &state.principal, &state.filters, state.issued_at);
                yield Ok(Event::default().event(event_name).id(cursor).data(data));
                continue;
            }
            state.params.after_id = state.position;
            state.params.high_watermark = None;
            match state.service.durable_stream_page(state.params.clone()).await {
                Ok(page) => {
                    let mut bytes = 0usize;
                    for row in page.rows.into_iter().take(MAX_BATCH_ITEMS as usize) {
                        let size = row_json(&row, event_name).len();
                        if !state.pending.is_empty() && bytes.saturating_add(size) > MAX_BATCH_BYTES { break; }
                        bytes = bytes.saturating_add(size);
                        state.pending.push_back(row);
                    }
                    state.pending_bytes = bytes;
                    if state.pending.is_empty() { tokio::time::sleep(POLL_INTERVAL).await; }
                }
                Err(_) => {
                    yield Ok(control_event("overload", serde_json::json!({"retryAfterMs":1000,"resync":false})));
                    break;
                }
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(10))
            .text("keepalive"),
    ))
}

fn row_json(row: &db::DurableStreamRow, kind: &str) -> String {
    let (message, truncated) = truncate_utf8(&row.message, MAX_EVENT_BYTES);
    serde_json::json!({"contractVersion":"1.0.0","kind":kind,"position":row.id,
        "timestamp":row.timestamp,"host":row.hostname,"severity":row.severity,
        "app":row.app_name,"message":message,"metadata":row.metadata_json,
        "parseWarning":row.parse_error,"truncated":truncated})
    .to_string()
}

fn control_event(kind: &'static str, data: serde_json::Value) -> Event {
    Event::default().event(kind).data(data.to_string())
}

fn require_read_scope(auth: &AuthContext) -> Result<(), StreamError> {
    if auth
        .scopes
        .iter()
        .any(|scope| scope == "cortex:read" || scope == "cortex:admin")
    {
        Ok(())
    } else {
        Err(StreamError::Forbidden("cortex:read scope required"))
    }
}

fn principal_key(auth: &AuthContext) -> String {
    format!("{}:{}", auth.issuer, auth.sub)
}
fn fingerprint<T: Serialize>(value: &T) -> Result<String, StreamError> {
    let bytes = serde_json::to_vec(value).map_err(|_| StreamError::Invalid("invalid filters"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn encode_cursor(position: i64, principal: &str, filters: &str, issued_at: i64) -> String {
    let cursor = StreamCursor {
        version: 1,
        position,
        principal: principal.into(),
        filters: filters.into(),
        issued_at,
    };
    hex::encode(serde_json::to_vec(&cursor).expect("cursor is serializable"))
}
fn decode_cursor(value: &str) -> Result<StreamCursor, StreamError> {
    if value.len() > 2048 {
        return Err(StreamError::Invalid("invalid cursor"));
    }
    let bytes = hex::decode(value).map_err(|_| StreamError::Invalid("invalid cursor"))?;
    let cursor: StreamCursor =
        serde_json::from_slice(&bytes).map_err(|_| StreamError::Invalid("invalid cursor"))?;
    if cursor.version != 1 || cursor.position < 0 {
        return Err(StreamError::Invalid("invalid cursor"));
    }
    Ok(cursor)
}
fn truncate_utf8(value: &str, max: usize) -> (String, bool) {
    if value.len() <= max {
        return (value.to_owned(), false);
    }
    let mut end = max;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}...[truncated]", &value[..end]), true)
}

#[derive(Debug)]
pub enum StreamError {
    Invalid(&'static str),
    Forbidden(&'static str),
    Expired,
    Overloaded,
    Gap { minimum: i64, requested: i64 },
    Service(crate::app::ServiceError),
}

impl axum::response::IntoResponse for StreamError {
    fn into_response(self) -> axum::response::Response {
        use axum::{Json, http::StatusCode};
        let (status, body) = match self {
            Self::Invalid(message) => (
                StatusCode::BAD_REQUEST,
                serde_json::json!({"error":"invalid_cursor","message":message}),
            ),
            Self::Forbidden(message) => (
                StatusCode::FORBIDDEN,
                serde_json::json!({"error":"forbidden","message":message}),
            ),
            Self::Expired => (
                StatusCode::GONE,
                serde_json::json!({"error":"cursor_expired","resync":true}),
            ),
            Self::Overloaded => (
                StatusCode::TOO_MANY_REQUESTS,
                serde_json::json!({"error":"stream_capacity_exhausted","retryAfterMs":1000}),
            ),
            Self::Gap { minimum, requested } => (
                StatusCode::GONE,
                serde_json::json!({"error":"retention_gap","minimumWatermark":minimum,"requestedWatermark":requested,"resync":true}),
            ),
            Self::Service(error) => (
                StatusCode::SERVICE_UNAVAILABLE,
                serde_json::json!({"error":"stream_unavailable","message":error.to_string()}),
            ),
        };
        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
