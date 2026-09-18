//! SSRF-safe HTTP fetching: every URL (including every redirect hop) passes
//! the kernel UrlGuard; responses are capped by size and time.

use act_kernel::error::{ActError, ActResult};
use act_kernel::SandboxContext;

#[derive(Clone)]
pub struct FetchOpts {
    pub max_bytes: u64,
    pub timeout_ms: u64,
    /// Extra request headers (e.g. API tokens).
    pub headers: Vec<(String, String)>,
}

pub struct FetchOutcome {
    pub final_url: String,
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub truncated: bool,
    pub redirects: Vec<String>,
}

const USER_AGENT: &str = "Mozilla/5.0 (compatible; AgentCoreTools/0.1)";

pub async fn fetch(
    ctx: &SandboxContext,
    raw_url: &str,
    opts: &FetchOpts,
) -> ActResult<FetchOutcome> {
    let policy = ctx.url_policy();
    let max_redirects = policy.max_redirects;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .gzip(true)
        .build()
        .map_err(|e| ActError::Other(format!("http client init: {e}")))?;

    let mut current = ctx.check_url(raw_url).await?;
    let mut redirects: Vec<String> = Vec::new();
    let mut redirects_used = 0usize;

    loop {
        let mut request = client
            .get(current.clone())
            .header("user-agent", USER_AGENT)
            .header("accept-language", "en,zh;q=0.8")
            .timeout(std::time::Duration::from_millis(opts.timeout_ms));
        for (name, value) in &opts.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        let resp = request.send().await.map_err(|e| {
            ActError::execution("Web_Fetch", format!("request to '{raw_url}' failed: {e}"))
        })?;

        let status = resp.status().as_u16();
        if (300..400).contains(&status) {
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let Some(location) = location else {
                return Err(ActError::execution(
                    "Web_Fetch",
                    format!("redirect status {status} without Location header"),
                ));
            };
            redirects_used += 1;
            if redirects_used > max_redirects {
                return Err(ActError::LimitExceeded {
                    reason: format!("more than {max_redirects} redirects"),
                });
            }
            // Resolve relative redirects and re-validate the hop.
            let next = current.join(&location).map_err(|e| {
                ActError::execution(
                    "Web_Fetch",
                    format!("bad redirect target '{location}': {e}"),
                )
            })?;
            let next = ctx.check_url(next.as_str()).await?;
            redirects.push(next.to_string());
            current = next;
            continue;
        }

        if !resp.status().is_success() {
            return Err(ActError::execution(
                "Web_Fetch",
                format!("'{raw_url}' returned HTTP {status}"),
            ));
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();

        // Only refuse absurdly large bodies; smaller oversizes are downloaded
        // and truncated according to max_bytes.
        if let Some(len) = resp.content_length() {
            let hard_cap = opts.max_bytes.max(16 * 1024 * 1024);
            if len > hard_cap {
                return Err(ActError::LimitExceeded {
                    reason: format!("content-length {len} exceeds cap {hard_cap}"),
                });
            }
        }

        let full = resp.bytes().await.map_err(|e| {
            ActError::execution("Web_Fetch", format!("reading body of '{raw_url}': {e}"))
        })?;
        let truncated = full.len() as u64 > opts.max_bytes;
        let body = if truncated {
            full[..opts.max_bytes as usize].to_vec()
        } else {
            full.to_vec()
        };

        return Ok(FetchOutcome {
            final_url: current.to_string(),
            status,
            content_type,
            body,
            truncated,
            redirects,
        });
    }
}
