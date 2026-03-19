use codexmanager_core::storage::{now_ts, Account, Storage, Token};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use crate::usage_account_meta::{derive_account_meta, patch_account_meta_in_place};

static CANDIDATE_SNAPSHOT_CACHE: OnceLock<Mutex<Option<CandidateSnapshotCache>>> = OnceLock::new();
static SELECTION_CONFIG_LOADED: OnceLock<()> = OnceLock::new();
static CANDIDATE_CACHE_TTL_MS: AtomicU64 = AtomicU64::new(DEFAULT_CANDIDATE_CACHE_TTL_MS);
static CURRENT_DB_PATH: OnceLock<RwLock<String>> = OnceLock::new();
const DEFAULT_CANDIDATE_CACHE_TTL_MS: u64 = 500;
const CANDIDATE_CACHE_TTL_ENV: &str = "CODEXMANAGER_CANDIDATE_CACHE_TTL_MS";

#[derive(Clone)]
struct CandidateSnapshotCache {
    db_path: String,
    expires_at: Instant,
    candidates: Vec<(Account, Token)>,
}

pub(crate) fn collect_gateway_candidates(
    storage: &Storage,
) -> Result<Vec<(Account, Token)>, String> {
    if let Some(cached) = read_candidate_cache() {
        return Ok(cached);
    }

    let candidates = collect_gateway_candidates_uncached(storage)?;
    write_candidate_cache(candidates.clone());
    Ok(candidates)
}

fn collect_gateway_candidates_uncached(storage: &Storage) -> Result<Vec<(Account, Token)>, String> {
    // 选择可用账号作为网关上游候选
    let candidates = storage
        .list_gateway_candidates()
        .map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(candidates.len());
    for (account, token) in candidates {
        let mut candidate_account = account.clone();
        let (chatgpt_account_id, workspace_id) = derive_account_meta(&token);
        if patch_account_meta_in_place(&mut candidate_account, chatgpt_account_id, workspace_id) {
            candidate_account.updated_at = now_ts();
            let _ = storage.insert_account(&candidate_account);
        }
        out.push((candidate_account, token));
    }
    if out.is_empty() {
        log_no_candidates(storage);
    }
    Ok(out)
}

fn read_candidate_cache() -> Option<Vec<(Account, Token)>> {
    let ttl = candidate_cache_ttl();
    if ttl.is_zero() {
        return None;
    }
    let db_path = cache_identity()?;
    let now = Instant::now();
    let mutex = CANDIDATE_SNAPSHOT_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            log::warn!("candidate snapshot cache lock poisoned; dropping cache and continuing");
            let mut guard = poisoned.into_inner();
            *guard = None;
            guard
        }
    };
    let cached = guard.as_ref()?;
    if cached.db_path != db_path || cached.expires_at <= now {
        *guard = None;
        return None;
    }
    Some(cached.candidates.clone())
}

fn write_candidate_cache(candidates: Vec<(Account, Token)>) {
    let ttl = candidate_cache_ttl();
    if ttl.is_zero() {
        return;
    }
    let Some(db_path) = cache_identity() else {
        return;
    };
    let expires_at = Instant::now() + ttl;
    let mutex = CANDIDATE_SNAPSHOT_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            log::warn!("candidate snapshot cache lock poisoned; recovering");
            poisoned.into_inner()
        }
    };
    *guard = Some(CandidateSnapshotCache {
        db_path,
        expires_at,
        candidates,
    });
}

fn cache_identity() -> Option<String> {
    let db_path = current_db_path();
    if db_path.trim().is_empty() || db_path == "<unset>" {
        return None;
    }
    Some(db_path)
}

fn candidate_cache_ttl() -> Duration {
    ensure_selection_config_loaded();
    let ttl_ms = CANDIDATE_CACHE_TTL_MS.load(Ordering::Relaxed);
    Duration::from_millis(ttl_ms)
}

fn current_db_path() -> String {
    ensure_selection_config_loaded();
    crate::lock_utils::read_recover(current_db_path_cell(), "current_db_path").clone()
}

fn log_no_candidates(storage: &Storage) {
    let accounts = storage.list_accounts().unwrap_or_default();
    let tokens = storage.list_tokens().unwrap_or_default();
    let snaps = storage
        .latest_usage_snapshots_by_account()
        .unwrap_or_default();
    let token_map = tokens
        .into_iter()
        .map(|token| (token.account_id.clone(), token))
        .collect::<std::collections::HashMap<_, _>>();
    let snap_map = snaps
        .into_iter()
        .map(|snap| (snap.account_id.clone(), snap))
        .collect::<std::collections::HashMap<_, _>>();
    let db_path = current_db_path();
    log::warn!(
        "gateway no candidates: db_path={}, accounts={}, tokens={}, snapshots={}",
        db_path,
        accounts.len(),
        token_map.len(),
        snap_map.len()
    );
    for account in accounts {
        let usage = snap_map.get(&account.id);
        log::warn!(
            "gateway account: id={}, status={}, has_token={}, primary=({:?}/{:?}) secondary=({:?}/{:?})",
            account.id,
            account.status,
            token_map.contains_key(&account.id),
            usage.and_then(|u| u.used_percent),
            usage.and_then(|u| u.window_minutes),
            usage.and_then(|u| u.secondary_used_percent),
            usage.and_then(|u| u.secondary_window_minutes),
        );
    }
}

pub(super) fn reload_from_env() {
    let ttl_ms = std::env::var(CANDIDATE_CACHE_TTL_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_CANDIDATE_CACHE_TTL_MS);
    CANDIDATE_CACHE_TTL_MS.store(ttl_ms, Ordering::Relaxed);

    let db_path = std::env::var("CODEXMANAGER_DB_PATH").unwrap_or_else(|_| "<unset>".to_string());
    let mut cached = crate::lock_utils::write_recover(current_db_path_cell(), "current_db_path");
    *cached = db_path;
    clear_candidate_cache();
}

fn ensure_selection_config_loaded() {
    let _ = SELECTION_CONFIG_LOADED.get_or_init(|| reload_from_env());
}

fn current_db_path_cell() -> &'static RwLock<String> {
    CURRENT_DB_PATH.get_or_init(|| RwLock::new("<unset>".to_string()))
}

fn clear_candidate_cache() {
    if let Some(mutex) = CANDIDATE_SNAPSHOT_CACHE.get() {
        let mut guard = match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                log::warn!("candidate snapshot cache lock poisoned; recovering for tests");
                poisoned.into_inner()
            }
        };
        *guard = None;
    }
}

#[cfg(test)]
fn clear_candidate_cache_for_tests() {
    clear_candidate_cache();
}

#[cfg(test)]
#[path = "tests/selection_tests.rs"]
mod tests;
