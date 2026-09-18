//! UrlGuard: scheme, domain and SSRF protection for all web access.
//!
//! Rules (deny-by-default):
//! - only configured schemes (default http/https)
//! - no userinfo (`user:pass@host`)
//! - literal IPs must be public (loopback/private/link-local/multicast blocked)
//! - hostnames are DNS-resolved and every resolved IP must be public
//! - optional domain allowlist (suffix match) and blocklist (precedence)

use std::net::IpAddr;
use std::sync::Arc;

use async_trait::async_trait;
use url::Url;

use crate::config::UrlPolicy;
use crate::error::{ActError, ActResult};

#[async_trait]
pub trait DnsResolver: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String>;
}

pub struct TokioDnsResolver;

#[async_trait]
impl DnsResolver for TokioDnsResolver {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String> {
        let socket_addrs = tokio::net::lookup_host((host, port))
            .await
            .map_err(|e| format!("dns lookup failed for '{host}': {e}"))?;
        Ok(socket_addrs.map(|sa| sa.ip()).collect())
    }
}

/// True when the IP must not be fetched (private/reserved ranges).
pub fn is_disallowed_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 0
                || o[0] == 10
                || (o[0] == 172 && (16..=31).contains(&o[1]))
                || (o[0] == 192 && o[1] == 168)
                || (o[0] == 169 && o[1] == 254)
                || o[0] == 127
                || o[0] >= 224 // multicast + reserved + broadcast
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_disallowed_ip(IpAddr::V4(mapped));
            }
            let s = v6.segments();
            v6.is_unspecified()
                || v6.is_loopback()
                || (s[0] & 0xff00) == 0xff00     // multicast ff00::/8
                || (s[0] & 0xfe00) == 0xfc00     // unique local fc00::/7
                || (s[0] & 0xffc0) == 0xfe80     // link-local fe80::/10
                || (s[0] & 0xfe00) == 0x0200 // site-local 0200::/7 (deprecated)
        }
    }
}

fn domain_matches(host: &str, pattern: &str) -> bool {
    let host = host.to_lowercase();
    let pattern = pattern.trim_start_matches('.').to_lowercase();
    host == pattern || host.ends_with(&format!(".{pattern}"))
}

pub struct UrlGuard {
    policy: UrlPolicy,
    dns: Arc<dyn DnsResolver>,
}

impl UrlGuard {
    pub fn new(policy: UrlPolicy) -> Self {
        Self {
            policy,
            dns: Arc::new(TokioDnsResolver),
        }
    }

    pub fn with_resolver(policy: UrlPolicy, dns: Arc<dyn DnsResolver>) -> Self {
        Self { policy, dns }
    }

    pub fn policy(&self) -> &UrlPolicy {
        &self.policy
    }

