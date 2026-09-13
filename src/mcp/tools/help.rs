use serde_json::{Value, json};

use super::super::actions;

pub(super) async fn tool_cortex_help() -> anyhow::Result<Value> {
    let mut cheap = Vec::new();
    let mut moderate = Vec::new();
    let mut expensive = Vec::new();
    let mut write = Vec::new();
    for spec in actions::ACTION_SPECS {
        match spec.cost {
            actions::Cost::Cheap => cheap.push(spec.name),
            actions::Cost::Moderate => moderate.push(spec.name),
            actions::Cost::Expensive => expensive.push(spec.name),
            actions::Cost::Write => write.push(spec.name),
        }
    }
    let cost_guide = format!(
        r#"## Agent Planning Cost Metadata

Use action cost metadata to keep first-class agents token-efficient:
- `cheap`: {}.
- `moderate`: {}.
- `expensive`: {}.
- `write`: {}.

Recommended flow: start with cheap bounded calls, use moderate actions after
the scope is narrowed, and reserve expensive actions for a specific unanswered
question. Write actions require admin scope and must never be used for read-only
diagnosis.

"#,
        cheap.join(", "),
        moderate.join(", "),
        expensive.join(", "),
        write.join(", ")
    );
    let mut help = String::from(
        "# cortex Tool Reference\n\nThe MCP server exposes one tool, `cortex`. Set the required `action` argument to select the operation.\n\n",
    );
    for spec in actions::ACTION_SPECS {
        help.push_str("## cortex ");
        help.push_str(spec.name);
        help.push('\n');
        help.push_str(spec.description);
        help.push_str("\n\n");
        help.push_str("**Cost:** ");
        help.push_str(spec.cost.as_str());
        help.push_str("\n\n");
        match spec.input_contract {
            actions::ActionInputContract::Exact { allowed, required } => {
                help.push_str("**MCP parameters:**\n");
                for name in allowed {
                    help.push_str(&format!(
                        "- `{name}`{}\n",
                        if required.contains(name) {
                            " (required)"
                        } else {
                            " (optional)"
                        }
                    ));
                }
            }
            actions::ActionInputContract::LegacyFlat => {
                help.push_str("**MCP parameters:** See the published `cortex` tools/list input schema for field descriptions and action-specific requirements.\n");
                if spec.name == "similar_incidents" {
                    help.push_str("Required: `query`. Optional filters: `host`, `app`, `severity_min`, `since`, `until`, `window_minutes`, `limit`.\n");
                }
            }
        }
        help.push('\n');
        if !spec.flags.is_empty() {
            help.push_str("**CLI flags (not MCP field names):**\n");
            for flag in spec.flags {
                help.push_str("- `");
                help.push_str(flag.flag);
                help.push('`');
                if !flag.short.is_empty() {
                    help.push_str(" / `");
                    help.push_str(flag.short);
                    help.push('`');
                }
                help.push_str(" - ");
                help.push_str(flag.help);
                help.push('\n');
            }
            help.push('\n');
        }
        if !spec.examples.is_empty() {
            help.push_str("**Examples:**\n");
            for example in spec.examples {
                help.push_str("- `");
                help.push_str(example);
                help.push_str("`\n");
            }
            help.push('\n');
        }
        help.push_str("---\n\n");
    }
    help.push_str(&cost_guide);
    Ok(json!({ "help": help }))
}
