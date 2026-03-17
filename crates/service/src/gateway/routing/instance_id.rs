use rand::RngCore;
use std::sync::{OnceLock, RwLock};

const ENV_INSTANCE_ID: &str = "CODEXMANAGER_INSTANCE_ID";
const INSTANCE_ID_SETTING_KEY: &str = "gateway.instance_id";

static INSTANCE_ID: OnceLock<RwLock<Option<String>>> = OnceLock::new();

pub(crate) fn current_instance_id() -> String {
    if let Some(from_env) = env_instance_id() {
        write_cached_instance_id(Some(from_env.clone()));
        return from_env;
    }

    if let Some(cached) = cached_instance_id() {
        return cached;
    }

    let resolved = crate::app_settings::get_persisted_app_setting(INSTANCE_ID_SETTING_KEY)
        .and_then(|value| normalize_instance_id(Some(value.as_str())))
        .unwrap_or_else(|| {
            let generated = generate_instance_id();
            if let Err(err) = crate::app_settings::save_persisted_app_setting(
                INSTANCE_ID_SETTING_KEY,
                Some(generated.as_str()),
            ) {
                log::warn!("persist gateway instance id failed: {err}");
            }
            generated
        });
    write_cached_instance_id(Some(resolved.clone()));
    resolved
}

pub(crate) fn reload_from_env() {
    write_cached_instance_id(env_instance_id());
}

fn env_instance_id() -> Option<String> {
    std::env::var(ENV_INSTANCE_ID)
        .ok()
        .and_then(|value| normalize_instance_id(Some(value.as_str())))
}

fn normalize_instance_id(raw: Option<&str>) -> Option<String> {
    let value = raw?.trim();
    if value.is_empty() || value.chars().any(|ch| ch.is_ascii_control()) {
        return None;
    }
    Some(value.to_string())
}

fn generate_instance_id() -> String {
    let mut bytes = [0_u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let mut id = String::from("inst_");
    for byte in bytes {
        id.push_str(&format!("{byte:02x}"));
    }
    id
}

fn cached_instance_id() -> Option<String> {
    crate::lock_utils::read_recover(instance_id_cell(), "gateway_instance_id").clone()
}

fn write_cached_instance_id(value: Option<String>) {
    let mut cached = crate::lock_utils::write_recover(instance_id_cell(), "gateway_instance_id");
    *cached = value;
}

fn instance_id_cell() -> &'static RwLock<Option<String>> {
    INSTANCE_ID.get_or_init(|| RwLock::new(None))
}

#[cfg(test)]
pub(crate) fn clear_instance_id_for_tests() {
    write_cached_instance_id(None);
    let _ = crate::app_settings::save_persisted_app_setting(INSTANCE_ID_SETTING_KEY, None);
}
