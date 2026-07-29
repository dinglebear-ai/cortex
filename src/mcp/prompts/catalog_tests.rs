use super::*;

#[test]
fn prompt_catalog_names_are_unique() {
    let mut names: Vec<_> = PROMPTS.iter().map(|prompt| prompt.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), PROMPTS.len());
}

#[test]
fn rendered_prompts_reference_cortex_tool_actions() {
    for spec in PROMPTS {
        let (_description, messages) = get_prompt(spec.name, None).unwrap();
        // rmcp 3.0 replaced PromptMessageContent with ContentBlock, so the text
        // variant is now a tuple variant wrapping TextContent.
        let text = match &messages[0].content {
            rmcp::model::ContentBlock::Text(text) => &text.text,
            _ => panic!("expected text prompt"),
        };
        assert!(text.contains("cortex"));
        assert!(text.contains("action="));
    }
}

#[test]
fn rendered_prompts_reference_only_known_actions() {
    let known_actions = crate::mcp::actions::action_names();
    for (name, text) in rendered_prompt_texts() {
        for action in action_references(&text) {
            assert!(
                known_actions.contains(&action.as_str()),
                "{name} references unknown action={action}"
            );
        }
    }
}

#[test]
fn rendered_prompts_use_valid_timeline_bucket_guidance() {
    for (name, text) in rendered_prompt_texts() {
        assert!(
            !text.contains("bucket=5m") && !text.contains("`5m`") && !text.contains(" 5m"),
            "{name} includes invalid timeline bucket guidance"
        );

        if text.contains("action=timeline") {
            assert!(
                text.contains("bucket=minute"),
                "{name} mentions timeline without minute bucket guidance"
            );
            assert!(
                text.contains("`minute`, `hour`, and `day`"),
                "{name} does not spell out valid timeline buckets"
            );
        }
    }
}

#[test]
fn rendered_prompts_require_bounded_queries_and_common_synthesis() {
    let expected_sections = [
        "- Verdict:",
        "- Evidence:",
        "- Likely Cause:",
        "- Not Supported:",
        "- Next Actions:",
        "- Telemetry Gaps:",
    ];

    for (name, text) in rendered_prompt_texts() {
        assert!(
            text.contains("limit=5"),
            "{name} lacks small search guidance"
        );
        assert!(
            text.contains("limit=10"),
            "{name} lacks bounded query guidance"
        );
        assert!(
            text.contains("before=3") && text.contains("after=3"),
            "{name} lacks bounded context guidance"
        );
        assert!(
            text.contains("Escalate only if needed"),
            "{name} lacks cheap-first escalation guidance"
        );
        assert!(
            text.contains("Cheap first pass:"),
            "{name} lacks cheap-first pass guidance"
        );

        for section in expected_sections {
            assert!(text.contains(section), "{name} lacks section {section}");
        }
    }
}

fn rendered_prompt_texts() -> Vec<(&'static str, String)> {
    PROMPTS
        .iter()
        .map(|spec| {
            let (_description, messages) = get_prompt(spec.name, None).unwrap();
            let text = match &messages[0].content {
                rmcp::model::ContentBlock::Text(text) => text.text.clone(),
                _ => panic!("expected text prompt"),
            };
            (spec.name, text)
        })
        .collect()
}

fn action_references(text: &str) -> Vec<String> {
    let mut actions = Vec::new();
    let mut rest = text;

    while let Some(index) = rest.find("action=") {
        let after_marker = &rest[index + "action=".len()..];
        let action: String = after_marker
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
            .collect();
        if !action.is_empty() {
            actions.push(action);
        }
        rest = after_marker;
    }

    actions
}
