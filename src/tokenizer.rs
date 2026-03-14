use anyhow::Result;
use rand::seq::SliceRandom;
use tokenizers::Tokenizer;

const CORPUS: &str = include_str!("corpus.txt");

pub struct PromptGenerator {
    tokenizer: Tokenizer,
    lines: Vec<String>,
}

impl PromptGenerator {
    pub fn new(tokenizer_name: &str) -> Result<Self> {
        let tokenizer = Tokenizer::from_pretrained(tokenizer_name, None)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer '{}': {}", tokenizer_name, e))?;

        let lines: Vec<String> = CORPUS
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.to_string())
            .collect();

        if lines.is_empty() {
            return Err(anyhow::anyhow!("Corpus is empty"));
        }

        Ok(Self { tokenizer, lines })
    }

    pub fn generate_prompt(&self, target_tokens: usize) -> Result<String> {
        let mut rng = rand::rng();
        let mut shuffled = self.lines.clone();
        shuffled.shuffle(&mut rng);

        let mut prompt = String::new();
        let mut remaining = target_tokens;

        loop {
            for line in &shuffled {
                if remaining == 0 {
                    return Ok(prompt);
                }

                let encoding = self.tokenizer.encode(line.as_str(), false)
                    .map_err(|e| anyhow::anyhow!("Tokenization error: {}", e))?;
                let line_tokens = encoding.get_ids().len();

                if line_tokens <= remaining {
                    if !prompt.is_empty() {
                        prompt.push(' ');
                    }
                    prompt.push_str(line);
                    remaining -= line_tokens;
                } else {
                    // Truncate at token boundary
                    let ids = &encoding.get_ids()[..remaining];
                    let truncated = self.tokenizer.decode(ids, true)
                        .map_err(|e| anyhow::anyhow!("Decode error: {}", e))?;
                    if !prompt.is_empty() {
                        prompt.push(' ');
                    }
                    prompt.push_str(&truncated);
                    remaining = 0;
                    return Ok(prompt);
                }
            }
            // Reshuffle if we need more tokens
            shuffled.shuffle(&mut rng);
        }
    }

    pub fn count_tokens(&self, text: &str) -> Result<usize> {
        let encoding = self.tokenizer.encode(text, false)
            .map_err(|e| anyhow::anyhow!("Tokenization error: {}", e))?;
        Ok(encoding.get_ids().len())
    }
}
