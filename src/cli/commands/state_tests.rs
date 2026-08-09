use super::*;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn parse_state_host() {
    let command = parse_state(&strings(&["host", "devhost", "--limit", "3", "--json"])).unwrap();

    match command {
        CliCommand::State(StateCommand::Host(args)) => {
            assert_eq!(args.host.as_deref(), Some("devhost"));
            assert_eq!(args.limit, Some(3));
            assert!(args.json);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_state_fleet_and_clock_skew() {
    let fleet = parse_state(&strings(&["fleet", "--exclude-ok", "--sort", "pressure"])).unwrap();
    match fleet {
        CliCommand::State(StateCommand::Fleet(args)) => {
            assert_eq!(args.include_ok, Some(false));
            assert_eq!(args.sort.as_deref(), Some("pressure"));
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let clock = parse_state(&strings(&["clockskew", "--limit", "10", "--json"])).unwrap();
    match clock {
        CliCommand::State(StateCommand::ClockSkew(args)) => {
            assert_eq!(args.limit, Some(10));
            assert!(args.json);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_state_rejects_unknown_subcommand() {
    let err = parse_state(&strings(&["memory"])).unwrap_err().to_string();

    assert!(
        err.contains("unknown state subcommand: memory"),
        "got: {err}"
    );
}
