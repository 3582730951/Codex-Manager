use axum::http::HeaderMap;
use rand::RngCore;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{
    derive_affinity_lock_keys, legacy_conversation_lock_key, normalize_key_part,
    DerivedAffinityKey, IncomingHeaderSnapshot,
};

const ENV_CLIENT_ENTITY_MODE: &str = "CODEXMANAGER_CLIENT_ENTITY_MODE";
const ENV_EDGE_ENTITY_TRUSTED_CIDRS: &str = "CODEXMANAGER_EDGE_ENTITY_TRUSTED_CIDRS";
const ENV_EDGE_ENTITY_HMAC_SECRET: &str = "CODEXMANAGER_EDGE_ENTITY_HMAC_SECRET";
const ENV_PEER_RUNTIME_TRUSTED_CIDRS: &str = "CODEXMANAGER_PEER_RUNTIME_TRUSTED_CIDRS";
const ENV_PEER_RUNTIME_TTL_SECS: &str = "CODEXMANAGER_PEER_RUNTIME_TTL_SECS";

const DEFAULT_PEER_RUNTIME_TTL_SECS: i64 = 1_800;
const EDGE_ENTITY_TS_SKEW_SECS: i64 = 60;
const NONCE_CACHE_TTL_SECS: i64 = EDGE_ENTITY_TS_SKEW_SECS * 2;

pub(crate) const INTERNAL_CLIENT_ENTITY_HEADER: &str = "x-codex-internal-client-entity";
pub(crate) const INTERNAL_ENTITY_TS_HEADER: &str = "x-codex-internal-entity-ts";
pub(crate) const INTERNAL_ENTITY_NONCE_HEADER: &str = "x-codex-internal-entity-nonce";
pub(crate) const INTERNAL_ENTITY_SIG_HEADER: &str = "x-codex-internal-entity-sig";
pub(crate) const INTERNAL_ENTITY_PEER_IP_HEADER: &str = "x-codex-internal-peer-ip";
pub(crate) const INTERNAL_HOP_SIG_HEADER: &str = "x-codex-internal-hop-sig";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientEntityMode {
    Off,
    EdgeEnforced,
    DockerPeerRuntime,
}

