use super::*;

#[test]
fn bind_positional_returns_value_for_action_with_positional() {
    // tail binds a bare positional to --host.
    let bound = positional_value("tail", &["devhost".to_string()]).unwrap();
    assert_eq!(bound.as_deref(), Some("devhost"));
}

#[test]
fn bind_positional_none_when_no_token_given() {
    assert_eq!(positional_value("tail", &[]).unwrap(), None);
}

#[test]
fn bind_positional_errors_when_action_takes_none() {
    // hosts takes no positional; a stray arg is an error.
    let err = positional_value("hosts", &["oops".to_string()])
        .unwrap_err()
        .to_string();
    assert!(err.contains("unexpected"), "{err}");
}

#[test]
fn bind_positional_errors_when_more_than_one_given() {
    let err = positional_value("tail", &["a".to_string(), "b".to_string()])
        .unwrap_err()
        .to_string();
    assert!(err.contains("at most one"), "{err}");
}
