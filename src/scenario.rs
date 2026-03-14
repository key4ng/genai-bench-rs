use anyhow::{anyhow, Result};

pub trait Scenario: Send + Sync {
    fn sample(&self) -> (usize, usize);
    fn name(&self) -> String;
    fn dir_name(&self) -> String;
}

pub struct DeterministicScenario {
    input_tokens: usize,
    output_tokens: usize,
}

impl DeterministicScenario {
    pub fn new(input_tokens: usize, output_tokens: usize) -> Self {
        Self {
            input_tokens,
            output_tokens,
        }
    }
}

impl Scenario for DeterministicScenario {
    fn sample(&self) -> (usize, usize) {
        (self.input_tokens, self.output_tokens)
    }

    fn name(&self) -> String {
        format!("D({},{})", self.input_tokens, self.output_tokens)
    }

    fn dir_name(&self) -> String {
        format!("D_{}_{}", self.input_tokens, self.output_tokens)
    }
}

pub fn parse_scenario(s: &str) -> Result<Box<dyn Scenario>> {
    let s = s.trim();
    if s.is_empty() {
        return Err(anyhow!("scenario string is empty"));
    }

    let type_char = s.chars().next().unwrap();
    match type_char {
        'D' => parse_deterministic(s),
        _ => Err(anyhow!(
            "unknown scenario type '{}'. Supported: D(N,M)",
            type_char
        )),
    }
}

fn parse_deterministic(s: &str) -> Result<Box<dyn Scenario>> {
    let inner = s
        .strip_prefix("D(")
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| {
            anyhow!(
                "invalid deterministic scenario format: '{}'. Expected D(N,M)",
                s
            )
        })?;

    let parts: Vec<&str> = inner.split(',').collect();
    if parts.len() != 2 {
        return Err(anyhow!(
            "D() requires exactly 2 arguments, got {}",
            parts.len()
        ));
    }

    let input: usize = parts[0]
        .trim()
        .parse()
        .map_err(|_| anyhow!("invalid input token count: '{}'", parts[0].trim()))?;
    let output: usize = parts[1]
        .trim()
        .parse()
        .map_err(|_| anyhow!("invalid output token count: '{}'", parts[1].trim()))?;

    if input == 0 {
        return Err(anyhow!("input token count must be > 0"));
    }
    if output == 0 {
        return Err(anyhow!("output token count must be > 0"));
    }

    Ok(Box::new(DeterministicScenario::new(input, output)))
}