impl Default for ClientEntityMode {
    fn default() -> Self {
        Self::Off
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ClientEntityRequestPreflight {
    pub(crate) mode: ClientEntityMode,
    pub(crate) legacy_allowed: bool,
    pub(crate) trusted_durable_affinity: Option<DerivedAffinityKey>,
    pub(crate) trusted_peer_runtime_key: Option<String>,
    pub(crate) lock_keys: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct PeerRuntimeRoutingHint {
    pub(crate) pinned_account_id: Option<String>,
}

#[derive(Debug, Clone)]
struct TrustedClientEntity {
    scheme: String,
    value: String,
}

#[derive(Debug, Clone, Default)]
struct NonceCacheState {
    entries: HashMap<String, i64>,
}

#[derive(Debug, Clone)]
struct PeerRuntimePin {
    account_id: String,
    updated_at: i64,
}

#[derive(Debug, Clone, Default)]
struct PeerRuntimeState {
    pins: HashMap<String, PeerRuntimePin>,
}

#[derive(Debug, Clone, Default)]
struct ClientEntityRuntimeConfig {
    mode: ClientEntityMode,
    edge_trusted_cidrs: Vec<IpCidr>,
    edge_hmac_secret: Option<String>,
    peer_runtime_trusted_cidrs: Vec<IpCidr>,
    peer_runtime_excluded_ips: Vec<IpAddr>,
    peer_runtime_ttl_secs: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct IpCidr {
    addr: IpAddr,
    prefix_len: u8,
}

static CLIENT_ENTITY_CONFIG: OnceLock<RwLock<ClientEntityRuntimeConfig>> = OnceLock::new();
static CLIENT_ENTITY_NONCES: OnceLock<Mutex<NonceCacheState>> = OnceLock::new();
static PEER_RUNTIME_STATE: OnceLock<Mutex<PeerRuntimeState>> = OnceLock::new();
static INTERNAL_HOP_SECRET: OnceLock<String> = OnceLock::new();

pub(crate) fn reload_from_env() {
    let requested_mode =
        match parse_client_entity_mode(std::env::var(ENV_CLIENT_ENTITY_MODE).ok().as_deref()) {
            Ok(mode) => mode,
            Err(()) => {
                log::warn!(
                    "event=client_entity_mode_invalid env={} fallback=off",
                    ENV_CLIENT_ENTITY_MODE
                );
                Some(ClientEntityMode::Off)
            }
        };
    let edge_trusted_cidrs = parse_cidr_list(std::env::var(ENV_EDGE_ENTITY_TRUSTED_CIDRS).ok());
    let edge_hmac_secret = std::env::var(ENV_EDGE_ENTITY_HMAC_SECRET)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let explicit_peer_runtime_trusted_cidrs =
        parse_cidr_list(std::env::var(ENV_PEER_RUNTIME_TRUSTED_CIDRS).ok());
    let (peer_runtime_trusted_cidrs, peer_runtime_excluded_ips) =
        resolve_peer_runtime_networks(requested_mode, explicit_peer_runtime_trusted_cidrs);
    let mode = resolve_client_entity_mode(
        requested_mode,
        edge_trusted_cidrs.as_slice(),
        edge_hmac_secret.as_deref(),
        peer_runtime_trusted_cidrs.as_slice(),
    );
    let config = ClientEntityRuntimeConfig {
        mode,
        edge_trusted_cidrs,
        edge_hmac_secret,
        peer_runtime_trusted_cidrs,
        peer_runtime_excluded_ips,
        peer_runtime_ttl_secs: std::env::var(ENV_PEER_RUNTIME_TTL_SECS)
            .ok()
            .and_then(|value| value.trim().parse::<i64>().ok())
            .unwrap_or(DEFAULT_PEER_RUNTIME_TTL_SECS)
            .max(1),
    };
    *crate::lock_utils::write_recover(config_lock(), "client_entity_config") = config;
    crate::lock_utils::lock_recover(peer_runtime_state(), "client_entity_peer_runtime_state")
        .pins
        .clear();
}

pub(crate) fn current_mode() -> ClientEntityMode {
    crate::lock_utils::read_recover(config_lock(), "client_entity_config").mode
}

pub(crate) fn runtime_peer_ttl_secs() -> i64 {
    crate::lock_utils::read_recover(config_lock(), "client_entity_config").peer_runtime_ttl_secs
}

pub(crate) fn should_strip_external_cli_affinity_id() -> bool {
    current_mode() == ClientEntityMode::EdgeEnforced
}

pub(crate) fn trusted_peer_runtime_entity(socket_peer_ip: IpAddr) -> Option<String> {
    let config = crate::lock_utils::read_recover(config_lock(), "client_entity_config").clone();
    if config
        .peer_runtime_excluded_ips
        .iter()
        .any(|candidate| *candidate == socket_peer_ip)
    {
        return None;
    }
    if !config
        .peer_runtime_trusted_cidrs
        .iter()
        .any(|cidr| cidr.contains(socket_peer_ip))
    {
        return None;
    }
    Some(format!("peerip:{socket_peer_ip}"))
}

pub(crate) fn prepare_request_preflight(
    incoming_headers: &IncomingHeaderSnapshot,
    platform_key_hash: &str,
    local_conversation_id: Option<&str>,
) -> ClientEntityRequestPreflight {
    let mode = current_mode();
    let trusted_entity = trusted_internal_client_entity(incoming_headers);
    let trusted_durable_affinity = match mode {
        ClientEntityMode::EdgeEnforced => trusted_entity
            .as_ref()
            .filter(|entity| entity.scheme == "mtlsfp" || entity.scheme == "wgpeer")
            .map(|entity| DerivedAffinityKey {
                key: format!(
                    "ent:{}:{}:{}",
                    entity.scheme, platform_key_hash, entity.value
                ),
                source: INTERNAL_CLIENT_ENTITY_HEADER,
            }),
        _ => None,
    };
    let trusted_peer_runtime_key = match mode {
        ClientEntityMode::DockerPeerRuntime => trusted_entity
            .as_ref()
            .filter(|entity| entity.scheme == "peerip")
            .map(|entity| format!("peer:{platform_key_hash}:{}", entity.value)),
        _ => None,
    };
    let mut lock_keys = BTreeSet::new();
    match mode {
        ClientEntityMode::EdgeEnforced => {
            if let Some(derived) = trusted_durable_affinity.as_ref() {
                lock_keys.insert(derived.key.clone());
                for key in derive_observed_lock_keys(incoming_headers, local_conversation_id) {
                    lock_keys.insert(key);
                }
            }
        }
        ClientEntityMode::DockerPeerRuntime => {
            for key in derive_affinity_lock_keys(incoming_headers, local_conversation_id) {
                lock_keys.insert(key);
            }
            if let Some(runtime_key) = trusted_peer_runtime_key.as_ref() {
                lock_keys.insert(runtime_key.clone());
            }
        }
        ClientEntityMode::Off => {
            for key in derive_affinity_lock_keys(incoming_headers, local_conversation_id) {
                lock_keys.insert(key);
            }
        }
    }
    ClientEntityRequestPreflight {
        mode,
        legacy_allowed: mode == ClientEntityMode::Off,
        trusted_durable_affinity,
        trusted_peer_runtime_key,
        lock_keys: lock_keys.into_iter().collect(),
    }
}

pub(crate) fn resolve_peer_runtime_hint(
    runtime_key: Option<&str>,
) -> Option<PeerRuntimeRoutingHint> {
    let runtime_key = runtime_key
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    Some(PeerRuntimeRoutingHint {
        pinned_account_id: lookup_peer_runtime_pin(runtime_key.as_str()),
    })
}

pub(crate) fn record_peer_runtime_success(runtime_key: &str, account_id: &str) {
    let now = now_ts();
    let ttl = runtime_peer_ttl_secs();
    let mut state =
        crate::lock_utils::lock_recover(peer_runtime_state(), "client_entity_peer_runtime_state");
    prune_peer_runtime_pins(&mut state, now, ttl);
    state.pins.insert(
        runtime_key.to_string(),
        PeerRuntimePin {
            account_id: account_id.to_string(),
            updated_at: now,
        },
    );
}

pub(crate) fn filter_and_validate_edge_entity_headers(
    headers: &HeaderMap,
    method: &str,
    path: &str,
    socket_peer_ip: IpAddr,
) -> Result<Option<String>, String> {
    let entity = exact_one_optional_header(headers, INTERNAL_CLIENT_ENTITY_HEADER)?;
    let ts = exact_one_optional_header(headers, INTERNAL_ENTITY_TS_HEADER)?;
    let nonce = exact_one_optional_header(headers, INTERNAL_ENTITY_NONCE_HEADER)?;
    let sig = exact_one_optional_header(headers, INTERNAL_ENTITY_SIG_HEADER)?;
    if entity.is_none() && ts.is_none() && nonce.is_none() && sig.is_none() {
        return Ok(None);
    }
    let Some(entity) = entity else {
        return Ok(None);
    };
    let Some(ts) = ts else {
        return Ok(None);
    };
    let Some(nonce) = nonce else {
        return Ok(None);
    };
    let Some(sig) = sig else {
        return Ok(None);
    };
    let config = crate::lock_utils::read_recover(config_lock(), "client_entity_config").clone();
    if !config
        .edge_trusted_cidrs
        .iter()
        .any(|cidr| cidr.contains(socket_peer_ip))
    {
        return Ok(None);
    }
    let Some(secret) = config.edge_hmac_secret.as_deref() else {
        return Ok(None);
    };
    let Some(timestamp) = ts.trim().parse::<i64>().ok() else {
        return Ok(None);
    };
    let now = now_ts();
    if (timestamp - now).abs() > EDGE_ENTITY_TS_SKEW_SECS {
        return Ok(None);
    }
    let canonical = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method.trim().to_ascii_uppercase(),
        normalize_path_for_signature(path),
        entity.trim(),
        timestamp,
        nonce.trim(),
        socket_peer_ip
    );
    let expected = sign_payload(secret, canonical.as_bytes());
    if !constant_time_eq_hex(expected.as_str(), sig.trim()) {
        return Ok(None);
    }
    if !mark_nonce_once(format!("{}:{}", socket_peer_ip, nonce.trim()).as_str(), now) {
        return Ok(None);
    }
    let Some((scheme, value)) = parse_entity_parts(entity.trim()) else {
        return Ok(None);
    };
    if !(scheme == "mtlsfp" || scheme == "wgpeer") {
        return Ok(None);
    }
    Ok(Some(format!("{scheme}:{value}")))
}

pub(crate) fn sign_internal_hop(entity: &str, peer_ip: &str) -> String {
    let secret = internal_hop_secret();
    let canonical = format!("{}\n{}", entity.trim(), peer_ip.trim());
    sign_payload(secret.as_str(), canonical.as_bytes())
}

fn trusted_internal_client_entity(
    incoming_headers: &IncomingHeaderSnapshot,
) -> Option<TrustedClientEntity> {
    let entity = incoming_headers.internal_client_entity()?;
    let peer_ip = incoming_headers.internal_peer_ip()?;
    let sig = incoming_headers.internal_hop_sig()?;
    let expected = sign_internal_hop(entity, peer_ip);
    if !constant_time_eq_hex(expected.as_str(), sig) {
        return None;
    }
    let (scheme, value) = parse_entity_parts(entity)?;
    Some(TrustedClientEntity { scheme, value })
}

fn derive_observed_lock_keys(
    incoming_headers: &IncomingHeaderSnapshot,
    local_conversation_id: Option<&str>,
) -> Vec<String> {
    let conversation_id = local_conversation_id.or(incoming_headers.conversation_id());
    let mut keys = BTreeSet::new();
    if let Some(value) = normalize_key_part(incoming_headers.cli_affinity_id()) {
        keys.insert(format!("cli:{value}"));
    }
    if let Some(value) = normalize_key_part(conversation_id) {
        keys.insert(format!("cid:{value}"));
    }
    if let Some(value) = normalize_key_part(incoming_headers.session_id()) {
        keys.insert(format!("sid:{value}"));
    }
    if let Some(value) = normalize_key_part(incoming_headers.subagent()) {
        keys.insert(format!("sub:{value}"));
    }
    if let Some(value) = normalize_key_part(incoming_headers.client_request_id()) {
        keys.insert(format!("rid:{value}"));
    }
    if let Some(legacy_key) = legacy_conversation_lock_key(conversation_id) {
        keys.insert(legacy_key);
    }
    keys.into_iter().collect()
}

fn lookup_peer_runtime_pin(runtime_key: &str) -> Option<String> {
    let now = now_ts();
    let ttl = runtime_peer_ttl_secs();
    let mut state =
        crate::lock_utils::lock_recover(peer_runtime_state(), "client_entity_peer_runtime_state");
    prune_peer_runtime_pins(&mut state, now, ttl);
    state
        .pins
        .get(runtime_key)
        .map(|entry| entry.account_id.clone())
}

fn prune_peer_runtime_pins(state: &mut PeerRuntimeState, now: i64, ttl_secs: i64) {
    state
        .pins
        .retain(|_, entry| now.saturating_sub(entry.updated_at) <= ttl_secs);
}

fn exact_one_optional_header(
    headers: &HeaderMap,
    name: &'static str,
) -> Result<Option<String>, String> {
    let values = headers.get_all(name).iter().collect::<Vec<_>>();
    if values.is_empty() {
        return Ok(None);
    }
    if values.len() != 1 {
        return Err(format!("duplicate internal header: {name}"));
    }
    values[0]
        .to_str()
        .ok()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .map(Some)
        .ok_or_else(|| format!("invalid internal header value: {name}"))
}

fn parse_client_entity_mode(raw: Option<&str>) -> Result<Option<ClientEntityMode>, ()> {
    match raw
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "" => Ok(Some(ClientEntityMode::Off)),
        "auto" => Ok(None),
        "off" | "disabled" => Ok(Some(ClientEntityMode::Off)),
        "edge-enforced" | "edge" | "enforced" => Ok(Some(ClientEntityMode::EdgeEnforced)),
        "docker-peer-runtime" | "peer-runtime" | "docker-peerip" => {
            Ok(Some(ClientEntityMode::DockerPeerRuntime))
        }
        _ => Err(()),
    }
}

fn resolve_client_entity_mode(
    requested_mode: Option<ClientEntityMode>,
    edge_trusted_cidrs: &[IpCidr],
    edge_hmac_secret: Option<&str>,
    peer_runtime_trusted_cidrs: &[IpCidr],
) -> ClientEntityMode {
    match requested_mode {
        Some(ClientEntityMode::Off) => ClientEntityMode::Off,
        Some(ClientEntityMode::EdgeEnforced) => ClientEntityMode::EdgeEnforced,
        Some(ClientEntityMode::DockerPeerRuntime) => {
            if peer_runtime_trusted_cidrs.is_empty() {
                ClientEntityMode::Off
            } else {
                ClientEntityMode::DockerPeerRuntime
            }
        }
        None => {
            let edge_ready = !edge_trusted_cidrs.is_empty()
                && edge_hmac_secret.is_some_and(|value| !value.trim().is_empty());
            if edge_ready {
                ClientEntityMode::EdgeEnforced
            } else if !peer_runtime_trusted_cidrs.is_empty() {
                ClientEntityMode::DockerPeerRuntime
            } else {
                ClientEntityMode::Off
            }
        }
    }
}

fn resolve_peer_runtime_networks(
    requested_mode: Option<ClientEntityMode>,
    explicit_peer_runtime_trusted_cidrs: Vec<IpCidr>,
) -> (Vec<IpCidr>, Vec<IpAddr>) {
    if !explicit_peer_runtime_trusted_cidrs.is_empty() {
        return (
            explicit_peer_runtime_trusted_cidrs,
            auto_detect_peer_runtime_excluded_ips(),
        );
    }
    match requested_mode {
        Some(ClientEntityMode::Off) | Some(ClientEntityMode::EdgeEnforced) => {
            (Vec::new(), Vec::new())
        }
        Some(ClientEntityMode::DockerPeerRuntime) | None => auto_detect_peer_runtime_networks(),
    }
}

fn auto_detect_peer_runtime_networks() -> (Vec<IpCidr>, Vec<IpAddr>) {
    if !is_containerized_runtime() {
        return (Vec::new(), Vec::new());
    }
    let cidrs = auto_detect_private_interface_cidrs();
    if cidrs.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let excluded_ips = auto_detect_peer_runtime_excluded_ips();
    (cidrs, excluded_ips)
}

fn is_containerized_runtime() -> bool {
    if Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists() {
        return true;
    }
    fs::read_to_string("/proc/1/cgroup")
        .ok()
        .is_some_and(|value| {
            let normalized = value.to_ascii_lowercase();
            normalized.contains("docker")
                || normalized.contains("containerd")
                || normalized.contains("kubepods")
                || normalized.contains("podman")
        })
}

fn auto_detect_private_interface_cidrs() -> Vec<IpCidr> {
    let mut cidrs = BTreeSet::new();
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    for interface in interfaces {
        if interface.is_loopback() {
            continue;
        }
        let if_addrs::IfAddr::V4(v4) = interface.addr else {
            continue;
        };
        if !v4.ip.is_private() {
            continue;
        }
        let prefix_len = ipv4_prefix_len(v4.netmask);
        if prefix_len == 0 {
            continue;
        }
        cidrs.insert(IpCidr {
            addr: IpAddr::V4(v4.ip),
            prefix_len,
        });
    }
    cidrs.into_iter().collect()
}

fn ipv4_prefix_len(netmask: std::net::Ipv4Addr) -> u8 {
    u32::from(netmask).count_ones() as u8
}

fn auto_detect_default_gateway_ips() -> Vec<IpAddr> {
    let Ok(contents) = fs::read_to_string("/proc/net/route") else {
        return Vec::new();
    };
    parse_proc_net_route_gateways(contents.as_str())
}

fn auto_detect_peer_runtime_excluded_ips() -> Vec<IpAddr> {
    if !is_containerized_runtime() {
        return Vec::new();
    }
    auto_detect_default_gateway_ips()
}

fn parse_proc_net_route_gateways(contents: &str) -> Vec<IpAddr> {
    let mut gateways = BTreeSet::new();
    for line in contents.lines().skip(1) {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 3 || columns[1] != "00000000" {
            continue;
        }
        let Ok(raw_gateway) = u32::from_str_radix(columns[2], 16) else {
            continue;
        };
        let octets = raw_gateway.to_le_bytes();
        gateways.insert(IpAddr::V4(std::net::Ipv4Addr::from(octets)));
    }
    gateways.into_iter().collect()
}

fn config_lock() -> &'static RwLock<ClientEntityRuntimeConfig> {
    CLIENT_ENTITY_CONFIG.get_or_init(|| RwLock::new(ClientEntityRuntimeConfig::default()))
}

fn nonce_state() -> &'static Mutex<NonceCacheState> {
    CLIENT_ENTITY_NONCES.get_or_init(|| Mutex::new(NonceCacheState::default()))
}

