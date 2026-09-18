//! Summarizers for the output overflow policy.
//!
//! - `extractive`: local, zero-dependency. Sentence split (CJK aware) +
//!   term-frequency scoring + top-k sentence selection in original order.
//! - `llm`: optional OpenAI-compatible chat API. Any failure silently falls
//!   back to the extractive summarizer.

use crate::config::LlmConfig;
use crate::error::ActResult;

const MAX_SUMMARIZER_INPUT_CHARS: usize = 200_000;

/// Extractive summary preserving original sentence order.
pub fn extractive(text: &str, max_chars: usize) -> String {
    let text = normalize_ws(text);
    if text.chars().count() <= max_chars {
        return text;
    }
    let sentences = split_sentences(&text);
    if sentences.is_empty() {
        return text.chars().take(max_chars).collect();
    }

    // Term frequencies: CJK bigrams + latin/digit words.
    let mut freq: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for term in terms(&text) {
        *freq.entry(term).or_default() += 1;
    }
    let max_tf = freq.values().copied().max().unwrap_or(1) as f64;

    let scored: Vec<(usize, f64, &str)> = sentences
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut score = 0.0;
            let mut count = 0;
            for term in terms(s) {
                if let Some(tf) = freq.get(&term) {
                    score += *tf as f64 / max_tf;
                    count += 1;
                }
            }
            if count > 0 {
                score /= count as f64;
            }
            // Slight positional bias toward the beginning of the document.
            score *= 1.0 + 0.1 * (1.0 - (i as f64 / sentences.len() as f64));
            (i, score, s.as_str())
        })
        .collect();

    // Pick the highest-scoring sentences until the budget is exhausted.
    let mut selected: Vec<usize> = Vec::new();
    let mut used = 0usize;
    let mut ranked: Vec<(usize, f64)> = scored.iter().map(|(i, s, _)| (*i, *s)).collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for (idx, _) in ranked {
        let sentence = sentences[idx].as_str();
        let cost = sentence.chars().count() + 1;
        if used + cost > max_chars && !selected.is_empty() {
            break;
        }
        selected.push(idx);
        used += cost;
        if used >= max_chars {
            break;
        }
    }
    selected.sort_unstable();
    let mut out = String::new();
    for (n, idx) in selected.into_iter().enumerate() {
        if n > 0 {
            out.push(' ');
        }
        out.push_str(&sentences[idx]);
    }
    if out.is_empty() {
        out = text.chars().take(max_chars).collect();
    }
    out
}

/// OpenAI-compatible chat summarizer. Returns None on any failure.
pub async fn llm(config: &LlmConfig, model: &str, text: &str, max_chars: usize) -> Option<String> {
    let base_url = config.base_url.as_deref()?;
    let model = if model.is_empty() {
        config.model.as_deref().unwrap_or("")
    } else {
        model
    };
    if model.is_empty() {
        return None;
    }
    let api_key = std::env::var(&config.api_key_env).ok()?;
    let head: String = text.chars().take(100_000).collect();
    let prompt = format!(
        "Summarize the following content in at most {max_chars} characters. Reply with the summary only.\n\n{head}"
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .ok()?;
    let resp = client
        .post(format!(
            "{}/chat/completions",
            base_url.trim_end_matches('/')
        ))
        .bearer_auth(api_key)
        .json(&serde_json::json!({
            "model": model,
            "messages": [
                {"role": "system", "content": "You are a precise summarizer."},
                {"role": "user", "content": prompt}
            ],
            "temperature": 0.2
        }))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    let summary = body
        .pointer("/choices/0/message/content")?
        .as_str()?
        .trim()
        .to_string();
    if summary.is_empty() {
        None
    } else {
        Some(summary)
    }
}

/// Extract readable text from an arbitrary JSON result for summarization.
pub fn collect_text(value: &serde_json::Value) -> String {
    let mut out = String::new();
    fn walk(v: &serde_json::Value, out: &mut String) {
        match v {
            serde_json::Value::String(s) => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(s);
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, out);
                }
            }
            serde_json::Value::Object(map) => {
                for item in map.values() {
                    walk(item, out);
                }
            }
            _ => {}
        }
    }
    walk(value, &mut out);
    if out.chars().count() > MAX_SUMMARIZER_INPUT_CHARS {
        out.chars().take(MAX_SUMMARIZER_INPUT_CHARS).collect()
    } else {
        out
    }
}

fn normalize_ws(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_ws = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !last_ws {
                out.push(' ');
            }
            last_ws = true;
        } else {
            out.push(ch);
            last_ws = false;
        }
    }
    out.trim().to_string()
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if matches!(ch, '。' | '！' | '？' | '；' | '.' | '!' | '?' | ';' | '\n') {
            let trimmed = current.trim().to_string();
            if trimmed.chars().count() >= 8 {
                sentences.push(trimmed);
            }
            current.clear();
        }
    }
    let rest = current.trim().to_string();
    if rest.chars().count() >= 8 {
        sentences.push(rest);
    }
    sentences
}

fn terms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut latin = String::new();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch.is_alphanumeric() && ch.is_ascii() {
            latin.push(ch.to_ascii_lowercase());
        } else {
            if !latin.is_empty() {
                if latin.chars().count() >= 2 {
                    out.push(std::mem::take(&mut latin));
                } else {
                    latin.clear();
                }
            }
            if is_cjk(ch) && i + 1 < chars.len() && is_cjk(chars[i + 1]) {
                out.push(format!("{}{}", ch, chars[i + 1]));
            }
        }
        i += 1;
    }
    if latin.chars().count() >= 2 {
        out.push(latin);
    }
    out
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x4E00..=0x9FFF   // CJK Unified Ideographs
        | 0x3400..=0x4DBF // Extension A
        | 0xF900..=0xFAFF // Compatibility Ideographs
    )
}

/// Combined summarizer with LLM-first, extractive-fallback semantics.
pub async fn summarize(text: &str, cfg: &crate::config::CompressionConfig) -> ActResult<String> {
    let max = cfg.max_summary_chars.max(100);
    if cfg.algorithm == crate::config::SummaryAlgorithm::Llm {
        if let Some(summary) = llm(&cfg.llm, "", text, max).await {
            return Ok(summary);
        }
        tracing::warn!("llm summarizer unavailable, falling back to extractive");
    }
    Ok(extractive(text, max))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extractive_short_text_passthrough() {
        assert_eq!(extractive("hello world", 100), "hello world");
    }

    #[test]
    fn extractive_picks_top_sentences() {
        let text = format!(
            "{} {} {}",
            "核心要点一句话关于 rust 内存安全主题的内容。".repeat(3),
            "无关填充句子其他内容一些不重要的文字材料。".repeat(1),
            "rust 内存安全是系统编程语言的重要主题并且被广泛讨论。".repeat(3),
        );
        let summary = extractive(&text, 80);
        assert!(!summary.is_empty());
        assert!(summary.chars().count() <= 100);
    }

    #[test]
    fn extractive_english() {
        let text = std::iter::repeat(
            "The kernel validates every path against the sandbox roots before execution. ",
        )
        .take(40)
        .collect::<String>();
        let summary = extractive(&text, 120);
        assert!(summary.contains("sandbox roots"));
    }

    #[test]
    fn collect_text_flattens() {
        let value = serde_json::json!({"a": "one", "b": ["two", {"c": "three"}], "d": 42});
        let text = collect_text(&value);
        assert!(text.contains("one") && text.contains("two") && text.contains("three"));
    }
}
