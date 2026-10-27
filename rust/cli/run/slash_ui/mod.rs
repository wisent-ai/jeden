//! The interactive views the run command can open.

use std::io::IsTerminal;
use std::path::Path;

use crate::cli::auth::{format_auth_status, provider_picker};
use crate::cli::config::schema::settings_picker;
use crate::cli::i18n::lang_code;
use crate::slash::{self, SlashContext};
use crate::tui::{CommandOutcome, PickerItem, PickerSpec};

mod columns;
mod picker;
mod rows;

use crate::cli::i18n::tr;
pub(crate) use picker::model_picker;

fn logout_picker() -> Result<PickerSpec, String> {
    let client = crate::control_plane::weles::WelesClient::from_env();
    if !client.health().available {
        return Err(client.health().detail);
    }
    let items = client
        .accounts(None)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|account| {
            PickerItem::action(&account.display_name, format!("/logout {}", account.id))
                .detail(format!("{} · {}", account.provider, account.status))
                .badge("ACCOUNT")
        })
        .collect();
    Ok(PickerSpec::new("Select account to logout", items))
}

pub(crate) fn interactive_view(
    cwd: &Path,
    input: &str,
    model: Option<&str>,
) -> Option<Result<CommandOutcome, String>> {
    let _capabilities = crate::capability::for_cwd(cwd);
    let trimmed = input.trim();
    // A command alone has no arguments.
    let (command, args) = match trimmed.split_once(char::is_whitespace) {
        Some((command, args)) => (command, args),
        None => (trimmed, ""),
    };
    if command == "/onboarding" {
        return Some(crate::onboarding::interactive(args.trim(), cwd));
    }
    if command == "/model" && matches!(args.trim(), "--all" | "-a") {
        return Some(model_picker(cwd, model, true).map(CommandOutcome::Picker));
    }
    if !args.trim().is_empty() {
        return None;
    }
    if let Some(view) = crate::capability::view_descriptor(cwd, command) {
        if !view.health.is_executable() || !view.ui.executable {
            // The capability's own detail is the cause; a view the capability
            // offers as not executable here, or a health without a detail, is
            // said as such rather than as a stock sentence.
            let detail = match view.health.detail.clone() {
                Some(detail) => detail,
                None if !view.ui.executable => {
                    format!("{} is not offered as a view here", view.ui.label)
                }
                None => format!(
                    "the {} capability reports {:?} without saying why",
                    view.ui.label, view.health.state
                ),
            };
            let lang = lang_code(cwd);
            return Some(Ok(CommandOutcome::Picker(PickerSpec::new(
                format!("{} unavailable", view.ui.label),
                vec![PickerItem::action(view.ui.label, "")
                    .detail(detail)
                    .badge(tr(&lang, "badge.unavailable"))
                    .disabled(true)],
            ))));
        }
    }
    match command {
        "/login" => {
            return Some(Ok(CommandOutcome::Text(format_auth_status(cwd))));
        }
        "/setup" if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() => {
            // Piped stdin/stdout: print the manual checklist instead of a view.
            let session_root = crate::session_root();
            let context = SlashContext {
                cwd,
                model,
                session_root: &session_root,
            };
            return Some(crate::slash::setup::handle_text("", &context).map(CommandOutcome::Text));
        }
        "/providers" => {
            return Some(provider_picker(cwd).map(CommandOutcome::Picker));
        }
        "/logout" => return Some(logout_picker().map(CommandOutcome::Picker)),
        "/model" => return Some(model_picker(cwd, model, false).map(CommandOutcome::Picker)),
        "/settings" => return Some(Ok(CommandOutcome::Picker(settings_picker(cwd)))),
        _ => {}
    }
    let session_root = crate::session_root();
    let context = SlashContext {
        cwd,
        model,
        session_root: &session_root,
    };
    slash::interactive_picker(&context, input).map(|picker| picker.map(CommandOutcome::Picker))
}