fn peer_runtime_state() -> &'static Mutex<PeerRuntimeState> {
    PEER_RUNTIME_STATE.get_or_init(|| Mutex::new(PeerRuntimeState::default()))
}

fn mark_nonce_once(key: &str, now: i64) -> bool {
    let mut state = crate::lock_utils::lock_recover(nonce_state(), "client_entity_nonce_state");
    state
        .entries
        .retain(|_, seen_at| now.saturating_sub(*seen_at) <= NONCE_CACHE_TTL_SECS);
    if state.entries.contains_key(key) {
        return false;
    }
    state.entries.insert(key.to_string(), now);
    true
}

fn internal_hop_secret() -> String {
    INTERNAL_HOP_SECRET
        .get_or_init(|| {
            let mut bytes = [0_u8; 32];
            rand::thread_rng().fill_bytes(&mut bytes);
            hex_encode(&bytes)
        })
        .clone()
}

fn sign_payload(secret: &str, payload: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    let mut key = secret.as_bytes().to_vec();
    if key.len() > 64 {
        key = Sha256::digest(key.as_slice()).to_vec();
    }
    key.resize(64, 0);

    let mut ipad = vec![0x36_u8; 64];
    let mut opad = vec![0x5c_u8; 64];
    for (idx, byte) in key.iter().enumerate() {
        ipad[idx] ^= *byte;
        opad[idx] ^= *byte;
    }

    let mut inner = Sha256::new();
    inner.update(ipad.as_slice());
    inner.update(payload);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad.as_slice());
    outer.update(inner_digest);
    hex_encode(&outer.finalize())
}

