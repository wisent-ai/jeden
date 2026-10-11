//! How the browser is run: the mode `/browser` stored, the profile it opens,
//! and whether the embedded bridge has what it needs to start.

use serde_json::Value;

use super::super::config;
use super::super::types::command_exists;

/// The two modes `/browser` stores; the browser service and the slash command
/// read these names, so neither keeps a list of its own.
pub(crate) const VISIBLE: &str = "visible";
pub(crate) const HEADLESS: &str = "headless";

/// Whether the browser opens a window, as `/browser visible` or `/browser
/// headless` stored it (browser.mode, or JEDEN_BROWSER_MODE). No stored mode
/// is a headless browser; a mode that command never stores is refused with
/// the value read instead of being run headless without a word.
pub(super) fn browser_visible(config: &Value) -> Result<bool, String> {
    let Some(mode) = config::string(config, &["browser", "mode"], "JEDEN_BROWSER_MODE") else {
        return Ok(false);
    };
    if mode == VISIBLE {
        return Ok(true);
    }
    if mode == HEADLESS {
        return Ok(false);
    }
    Err(format!(
        "browser.mode (or JEDEN_BROWSER_MODE) is {mode:?}, which /browser never stores; set it with /browser {VISIBLE} or /browser {HEADLESS}"
    ))
}

pub(super) fn browser_profile(config: &Value) -> Option<String> {
    config::string(
        config,
        &["browser", "profile", "path"],
        "JEDEN_BROWSER_PROFILE",
    )
}

pub(super) fn embedded_backend_health(chrome: Option<&str>) -> Result<&str, String> {
    if !command_exists("node") {
        return Err("Node.js is required by the embedded Chromium/CDP bridge".into());
    }
    chrome
        .filter(|candidate| command_exists(candidate))
        .ok_or_else(|| {
            "no Chromium executable found; set JEDEN_CHROME_EXECUTABLE or browser.launch.executablePath"
                .into()
        })
}
