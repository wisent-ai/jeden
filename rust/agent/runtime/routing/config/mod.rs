use super::*;

pub(crate) fn model_router_config(config: &Config, args: &Args) -> ChatConfig {
    let fast = crate::slash::read_mode_state(&args.cwd).fast;
    let mode_tier = Some(fast.service_tier).filter(|tier| fast.enabled && !tier.trim().is_empty());
    let merged = crate::cli::config::merged_config_value(&args.cwd);
    let absent = Value::Null;
    let routing = match merged.get("modelRouting") {
        Some(value) => value,
        None => &absent,
    };
    let retry = retry_policy(routing);
    let configured_fallbacks = route_descriptors(routing.get("fallbacks"), "fallbacks");
    let configured_promotions =
        route_descriptors(routing.get("contextPromotions"), "contextPromotions");

    // An explicit valid provider endpoint turns Brama off; a malformed one
    // refuses rather than falling through to the gateway. A Brama this
    // process cannot ask at all is stood in for before any request.
    let explicit_refusal = crate::agent::credential::direct_refusal().is_some();
    if crate::agent::credential::direct_provider().is_none() && !explicit_refusal {
        if let Some(cause) = crate::agent::credential::stand_in::brama_unconfigured() {
            if let Err(refused) = crate::agent::credential::stand_in::engage(&cause) {
                eprintln!("jeden: {refused}");
            }
        }
    }
    let direct = crate::agent::credential::direct_provider();
    let (endpoint, bearer_read, secret_read) = match &direct {
        Some((url, key)) => (Some(url.clone()), Ok(key.clone()), Ok(String::new())),
        None => (
            crate::agent::credential::brama_url(),
            crate::agent::credential::bearer_token(),
            env::var("WISENT_APP_AGENT_AUTH_SECRET"),
        ),
    };
    let (bearer_token, bearer_error) = match bearer_read {
        Ok(Some(token)) => (Some(token), None),
        Ok(None) if direct.is_some() => (None, None),
        Ok(None) => (
            None,
            Some("BRAMA_TOKEN is required for Brama requests".to_owned()),
        ),
        Err(cause) => (None, Some(cause)),
    };
    let (selected_model, model_error) =
        match inputs::model(args.model.as_deref(), config.model.as_deref()) {
            Ok(model) => (model, None),
            Err(cause) => (None, Some(cause)),
        };
    let client = crate::control_plane::brama::BramaClient::configured(
        endpoint.clone(),
        bearer_token.clone(),
    );
    let catalog = crate::control_plane::model_catalog(&args.cwd, &client, false);
    // A Brama that refuses, fails or does not answer the catalog read does not
    // serve this process: the provider standing in for it serves the turn.
    if direct.is_none() && !explicit_refusal {
        if let Err(error) = &catalog {
            if crate::agent::credential::stand_in::brama_not_serving(error) {
                match crate::agent::credential::stand_in::engage(&format!(
                    "Brama does not serve this process: {error}"
                )) {
                    Ok(_) => return model_router_config(config, args),
                    Err(refused) => eprintln!("jeden: {refused}"),
                }
            }
        }
    }
    let mut bare_model_error = None;
    let selected_model = match (selected_model, &catalog) {
        (Some(model), Ok(catalog))
            if !model.contains('/')
                && !crate::model_router::is_virtual_model_route(&model)
                && !catalog.models.iter().any(|entry| entry.id == model) =>
        {
            match catalog.resolve_bare(&model) {
                Ok(Some(entry)) => Some(entry.id.clone()),
                Ok(None) => Some(model),
                Err(error) => {
                    bare_model_error = Some(error);
                    Some(model)
                }
            }
        }
        (model, _) => model,
    };
    let catalog_error = bare_model_error.or(match (&selected_model, &catalog) {
        (None, _) => Some("no model selected; choose a model from /model".to_owned()),
        (_, Err(error)) => Some(error.to_string()),
        (Some(model), Ok(_)) if crate::model_router::is_virtual_model_route(model) => None,
        (Some(model), Ok(catalog)) => catalog.resolve(model).err().map(|error| error.to_string()),
    });
    if let Err(error) = &catalog {
        eprintln!("jeden: model catalog could not supply image modalities: {error}");
    }
    let image_capable_models: std::collections::BTreeSet<String> = catalog
        .iter()
        .flat_map(|catalog| catalog.models.iter())
        .filter(|entry| {
            entry.available
                && entry
                    .input_modalities
                    .iter()
                    .any(|modality| modality.eq_ignore_ascii_case("image"))
        })
        .map(|entry| entry.id.clone())
        .collect();
    let validate_routes = |routes: &Result<Vec<RouteDescriptor>, String>| -> Option<String> {
        let (Ok(catalog), Ok(routes)) = (&catalog, routes) else {
            return None;
        };
        routes.iter().find_map(|route| {
            if crate::model_router::is_virtual_model_route(&route.model) {
                None
            } else {
                catalog
                    .resolve(&route.model)
                    .err()
                    .map(|error| error.to_string())
            }
        })
    };
    let endpoint_error = if endpoint.is_none() {
        let route = crate::agent::credential::brama_route();
        match &route.source {
            crate::agent::credential::Source::DirectRefused(cause) => Some(cause.clone()),
            other => {
                let base =
                    "BRAMA_URL is required; declare Jeden's route with `stado service directory \
                    consumer add brama jeden --capability model-routing --target <this host>`, \
                    export BRAMA_URL, or set JEDEN_MODEL_ENDPOINT to an OpenAI-compatible provider";
                Some(match other.refusal() {
                    Some(cause) => format!("{base} (Stado did not route it: {cause})"),
                    None => base.to_owned(),
                })
            }
        }
    } else {
        None
    };
    let (secret, secret_error) = match secret_read {
        Ok(secret) if !secret.is_empty() || direct.is_some() => (secret, None),
        Ok(_) | Err(env::VarError::NotPresent) => (
            String::new(),
            Some("WISENT_APP_AGENT_AUTH_SECRET is required to sign Brama requests".to_owned()),
        ),
        Err(error) => (
            String::new(),
            Some(format!(
                "WISENT_APP_AGENT_AUTH_SECRET cannot be read: {error}"
            )),
        ),
    };
    let inherited_lease = if direct.is_some() {
        Ok(None)
    } else {
        crate::model_router::lease::inherited()
    };
    let (agent_id, agent_error) =
        match inputs::agent_id(config.agent_id.as_deref(), direct.is_some()) {
            Ok(value) => (value, None),
            Err(cause) => (String::new(), Some(cause)),
        };
    let (service_tier, tier_error) = match inputs::service_tier(mode_tier) {
        Ok(value) => (value, None),
        Err(cause) => (String::new(), Some(cause)),
    };
    let virtual_direct_error = selected_model
        .as_deref()
        .filter(|model| direct.is_some() && crate::model_router::is_virtual_model_route(model))
        .map(|model| format!("{model} is a Brama virtual route, not a direct provider model ID"));
    let precondition_error = endpoint_error
        .or(bearer_error)
        .or(secret_error)
        .or(model_error)
        .or(virtual_direct_error)
        .or(agent_error)
        .or(tier_error)
        .or_else(|| inherited_lease.as_ref().err().cloned())
        .or_else(|| retry.as_ref().err().cloned())
        .or_else(|| configured_fallbacks.as_ref().err().cloned())
        .or_else(|| configured_promotions.as_ref().err().cloned());
    let catalog_unanswered = precondition_error.is_none()
        && matches!((&selected_model, &catalog), (Some(_), Err(error)) if error.left_unanswered());
    let config_error = precondition_error
        .or(catalog_error)
        .or_else(|| validate_routes(&configured_fallbacks))
        .or_else(|| validate_routes(&configured_promotions));
    let catalog_routes = |fallback: bool| -> Vec<RouteDescriptor> {
        let Some(model) = selected_model.as_deref() else {
            return Vec::new();
        };
        let Ok(catalog) = &catalog else {
            return Vec::new();
        };
        let Ok(entry) = catalog.resolve(model) else {
            return Vec::new();
        };
        let ids = if fallback {
            &entry.fallback
        } else {
            &entry.promotion
        };
        ids.iter()
            .filter(|id| catalog.resolve(id).is_ok())
            .map(|id| RouteDescriptor {
                model: id.clone(),
                service_tier: None,
            })
            .collect()
    };
    let fallbacks = match configured_fallbacks {
        Ok(routes) => routes,
        Err(cause) => {
            eprintln!("jeden: modelRouting.fallbacks refused: {cause}");
            Vec::new()
        }
    };
    let promotions = match configured_promotions {
        Ok(routes) => routes,
        Err(cause) => {
            eprintln!("jeden: modelRouting.contextPromotions refused: {cause}");
            Vec::new()
        }
    };
    let resolved_fallbacks = if fallbacks.is_empty() {
        catalog_routes(true)
    } else {
        fallbacks
    };
    let resolved_promotions = if promotions.is_empty() {
        catalog_routes(false)
    } else {
        promotions
    };
    ChatConfig {
        url: match endpoint {
            Some(url) => url,
            None => String::new(),
        },
        endpoint_via_stado: direct.is_none()
            && matches!(
                crate::agent::credential::brama_route().source,
                crate::agent::credential::Source::Stado
            ),
        endpoint_origin: if direct.is_some() {
            format!(
                "OpenAI-compatible provider selected by {}",
                crate::agent::credential::DIRECT_URL
            )
        } else {
            format!("Brama; {}", crate::agent::credential::route_origin())
        },
        bearer_token: match bearer_token {
            Some(token) => token,
            None => String::new(),
        },
        agent_id,
        secret,
        model: match selected_model {
            Some(model) => model,
            None => String::new(),
        },
        service_tier,
        retry: match retry {
            Ok(policy) => policy,
            Err(cause) => {
                eprintln!("jeden: modelRouting.retry refused: {cause}");
                Default::default()
            }
        },
        fallbacks: resolved_fallbacks,
        context_promotions: resolved_promotions,
        image_capable_models,
        via_brama: direct.is_none(),
        session_id: None,
        inherited_lease: match inherited_lease {
            Ok(lease) => lease,
            Err(cause) => {
                eprintln!("jeden: JEDEN_LEASE refused: {cause}");
                None
            }
        },
        config_error,
        catalog_unanswered,
    }
}