fn constant_time_eq_hex(left: &str, right: &str) -> bool {
    let left = left.trim().as_bytes();
    let right = right.trim().as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (l, r) in left.iter().zip(right.iter()) {
        diff |= l ^ r;
    }
    diff == 0
}

fn normalize_path_for_signature(path: &str) -> &str {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        "/"
    } else {
        trimmed
    }
}

fn parse_entity_parts(raw: &str) -> Option<(String, String)> {
    let (scheme, value) = raw.split_once(':')?;
    let scheme = scheme.trim().to_ascii_lowercase();
    let value = value.trim();
    if scheme.is_empty() || value.is_empty() {
        return None;
    }
    Some((scheme, value.to_string()))
}

fn parse_cidr_list(raw: Option<String>) -> Vec<IpCidr> {
    raw.unwrap_or_default()
        .split(',')
        .filter_map(|item| IpCidr::parse(item.trim()))
        .collect()
}

impl IpCidr {
    fn parse(raw: &str) -> Option<Self> {
        if raw.is_empty() {
            return None;
        }
        if let Some((addr, prefix_len)) = raw.split_once('/') {
            let addr = addr.trim().parse::<IpAddr>().ok()?;
            let prefix_len = prefix_len.trim().parse::<u8>().ok()?;
            let max_prefix = match addr {
                IpAddr::V4(_) => 32,
                IpAddr::V6(_) => 128,
            };
            if prefix_len > max_prefix {
                return None;
            }
            return Some(Self { addr, prefix_len });
        }
        raw.parse::<IpAddr>().ok().map(|addr| Self {
            prefix_len: match addr {
                IpAddr::V4(_) => 32,
                IpAddr::V6(_) => 128,
            },
            addr,
        })
    }

