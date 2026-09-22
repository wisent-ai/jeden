//! What happens to a configuration file: migrating it forward, merging the
//! layers that apply, and reading and writing the operator's own.

mod layers;
mod migrate;
mod user;

pub(crate) use layers::{
    config_layer_paths, read_config_typed,
    config_remove_value, config_set_value, config_value_at, merged_config_value,
    parse_config_literal, read_config_value,
};
pub(crate) use migrate::{config_migration_plan, migrate_config_file};
pub(crate) use user::{
    read_user_writable_config, read_user_writable_config_strict, write_user_config,
};
