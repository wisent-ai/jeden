//! Bringing an older configuration file up to the shape this build reads.
//!
//! Split out of `cli/config/mod.rs`, which had grown past the module line cap.

use super::super::migrations;
use serde_json::json;
use serde_json::Value;
use std::path::Path;

pub(crate) const CONFIG_SCHEMA_VERSION: u32 = 4;

fn config_v0_to_v1(value: &mut Value) -> Result<(), String> {
    value
        .as_object_mut()
        .ok_or_else(|| "config root must be an object".to_string())?;
    Ok(())
}

fn config_v1_to_v2(value: &mut Value) -> Result<(), String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| "config root must be an object".to_string())?;
    object.remove("model_url");
    object.remove("modelRouterUrl");
    Ok(())
}

fn config_v2_to_v3(value: &mut Value) -> Result<(), String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| "config root must be an object".to_string())?;
    object.entry("billing").or_insert_with(|| {
        json!({
            "autoPurchaseEnabled": false,
            "autoRenewEnabled": false,
            "preferredCurrency": null,
            "maxSingleMicrounits": 0,
            "maxPeriodMicrounits": 0
        })
    });
    Ok(())
}

fn config_v3_to_v4(value: &mut Value) -> Result<(), String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| "config root must be an object".to_string())?;
    let contracts = object
        .entry("contracts")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "config contracts must be an object".to_string())?;
    contracts
        .entry("communication")
        .or_insert_with(|| json!(""));
    contracts
        .entry("functionality")
        .or_insert_with(|| json!(""));
    Ok(())
}

static CONFIG_MIGRATION_STEPS: [migrations::MigrationStep; 4] = [
    migrations::MigrationStep {
        name: "version-envelope",
        from: 0,
        to: 1,
        apply: config_v0_to_v1,
    },
    migrations::MigrationStep {
        name: "remove-legacy-model-router-endpoints",
        from: 1,
        to: 2,
        apply: config_v1_to_v2,
    },
    migrations::MigrationStep {
        name: "safe-billing-preferences",
        from: 2,
        to: 3,
        apply: config_v2_to_v3,
    },
    migrations::MigrationStep {
        name: "operator-contracts",
        from: 3,
        to: 4,
        apply: config_v3_to_v4,
    },
];

pub(crate) fn config_migration_plan() -> migrations::MigrationPlan {
    migrations::MigrationPlan {
        store: "config",
        from: 0,
        to: CONFIG_SCHEMA_VERSION,
        reversible: true,
        preflight: migrations::object_preflight,
        steps: &CONFIG_MIGRATION_STEPS,
        compatibility_window: migrations::CompatibilityWindow {
            oldest_readable: 0,
            newest_readable: 4,
            rollback_floor: 3,
        },
    }
}

pub(crate) fn migrate_config_file(path: &Path) -> Result<migrations::MigrationOutcome, String> {
    migrations::migrate_json(path, &config_migration_plan())
}
