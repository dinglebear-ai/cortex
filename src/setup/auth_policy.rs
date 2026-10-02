//! Setup uses the same boolean spellings and auth alias precedence as runtime.
use std::{collections::BTreeMap, io};

pub(crate) fn enabled(value: Option<&str>) -> bool {
    value
        .and_then(crate::config::parse_env_bool)
        .unwrap_or(false)
}

pub(crate) fn no_auth(values: &BTreeMap<String, String>) -> io::Result<bool> {
    let mut result = false;
    for key in ["NO_AUTH", "CORTEX_NO_AUTH"] {
        if let Some(value) = values.get(key).filter(|value| !value.is_empty()) {
            result = crate::config::parse_env_bool(value).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{key} requires a boolean value"),
                )
            })?;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authentication_aliases_follow_runtime_spelling_and_precedence() {
        for (alias, canonical, expected) in [
            (None, Some("true"), true),
            (Some("1"), None, true),
            (Some("true"), Some("false"), false),
            (Some("false"), Some("YES"), true),
            (Some("on"), Some(""), true),
        ] {
            let mut values = BTreeMap::new();
            if let Some(value) = alias {
                values.insert("NO_AUTH".into(), value.into());
            }
            if let Some(value) = canonical {
                values.insert("CORTEX_NO_AUTH".into(), value.into());
            }
            assert_eq!(no_auth(&values).unwrap(), expected);
        }
        assert!(no_auth(&[("CORTEX_NO_AUTH".into(), "invalid".into())].into()).is_err());
    }
}
