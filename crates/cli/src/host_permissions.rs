//! Explicit user settings only; model-controlled tools never write permission rules.
use anyhow::{Result, anyhow};
use tool::{HostPermissionStore, HostSurface, HostTarget, PermissionDecision};

pub(crate) fn manage(
    surface: Option<&str>,
    target: Option<&str>,
    decision: Option<&str>,
    remove: bool,
    browser_enabled: Option<bool>,
) -> Result<tool::HostPermissions> {
    let store = HostPermissionStore::new(crate::config::ax_home());
    if remove && decision.is_some() {
        return Err(anyhow!("Choose --remove or --decision, not both"));
    }
    if target.is_none() && (surface.is_some() || decision.is_some() || remove) {
        return Err(anyhow!(
            "A target and surface are required when editing access"
        ));
    }
    let target = target
        .map(|id| -> Result<HostTarget> {
            let surface = match surface {
                Some("computer") => HostSurface::Computer,
                Some("browser") => HostSurface::Browser,
                _ => return Err(anyhow!("Choose computer or browser")),
            };
            let id = match surface {
                HostSurface::Computer => tool::app_id(std::path::Path::new(id))?,
                HostSurface::Browser => tool::browser_origin(id)?,
            };
            Ok(HostTarget {
                label: id.clone(),
                id,
                surface,
            })
        })
        .transpose()?;
    let decision = decision
        .map(|value| match value {
            "allow" => Ok(PermissionDecision::Allow),
            "ask" => Ok(PermissionDecision::Ask),
            "deny" => Ok(PermissionDecision::Deny),
            _ => Err(anyhow!("Choose allow, ask or deny")),
        })
        .transpose()?;
    if target.is_some() && decision.is_none() && !remove {
        return Err(anyhow!("Choose a decision or --remove"));
    }
    if target.is_none() && browser_enabled.is_none() {
        return Ok(store.load()?);
    }
    Ok(store.edit(target.as_ref(), decision, remove, browser_enabled)?)
}
