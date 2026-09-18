//! Command naming convention: `<Domain>_<Action>` in PascalCase.
//!
//! The registry rejects any name that does not match this convention, so the
//! MCP/CLI surface can never expose ad-hoc or inconsistently named commands.

use crate::error::{ActError, ActResult};
use std::sync::OnceLock;

static NAME_RE: OnceLock<regex::Regex> = OnceLock::new();

/// Domains that ship built-in. Extensions may register additional domains
/// through `CommandRegistry::register_domain`.
pub const BUILTIN_DOMAINS: &[&str] = &["Fs", "Web"];

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommandName {
    domain: String,
    action: String,
    full: String,
}

impl CommandName {
    /// Parse and validate a command name such as `Fs_ReadFile`.
    pub fn parse(name: &str) -> ActResult<Self> {
        let re = NAME_RE.get_or_init(|| {
            regex::Regex::new(r"^([A-Z][A-Za-z0-9]{1,20})_([A-Z][A-Za-z0-9]{1,30})$")
                .expect("static regex must compile")
        });
        let caps = re.captures(name).ok_or_else(|| ActError::InvalidName {
            name: name.to_string(),
        })?;
        let domain = caps.get(1).expect("capture group 1").as_str().to_string();
        let action = caps.get(2).expect("capture group 2").as_str().to_string();
        Ok(Self {
            domain,
            action,
            full: name.to_string(),
        })
    }

    pub fn domain(&self) -> &str {
        &self.domain
    }

    pub fn action(&self) -> &str {
        &self.action
    }

    pub fn as_str(&self) -> &str {
        &self.full
    }

    /// Ensure the domain is either builtin or explicitly registered.
    pub fn ensure_domain_known(&self, extra: &[String]) -> ActResult<()> {
        let known = extra.iter().any(|d| d == &self.domain)
            || BUILTIN_DOMAINS.iter().any(|d| *d == self.domain);
        if known {
            Ok(())
        } else {
            let mut all: Vec<String> = BUILTIN_DOMAINS.iter().map(|s| s.to_string()).collect();
            all.extend_from_slice(extra);
            Err(ActError::UnknownDomain {
                domain: self.domain.clone(),
                known: all.join(", "),
            })
        }
    }
}

impl std::fmt::Display for CommandName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.full)
    }
}
