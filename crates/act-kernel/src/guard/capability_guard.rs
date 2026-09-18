//! CapabilityGuard: per-mode capability switches (read/write/net).

use crate::config::{Capabilities, ModeCapabilities};
use crate::context::InvokeMode;
use crate::error::{ActError, ActResult};
use crate::registry::Capability;

pub fn check(capability: &Capability, caps: &Capabilities, mode: &InvokeMode) -> ActResult<()> {
    let mode_caps: &ModeCapabilities = match mode {
        InvokeMode::Cli => &caps.cli,
        InvokeMode::Mcp => &caps.mcp,
    };
    let (label, enabled) = match capability {
        Capability::Read => ("read", mode_caps.read),
        Capability::Write => ("write", mode_caps.write),
        Capability::Net => ("net", mode_caps.net),
    };
    if enabled {
        Ok(())
    } else {
        Err(ActError::CapabilityDisabled {
            capability: label.to_string(),
            mode: mode.label().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_disabled_for_mcp() {
        let mut caps = Capabilities::default();
        caps.mcp.net = false;
        let err = check(&Capability::Net, &caps, &InvokeMode::Mcp).unwrap_err();
        assert!(matches!(err, ActError::CapabilityDisabled { .. }));
        check(&Capability::Net, &caps, &InvokeMode::Cli).expect("cli net still on");
    }
}
