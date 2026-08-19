//! Agent Observatory privacy policy for OTLP attributes.
//!
//! OTLP is a generic carrier, so semantic-convention content and arbitrary
//! provider attributes can contain prompts, tool payloads, identities, paths,
//! or credentials. This module applies the configured privacy policy before
//! trace/metric payloads are persisted while reusing Cortex's generic secret
//! scrubbing and metadata bounds.

use std::collections::BTreeMap;

use opentelemetry_proto::tonic::common::v1::{
    AnyValue, KeyValue, any_value::Value as AnyValueKind,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::AgentObservatoryPrivacyConfig;
use crate::ingest_metadata::attrs_to_metadata_object_with_limit;
use crate::receiver::enrichment::scrub_ai_message;

use super::normalization::attr_key;

const REDACTED: &str = "[REDACTED]";
const MAX_NESTED_FIELDS: usize = 128;

pub(crate) fn private_text(value: &str) -> String {
    scrub_ai_message(value, None)
}

pub(crate) fn private_attributes(
    kvs: &[KeyValue],
    max_fields: usize,
    privacy: &AgentObservatoryPrivacyConfig,
) -> Value {
    let attrs: BTreeMap<&str, &AnyValue> = kvs
        .iter()
        .filter_map(|kv| {
            let key = attr_key(kv)?;
            kv.value.as_ref().map(|value| (key, value))
        })
        .collect();
    attrs_to_metadata_object_with_limit(
        attrs
            .into_iter()
            .map(|(key, value)| (key, private_attribute_value(key, value, privacy))),
        max_fields,
    )
}

fn private_attribute_value(
    key: &str,
    value: &AnyValue,
    privacy: &AgentObservatoryPrivacyConfig,
) -> Value {
    let normalized = key.to_ascii_lowercase();
    if is_prompt_content_key(&normalized) && !privacy.include_prompt_content
        || is_tool_content_key(&normalized) && !privacy.include_tool_content
        || is_command_content_key(&normalized) && !privacy.include_command_content
        || is_path_key(&normalized) && !privacy.include_paths
    {
        return Value::String(REDACTED.to_string());
    }
    if is_email_key(&normalized) && privacy.hash_email {
        return email_hash(value);
    }
    if is_user_identity_key(&normalized) && !privacy.include_user_identity {
        return Value::String(REDACTED.to_string());
    }
    private_any_value(value, privacy)
}

fn private_any_value(value: &AnyValue, privacy: &AgentObservatoryPrivacyConfig) -> Value {
    match value.value.as_ref() {
        Some(AnyValueKind::StringValue(value)) => Value::String(private_text(value)),
        Some(AnyValueKind::BoolValue(value)) => Value::Bool(*value),
        Some(AnyValueKind::IntValue(value)) => Value::Number((*value).into()),
        Some(AnyValueKind::DoubleValue(value)) => serde_json::Number::from_f64(*value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Some(AnyValueKind::BytesValue(value)) => serde_json::json!({"bytes_len": value.len()}),
        Some(AnyValueKind::ArrayValue(value)) => private_array(&value.values, privacy),
        Some(AnyValueKind::KvlistValue(value)) => private_kvlist(&value.values, privacy),
        Some(AnyValueKind::StringValueStrindex(index)) => {
            serde_json::json!({"string_table_index": index})
        }
        None => Value::Null,
    }
}

fn private_array(values: &[AnyValue], privacy: &AgentObservatoryPrivacyConfig) -> Value {
    if values.len() <= MAX_NESTED_FIELDS {
        return Value::Array(
            values
                .iter()
                .map(|value| private_any_value(value, privacy))
                .collect(),
        );
    }
    let kept = MAX_NESTED_FIELDS.saturating_sub(1);
    let mut output = values
        .iter()
        .take(kept)
        .map(|value| private_any_value(value, privacy))
        .collect::<Vec<_>>();
    output.push(serde_json::json!({
        "_cortex_diagnostics": {
            "kind": "array",
            "cortex_omitted": values.len() - kept,
            "reason": "max_nested_items",
            "maximum_items": MAX_NESTED_FIELDS,
        }
    }));
    Value::Array(output)
}

fn private_kvlist(kvs: &[KeyValue], privacy: &AgentObservatoryPrivacyConfig) -> Value {
    let attrs: BTreeMap<&str, &AnyValue> = kvs
        .iter()
        .filter_map(|kv| {
            let key = attr_key(kv)?;
            kv.value.as_ref().map(|value| (key, value))
        })
        .collect();
    attrs_to_metadata_object_with_limit(
        attrs
            .into_iter()
            .map(|(key, value)| (key, private_attribute_value(key, value, privacy))),
        MAX_NESTED_FIELDS,
    )
}

fn email_hash(value: &AnyValue) -> Value {
    let Some(AnyValueKind::StringValue(value)) = value.value.as_ref() else {
        return Value::String(REDACTED.to_string());
    };
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Value::String(REDACTED.to_string());
    }
    Value::String(format!(
        "sha256:{:x}",
        Sha256::digest(normalized.as_bytes())
    ))
}

fn is_prompt_content_key(key: &str) -> bool {
    matches!(
        key,
        "gen_ai.input.messages"
            | "gen_ai.output.messages"
            | "gen_ai.system_instructions"
            | "gen_ai.prompt"
            | "gen_ai.completion"
            | "gen_ai.prompt.template"
            | "llm.prompts"
            | "llm.completions"
    ) || key.starts_with("gen_ai.prompt.variable.")
}

fn is_tool_content_key(key: &str) -> bool {
    matches!(
        key,
        "gen_ai.tool.call.arguments" | "gen_ai.tool.call.result" | "gen_ai.tool.definitions"
    )
}

fn is_command_content_key(key: &str) -> bool {
    matches!(
        key,
        "process.command" | "process.command_line" | "shell.command" | "command"
    )
}

fn is_path_key(key: &str) -> bool {
    key.ends_with(".path")
        || matches!(
            key,
            "session.cwd" | "codebase.root_path" | "project.path" | "cwd" | "working_directory"
        )
}

fn is_email_key(key: &str) -> bool {
    key == "email" || key.ends_with(".email") || key.ends_with("_email")
}

fn is_user_identity_key(key: &str) -> bool {
    matches!(
        key,
        "user.id"
            | "user.name"
            | "user.full_name"
            | "user.email"
            | "enduser.id"
            | "enduser.role"
            | "enduser.scope"
            | "user_id"
            | "username"
    )
}

#[cfg(test)]
#[path = "privacy_tests.rs"]
mod tests;
