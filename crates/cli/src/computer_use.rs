//! User-facing global Computer Use configuration, before model/session startup.
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

pub(crate) fn manage(
    enabled: Option<bool>,
    include_screenshot: Option<bool>,
    max_nodes: Option<usize>,
    screenshot_width: Option<u32>,
) -> Result<Value> {
    let mut config = crate::config::AxConfig::load()?;
    let mut next = config.computer_use.clone();
    if let Some(value) = enabled {
        next.enabled = value;
    }
    if let Some(value) = include_screenshot {
        next.include_screenshot = value;
    }
    if let Some(value) = max_nodes {
        next.max_nodes = value;
    }
    if let Some(value) = screenshot_width {
        next.screenshot_width = value;
    }
    next.validate().map_err(|error| anyhow!(error))?;
    if enabled == Some(true) && !cfg!(windows) {
        return Err(anyhow!(
            "Computer Use currently supports native Windows only"
        ));
    }
    if next != config.computer_use {
        config.computer_use = next;
        config.save()?;
    }
    let supported =
        cfg!(windows) && config.execution.environment == crate::config::AgentEnvironment::Native;
    Ok(
        json!({"settings":config.computer_use,"supported":supported,"limits":{
            "max_nodes":tool::MAX_DESKTOP_NODES,"min_screenshot_width":tool::MIN_SCREENSHOT_WIDTH,"max_screenshot_width":tool::MAX_SCREENSHOT_WIDTH
        }}),
    )
}