    /// Validate a raw URL string. Returns the parsed URL on success.
    pub async fn check(&self, raw: &str) -> ActResult<Url> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(ActError::invalid_params("url", "empty url"));
        }
        let url = Url::parse(trimmed).map_err(|e| {
            ActError::invalid_params("url", format!("invalid url '{trimmed}': {e}"))
        })?;

        let scheme = url.scheme().to_lowercase();
        if !self
            .policy
            .allowed_schemes
            .iter()
            .any(|s| s.to_lowercase() == scheme)
        {
            return Err(ActError::permission_denied(
                "UrlGuard",
                format!(
                    "scheme '{scheme}' is not allowed (allowed: {:?})",
                    self.policy.allowed_schemes
                ),
            ));
        }

        if !url.username().is_empty() || url.password().is_some() {
            return Err(ActError::permission_denied(
                "UrlGuard",
                "urls with embedded credentials are not allowed",
            ));
        }

        let host = url
            .host_str()
            .ok_or_else(|| ActError::invalid_params("url", format!("url has no host: {trimmed}")))?
            .to_lowercase();

        // Domain policy (punycode already normalized by Url::parse).
        for pattern in &self.policy.blocked_domains {
            if domain_matches(&host, pattern) {
                return Err(ActError::permission_denied(
                    "UrlGuard",
                    format!("domain '{host}' is blocked"),
                ));
            }
        }
        if !self.policy.allowed_domains.is_empty()
            && !self
                .policy
                .allowed_domains
                .iter()
                .any(|p| domain_matches(&host, p))
        {
            return Err(ActError::permission_denied(
                "UrlGuard",
                format!(
                    "domain '{host}' is not in the allow list {:?}",
                    self.policy.allowed_domains
                ),
            ));
        }

        // IP policy: literal or resolved.
        let port = url.port_or_known_default().unwrap_or(80);
        if let Ok(ip) = host.parse::<IpAddr>() {
            if !self.policy.allow_private_ips && is_disallowed_ip(ip) {
                return Err(ActError::permission_denied(
                    "UrlGuard",
                    format!("ip {ip} is private/reserved and not allowed"),
                ));
            }
        } else if !host.starts_with('[') {
            // Hostname: resolve DNS and require every address to be public.
            if self.policy.allow_private_ips {
                return Ok(url);
            }
            let addrs = self
                .dns
                .resolve(&host, port)
                .await
                .map_err(|e| ActError::Execution {
                    command: "UrlGuard".into(),
                    detail: e,
                })?;
            if addrs.is_empty() {
                return Err(ActError::Execution {
                    command: "UrlGuard".into(),
                    detail: format!("host '{host}' resolved to no addresses"),
                });
            }
            for ip in addrs {
                if is_disallowed_ip(ip) {
                    return Err(ActError::permission_denied(
                        "UrlGuard",
                        format!("host '{host}' resolves to private/reserved ip {ip}"),
                    ));
                }
            }
        } else if let Some(ip) = host
            .trim_matches(|c| c == '[' || c == ']')
            .parse::<IpAddr>()
            .ok()
        {
            if !self.policy.allow_private_ips && is_disallowed_ip(ip) {
                return Err(ActError::permission_denied(
                    "UrlGuard",
                    format!("ip {ip} is private/reserved and not allowed"),
                ));
            }
        }

        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubDns(Vec<IpAddr>);
    #[async_trait]
    impl DnsResolver for StubDns {
        async fn resolve(&self, _host: &str, _port: u16) -> Result<Vec<IpAddr>, String> {
            Ok(self.0.clone())
        }
    }

    async fn check_ok(raw: &str) -> Url {
        let guard = UrlGuard::with_resolver(
            UrlPolicy::default(),
            Arc::new(StubDns(vec!["8.8.8.8".parse().unwrap()])),
        );
        guard.check(raw).await.expect("should be allowed")
    }

    async fn check_err(raw: &str) -> ActError {
        let guard = UrlGuard::with_resolver(
            UrlPolicy::default(),
            Arc::new(StubDns(vec!["8.8.8.8".parse().unwrap()])),
        );
        guard.check(raw).await.expect_err("should be denied")
    }

    #[tokio::test]
    async fn http_https_allowed() {
        let u = check_ok("https://example.com/a?b=1").await;
        assert_eq!(u.host_str().unwrap(), "example.com");
        check_ok("http://93.184.216.34/").await;
    }

    #[tokio::test]
    async fn private_v4_blocked() {
        for raw in [
            "http://127.0.0.1/",
            "http://10.0.0.1/",
            "http://172.16.0.1/",
            "http://192.168.1.1/",
            "http://169.254.169.254/latest/meta-data",
            "http://0.0.0.0/",
            "http://224.0.0.1/",
        ] {
            let err = check_err(raw).await;
            assert!(
                matches!(
                    err,
                    ActError::PermissionDenied {
                        guard: "UrlGuard",
                        ..
                    }
                ),
                "{raw}: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn private_v6_blocked() {
        check_err("http://[::1]/").await;
        check_err("http://[::ffff:127.0.0.1]/").await;
        check_err("http://[fe80::1]/").await;
    }

    #[tokio::test]
    async fn bad_scheme_and_credentials_blocked() {
        check_err("ftp://example.com/").await;
        check_err("file:///etc/passwd").await;
        check_err("https://user:pass@example.com/").await;
    }

    #[tokio::test]
    async fn hostname_resolving_to_private_blocked() {
        let guard = UrlGuard::with_resolver(
            UrlPolicy::default(),
            Arc::new(StubDns(vec![
                "8.8.8.8".parse().unwrap(),
                "10.0.0.5".parse().unwrap(),
            ])),
        );
        let err = guard
            .check("https://internal.example.com/")
            .await
            .unwrap_err();
        assert!(matches!(err, ActError::PermissionDenied { .. }));
    }

    #[tokio::test]
    async fn allow_private_ips_flag_permits_loopback() {
        let mut policy = UrlPolicy::default();
        policy.allow_private_ips = true;
        let guard = UrlGuard::with_resolver(policy, Arc::new(StubDns(vec![])));
        guard
            .check("http://127.0.0.1:8080/")
            .await
            .expect("private allowed by flag");
    }

    #[tokio::test]
    async fn domain_lists() {
        let mut policy = UrlPolicy::default();
        policy.allowed_domains = vec!["example.com".into()];
        let guard =
            UrlGuard::with_resolver(policy, Arc::new(StubDns(vec!["8.8.8.8".parse().unwrap()])));
        guard
            .check("https://api.example.com/")
            .await
            .expect("subdomain allowed");
        let err = guard.check("https://other.org/").await.unwrap_err();
        assert!(matches!(err, ActError::PermissionDenied { .. }));

        let mut policy = UrlPolicy::default();
        policy.blocked_domains = vec!["evil.com".into()];
        let guard =
            UrlGuard::with_resolver(policy, Arc::new(StubDns(vec!["8.8.8.8".parse().unwrap()])));
        let err = guard.check("https://sub.evil.com/").await.unwrap_err();
        assert!(matches!(err, ActError::PermissionDenied { .. }));
    }
}
