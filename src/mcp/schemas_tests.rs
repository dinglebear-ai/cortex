use super::*;

#[test]
fn tool_definitions_include_expected_public_tools() {
    let tools = tool_definitions();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["cortex"]);

    let action = &tools[0]["inputSchema"]["properties"]["action"];
    assert_eq!(action["type"], "string");
    let actions: Vec<&str> = action["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert_eq!(actions, super::actions::action_names());
}

#[test]
fn exact_action_contracts_are_generated_from_the_action_registry() {
    let tools = tool_definitions();
    let schema = &tools[0]["inputSchema"];
    let root_properties = schema["properties"].as_object().unwrap();
    let branches = schema["oneOf"].as_array().expect("action contract oneOf");

    let exact_branch = |action: &str| {
        branches
            .iter()
            .find(|branch| branch["properties"]["action"]["const"] == action)
            .unwrap_or_else(|| panic!("missing exact branch for {action}"))
    };

    let project_context = exact_branch("project_context");
    assert_eq!(project_context["additionalProperties"], false);
    let project_fields = project_context["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        project_fields,
        ["action", "limit", "project", "tool"].into_iter().collect()
    );
    assert_eq!(
        project_context["required"],
        serde_json::json!(["action", "project"])
    );
    for field in ["project", "tool", "limit"] {
        assert_eq!(project_context["properties"][field], root_properties[field]);
    }

    let list_ai_projects = exact_branch("list_ai_projects");
    assert_eq!(list_ai_projects["additionalProperties"], false);
    let list_fields = list_ai_projects["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        list_fields,
        ["action", "since", "tool", "until"].into_iter().collect()
    );
    assert_eq!(list_ai_projects["required"], serde_json::json!(["action"]));
    for field in ["tool", "since", "until"] {
        assert_eq!(
            list_ai_projects["properties"][field],
            root_properties[field]
        );
    }

    let fallback = branches
        .iter()
        .find(|branch| branch["properties"]["action"]["enum"].is_array())
        .expect("legacy fallback branch");
    let legacy_actions = fallback["properties"]["action"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(!legacy_actions.contains("project_context"));
    assert!(!legacy_actions.contains("list_ai_projects"));
    assert!(legacy_actions.contains("sessions"));
    assert!(legacy_actions.contains("search_sessions"));

    for action in super::actions::action_names() {
        let exact_count = branches
            .iter()
            .filter(|branch| branch["properties"]["action"]["const"] == action)
            .count();
        let fallback_count = usize::from(legacy_actions.contains(action));
        assert_eq!(
            exact_count + fallback_count,
            1,
            "action {action} must appear in exactly one contract branch"
        );
    }
}

#[test]
fn exact_action_contracts_accept_only_runtime_request_fields() {
    use super::actions::ActionInputContract;

    let accepts = |action: &str, fields: &[&str]| {
        let spec = super::actions::ACTION_SPECS
            .iter()
            .find(|spec| spec.name == action)
            .unwrap();
        let ActionInputContract::Exact { allowed, required } = spec.input_contract else {
            panic!("{action} must use an exact contract");
        };
        required.iter().all(|field| fields.contains(field))
            && fields
                .iter()
                .all(|field| *field == "action" || allowed.contains(field))
    };

    assert!(accepts(
        "project_context",
        &["action", "project", "tool", "limit"]
    ));
    assert!(!accepts("project_context", &["action", "project", "since"]));
    assert!(!accepts("project_context", &["action", "tool"]));

    assert!(accepts(
        "list_ai_projects",
        &["action", "tool", "since", "until"]
    ));
    assert!(!accepts("list_ai_projects", &["action", "limit"]));
    assert!(!accepts("list_ai_projects", &["action", "project", "tool"]));

    assert!(accepts(
        "artifact_evidence",
        &[
            "action",
            "artifactId",
            "correlationId",
            "since",
            "until",
            "limit"
        ]
    ));
    assert!(!accepts(
        "artifact_evidence",
        &["action", "artifactId", "metadata"]
    ));

    assert!(accepts(
        "artifact_evidence_record",
        &[
            "action",
            "schemaVersion",
            "eventId",
            "eventKind",
            "sourceSystem",
            "sourceIssuer",
            "observedAt",
            "artifactId",
        ]
    ));
    assert!(!accepts(
        "artifact_evidence_record",
        &[
            "action",
            "schemaVersion",
            "eventId",
            "eventKind",
            "sourceSystem",
            "sourceIssuer",
            "artifactId",
        ]
    ));
}

#[test]
fn event_actions_advertise_canonical_mcp_time_bounds() {
    let properties = &tool_definitions()[0]["inputSchema"]["properties"];
    assert!(properties.get("since").is_some());
    assert!(properties.get("until").is_some());
    assert!(properties.get("from").is_none());
    assert!(properties.get("to").is_none());

    for action in ["skill_events", "mcp_events", "hook_events"] {
        assert!(super::actions::action_names().contains(&action));
    }
}

#[test]
fn tool_definition_exposes_agent_cost_metadata() {
    let tools = tool_definitions();
    let metadata = tools[0]["x-cortex-action-metadata"].as_array().unwrap();
    assert_eq!(metadata.len(), super::actions::ACTION_SPECS.len());
    assert!(
        metadata
            .iter()
            .any(|action| { action["name"] == "status" && action["cost"] == "cheap" })
    );
    assert!(
        metadata
            .iter()
            .any(|action| { action["name"] == "patterns" && action["cost"] == "expensive" })
    );
    assert_eq!(
        tools[0]["x-cortex-agent-guidance"]["default_bounds"]["timeline_bucket"],
        "minute"
    );
}

#[test]
fn schema_source_ips_exposes_pagination() {
    let tools = tool_definitions();
    let props = &tools[0]["inputSchema"]["properties"];
    let limit_desc = props["limit"]["description"].as_str().unwrap();
    let offset_desc = props["offset"]["description"].as_str().unwrap();
    assert!(
        limit_desc.contains("source_ips"),
        "limit description must document source_ips page size"
    );
    assert!(
        limit_desc.contains("total"),
        "limit description must mention total count in source_ips response"
    );
    assert!(
        offset_desc.contains("source_ips"),
        "offset description must mention source_ips"
    );
    assert!(
        offset_desc.contains("paginate") || offset_desc.contains("page"),
        "offset description must explain pagination usage"
    );
}

#[test]
fn schema_exposes_topic_correlate_fields() {
    let tools = tool_definitions();
    let schema = &tools[0]["inputSchema"];
    let props = &schema["properties"];

    assert_eq!(props["topic"]["type"], "string");
    assert!(
        props["topic"]["description"]
            .as_str()
            .unwrap()
            .contains("topic_correlate")
    );
    assert!(props["source_kinds"]["oneOf"].is_array());
    assert!(
        props["source_kinds"]["description"]
            .as_str()
            .unwrap()
            .contains("syslog-udp")
    );
    assert!(
        !props["source_kinds"]["description"]
            .as_str()
            .unwrap()
            .contains("graph_evidence")
    );
    assert_eq!(props["depth"]["maximum"], 6);

    let topic_rule = schema["allOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| rule["if"]["properties"]["action"]["const"] == "topic_correlate")
        .expect("topic_correlate conditional");
    assert!(
        topic_rule["then"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "topic")
    );
}

#[test]
fn schema_usage_blocks_accepts_limit() {
    let tools = tool_definitions();
    let props = &tools[0]["inputSchema"]["properties"];
    let limit_desc = props["limit"]["description"].as_str().unwrap();
    assert!(
        limit_desc.contains("usage_blocks"),
        "limit description must document usage_blocks"
    );
}

#[test]
fn schema_includes_file_tails_action() {
    let tool = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "cortex")
        .expect("cortex tool");
    let schema = serde_json::to_value(tool["inputSchema"].clone()).unwrap();
    let properties = &schema["properties"];
    let action_enum = properties["action"]["enum"].as_array().unwrap();
    assert!(action_enum.iter().any(|value| value == "file_tails"));
    assert_eq!(
        properties["op"]["description"],
        "For action=file_tails: required operation, one of list, add, remove, enable, disable, or status."
    );
    assert_eq!(
        properties["op"]["enum"],
        serde_json::json!(["list", "add", "remove", "enable", "disable", "status"])
    );
    let source_kind_enum = properties["source_kind"]["enum"].as_array().unwrap();
    assert!(source_kind_enum.iter().any(|value| value == "file-tail"));

    let all_of = schema["allOf"].as_array().unwrap();
    assert!(all_of.iter().any(|rule| {
        rule["if"]["properties"]["action"]["const"] == "get"
            && rule["then"]["properties"]["id"]["type"] == "integer"
            && rule["then"]["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == "id")
    }));
    let file_tails_rule = all_of
        .iter()
        .find(|rule| rule["if"]["properties"]["action"]["const"] == "file_tails")
        .expect("file_tails conditional");
    assert_eq!(
        file_tails_rule["then"]["properties"]["id"]["type"],
        "string"
    );
    assert_eq!(
        file_tails_rule["then"]["properties"]["op"]["enum"],
        serde_json::json!(["list", "add", "remove", "enable", "disable", "status"])
    );
    assert!(
        file_tails_rule["then"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "op")
    );
    let nested = file_tails_rule["then"]["allOf"].as_array().unwrap();
    assert!(nested.iter().any(|rule| {
        let required = rule["then"]["required"].as_array().unwrap();
        rule["if"]["properties"]["op"]["const"] == "add"
            && required.as_slice() == [serde_json::json!("path")]
    }));
    assert!(nested.iter().any(|rule| {
        rule["if"]["properties"]["op"]["enum"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == "remove"))
            && rule["then"]["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == "id")
    }));
}

#[test]
fn schema_apps_exposes_pagination_and_total() {
    let tools = tool_definitions();
    let props = &tools[0]["inputSchema"]["properties"];
    let from_desc = props["since"]["description"].as_str().unwrap();
    let to_desc = props["until"]["description"].as_str().unwrap();
    let limit_desc = props["limit"]["description"].as_str().unwrap();
    let offset_desc = props["offset"]["description"].as_str().unwrap();
    assert!(
        from_desc.contains("apps"),
        "from description must include apps action"
    );
    assert!(
        to_desc.contains("apps"),
        "to description must include apps action"
    );
    assert!(
        limit_desc.contains("apps"),
        "limit description must document apps page size"
    );
    assert!(
        limit_desc.contains("total"),
        "limit description must mention total count in apps response"
    );
    assert!(
        offset_desc.contains("apps"),
        "offset description must mention apps"
    );
}

#[test]
fn schema_graph_exposes_lookup_and_neighborhood_arguments() {
    let tools = tool_definitions();
    let props = &tools[0]["inputSchema"]["properties"];
    assert_eq!(tools[0]["inputSchema"]["additionalProperties"], false);
    assert_eq!(props["entity_id"]["minimum"], 1);
    assert_eq!(props["depth"]["minimum"], 1);
    assert_eq!(props["depth"]["maximum"], 6);
    for name in [
        "mode",
        "entity_id",
        "entity_type",
        "key",
        "alias_type",
        "alias_key",
        "depth",
        "evidence_sample_limit",
        "payload_budget",
    ] {
        let desc = props[name]["description"].as_str().unwrap_or("");
        assert!(
            desc.contains("action=graph"),
            "{name} description must document graph usage: {desc}"
        );
    }
    assert!(
        props["limit"]["description"]
            .as_str()
            .unwrap()
            .contains("action=graph"),
        "limit description must document graph caps"
    );
}

#[test]
fn schema_graph_target_constraints_match_runtime_validation() {
    let tools = tool_definitions();
    let constraints = tools[0]["inputSchema"]["allOf"].as_array().unwrap();
    let graph_then = constraints
        .iter()
        .find(|constraint| constraint["if"]["properties"]["action"]["const"] == "graph")
        .map(|constraint| &constraint["then"])
        .unwrap();
    assert_eq!(graph_then["properties"]["depth"]["maximum"], 3);

    let target_or_evidence = graph_then["oneOf"].as_array().unwrap();
    assert_eq!(target_or_evidence.len(), 2);
    let target_strategies = target_or_evidence[0]["oneOf"].as_array().unwrap();
    assert_eq!(target_strategies.len(), 3);
    assert_eq!(target_strategies[0]["required"][0], "entity_id");
    assert_eq!(target_strategies[1]["required"][0], "entity_type");
    assert_eq!(target_strategies[1]["required"][1], "key");
    assert_eq!(target_strategies[2]["required"][0], "alias_type");
    assert_eq!(target_strategies[2]["required"][1], "alias_key");
    assert_eq!(target_or_evidence[1]["required"][0], "mode");
    assert_eq!(target_or_evidence[1]["required"][1], "evidence_id");

    let depth_constraints = graph_then["allOf"].as_array().unwrap();
    assert!(depth_constraints.iter().any(|constraint| {
        constraint["then"]["properties"]["depth"]["maximum"] == 1
            && constraint["then"]["properties"]["depth"]["minimum"] == 1
    }));
}

#[test]
fn schema_mode_constraints_are_action_specific() {
    let tools = tool_definitions();
    let schema = &tools[0]["inputSchema"];
    let constraints = schema["allOf"].as_array().unwrap();

    let graph_modes = constraints
        .iter()
        .find(|constraint| constraint["if"]["properties"]["action"]["const"] == "graph")
        .and_then(|constraint| constraint["then"]["properties"]["mode"]["enum"].as_array())
        .unwrap();
    assert!(graph_modes.iter().any(|mode| mode == "around"));
    assert!(!graph_modes.iter().any(|mode| mode == "host_services"));

    let map_modes = constraints
        .iter()
        .find(|constraint| constraint["if"]["properties"]["action"]["const"] == "map")
        .and_then(|constraint| constraint["then"]["properties"]["mode"]["enum"].as_array())
        .unwrap();
    assert!(map_modes.iter().any(|mode| mode == "host_services"));
    assert!(!map_modes.iter().any(|mode| mode == "around"));
}

#[test]
fn schema_map_findings_exposes_findings_arguments() {
    let tools = tool_definitions();
    let props = &tools[0]["inputSchema"]["properties"];

    assert!(
        props["mode"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .any(|mode| mode == "findings")
    );
    assert_eq!(props["finding_limit"]["minimum"], 1);
    assert_eq!(props["finding_limit"]["maximum"], 100);
    assert_eq!(props["evidence_per_finding"]["minimum"], 1);
    assert_eq!(props["evidence_per_finding"]["maximum"], 5);
    assert!(
        props["payload_budget"]["description"]
            .as_str()
            .unwrap()
            .contains("mode=findings")
    );
    assert_eq!(
        props["finding_types"]["items"]["enum"],
        serde_json::json!(crate::app::topology_findings::TYPES)
    );
    assert!(
        props["finding_types"]["description"]
            .as_str()
            .unwrap()
            .contains("supported finding types")
    );
}

#[test]
fn schema_documents_bounded_timeline_and_patterns_defaults() {
    let tools = tool_definitions();
    let props = &tools[0]["inputSchema"]["properties"];
    let from_desc = props["since"]["description"].as_str().unwrap();
    assert_eq!(
        props["scan_limit"]["maximum"],
        crate::db::PATTERN_SCAN_LIMIT_MAX
    );
    assert!(
        props["scan_limit"]["description"]
            .as_str()
            .unwrap()
            .contains("max 10000")
    );
    assert!(from_desc.contains("24 hours ago for patterns"));
    assert!(from_desc.contains("Timeline applies a bucket-sized default lookback"));
}

#[test]
fn schema_source_kinds_enum_matches_source_kind_wire_names() {
    let tools = tool_definitions();
    let one_of = tools[0]["inputSchema"]["properties"]["source_kinds"]["oneOf"]
        .as_array()
        .unwrap();
    let expected = crate::enrich::parser::SourceKind::all_wire_names();

    // Both the array-items branch and the string branch must be generated from
    // SourceKind so the wire schema can never drift from the enum.
    let array_enum: Vec<&str> = one_of[0]["items"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert_eq!(array_enum, expected);

    let string_enum: Vec<&str> = one_of[1]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert_eq!(string_enum, expected);
}

// Eng review fix (agent-native-reviewer): `since`/`limit` enumerate every
// action that consumes them by name, but omitted `llm_invocations` even
// though this PR added it as a filterable action. There was also no
// `status` schema property at all, so an MCP client introspecting the
// tool schema could not discover that `llm_invocations` accepts a
// `status` filter.
#[test]
fn schema_llm_invocations_since_and_limit_are_documented() {
    let tools = tool_definitions();
    let props = &tools[0]["inputSchema"]["properties"];
    let since_desc = props["since"]["description"].as_str().unwrap();
    let limit_desc = props["limit"]["description"].as_str().unwrap();
    assert!(
        since_desc.contains("llm_invocations"),
        "since description must document action=llm_invocations, got: {since_desc}"
    );
    assert!(
        limit_desc.contains("llm_invocations"),
        "limit description must document action=llm_invocations, got: {limit_desc}"
    );
}

#[test]
fn schema_exposes_llm_invocations_status_property() {
    let tools = tool_definitions();
    let props = &tools[0]["inputSchema"]["properties"];
    let status = &props["status"];
    assert_eq!(status["type"], "string");
    let status_desc = status["description"].as_str().unwrap();
    assert!(
        status_desc.contains("llm_invocations"),
        "status description must document action=llm_invocations, got: {status_desc}"
    );
    // Verify the documented status values match the exact set written by
    // LlmRunner::run/dry_run (src/app/llm_runner.rs write_start_row/
    // write_finish_row call sites) — not a guessed superset.
    for expected_status in [
        "running",
        "success",
        "error",
        "denied",
        "rate_limited",
        "circuit_open",
        "disabled",
        "dry_run",
        "timeout",
    ] {
        assert!(
            status_desc.contains(expected_status),
            "status description must mention '{expected_status}', got: {status_desc}"
        );
    }
}

#[test]
fn schema_graph_entity_types_expose_resolver_identity_not_legacy_service() {
    let tools = tool_definitions();
    let entity_type = &tools[0]["inputSchema"]["properties"]["entity_type"];
    let values: Vec<&str> = entity_type["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert!(values.contains(&"logical_service"));
    assert!(values.contains(&"service_instance"));
    for extended in ["git_commit", "user", "device"] {
        assert!(
            values.contains(&extended),
            "entity_type enum must include `{extended}`"
        );
    }
    assert!(
        !values.contains(&"service"),
        "legacy `service` must not be offered as a supported graph identity input"
    );
    let desc = entity_type["description"].as_str().unwrap();
    assert!(desc.contains("logical_service"));
    assert!(desc.contains("service_instance"));
    assert!(desc.contains("rejected_legacy_shape"));
}
