use std::collections::BTreeMap;

pub(super) fn substitute_nginx_variables(
    value: &str,
    variables: &BTreeMap<String, String>,
) -> String {
    let mut resolved = value.to_string();
    for _ in 0..8 {
        let (next, changed) = substitute_once(&resolved, variables);
        resolved = next;
        if !changed {
            break;
        }
    }
    resolved
}

fn substitute_once(value: &str, variables: &BTreeMap<String, String>) -> (String, bool) {
    let bytes = value.as_bytes();
    let mut output = String::with_capacity(value.len());
    let mut index = 0;
    let mut changed = false;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            let ch = value[index..].chars().next().expect("valid UTF-8 boundary");
            output.push(ch);
            index += ch.len_utf8();
            continue;
        }

        let token_start = index;
        index += 1;
        let (name_start, name_end, token_end) = if bytes.get(index) == Some(&b'{') {
            let name_start = index + 1;
            let Some(relative_end) = bytes[name_start..].iter().position(|byte| *byte == b'}')
            else {
                output.push_str(&value[token_start..]);
                break;
            };
            let name_end = name_start + relative_end;
            (name_start, name_end, name_end + 1)
        } else {
            let name_start = index;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            (name_start, index, index)
        };

        if name_start == name_end {
            output.push('$');
            continue;
        }
        let name = &value[name_start..name_end];
        if let Some(replacement) = variables.get(name) {
            output.push_str(replacement);
            changed = true;
        } else {
            output.push_str(&value[token_start..token_end]);
        }
        index = token_end;
    }
    (output, changed)
}
