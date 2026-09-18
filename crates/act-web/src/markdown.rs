//! HTML to Markdown / plain text extraction.

use act_kernel::error::{ActError, ActResult};

/// Remove noisy blocks before conversion.
pub fn pre_clean(html: &str) -> String {
    let mut out = html.to_string();
    for tag in [
        "script", "style", "nav", "header", "footer", "aside", "noscript", "svg", "form",
    ] {
        loop {
            let lower = out.to_lowercase();
            let Some(start) = lower.find(&format!("<{tag}")) else {
                break;
            };
            let Some(end_rel) = lower[start..].find(&format!("</{tag}>")) else {
                // Unclosed tag: drop to the end.
                out.replace_range(start.., "");
                break;
            };
            let end = start + end_rel + tag.len() + 3;
            out.replace_range(start..end, "");
        }
    }
    out
}

/// Convert HTML to Markdown via htmd.
pub fn to_markdown(html: &str) -> ActResult<String> {
    let cleaned = pre_clean(html);
    let md = htmd::convert(&cleaned)
        .map_err(|e| ActError::execution("Web_Fetch", format!("html to markdown: {e}")))?;
    Ok(collapse_blank_lines(&md))
}

/// HTML to plain text (tag stripping via scraper DOM walk).
pub fn to_text(html: &str) -> String {
    let cleaned = pre_clean(html);
    let document = scraper::Html::parse_document(&cleaned);
    let mut text: String = document.root_element().text().collect::<Vec<_>>().join("");
    // Unescape entities minimally.
    for (entity, ch) in [
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
    ] {
        text = text.replace(entity, ch);
    }
    collapse_blank_lines(&text.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn collapse_blank_lines(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut blanks = 0;
    for line in input.lines() {
        if line.trim().is_empty() {
            blanks += 1;
            if blanks <= 1 {
                out.push('\n');
            }
        } else {
            blanks = 0;
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_basic_conversion() {
        let html = r#"<html><body><script>evil()</script><header>site nav</header>
        <h1>标题 Title</h1><p>第一段 <code>code</code>。</p>
        <footer>copyright</footer></body></html>"#;
        let md = to_markdown(html).unwrap();
        assert!(md.contains("# 标题 Title"), "md: {md}");
        assert!(md.contains("第一段"));
        assert!(!md.contains("evil()"));
        assert!(!md.contains("copyright"));
        assert!(!md.contains("site nav"));
    }

    #[test]
    fn text_extraction() {
        let html = r#"<p>Hello <b>world</b> &amp; friends</p><script>x()</script>"#;
        let text = to_text(html);
        assert_eq!(text, "Hello world & friends");
    }
}
