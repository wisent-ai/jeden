//! The rows shown when a collaboration command is opened without arguments.
//!
//! Split out of `slash/session/collab.rs`, which had grown past the module
//! line cap. A collaboration state that cannot be read is one disabled row
//! naming the file and the cause, never a picker that says no session exists.
//! A role the saved state does not carry indexes as `null`: not hosting, or
//! not joined.

use super::relay::{on_http_relay, read_collab_state};
use super::status::picker_role_detail;
use crate::slash::common::refusal_picker;
use crate::slash::session::collab::relay::collab_default_relay;
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};
use serde_json::Value;

/// The operation every collaboration picker starts with, named in its refusal.
const READ_STATE: &str = "Read the collaboration state";

pub(crate) fn build_collab_picker(context: &SlashContext<'_>) -> PickerSpec {
    let state = match read_collab_state(context.cwd) {
        Ok(state) => state,
        Err(cause) => return refusal_picker("Collaboration", READ_STATE, cause),
    };
    let host = &state["host"];
    let guest = &state["guest"];
    let detail = format!(
        "{}; {}",
        picker_role_detail("host", host),
        picker_role_detail("guest", guest)
    );
    PickerSpec::new(
        "Collaboration",
        vec![
            PickerItem::action("Show collaboration status", "/collab status")
                .detail(&detail)
                .badge("status"),
            PickerItem::action("View collaboration events", "/collab view")
                .detail(&detail)
                .badge("event log"),
            PickerItem::action("Start host on default durable relay", "/collab start")
                .detail(format!(
                    "Default relay: {}",
                    collab_default_relay(context.cwd).display()
                ))
                .badge("writes relay")
                .disabled(!host.is_null()),
            PickerItem::action("Stop active host", "/collab stop")
                .detail(picker_role_detail("host", host))
                .badge("destructive")
                .disabled(host.is_null()),
        ],
    )
}

pub(crate) fn build_join_picker(context: &SlashContext<'_>) -> PickerSpec {
    let state = match read_collab_state(context.cwd) {
        Ok(state) => state,
        Err(cause) => return refusal_picker("Join collaboration", READ_STATE, cause),
    };
    let host = &state["host"];
    let guest = &state["guest"];
    let mut items = Vec::new();
    if let Some(relay_url) = host.get("relayUrl").and_then(Value::as_str) {
        items.push(
            PickerItem::action(
                "Join the active local host relay",
                format!("/join {relay_url}"),
            )
            .detail(picker_role_detail("host", host))
            .badge("durable file")
            .disabled(!guest.is_null()),
        );
    }
    let instruction = if on_http_relay(host) {
        "The HTTP join key is intentionally not persisted; paste the private join URL as `/join <url>#key=<key>`."
    } else {
        "A relay target is required; type `/join <relay-file-or-file-url>` manually."
    };
    items.push(
        PickerItem::action("Enter another relay target", "/join ")
            .detail(instruction)
            .badge("INPUT")
            .prefill(),
    );
    if !guest.is_null() {
        items.push(
            PickerItem::action("Already attached as guest", "")
                .detail(picker_role_detail("guest", guest))
                .badge("current")
                .disabled(true),
        );
    }
    PickerSpec::new("Join collaboration", items)
}

pub(crate) fn build_leave_picker(context: &SlashContext<'_>) -> PickerSpec {
    let state = match read_collab_state(context.cwd) {
        Ok(state) => state,
        Err(cause) => return refusal_picker("Leave collaboration", READ_STATE, cause),
    };
    let guest = &state["guest"];
    PickerSpec::new(
        "Leave collaboration",
        vec![
            PickerItem::action("Leave active guest relay", "/leave confirmed")
                .detail(picker_role_detail("guest", guest))
                .badge("destructive")
                .disabled(guest.is_null()),
        ],
    )
}
