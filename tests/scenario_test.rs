use genai_bench_rs::scenario::{parse_scenario, Scenario};

#[test]
fn test_parse_deterministic_scenario() {
    let s = parse_scenario("D(100,100)").unwrap();
    assert_eq!(s.name(), "D(100,100)");
    let (input, output) = s.sample();
    assert_eq!(input, 100);
    assert_eq!(output, 100);
}

#[test]
fn test_parse_deterministic_with_spaces() {
    let s = parse_scenario("D(100, 200)").unwrap();
    let (input, output) = s.sample();
    assert_eq!(input, 100);
    assert_eq!(output, 200);
}

#[test]
fn test_parse_deterministic_large_values() {
    let s = parse_scenario("D(7200,1000)").unwrap();
    let (input, output) = s.sample();
    assert_eq!(input, 7200);
    assert_eq!(output, 1000);
}

#[test]
fn test_parse_invalid_scenario_type() {
    let result = parse_scenario("X(100,100)");
    assert!(result.is_err());
}

#[test]
fn test_parse_invalid_format() {
    assert!(parse_scenario("D(100)").is_err());
    assert!(parse_scenario("D(,100)").is_err());
    assert!(parse_scenario("D(100,)").is_err());
    assert!(parse_scenario("").is_err());
    assert!(parse_scenario("D").is_err());
}

#[test]
fn test_deterministic_always_same() {
    let s = parse_scenario("D(50,75)").unwrap();
    for _ in 0..100 {
        let (input, output) = s.sample();
        assert_eq!(input, 50);
        assert_eq!(output, 75);
    }
}

#[test]
fn test_scenario_dir_name() {
    let s = parse_scenario("D(100,100)").unwrap();
    assert_eq!(s.dir_name(), "D_100_100");
}
