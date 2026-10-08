use super::*;
use crate::model_router::RouteDescriptor;

mod pool;
mod settings;
mod usage;

use pool::subscription_pool_from_platform_billing;
use settings::{retry_policy, route_descriptors};

pub(in crate::agent) use usage::{append_usage_event, usage_cost};

pub(crate) fn model_router_config(config: &Config, args: &Args) -> ChatConfig {
    let mode_state = read_mode_state(&args.cwd);
    let mode_service_tier = if mode_state
        .pointer("/fast/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        mode_state
            .pointer("/fast/serviceTier")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
    } else {
        None
    };
    let merged = crate::cli::config::merged_config_value(&args.cwd);
    let routing = merged.get("modelRouting").unwrap_or(&Value::Null);
    let retry = retry_policy(routing);
    let configured_fallbacks = route_descriptors(routing.get("fallbacks"), "fallbacks");
    let configured_promotions =
        route_descriptors(routing.get("contextPromotions"), "contextPromotions");
    let brama_variable = |primary: &str, legacy: &str| {
        env::var(primary)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                env::var(legacy)
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            })
    };
    // A provider named in JEDEN_MODEL_ENDPOINT replaces Brama outright: its
    // own key is the bearer and its requests carry no Brama signature.
    let direct = crate::agent::credential::direct_provider();
    let (endpoint, bearer_token, secret) = match &direct {
        Some((url, key)) => (Some(url.clone()), key.clone(), String::new()),
        None => (
            crate::agent::credential::brama_url().or_else(|| {
                env::var("STADO_MODEL_ROUTER_URL")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            }),
            brama_variable("BRAMA_TOKEN", "STADO_MODEL_ROUTER_TOKEN"),
            env::var("WISENT_APP_AGENT_AUTH_SECRET").unwrap_or_default(),
        ),
    };
    let selected_model = args
        .model
        .clone()
        .or(config.model.clone())
        .or_else(|| env::var("JEDEN_MODEL").ok())
        .filter(|value| !value.trim().is_empty());
    let catalog_client = crate::control_plane::brama::BramaClient::configured(
        endpoint.clone(),
        bearer_token.clone(),
    );
    // One read: a gateway that cannot answer now names its own refusal, and
    // that error is what the run reports.
    let catalog = crate::control_plane::model_catalog(&args.cwd, &catalog_client, false);
    // Bare (provider-less) model ids resolve to the unique catalog route whose
    // id ends with `/<model>`; an ambiguous id names every matching route.
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
    let subscription_pool = subscription_pool_from_platform_billing();
    let catalog_error = bare_model_error.or(match (&selected_model, &catalog) {
        (None, _) => Some(
            "no model selected; choose a model advertised by Brama; run /setup to configure"
                .to_string(),
        ),
        (_, Err(error)) => Some(error.to_string()),
        (Some(model), Ok(_)) if crate::model_router::is_virtual_model_route(model) => None,
        (Some(model), Ok(catalog)) => catalog.resolve(model).err().map(|error| error.to_string()),
    });
    let image_capable_models = catalog
        .as_ref()
        .map(|catalog| {
            catalog
                .models
                .iter()
                .filter(|entry| {
                    entry.available
                        && entry
                            .input_modalities
                            .iter()
                            .any(|modality| modality.eq_ignore_ascii_case("image"))
                })
                .map(|entry| entry.id.clone())
                .collect()
        })
        .unwrap_or_default();
    let validate_routes = |routes: &Result<Vec<RouteDescriptor>, String>| -> Option<String> {
        let catalog = catalog.as_ref().ok()?;
        routes.as_ref().ok()?.iter().find_map(|route| {
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
    let endpoint_error = endpoint.is_none().then(|| {
        let base = "BRAMA_URL is required; declare Jeden's route with `stado service directory \
                    consumer-add brama jeden --capability model-routing --target <this host>`, \
                    export BRAMA_URL, or set JEDEN_MODEL_ENDPOINT to an OpenAI-compatible \
                    provider to run without Brama";
        match crate::agent::credential::brama_route().source.refusal() {
            Some(said) => format!("{base} (Stado did not route it: {said})"),
            None => base.to_string(),
        }
    });
    let token_error = (direct.is_none() && bearer_token.is_none()).then(|| {
        "BRAMA_TOKEN is required; obtain the scoped Jeden model-router credential".to_string()
    });
    let secret_error = (direct.is_none() && secret.is_empty())
        .then(|| "WISENT_APP_AGENT_AUTH_SECRET is required to sign requests to Brama".to_string());
    let precondition_error = endpoint_error
        .or(token_error)
        .or(secret_error)
        .or_else(|| retry.as_ref().err().cloned())
        .or_else(|| configured_fallbacks.as_ref().err().cloned())
        .or_else(|| configured_promotions.as_ref().err().cloned());
    let catalog_unanswered = precondition_error.is_none()
        && matches!(
            (&selected_model, &catalog),
            (Some(_), Err(error)) if error.left_unanswered()
        );
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
    let fallbacks = configured_fallbacks.unwrap_or_default();
    let promotions = configured_promotions.unwrap_or_default();
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
    let subscription_pool = subscription_pool.unwrap_or(None);
    let subscription_cooldown_path = subscription_pool
        .as_ref()
        .map(|_| args.cwd.join(".jeden/subscription-cooldowns.json"));
    ChatConfig {
        url: endpoint.unwrap_or_default(),
        bearer_token: bearer_token.unwrap_or_default(),
        agent_id: env::var("WISENT_APP_AGENT_ID")
            .ok()
            .or(config.agent_id.clone())
            .unwrap_or_else(|| "wisent-app".into()),
        secret,
        model: selected_model.unwrap_or_default(),
        service_tier: env::var("JEDEN_SERVICE_TIER")
            .ok()
            .or_else(|| env::var("MODEL_SERVICE_TIER").ok())
            .or(mode_service_tier)
            .unwrap_or_default(),
        retry: retry.unwrap_or_default(),
        fallbacks: resolved_fallbacks,
        context_promotions: resolved_promotions,
        image_capable_models,
        subscription_pool,
        subscription_cooldown_path,
        config_error,
        catalog_unanswered,
    }
}

pub(in crate::agent) fn env_usize(name: &str) -> Option<usize> {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

pub(in crate::agent) fn memory_guidance_for_prompt(cwd: &Path) -> Option<String> {
    let store = crate::memory::MemoryStore::open().ok()?;
    let scope = crate::memory::MemoryScope {
        kind: "repo".into(),
        id: cwd.display().to_string(),
    };
    let context = store.pre_compaction_context(&scope, "", 12_000).ok()?;
    (!context.is_empty()).then_some(context)
}
