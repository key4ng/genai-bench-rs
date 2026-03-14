use genai_bench_rs::cli::parse_duration;
use std::time::Duration;

#[test]
fn test_parse_seconds() {
    assert_eq!(parse_duration("60s").unwrap(), Duration::from_secs(60));
    assert_eq!(parse_duration("1s").unwrap(), Duration::from_secs(1));
}

#[test]
fn test_parse_minutes() {
    assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
    assert_eq!(parse_duration("1m").unwrap(), Duration::from_secs(60));
}

#[test]
fn test_parse_hours() {
    assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
    assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7200));
}

#[test]
fn test_parse_invalid() {
    assert!(parse_duration("").is_err());
    assert!(parse_duration("60").is_err());
    assert!(parse_duration("abc").is_err());
    assert!(parse_duration("5x").is_err());
}
