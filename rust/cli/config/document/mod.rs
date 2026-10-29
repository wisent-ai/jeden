//! What happens to a configuration file: migrating it forward, merging the
//! layers that apply, and reading and writing the operator's own.

mod layers;
mod migrate;
mod user;

pub(crate) use layers::{
    config_layer_paths, config_remove_value, config_set_value, config_texts, config_value_at,
    merged_config_value, read_config_layer, read_config_typed, read_config_value,
    user_config_value,
};
pub(crate) use user::{read_user_writable_config_strict, write_user_config};
