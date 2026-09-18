//! DuckDuckGo HTML endpoint (no API key required).

use act_kernel::error::{ActError, ActResult};
use act_kernel::SandboxContext;
use async_trait::async_trait;
use scraper::{Html, Selector};

use super::{percent_decode, SearchEngine, SearchHit};
use crate::http::{fetch, FetchOpts};

pub struct DuckDuckGo;

#[async_trait]
impl SearchEngine for DuckDuckGo {
    fn name(&self) -> &'static str {
        "duckduckgo"
    }

    async fn search(
        &self,
        ctx: &SandboxContext,
        query: &str,
        count: usize,
    ) -> ActResult<Vec<SearchHit>> {
        let url = format!("https://html.duckduckgo.com/html/?q={}", urlencode(query));
        let outcome = fetch(
            ctx,
            &url,
            &FetchOpts {
                max_bytes: ctx.config.limits.fetch_max_bytes,
                timeout_ms: ctx.config.limits.web_timeout_ms,
                headers: vec![],
            },
        )
        .await?;
        let html = String::from_utf8_lossy(&outcome.body).into_owned();
        parse(&html, count)
    }
}

pub fn urlencode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

/// Unwrap DuckDuckGo redirect links (`//duckduckgo.com/l/?uddg=<encoded>`).
pub fn unwrap_link(href: &str) -> String {
    let href = href.trim().replace("&amp;", "&");
    let href = href.as_str();
    for prefix in ["//duckduckgo.com/l/", "/l/"] {
        if let Some(rest) = href.strip_prefix(prefix) {
            let rest = rest.trim_start_matches('?');
            if let Some(uddg) = rest.split('&').find_map(|part| part.strip_prefix("uddg=")) {
                return percent_decode(uddg);
            }
        }
    }
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    format!("https://{href}")
}

pub fn parse(html: &str, count: usize) -> ActResult<Vec<SearchHit>> {
    let document = Html::parse_document(html);
    let result_sel = Selector::parse(".result, .web-result")
        .map_err(|e| ActError::Other(format!("selector: {e:?}")))?;
    let link_sel =
        Selector::parse("a.result__a").map_err(|e| ActError::Other(format!("selector: {e:?}")))?;
    let snippet_sel = Selector::parse(".result__snippet")
        .map_err(|e| ActError::Other(format!("selector: {e:?}")))?;

    let mut hits = Vec::new();
    for node in document.select(&result_sel) {
        if hits.len() >= count {
            break;
        }
        let Some(link) = node.select(&link_sel).next() else {
            continue;
        };
        let title: String = link.text().collect::<Vec<_>>().join("").trim().to_string();
        if title.is_empty() {
            continue;
        }
        let url = link
            .value()
            .attr("href")
            .map(unwrap_link)
            .unwrap_or_default();
        if url.is_empty() {
            continue;
        }
        let snippet: String = node
            .select(&snippet_sel)
            .next()
            .map(|s| s.text().collect::<Vec<_>>().join("").trim().to_string())
            .unwrap_or_default();
        hits.push(SearchHit {
            title,
            url,
            snippet,
        });
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ddg_html() {
        let html = r#"
        <div class="result">
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Frust&rut=abc">Rust 官网</a>
            <a class="result__snippet">系统编程语言 Rust</a>
        </div>
        <div class="result">
            <a class="result__a" href="https://other.org/page">Other</a>
            <div class="result__snippet">Another snippet</div>
        </div>
        "#;
        let hits = parse(html, 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Rust 官网");
        assert_eq!(hits[0].url, "https://example.com/rust");
        assert_eq!(hits[0].snippet, "系统编程语言 Rust");
        assert_eq!(hits[1].url, "https://other.org/page");
    }

    #[test]
    fn unwraps_links() {
        assert_eq!(
            unwrap_link("//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa"),
            "https://example.com/a"
        );
        assert_eq!(unwrap_link("https://x.com/y"), "https://x.com/y");
    }
}
