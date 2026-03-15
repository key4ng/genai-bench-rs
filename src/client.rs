use anyhow::Result;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::time::Instant;

use crate::metrics::{RawRequestResult, RequestError};

#[derive(Debug)]
pub struct SseChunk {
    pub content: Option<String>,
    pub usage: Option<Usage>,
    pub done: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    #[serde(default)]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompletionTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: Option<u32>,
}

pub fn parse_sse_chunk(line: &str) -> Option<SseChunk> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return None;
    }

    let data = line.strip_prefix("data: ")?;

    if data == "[DONE]" {
        return Some(SseChunk {
            content: None,
            usage: None,
            done: true,
        });
    }

    let v: Value = serde_json::from_str(data).ok()?;

    let delta = v["choices"].get(0).map(|c| &c["delta"]);
    let content = delta
        .and_then(|d| {
            d["content"]
                .as_str()
                .or_else(|| d["reasoning_content"].as_str())
        })
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let usage = v.get("usage").and_then(|u| {
        if u.is_null() {
            None
        } else {
            serde_json::from_value::<Usage>(u.clone()).ok()
        }
    });

    Some(SseChunk {
        content,
        usage,
        done: false,
    })
}

pub struct BenchmarkClient {
    client: Client,
    api_base: String,
    model: String,
    api_key: Option<String>,
    ignore_eos: bool,
    timeout: std::time::Duration,
}

impl BenchmarkClient {
    pub fn new(
        api_base: String,
        model: String,
        api_key: Option<String>,
        ignore_eos: bool,
        timeout: std::time::Duration,
        max_concurrency: u32,
    ) -> Result<Self> {
        let client = Client::builder()
            .pool_max_idle_per_host(max_concurrency as usize)
            .build()?;

        Ok(Self {
            client,
            api_base,
            model,
            api_key,
            ignore_eos,
            timeout,
        })
    }

    pub async fn send_request(
        &self,
        request_id: u64,
        prompt: &str,
        max_tokens: usize,
        run_start: Instant,
    ) -> RawRequestResult {
        let url = format!("{}/chat/completions", self.api_base);

        let mut payload = serde_json::json!({
            "model": &self.model,
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": max_tokens,
            "temperature": 0.0,
            "stream": true,
            "stream_options": {"include_usage": true}
        });

        if self.ignore_eos {
            payload["ignore_eos"] = serde_json::json!(true);
        }

        let start_ns = run_start.elapsed().as_nanos() as u64;

        let error_result = |error: RequestError| RawRequestResult {
            request_id,
            start_ns,
            first_token_ns: 0,
            end_ns: 0,
            num_input_tokens: 0,
            num_output_tokens: 0,
            reasoning_tokens: 0,
            error: Some(error),
        };

        let mut request = self.client.post(&url).json(&payload);
        if let Some(ref key) = self.api_key {
            request = request.bearer_auth(key);
        }

        let response = match tokio::time::timeout(self.timeout, request.send()).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => {
                return error_result(RequestError {
                    code: 0,
                    message: format!("Connection error: {}", e),
                });
            }
            Err(_) => {
                return error_result(RequestError {
                    code: 0,
                    message: "Request timeout".to_string(),
                });
            }
        };

        if !response.status().is_success() {
            let code = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return error_result(RequestError {
                code,
                message: body,
            });
        }

        let mut first_token_ns: Option<u64> = None;
        let mut usage: Option<Usage> = None;

        // Stream SSE response
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        use futures::StreamExt;
        while let Some(chunk_result) = stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    return error_result(RequestError {
                        code: 0,
                        message: format!("Stream error: {}", e),
                    });
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Process complete lines
            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if let Some(sse) = parse_sse_chunk(&line) {
                    if sse.done {
                        let end_ns = run_start.elapsed().as_nanos() as u64;
                        let u = usage.unwrap_or(Usage {
                            prompt_tokens: 0,
                            completion_tokens: 0,
                            completion_tokens_details: None,
                        });
                        let reasoning = u
                            .completion_tokens_details
                            .as_ref()
                            .and_then(|d| d.reasoning_tokens)
                            .unwrap_or(0);
                        return RawRequestResult {
                            request_id,
                            start_ns,
                            first_token_ns: first_token_ns.unwrap_or(start_ns),
                            end_ns,
                            num_input_tokens: u.prompt_tokens,
                            num_output_tokens: u.completion_tokens,
                            reasoning_tokens: reasoning,
                            error: None,
                        };
                    }

                    if sse.content.is_some() && first_token_ns.is_none() {
                        // Capture BEFORE processing content
                        first_token_ns = Some(run_start.elapsed().as_nanos() as u64);
                    }

                    if let Some(u) = sse.usage {
                        usage = Some(u);
                    }
                }
            }
        }

        // Stream ended without [DONE]
        let end_ns = run_start.elapsed().as_nanos() as u64;
        let u = usage.unwrap_or(Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            completion_tokens_details: None,
        });
        let reasoning = u
            .completion_tokens_details
            .as_ref()
            .and_then(|d| d.reasoning_tokens)
            .unwrap_or(0);
        RawRequestResult {
            request_id,
            start_ns,
            first_token_ns: first_token_ns.unwrap_or(start_ns),
            end_ns,
            num_input_tokens: u.prompt_tokens,
            num_output_tokens: u.completion_tokens,
            reasoning_tokens: reasoning,
            error: None,
        }
    }
}