    fn contains(&self, candidate: IpAddr) -> bool {
        match (self.addr, candidate) {
            (IpAddr::V4(expected), IpAddr::V4(candidate)) => {
                let expected = u32::from(expected);
                let candidate = u32::from(candidate);
                let shift = 32_u32.saturating_sub(u32::from(self.prefix_len));
                let mask = if self.prefix_len == 0 {
                    0
                } else {
                    u32::MAX << shift
                };
                (expected & mask) == (candidate & mask)
            }
            (IpAddr::V6(expected), IpAddr::V6(candidate)) => {
                let expected = u128::from(expected);
                let candidate = u128::from(candidate);
                let shift = 128_u32.saturating_sub(u32::from(self.prefix_len));
                let mask = if self.prefix_len == 0 {
                    0
                } else {
                    u128::MAX << shift
                };
                (expected & mask) == (candidate & mask)
            }
            _ => false,
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        constant_time_eq_hex, filter_and_validate_edge_entity_headers, parse_client_entity_mode,
        sign_internal_hop, sign_payload, ClientEntityMode, INTERNAL_CLIENT_ENTITY_HEADER,
        INTERNAL_ENTITY_NONCE_HEADER, INTERNAL_ENTITY_SIG_HEADER, INTERNAL_ENTITY_TS_HEADER,
    };
    use axum::http::{HeaderMap, HeaderValue};
    use std::net::{IpAddr, Ipv4Addr};

    struct EnvGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.original {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn parse_client_entity_mode_accepts_expected_values() {
        assert_eq!(
            parse_client_entity_mode(Some("edge-enforced")),
            Ok(Some(ClientEntityMode::EdgeEnforced))
        );
        assert_eq!(
            parse_client_entity_mode(Some("docker-peer-runtime")),
            Ok(Some(ClientEntityMode::DockerPeerRuntime))
        );
        assert_eq!(
            parse_client_entity_mode(None),
            Ok(Some(ClientEntityMode::Off))
        );
        assert_eq!(parse_client_entity_mode(Some("auto")), Ok(None));
        assert_eq!(
            parse_client_entity_mode(Some("off")),
            Ok(Some(ClientEntityMode::Off))
        );
        assert!(parse_client_entity_mode(Some("bad-mode")).is_err());
    }

    #[test]
    fn resolve_client_entity_mode_prefers_edge_for_auto() {
        let actual = super::resolve_client_entity_mode(
            None,
            &[super::IpCidr {
                addr: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 0)),
                prefix_len: 24,
            }],
            Some("secret"),
            &[super::IpCidr {
                addr: IpAddr::V4(Ipv4Addr::new(172, 18, 0, 0)),
                prefix_len: 16,
            }],
        );
        assert_eq!(actual, ClientEntityMode::EdgeEnforced);
    }

