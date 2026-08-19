use opentelemetry_proto::tonic::trace::v1::Span;
use serde_json::{Value, json};

use crate::config::AgentObservatoryPrivacyConfig;

use super::{MAX_METADATA_JSON_BYTES, TraceNormalizeError, hex_id};
use crate::otlp::normalization::MAX_SIGNAL_ATTRIBUTES;
use crate::otlp::privacy::{private_attributes, private_text};

struct BoundedJsonArray {
    items: Vec<String>,
    encoded_bytes: usize,
    truncating: bool,
    omitted: usize,
}

impl BoundedJsonArray {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            encoded_bytes: 2,
            truncating: false,
            omitted: 0,
        }
    }

    fn push(&mut self, value: Value) {
        if self.truncating {
            self.omitted += 1;
            return;
        }
        let encoded = value.to_string();
        let separator = usize::from(!self.items.is_empty());
        if self.encoded_bytes + separator + encoded.len() > MAX_METADATA_JSON_BYTES {
            self.truncating = true;
            self.omitted += 1;
            return;
        }
        self.encoded_bytes += separator + encoded.len();
        self.items.push(encoded);
    }

    fn finish(mut self, producer_dropped: u32, kind: &'static str) -> String {
        if producer_dropped > 0 || self.omitted > 0 {
            loop {
                let diagnostic = json!({
                    "_cortex_diagnostics": {
                        "kind": kind,
                        "producer_dropped": producer_dropped,
                        "cortex_omitted": self.omitted,
                        "reason": if self.omitted > 0 { "max_serialized_bytes" } else { "producer_reported_drop" },
                        "maximum_bytes": MAX_METADATA_JSON_BYTES,
                    }
                })
                .to_string();
                let separator = usize::from(!self.items.is_empty());
                if self.encoded_bytes + separator + diagnostic.len() <= MAX_METADATA_JSON_BYTES {
                    self.encoded_bytes += separator + diagnostic.len();
                    self.items.push(diagnostic);
                    break;
                }
                if let Some(removed) = self.items.pop() {
                    self.encoded_bytes -= removed.len();
                    if !self.items.is_empty() {
                        self.encoded_bytes -= 1;
                    }
                    self.omitted += 1;
                    continue;
                }
                // The diagnostic is deliberately tiny relative to the 256 KiB
                // contract cap, so reaching this branch would require changing
                // the cap below its own metadata overhead.
                return "[]".to_string();
            }
        }
        format!("[{}]", self.items.join(","))
    }
}

pub(super) fn serialize_events(
    span: &Span,
    privacy: &AgentObservatoryPrivacyConfig,
) -> Result<String, TraceNormalizeError> {
    let mut output = BoundedJsonArray::new();
    for event in &span.events {
        if event.attributes.len() > MAX_SIGNAL_ATTRIBUTES {
            return Err(TraceNormalizeError::AttributeLimit {
                field: "event",
                actual: event.attributes.len(),
                maximum: MAX_SIGNAL_ATTRIBUTES,
            });
        }
        output.push(json!({
            "time_unix_nano": event.time_unix_nano,
            "name": private_text(&event.name),
            "attributes": private_attributes(&event.attributes, MAX_SIGNAL_ATTRIBUTES, privacy),
            "dropped_attributes_count": event.dropped_attributes_count,
        }));
    }
    Ok(output.finish(span.dropped_events_count, "events"))
}

pub(super) fn serialize_links(
    span: &Span,
    privacy: &AgentObservatoryPrivacyConfig,
) -> Result<String, TraceNormalizeError> {
    let mut output = BoundedJsonArray::new();
    for link in &span.links {
        if link.attributes.len() > MAX_SIGNAL_ATTRIBUTES {
            return Err(TraceNormalizeError::AttributeLimit {
                field: "link",
                actual: link.attributes.len(),
                maximum: MAX_SIGNAL_ATTRIBUTES,
            });
        }
        let trace_id = hex_id(&link.trace_id, 16, "link.trace_id")?;
        let span_id = hex_id(&link.span_id, 8, "link.span_id")?;
        output.push(json!({
            "trace_id": trace_id,
            "span_id": span_id,
            "trace_state": private_text(&link.trace_state),
            "attributes": private_attributes(&link.attributes, MAX_SIGNAL_ATTRIBUTES, privacy),
            "dropped_attributes_count": link.dropped_attributes_count,
            "flags": link.flags,
        }));
    }
    Ok(output.finish(span.dropped_links_count, "links"))
}
