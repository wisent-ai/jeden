//! The direct-provider branch of the setup picker. It never asks for Brama.
use super::*;

pub(super) fn items(state: &SetupState) -> Result<Option<Vec<PickerItem>>, String> {
    let Some((endpoint, key)) = crate::agent::credential::direct_provider() else {
        return Ok(crate::agent::credential::direct_refusal().map(|cause| {
            vec![error_row(
                "2. Direct model provider is refused",
                cause.to_owned(),
            )]
        }));
    };
    let mut items = vec![configured_row(
        "2. Direct model provider",
        "selected by JEDEN_MODEL_ENDPOINT",
    )];
    if key.is_some() {
        items.push(configured_row(
            "3. Provider key in process environment",
            "JEDEN_MODEL_KEY takes precedence over locally stored tokens",
        ));
    } else {
        let count = match crate::agent::credential::tokens::Store::open_existing()? {
            Some(store) => store.count_for(&endpoint)?,
            None => Default::default(),
        };
        if count > Default::default() {
            items.push(configured_row(
                "3. Jeden token list",
                format!("{count} token(s) for this endpoint; inspect with jeden token list"),
            ));
        } else {
            items.push(PickerItem::action("3. Provider authentication (optional)", "")
                .detail("No key configured; anonymous providers may work. If this provider requires a key, add one with `jeden token add <label> --endpoint <url>` (key on stdin), or set JEDEN_MODEL_KEY")
                .badge("OPTIONAL").disabled(true));
        }
    }
    match &state.model {
        Some(model) => items.push(configured_row(
            format!("4. Model route: {model}"),
            "change anytime with /model",
        )),
        None => items.push(
            PickerItem::action("4. Select model route", "/setup model")
                .detail("fetch the provider catalog and pick a route")
                .badge("MODEL"),
        ),
    }
    items.push(
        PickerItem::action("5. Language & theme (optional)", "/setup preferences")
            .detail(format!(
                "current: language {} · theme {}",
                state.language, state.theme
            ))
            .badge("PREFS"),
    );
    items.push(
        PickerItem::action("6. Validate setup", "/setup validate")
            .detail("run doctor and show the final summary")
            .badge("CHECK"),
    );
    Ok(Some(items))
}
