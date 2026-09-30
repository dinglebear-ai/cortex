//! Logging initialization — Aurora console formatter + JSON file layer.
//!
//! Aurora color palette for console output is defined in `aurora.rs`.
//! Colors match `lab/crates/lab/src/output/theme.rs` exactly.

pub mod aurora;

use std::io::IsTerminal;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use opentelemetry::{KeyValue, trace::TracerProvider};
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::{
    Resource,
    trace::{Sampler, SdkTracerProvider},
};
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Initialize tracing with:
/// - Console layer: Aurora-colored output to stderr (color auto-detected)
/// - File layer: JSON to `{data_dir}/logs/cortex.log` (if writable)
///
/// Returns a string describing the active log filter for startup log output.
pub fn init(default_filter: &str) -> String {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let filter_str = filter.to_string();

    let colorize = should_colorize();

    let console = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(colorize)
        .with_target(false)
        .event_format(AuroraLevelFormatter);

    tracing_subscriber::registry()
        .with(console.with_filter(filter))
        .init();

    filter_str
}

/// Enable bounded, opt-in HTTP request traces for the server only.
/// The exporter uses Cortex's existing machine token and a loopback OTLP route.
pub fn init_server(default_filter: &str) -> Result<Option<SdkTracerProvider>> {
    let Ok(endpoint) = std::env::var("CORTEX_TRACE_OTLP_ENDPOINT") else {
        init(default_filter);
        return Ok(None);
    };
    let endpoint = validate_trace_endpoint(&endpoint)?;
    let token = std::env::var("CORTEX_TOKEN")
        .context("CORTEX_TRACE_OTLP_ENDPOINT requires CORTEX_TOKEN")?;
    if token.is_empty() {
        bail!("CORTEX_TRACE_OTLP_ENDPOINT requires a nonempty CORTEX_TOKEN");
    }
    let host_name = std::env::var("CORTEX_TRACE_HOST_NAME")
        .context("CORTEX_TRACE_OTLP_ENDPOINT requires CORTEX_TRACE_HOST_NAME")?;
    if host_name.trim().is_empty() {
        bail!("CORTEX_TRACE_HOST_NAME must be nonempty");
    }
    let sample_ratio =
        trace_sample_ratio(std::env::var("CORTEX_TRACE_SAMPLE_RATIO").ok().as_deref())?;

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint.as_str())
        .with_timeout(Duration::from_secs(5))
        .with_headers(std::collections::HashMap::from([(
            "Authorization".to_string(),
            format!("Bearer {token}"),
        )]))
        .build()
        .context("failed to initialize Cortex OTLP trace exporter")?;
    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_sampler(Sampler::TraceIdRatioBased(sample_ratio))
        .with_resource(
            Resource::builder()
                .with_service_name("cortex")
                .with_attributes([KeyValue::new("host.name", host_name)])
                .build(),
        )
        .build();
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let console = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(should_colorize())
        .with_target(false)
        .event_format(AuroraLevelFormatter);
    let otel = tracing_opentelemetry::layer()
        .with_tracer(provider.tracer("cortex.http"))
        .with_location(false)
        .with_threads(false)
        .with_target(false)
        .with_tracked_inactivity(false)
        .with_filter(filter_fn(|metadata| {
            metadata.is_span() && metadata.target() == "cortex_http"
        }));
    tracing_subscriber::registry()
        .with(console.with_filter(filter))
        .with(otel)
        .init();
    Ok(Some(provider))
}

fn validate_trace_endpoint(raw: &str) -> Result<url::Url> {
    let endpoint = url::Url::parse(raw).context("invalid CORTEX_TRACE_OTLP_ENDPOINT URL")?;
    if endpoint.scheme() != "http"
        || !matches!(
            endpoint.host_str(),
            Some("127.0.0.1" | "localhost" | "[::1]")
        )
        || endpoint.path() != "/v1/traces"
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
    {
        bail!(
            "CORTEX_TRACE_OTLP_ENDPOINT must be a loopback HTTP /v1/traces URL without credentials or query"
        );
    }
    Ok(endpoint)
}

fn trace_sample_ratio(raw: Option<&str>) -> Result<f64> {
    let ratio = raw
        .unwrap_or("0.1")
        .parse::<f64>()
        .context("invalid CORTEX_TRACE_SAMPLE_RATIO")?;
    if !ratio.is_finite() || !(0.0..=1.0).contains(&ratio) {
        bail!("CORTEX_TRACE_SAMPLE_RATIO must be between 0 and 1");
    }
    Ok(ratio)
}

#[cfg(test)]
#[path = "logging_tests.rs"]
mod tests;

/// Returns whether stderr should emit ANSI escape codes.
///
/// Delegates to the unified [`crate::color_policy`] so the `--color` override
/// (set once at startup) plus `NO_COLOR` / `FORCE_COLOR` / `CLICOLOR_FORCE` and
/// TTY detection govern the tracing console the same as every other surface.
pub fn should_colorize() -> bool {
    crate::color_policy::resolve(std::io::stderr().is_terminal())
}

// ── Minimal level-only formatter ─────────────────────────────────────────────
//
// Uses aurora ANSI 256 level colors while keeping the rest of the default
// tracing_subscriber format (timestamp, target, fields).

use std::fmt as stdfmt;
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::{
    FmtContext,
    format::{FormatEvent, FormatFields, Writer},
};
use tracing_subscriber::registry::LookupSpan;

struct AuroraLevelFormatter;

impl<S, N> FormatEvent<S, N> for AuroraLevelFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> stdfmt::Result {
        let ansi = writer.has_ansi_escapes();
        let level = *event.metadata().level();

        // Timestamp (HH:MM:SS local)
        let ts = chrono::Local::now().format("%H:%M:%S").to_string();
        if ansi {
            write!(writer, "{}  ", aurora::dim(&ts))?;
        } else {
            write!(writer, "{ts}  ")?;
        }

        // Level — Aurora colors
        let level_str = if ansi {
            match level {
                tracing::Level::ERROR => aurora::bold(aurora::ERROR, "ERROR"),
                tracing::Level::WARN => aurora::bold(aurora::WARN, " WARN"),
                tracing::Level::INFO => " INFO".to_string(),
                tracing::Level::DEBUG => aurora::dim("DEBUG"),
                tracing::Level::TRACE => aurora::dim("TRACE"),
            }
        } else {
            match level {
                tracing::Level::ERROR => "ERROR".to_string(),
                tracing::Level::WARN => " WARN".to_string(),
                tracing::Level::INFO => " INFO".to_string(),
                tracing::Level::DEBUG => "DEBUG".to_string(),
                tracing::Level::TRACE => "TRACE".to_string(),
            }
        };
        write!(writer, "{level_str}  ")?;

        // Message + fields (delegated to the default field formatter)
        ctx.format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}
