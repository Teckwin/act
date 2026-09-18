//! Bing web search (HTML scraping, no API key required).

use act_kernel::error::{ActError, ActResult};
use act_kernel::SandboxContext;
use async_trait::async_trait;
use scraper::{Html, Selector};

use super::duckduckgo::urlencode;
use super::{SearchEngine, SearchHit};
use crate::http::{fetch, FetchOpts};

pub struct Bing;

/// Unwrap Bing's `bing.com/ck/a?...&u=a1<base64url>` redirect links.
pub fn unwrap_link(href: &str) -> String {
    if !href.contains("bing.com/ck/") {
        return href.to_string();
    }
    let Some(pos) = href.find("u=a1") else {
        return href.to_string();
    };
    let encoded: String = href[pos + 4..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    decode_base64_url(&encoded).unwrap_or_else(|| href.to_string())
}

fn decode_base64_url(input: &str) -> Option<String> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    let mut out: Vec<u8> = Vec::with_capacity(input.len() * 3 / 4);
    for ch in input.bytes() {
        let value = TABLE.iter().position(|t| *t == ch)? as u32;
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    String::from_utf8(out).ok()
}

#[async_trait]
impl SearchEngine for Bing {
    fn name(&self) -> &'static str {
        "bing"
    }

    async fn search(
        &self,
        ctx: &SandboxContext,
        query: &str,
        count: usize,
    ) -> ActResult<Vec<SearchHit>> {
        let url = format!(
            "https://www.bing.com/search?q={}&count={}&mkt=en-US&setlang=en",
            urlencode(query),
            count
        );
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

pub fn parse(html: &str, count: usize) -> ActResult<Vec<SearchHit>> {
    let document = Html::parse_document(html);
    let item_sel =
        Selector::parse("li.b_algo").map_err(|e| ActError::Other(format!("selector: {e:?}")))?;
    let link_sel =
        Selector::parse("h2 a").map_err(|e| ActError::Other(format!("selector: {e:?}")))?;
    let snippet_sel = Selector::parse(".b_caption p, .b_caption")
        .map_err(|e| ActError::Other(format!("selector: {e:?}")))?;

    let mut hits = Vec::new();
    for node in document.select(&item_sel) {
        if hits.len() >= count {
            break;
        }
        let Some(link) = node.select(&link_sel).next() else {
            continue;
        };
        let title: String = link.text().collect::<Vec<_>>().join("").trim().to_string();
        let url = link
            .value()
            .attr("href")
            .map(|href| unwrap_link(href))
            .unwrap_or_default();
        if title.is_empty() || url.is_empty() {
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
    fn parses_bing_html() {
        let html = r#"
        <ol><li class="b_algo">
            <h2><a href="https://example.com/rust">Rust 编程语言 - 官网</a></h2>
            <div class="b_caption"><p>Rust 是一门系统级编程语言</p></div>
        </li>
        <li class="b_algo">
            <h2><a href="https://doc.rust-lang.org/">Rust 文档</a></h2>
        </li></ol>
        "#;
        let hits = parse(html, 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Rust 编程语言 - 官网");
        assert_eq!(hits[0].snippet, "Rust 是一门系统级编程语言");
        assert_eq!(hits[1].url, "https://doc.rust-lang.org/");
    }

    #[test]
    fn unwraps_bing_ck_redirects() {
        let href =
            "https://www.bing.com/ck/a?!&&p=abc&u=a1aHR0cHM6Ly9naXRodWIuY29tL3J1c3QtdGRr&ntb=1";
        assert_eq!(unwrap_link(href), "https://github.com/rust-tdk");
        assert_eq!(
            unwrap_link("https://plain.example/x"),
            "https://plain.example/x"
        );
    }
}
