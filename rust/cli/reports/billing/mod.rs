//! The billing commands an operator can run, and the backend that carries
//! them out.

pub(crate) const BILLING_SLASH_HANDLERS: [(&str, &str); 9] = [
    ("/payment-method setup", "payment-method.setup"),
    ("/billing policy get", "billing.policy.get"),
    ("/billing policy set", "billing.policy.set"),
    ("/billing policy reset", "billing.policy.reset"),
    ("/subscriptions list", "subscriptions.list"),
    ("/subscriptions status", "subscriptions.status"),
    ("/subscriptions disable", "subscriptions.disable"),
    ("/subscriptions purchase", "subscriptions.purchase"),
    ("/subscriptions renew", "subscriptions.renew"),
];

/// The slash prefixes the billing backend answers. f3e10a9 kept the table
/// and dropped this accessor, which cli/run/slash.rs calls to route input.
pub(crate) fn billing_slash_handlers() -> &'static [(&'static str, &'static str)] {
    &BILLING_SLASH_HANDLERS
}

mod execute;
mod model;
mod parse;
mod weles;

pub(crate) use execute::execute_billing_command;
pub(crate) use parse::parse_billing_command;
pub(crate) use weles::WelesBillingBackend;

pub(crate) fn handle_billing_slash(input: &str, json_output: bool) -> Result<String, String> {
    let command = parse_billing_command(input).map_err(|error| error.to_string())?;
    execute_billing_command(&WelesBillingBackend::from_env(), command, json_output)
        .map_err(|error| error.to_string())
}
