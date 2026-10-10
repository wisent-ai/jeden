use super::ChatConfig;

impl ChatConfig {
    pub(crate) fn endpoint_failure(
        &self,
        operation: &str,
        cause: impl std::fmt::Display,
    ) -> String {
        let mut message = format!(
            "{operation} at {}/v1/chat/completions ({}) failed: {cause}",
            self.url.trim_end_matches('/'),
            self.endpoint_origin
        );
        if self.endpoint_via_stado {
            match channel_observation(&self.url) {
                Ok(observation) => {
                    message.push_str("; local resolver observation: ");
                    message.push_str(&observation);
                }
                Err(error) => {
                    message.push_str("; local resolver observation failed: ");
                    message.push_str(&error);
                }
            }
        }
        message
    }
}

fn channel_observation(endpoint: &str) -> Result<String, String> {
    let url = url::Url::parse(endpoint)
        .map_err(|error| format!("cannot parse selected resolver endpoint {endpoint}: {error}"))?;
    let bind = &url[url::Position::BeforeHost..url::Position::AfterPort];
    let operation = format!("stado resolver channels --bind {bind} --json");
    let output = stado_wait::output(
        std::process::Command::new("stado")
            .args(["resolver", "channels", "--bind", bind, "--json"]),
    )
    .map_err(|error| format!("cannot run {operation}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{operation} ended {}; stdout: {}; stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let observation: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("{operation} returned invalid JSON: {error}"))?;
    if observation["bind"].as_str() != Some(bind)
        || observation["health_observed"].as_bool() != Some(false)
        || !observation["channels"].is_array()
        || !observation["source"].is_string()
    {
        return Err(format!(
            "{operation} returned an invalid channel observation: {observation}"
        ));
    }
    Ok(observation.to_string())
}
