// DO NOT import src::notifications from src/syslog/, src/ingest, or src/syslog/writer — ingest isolation critical

pub mod apprise;
pub mod digest;
pub mod dispatcher;
pub mod evaluator;
pub mod rules;