    #[test]
    fn resolve_client_entity_mode_uses_peer_runtime_for_auto_without_edge() {
        let actual = super::resolve_client_entity_mode(
            None,
            &[],
            None,
            &[super::IpCidr {
                addr: IpAddr::V4(Ipv4Addr::new(172, 18, 0, 0)),
                prefix_len: 16,
            }],
        );
        assert_eq!(actual, ClientEntityMode::DockerPeerRuntime);
    }

    #[test]
    fn parse_proc_net_route_gateways_reads_default_gateway() {
        let actual = super::parse_proc_net_route_gateways(
            "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\neth0\t00000000\t010012AC\t0003\t0\t0\t0\t00000000\t0\t0\t0\n",
        );
        assert_eq!(actual, vec![IpAddr::V4(Ipv4Addr::new(172, 18, 0, 1))]);
    }

    #[test]
    fn hop_signature_is_stable_for_same_inputs() {
        let left = sign_internal_hop("peerip:172.18.0.8", "172.18.0.8");
        let right = sign_internal_hop("peerip:172.18.0.8", "172.18.0.8");
        assert_eq!(left, right);
    }

    #[test]
    fn constant_time_eq_hex_rejects_different_values() {
        assert!(!constant_time_eq_hex("abcd", "abce"));
    }

