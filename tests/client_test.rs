use genai_bench_rs::client::parse_sse_chunk;

#[test]
fn test_parse_sse_content_chunk() {
    let line = r#"data: {"choices":[{"delta":{"content":"Hello"},"index":0}]}"#;
    let result = parse_sse_chunk(line).unwrap();
    assert_eq!(result.content, Some("Hello".to_string()));
    assert!(result.usage.is_none());
    assert!(!result.done);
}

#[test]
fn test_parse_sse_role_only_chunk() {
    let line = r#"data: {"choices":[{"delta":{"role":"assistant"},"index":0}]}"#;
    let result = parse_sse_chunk(line).unwrap();
    assert_eq!(result.content, None);
}

#[test]
fn test_parse_sse_done() {
    let line = "data: [DONE]";
    let result = parse_sse_chunk(line).unwrap();
    assert!(result.done);
}

#[test]
fn test_parse_sse_usage_chunk() {
    let line = r#"data: {"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":50}}"#;
    let result = parse_sse_chunk(line).unwrap();
    let usage = result.usage.unwrap();
    assert_eq!(usage.prompt_tokens, 100);
    assert_eq!(usage.completion_tokens, 50);
}

#[test]
fn test_parse_sse_empty_line() {
    let result = parse_sse_chunk("");
    assert!(result.is_none());
}

#[test]
fn test_parse_sse_comment_line() {
    let result = parse_sse_chunk(": comment");
    assert!(result.is_none());
}
