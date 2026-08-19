use super::agent_observatory::{
    AgentEventKind, EvidenceTrustLevel, RepositoryObservationKind, RunStatus, StreamEventName,
};
use std::str::FromStr;

#[test]
fn observatory_text_enums_round_trip_and_reject_unknown_values() {
    for value in RunStatus::ALL {
        assert_eq!(RunStatus::from_str(value.as_str()).unwrap(), *value);
    }
    for value in AgentEventKind::ALL {
        assert_eq!(AgentEventKind::from_str(value.as_str()).unwrap(), *value);
    }
    for value in EvidenceTrustLevel::ALL {
        assert_eq!(
            EvidenceTrustLevel::from_str(value.as_str()).unwrap(),
            *value
        );
    }
    for value in RepositoryObservationKind::ALL {
        assert_eq!(
            RepositoryObservationKind::from_str(value.as_str()).unwrap(),
            *value
        );
    }
    for value in StreamEventName::ALL {
        assert_eq!(StreamEventName::from_str(value.as_str()).unwrap(), *value);
    }
    assert!(RunStatus::from_str("running").is_err());
    assert!(AgentEventKind::from_str("unknown").is_err());
    assert!(EvidenceTrustLevel::from_str("trusted").is_err());
    assert!(RepositoryObservationKind::from_str("poll").is_err());
    assert!(StreamEventName::from_str("run.deleted").is_err());
}