    #[test]
    fn edge_entity_validation_accepts_valid_signed_header() {
        let _mode = EnvGuard::set(super::ENV_CLIENT_ENTITY_MODE, "edge-enforced");
        let _cidrs = EnvGuard::set(super::ENV_EDGE_ENTITY_TRUSTED_CIDRS, "10.0.0.0/24");
        let _secret = EnvGuard::set(super::ENV_EDGE_ENTITY_HMAC_SECRET, "top-secret");
        super::reload_from_env();

        let peer_ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 4));
        let ts = super::now_ts().to_string();
        let nonce = "nonce-1";
        let canonical = format!(
            "POST\n/v1/responses\nmtlsfp:client-a\n{}\n{}\n{}",
            ts, nonce, peer_ip
        );
        let sig = sign_payload("top-secret", canonical.as_bytes());

        let mut headers = HeaderMap::new();
        headers.insert(
            INTERNAL_CLIENT_ENTITY_HEADER,
            HeaderValue::from_static("mtlsfp:client-a"),
        );
        headers.insert(
            INTERNAL_ENTITY_TS_HEADER,
            HeaderValue::from_str(ts.as_str()).expect("ts"),
        );
        headers.insert(
            INTERNAL_ENTITY_NONCE_HEADER,
            HeaderValue::from_static("nonce-1"),
        );
        headers.insert(
            INTERNAL_ENTITY_SIG_HEADER,
            HeaderValue::from_str(sig.as_str()).expect("sig"),
        );

        let actual =
            filter_and_validate_edge_entity_headers(&headers, "POST", "/v1/responses", peer_ip)
                .expect("validation result");
        assert_eq!(actual.as_deref(), Some("mtlsfp:client-a"));
    }

    #[test]
    fn reload_from_env_clears_peer_runtime_pins() {
        let _mode = EnvGuard::set(super::ENV_CLIENT_ENTITY_MODE, "docker-peer-runtime");
        let _cidrs = EnvGuard::set(super::ENV_PEER_RUNTIME_TRUSTED_CIDRS, "172.18.0.0/16");
        super::reload_from_env();

        super::record_peer_runtime_success("peer:pk:172.18.0.8", "acc-1");
        let before = super::resolve_peer_runtime_hint(Some("peer:pk:172.18.0.8"))
            .expect("hint before reload");
        assert_eq!(before.pinned_account_id.as_deref(), Some("acc-1"));

        super::reload_from_env();

        let after = super::resolve_peer_runtime_hint(Some("peer:pk:172.18.0.8"))
            .expect("hint after reload");
        assert!(after.pinned_account_id.is_none());
    }
}
