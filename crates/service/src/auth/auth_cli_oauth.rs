use base64::Engine;
use codexmanager_core::storage::{now_ts, ApiKey, CliChildKey, CliOAuthSession};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tiny_http::{Header, Request, Response, StatusCode};
use url::Url;

use crate::storage_helpers::{
    generate_key_id, generate_platform_key, hash_platform_key, open_storage,
};

const AUTHORIZATION_CODE_EXPIRES_SECS: i64 = 10 * 60;
const ACCESS_TOKEN_EXPIRES_SECS: i64 = 365 * 24 * 60 * 60;
const REFRESH_TOKEN_EXPIRES_SECS: i64 = 90 * 24 * 60 * 60;
const OAUTH_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const DEVICE_FLOW_ERROR: &str = "unsupported_device_flow";
const DEVICE_FLOW_ERROR_DESCRIPTION: &str =
    "device flow is not supported by CodexManager CLI OAuth proxy";

#[derive(Debug, Clone)]
struct AuthorizeParams {
    client_id: String,
    redirect_uri: String,
    state: String,
    code_challenge: String,
    code_challenge_method: String,
    scope: Option<String>,
}

#[derive(Debug, Clone)]
struct ParentKeyBinding {
    owner_key_id: String,
    child_key_id: String,
    cli_instance_uuid: String,
}

#[derive(Debug, Deserialize)]
struct TokenExchangeForm {
    grant_type: String,
    code: Option<String>,
    redirect_uri: Option<String>,
    client_id: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
    requested_token: Option<String>,
    subject_token: Option<String>,
    subject_token_type: Option<String>,
}

pub(crate) fn handle_authorize_request(request: Request) -> Result<(), String> {
    let params = match parse_authorize_request(&request) {
        Ok(params) => params,
        Err(err) => {
            let _ = request.respond(text_response(400, err.as_str()));
            return Ok(());
        }
    };
    let password_required = crate::auth::web_access_password_configured();
    let issuer_base = issuer_base_url(&request);
    let body = build_authorize_page(&params, issuer_base.as_deref(), password_required, None);
    let _ = request.respond(html_response(200, body));
    Ok(())
}

pub(crate) fn handle_authorize_approve_request(mut request: Request) -> Result<(), String> {
    if request.method().as_str() != "POST" {
        let _ = request.respond(text_response(405, "method not allowed"));
        return Ok(());
    }
    let body = read_request_body(&mut request)?;
    let form = parse_form_map(&body);
    let params = match parse_authorize_form(&form) {
        Ok(params) => params,
        Err(err) => {
            let _ = request.respond(text_response(400, err.as_str()));
            return Ok(());
        }
    };
    let password_required = crate::auth::web_access_password_configured();
    if password_required {
        let provided_password = form
            .get("web_password")
            .map(String::as_str)
            .unwrap_or_default();
        if !crate::auth::verify_web_access_password(provided_password) {
            let body = build_authorize_page(
                &params,
                issuer_base_url(&request).as_deref(),
                password_required,
                Some("Invalid web access password."),
            );
            let _ = request.respond(html_response(403, body));
            return Ok(());
        }
    }
    let employee_api_key = form
        .get("employee_api_key")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing employee_api_key".to_string())?;

    let storage = open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    let binding = resolve_parent_key_binding(&storage, employee_api_key)?;
    let now = now_ts();
    let authorization_code = generate_platform_key();
    let refresh_token = generate_platform_key();
    let session_id = format!("sess_{}", generate_platform_key());
    let id_token = build_id_token(
        params.client_id.as_str(),
        binding.cli_instance_uuid.as_str(),
        session_id.as_str(),
        now,
        now + AUTHORIZATION_CODE_EXPIRES_SECS,
        issuer_base_url(&request).as_deref(),
    )?;
    let session = CliOAuthSession {
        session_id,
        child_key_id: binding.child_key_id.clone(),
        owner_key_id: binding.owner_key_id.clone(),
        cli_instance_uuid: binding.cli_instance_uuid.clone(),
        client_id: params.client_id.clone(),
        redirect_uri: params.redirect_uri.clone(),
        pkce_challenge: params.code_challenge.clone(),
        pkce_method: params.code_challenge_method.clone(),
        state: params.state.clone(),
        authorization_code_hash: Some(hash_platform_key(&authorization_code)),
        refresh_token_hash: hash_platform_key(&refresh_token),
        status: "authorized".to_string(),
        id_token,
        expires_at: now + AUTHORIZATION_CODE_EXPIRES_SECS,
        refresh_expires_at: now + REFRESH_TOKEN_EXPIRES_SECS,
        created_at: now,
        updated_at: now,
        last_seen_at: now,
    };
    storage
        .save_cli_oauth_session(&session)
        .map_err(|err| format!("save cli oauth session failed: {err}"))?;

    let mut redirect = validate_cli_redirect_uri(&params.redirect_uri)?;
    {
        let mut pairs = redirect.query_pairs_mut();
        pairs.append_pair("code", authorization_code.as_str());
        pairs.append_pair("state", params.state.as_str());
    }
    let _ = request.respond(redirect_response(redirect.as_str()));
    Ok(())
}

pub(crate) fn handle_token_request(mut request: Request) -> Result<(), String> {
    if request.method().as_str() != "POST" {
        let _ = request.respond(json_error_response(
            405,
            "unsupported_method",
            "method not allowed",
        ));
        return Ok(());
    }
    let body = read_request_body(&mut request)?;
    let token_form_map = parse_form_map(&body);
    let form = TokenExchangeForm {
        grant_type: required_form_value(&token_form_map, "grant_type")?,
        code: optional_form_value(&token_form_map, "code"),
        redirect_uri: optional_form_value(&token_form_map, "redirect_uri"),
        client_id: optional_form_value(&token_form_map, "client_id"),
        code_verifier: optional_form_value(&token_form_map, "code_verifier"),
        refresh_token: optional_form_value(&token_form_map, "refresh_token"),
        requested_token: optional_form_value(&token_form_map, "requested_token"),
        subject_token: optional_form_value(&token_form_map, "subject_token"),
        subject_token_type: optional_form_value(&token_form_map, "subject_token_type"),
    };
    let storage = open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    match form.grant_type.as_str() {
        "authorization_code" => respond_authorization_code_token(request, &storage, form),
        "refresh_token" => respond_refresh_token(request, &storage, form),
        "urn:ietf:params:oauth:grant-type:token-exchange" => {
            respond_token_exchange(request, &storage, form)
        }
        _ => {
            let _ = request.respond(json_error_response(
                400,
                "unsupported_grant_type",
                "unsupported grant_type",
            ));
            Ok(())
        }
    }
}

pub(crate) fn handle_device_usercode_request(request: Request) -> Result<(), String> {
    let _ = request.respond(json_error_response(
        501,
        DEVICE_FLOW_ERROR,
        DEVICE_FLOW_ERROR_DESCRIPTION,
    ));
    Ok(())
}

pub(crate) fn handle_device_token_request(request: Request) -> Result<(), String> {
    let _ = request.respond(json_error_response(
        501,
        DEVICE_FLOW_ERROR,
        DEVICE_FLOW_ERROR_DESCRIPTION,
    ));
    Ok(())
}

pub(crate) fn handle_device_verify_request(request: Request) -> Result<(), String> {
    let _ = request.respond(html_response(
        501,
        "<!doctype html><html><body><p>Device flow is not supported.</p></body></html>".to_string(),
    ));
    Ok(())
}

fn respond_authorization_code_token(
    request: Request,
    storage: &crate::storage_helpers::StorageHandle,
    form: TokenExchangeForm,
) -> Result<(), String> {
    let code = form
        .code
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing authorization code".to_string())?;
    let redirect_uri = form
        .redirect_uri
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing redirect_uri".to_string())?;
    let client_id = form
        .client_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing client_id".to_string())?;
    let code_verifier = form
        .code_verifier
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing code_verifier".to_string())?;
    let Some(mut session) = storage
        .find_cli_oauth_session_by_authorization_code_hash(hash_platform_key(code).as_str())
        .map_err(|err| format!("read oauth session failed: {err}"))?
    else {
        let _ = request.respond(json_error_response(
            400,
            "invalid_grant",
            "authorization code is invalid or expired",
        ));
        return Ok(());
    };
    if session.client_id != client_id || session.redirect_uri != redirect_uri {
        let _ = request.respond(json_error_response(
            400,
            "invalid_grant",
            "authorization code does not match client",
        ));
        return Ok(());
    }
    if session.pkce_method != "S256"
        || pkce_challenge_for_verifier(code_verifier)? != session.pkce_challenge
    {
        let _ = request.respond(json_error_response(
            400,
            "invalid_grant",
            "PKCE verification failed",
        ));
        return Ok(());
    }
    let now = now_ts();
    if let Err(message) =
        validate_cli_oauth_session(storage, &session, CliOAuthGrant::AuthorizationCode, now)
    {
        let _ = request.respond(json_error_response(400, "invalid_grant", message.as_str()));
        return Ok(());
    }
    let refresh_token = generate_platform_key();
    let access_expires_at = now + ACCESS_TOKEN_EXPIRES_SECS;
    session.id_token = build_id_token(
        session.client_id.as_str(),
        session.cli_instance_uuid.as_str(),
        session.session_id.as_str(),
        now,
        access_expires_at,
        issuer_base_url(&request).as_deref(),
    )?;
    session.authorization_code_hash = None;
    session.refresh_token_hash = hash_platform_key(&refresh_token);
    session.status = "active".to_string();
    session.expires_at = access_expires_at;
    session.updated_at = now;
    session.last_seen_at = now;
    storage
        .save_cli_oauth_session(&session)
        .map_err(|err| format!("update oauth session failed: {err}"))?;
    let access_token = build_access_token(
        session.client_id.as_str(),
        session.cli_instance_uuid.as_str(),
        session.session_id.as_str(),
        now,
        access_expires_at,
        issuer_base_url(&request).as_deref(),
    )?;
    let _ = request.respond(json_response(
        200,
        serde_json::json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "expires_in": ACCESS_TOKEN_EXPIRES_SECS,
            "refresh_token": refresh_token,
            "id_token": session.id_token,
            "scope": OAUTH_SCOPE,
            "account_id": format!("cli:{}", session.cli_instance_uuid),
        }),
    ));
    Ok(())
}

fn respond_refresh_token(
    request: Request,
    storage: &crate::storage_helpers::StorageHandle,
    form: TokenExchangeForm,
) -> Result<(), String> {
    let refresh_token = form
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing refresh_token".to_string())?;
    let Some(mut session) = storage
        .find_cli_oauth_session_by_refresh_token_hash(hash_platform_key(refresh_token).as_str())
        .map_err(|err| format!("read oauth session failed: {err}"))?
    else {
        let _ = request.respond(json_error_response(
            400,
            "invalid_grant",
            "refresh token is invalid or expired",
        ));
        return Ok(());
    };
    if let Some(client_id) = form.client_id.as_deref() {
        let client_id = client_id.trim();
        if !client_id.is_empty() && client_id != session.client_id {
            let _ = request.respond(json_error_response(
                400,
                "invalid_grant",
                "refresh token does not match client",
            ));
            return Ok(());
        }
    }
    let now = now_ts();
    if let Err(message) =
        validate_cli_oauth_session(storage, &session, CliOAuthGrant::RefreshToken, now)
    {
        let _ = request.respond(json_error_response(400, "invalid_grant", message.as_str()));
        return Ok(());
    }
    let rotated_refresh_token = generate_platform_key();
    let access_expires_at = now + ACCESS_TOKEN_EXPIRES_SECS;
    session.id_token = build_id_token(
        session.client_id.as_str(),
        session.cli_instance_uuid.as_str(),
        session.session_id.as_str(),
        now,
        access_expires_at,
        issuer_base_url(&request).as_deref(),
    )?;
    session.refresh_token_hash = hash_platform_key(&rotated_refresh_token);
    session.status = "active".to_string();
    session.expires_at = access_expires_at;
    session.updated_at = now;
    session.last_seen_at = now;
    storage
        .save_cli_oauth_session(&session)
        .map_err(|err| format!("update oauth session failed: {err}"))?;
    let access_token = build_access_token(
        session.client_id.as_str(),
        session.cli_instance_uuid.as_str(),
        session.session_id.as_str(),
        now,
        access_expires_at,
        issuer_base_url(&request).as_deref(),
    )?;
    let _ = request.respond(json_response(
        200,
        serde_json::json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "expires_in": ACCESS_TOKEN_EXPIRES_SECS,
            "refresh_token": rotated_refresh_token,
            "id_token": session.id_token,
            "scope": OAUTH_SCOPE,
            "account_id": format!("cli:{}", session.cli_instance_uuid),
        }),
    ));
    Ok(())
}

fn respond_token_exchange(
    request: Request,
    storage: &crate::storage_helpers::StorageHandle,
    form: TokenExchangeForm,
) -> Result<(), String> {
    let requested_token = form
        .requested_token
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    let subject_token = form
        .subject_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing subject_token".to_string())?;
    let subject_token_type = form
        .subject_token_type
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if requested_token != "openai-api-key"
        || subject_token_type != "urn:ietf:params:oauth:token-type:id_token"
    {
        let _ = request.respond(json_error_response(
            400,
            "invalid_request",
            "unsupported token exchange request",
        ));
        return Ok(());
    }
    let Some(mut session) = storage
        .find_cli_oauth_session_by_id_token(subject_token)
        .map_err(|err| format!("read oauth session failed: {err}"))?
    else {
        let _ = request.respond(json_error_response(
            400,
            "invalid_grant",
            "subject token is invalid or expired",
        ));
        return Ok(());
    };
    if let Some(client_id) = form.client_id.as_deref() {
        let client_id = client_id.trim();
        if !client_id.is_empty() && client_id != session.client_id {
            let _ = request.respond(json_error_response(
                400,
                "invalid_grant",
                "subject token does not match client",
            ));
            return Ok(());
        }
    }
    let now = now_ts();
    let child_key_secret =
        match validate_cli_oauth_session(storage, &session, CliOAuthGrant::TokenExchange, now) {
            Ok(secret) => secret,
            Err(message) => {
                let _ =
                    request.respond(json_error_response(400, "invalid_grant", message.as_str()));
                return Ok(());
            }
        };
    session.status = "active".to_string();
    session.updated_at = now;
    session.last_seen_at = now;
    storage
        .save_cli_oauth_session(&session)
        .map_err(|err| format!("update oauth session failed: {err}"))?;
    let _ = request.respond(json_response(
        200,
        serde_json::json!({
            "access_token": child_key_secret,
            "token_type": "Bearer",
            "issued_token_type": "urn:ietf:params:oauth:token-type:access_token",
        }),
    ));
    Ok(())
}

fn parse_authorize_request(request: &Request) -> Result<AuthorizeParams, String> {
    let url = Url::parse(&format!("http://localhost{}", request.url()))
        .map_err(|err| format!("invalid authorize url: {err}"))?;
    parse_authorize_params(url.query_pairs().into_owned().collect())
}

fn parse_authorize_form(form: &HashMap<String, String>) -> Result<AuthorizeParams, String> {
    parse_authorize_params(form.clone())
}

fn parse_authorize_params(params: HashMap<String, String>) -> Result<AuthorizeParams, String> {
    let response_type = params
        .get("response_type")
        .map(String::as_str)
        .unwrap_or("code");
    if response_type != "code" {
        return Err("unsupported response_type".to_string());
    }
    let client_id = required_form_value(&params, "client_id")?;
    let redirect_uri = required_form_value(&params, "redirect_uri")?;
    let state = required_form_value(&params, "state")?;
    let code_challenge = required_form_value(&params, "code_challenge")?;
    let code_challenge_method = required_form_value(&params, "code_challenge_method")?;
    if code_challenge_method != "S256" {
        return Err("unsupported code_challenge_method".to_string());
    }
    validate_cli_redirect_uri(&redirect_uri)?;
    Ok(AuthorizeParams {
        client_id,
        redirect_uri,
        state,
        code_challenge,
        code_challenge_method,
        scope: params
            .get("scope")
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    })
}

fn resolve_parent_key_binding(
    storage: &crate::storage_helpers::StorageHandle,
    employee_api_key: &str,
) -> Result<ParentKeyBinding, String> {
    let key_hash = hash_platform_key(employee_api_key);
    let parent_key = storage
        .find_api_key_by_hash(&key_hash)
        .map_err(|err| format!("read api key failed: {err}"))?
        .ok_or_else(|| "invalid employee API key".to_string())?;
    if parent_key.status != "active" {
        return Err("employee API key is disabled".to_string());
    }
    if storage
        .lookup_api_key_owner_context(parent_key.id.as_str())
        .map_err(|err| format!("read api key ownership failed: {err}"))?
        .is_some()
    {
        return Err("employee API key must be a parent key, not a CLI child key".to_string());
    }
    create_child_key_for_parent(storage, &parent_key)
}

fn create_child_key_for_parent(
    storage: &crate::storage_helpers::StorageHandle,
    parent_key: &ApiKey,
) -> Result<ParentKeyBinding, String> {
    let child_key_secret = generate_platform_key();
    let child_key_id = generate_key_id();
    let now = now_ts();
    let child_key = ApiKey {
        id: child_key_id.clone(),
        name: parent_key
            .name
            .as_deref()
            .map(|name| format!("{name} CLI child"))
            .or_else(|| Some(format!("{} CLI child", parent_key.id))),
        model_slug: parent_key.model_slug.clone(),
        reasoning_effort: parent_key.reasoning_effort.clone(),
        service_tier: parent_key.service_tier.clone(),
        rotation_strategy: parent_key.rotation_strategy.clone(),
        aggregate_api_id: parent_key.aggregate_api_id.clone(),
        aggregate_api_url: None,
        client_type: parent_key.client_type.clone(),
        protocol_type: parent_key.protocol_type.clone(),
        auth_scheme: parent_key.auth_scheme.clone(),
        upstream_base_url: parent_key.upstream_base_url.clone(),
        static_headers_json: parent_key.static_headers_json.clone(),
        key_hash: hash_platform_key(&child_key_secret),
        status: parent_key.status.clone(),
        created_at: now,
        last_used_at: None,
    };
    storage
        .insert_api_key(&child_key)
        .map_err(|err| format!("insert child api key failed: {err}"))?;
    if let Err(err) = storage.upsert_api_key_secret(child_key_id.as_str(), &child_key_secret) {
        let _ = storage.delete_api_key(child_key_id.as_str());
        return Err(format!("persist child api key secret failed: {err}"));
    }
    let cli_instance_uuid = generate_cli_instance_uuid();
    let child_link = CliChildKey {
        child_key_id: child_key_id.clone(),
        owner_key_id: parent_key.id.clone(),
        cli_instance_uuid: cli_instance_uuid.clone(),
        status: "active".to_string(),
        created_at: now,
        updated_at: now,
        last_seen_at: now,
    };
    if let Err(err) = storage.save_cli_child_key(&child_link) {
        let _ = storage.delete_api_key(child_key_id.as_str());
        return Err(format!("persist cli child key failed: {err}"));
    }
    Ok(ParentKeyBinding {
        owner_key_id: parent_key.id.clone(),
        child_key_id,
        cli_instance_uuid,
    })
}

fn resolve_child_key_secret(
    storage: &crate::storage_helpers::StorageHandle,
    child_key_id: &str,
) -> Result<String, String> {
    storage
        .find_api_key_secret_by_id(child_key_id)
        .map_err(|err| format!("read child api key secret failed: {err}"))?
        .ok_or_else(|| "child api key secret not found".to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliOAuthGrant {
    AuthorizationCode,
    RefreshToken,
    TokenExchange,
}

fn validate_cli_oauth_session(
    storage: &crate::storage_helpers::StorageHandle,
    session: &CliOAuthSession,
    grant: CliOAuthGrant,
    now: i64,
) -> Result<String, String> {
    match grant {
        CliOAuthGrant::AuthorizationCode => {
            if session.status != "authorized" || session.authorization_code_hash.is_none() {
                return Err("authorization code is invalid or expired".to_string());
            }
            if session.expires_at <= now {
                let _ = storage.invalidate_cli_oauth_sessions_for_child_key(
                    session.child_key_id.as_str(),
                    "expired",
                );
                return Err("authorization code is invalid or expired".to_string());
            }
        }
        CliOAuthGrant::RefreshToken => {
            if session.status != "active"
                || session.refresh_token_hash.trim().is_empty()
                || session.refresh_expires_at <= now
            {
                let _ = storage.invalidate_cli_oauth_sessions_for_child_key(
                    session.child_key_id.as_str(),
                    "expired",
                );
                return Err("refresh token is invalid or expired".to_string());
            }
        }
        CliOAuthGrant::TokenExchange => {
            if session.status != "active" || session.expires_at <= now {
                let _ = storage.invalidate_cli_oauth_sessions_for_child_key(
                    session.child_key_id.as_str(),
                    "expired",
                );
                return Err("subject token is invalid or expired".to_string());
            }
        }
    }
    let child_link = storage
        .find_cli_child_key(session.child_key_id.as_str())
        .map_err(|err| format!("read cli child key failed: {err}"))?
        .ok_or_else(|| "cli child key binding not found".to_string())?;
    if child_link.owner_key_id != session.owner_key_id
        || child_link.cli_instance_uuid != session.cli_instance_uuid
        || child_link.status != "active"
    {
        return Err("cli child key binding is inactive".to_string());
    }
    let parent_key = storage
        .find_api_key_by_id(session.owner_key_id.as_str())
        .map_err(|err| format!("read parent api key failed: {err}"))?
        .ok_or_else(|| "parent api key is missing".to_string())?;
    let child_key = storage
        .find_api_key_by_id(session.child_key_id.as_str())
        .map_err(|err| format!("read child api key failed: {err}"))?
        .ok_or_else(|| "child api key is missing".to_string())?;
    if parent_key.status != "active" || child_key.status != "active" {
        return Err("parent or child api key is disabled".to_string());
    }
    resolve_child_key_secret(storage, session.child_key_id.as_str())
}

fn required_form_value(form: &HashMap<String, String>, key: &str) -> Result<String, String> {
    form.get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("missing {key}"))
}

fn optional_form_value(form: &HashMap<String, String>, key: &str) -> Option<String> {
    form.get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn validate_cli_redirect_uri(raw: &str) -> Result<Url, String> {
    let redirect = Url::parse(raw).map_err(|err| format!("invalid redirect_uri: {err}"))?;
    match redirect.scheme() {
        "http" | "https" => {}
        _ => return Err("redirect_uri must use http or https".to_string()),
    }
    let host = redirect
        .host_str()
        .ok_or_else(|| "redirect_uri host is missing".to_string())?;
    if !matches!(host, "localhost" | "127.0.0.1" | "::1") {
        return Err(format!(
            "redirect_uri must target loopback (localhost/127.0.0.1/::1), got {host}"
        ));
    }
    if redirect.port_or_known_default().is_none() {
        return Err("redirect_uri must include a valid port".to_string());
    }
    Ok(redirect)
}

fn read_request_body(request: &mut Request) -> Result<String, String> {
    let mut body = String::new();
    request
        .as_reader()
        .read_to_string(&mut body)
        .map_err(|err| format!("read request body failed: {err}"))?;
    Ok(body)
}

fn parse_form_map(raw: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(raw.as_bytes())
        .into_owned()
        .collect::<HashMap<_, _>>()
}

fn generate_cli_instance_uuid() -> String {
    let raw = generate_platform_key();
    format!(
        "{}-{}-{}-{}-{}",
        &raw[0..8],
        &raw[8..12],
        &raw[12..16],
        &raw[16..20],
        &raw[20..32],
    )
}

fn pkce_challenge_for_verifier(code_verifier: &str) -> Result<String, String> {
    let digest = Sha256::digest(code_verifier.as_bytes());
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest))
}

fn build_id_token(
    client_id: &str,
    cli_instance_uuid: &str,
    session_id: &str,
    issued_at: i64,
    expires_at: i64,
    issuer_base: Option<&str>,
) -> Result<String, String> {
    let header = serde_json::json!({
        "alg": "none",
        "typ": "JWT",
    });
    let subject = format!("cli:{cli_instance_uuid}");
    let payload = serde_json::json!({
        "iss": issuer_base.unwrap_or("http://localhost"),
        "aud": client_id,
        "sub": subject,
        "sid": session_id,
        "iat": issued_at,
        "exp": expires_at,
        "email": format!("cli+{}@codexmanager.local", &cli_instance_uuid.replace('-', "")),
        "https://api.openai.com/auth": {
            "chatgpt_account_id": format!("cli:{cli_instance_uuid}"),
            "chatgpt_user_id": format!("cli:{cli_instance_uuid}"),
            "user_id": format!("cli:{cli_instance_uuid}"),
            "chatgpt_plan_type": "enterprise"
        }
    });
    let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&header).map_err(|err| err.to_string())?);
    let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&payload).map_err(|err| err.to_string())?);
    Ok(format!("{header_b64}.{payload_b64}."))
}

fn build_access_token(
    client_id: &str,
    cli_instance_uuid: &str,
    session_id: &str,
    issued_at: i64,
    expires_at: i64,
    issuer_base: Option<&str>,
) -> Result<String, String> {
    let header = serde_json::json!({
        "alg": "none",
        "typ": "JWT",
    });
    let subject = format!("cli:{cli_instance_uuid}");
    let payload = serde_json::json!({
        "iss": issuer_base.unwrap_or("http://localhost"),
        "aud": client_id,
        "sub": subject,
        "sid": session_id,
        "iat": issued_at,
        "exp": expires_at,
        "scope": OAUTH_SCOPE,
        "https://api.openai.com/auth": {
            "chatgpt_account_id": format!("cli:{cli_instance_uuid}"),
            "chatgpt_user_id": format!("cli:{cli_instance_uuid}"),
            "user_id": format!("cli:{cli_instance_uuid}"),
            "chatgpt_plan_type": "enterprise"
        }
    });
    let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&header).map_err(|err| err.to_string())?);
    let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&payload).map_err(|err| err.to_string())?);
    Ok(format!("{header_b64}.{payload_b64}."))
}

fn issuer_base_url(request: &Request) -> Option<String> {
    if let Ok(value) = std::env::var("CODEXMANAGER_DOWNSTREAM_ISSUER_BASE_URL") {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.trim_end_matches('/').to_string());
        }
    }
    let host = header_value(request, "host")?;
    let forwarded_proto = header_value(request, "x-forwarded-proto");
    let scheme = forwarded_proto.as_deref().unwrap_or("http");
    Some(format!("{scheme}://{}", host.trim_end_matches('/')))
}

fn header_value(request: &Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|header| header.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str().trim().to_string())
        .filter(|value| !value.is_empty())
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn build_authorize_page(
    params: &AuthorizeParams,
    issuer_base: Option<&str>,
    password_required: bool,
    error: Option<&str>,
) -> String {
    let error_html = error
        .map(html_escape)
        .map(|message| format!(r#"<p class="error">{message}</p>"#))
        .unwrap_or_default();
    let password_html = if password_required {
        r#"<label>Web Access Password<input type="password" name="web_password" autocomplete="current-password"></label>"#
            .to_string()
    } else {
        String::new()
    };
    let issuer_hint = issuer_base
        .map(html_escape)
        .map(|value| format!(r#"<p class="muted">Issuer: <code>{value}</code></p>"#))
        .unwrap_or_default();
    let scope_value = params.scope.as_deref().unwrap_or(OAUTH_SCOPE);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>Authorize CLI</title>
  <style>
    body {{ font-family: -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif; padding: 32px; color: #0f172a; background: #f8fafc; }}
    .card {{ max-width: 640px; margin: 40px auto; background: #fff; border: 1px solid #dbe3ee; border-radius: 18px; padding: 24px; box-shadow: 0 12px 32px rgba(15,23,42,.08); }}
    h1 {{ margin: 0 0 8px; font-size: 24px; }}
    p {{ margin: 8px 0; line-height: 1.6; }}
    label {{ display: block; margin-top: 16px; font-size: 14px; font-weight: 600; }}
    input {{ width: 100%; margin-top: 8px; padding: 12px 14px; border-radius: 12px; border: 1px solid #cbd5e1; font: inherit; }}
    button {{ margin-top: 20px; padding: 12px 16px; border: 0; border-radius: 12px; background: #0f172a; color: #fff; font: inherit; cursor: pointer; }}
    .muted {{ color: #64748b; font-size: 13px; }}
    .error {{ color: #b91c1c; background: #fff1f2; border-radius: 12px; padding: 12px 14px; }}
    code {{ background: #e2e8f0; padding: 2px 6px; border-radius: 6px; }}
  </style>
</head>
<body>
  <div class="card">
    <h1>Authorize CLI Access</h1>
    <p>This CLI instance will be linked to one employee parent API key. OAuth tokens stay local to the login flow; the CLI child key is returned by OAuth token exchange.</p>
    {issuer_hint}
    <p class="muted">client_id=<code>{client_id}</code></p>
    <p class="muted">scope=<code>{scope}</code></p>
    {error_html}
    <form method="post" action="/oauth/authorize/approve">
      <input type="hidden" name="response_type" value="code">
      <input type="hidden" name="client_id" value="{client_id}">
      <input type="hidden" name="redirect_uri" value="{redirect_uri}">
      <input type="hidden" name="state" value="{state}">
      <input type="hidden" name="code_challenge" value="{code_challenge}">
      <input type="hidden" name="code_challenge_method" value="{code_challenge_method}">
      <input type="hidden" name="scope" value="{scope}">
      <label>Employee Parent API Key<input type="password" name="employee_api_key" autocomplete="off" required></label>
      {password_html}
      <button type="submit">Authorize</button>
    </form>
  </div>
</body>
</html>"#,
        issuer_hint = issuer_hint,
        client_id = html_escape(&params.client_id),
        redirect_uri = html_escape(&params.redirect_uri),
        state = html_escape(&params.state),
        code_challenge = html_escape(&params.code_challenge),
        code_challenge_method = html_escape(&params.code_challenge_method),
        scope = html_escape(scope_value),
        password_html = password_html,
        error_html = error_html,
    )
}

fn html_response(status: u16, body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut response = Response::from_string(body).with_status_code(StatusCode(status));
    if let Ok(header) = Header::from_bytes(
        b"Content-Type".as_slice(),
        b"text/html; charset=utf-8".as_slice(),
    ) {
        response = response.with_header(header);
    }
    response
}

fn text_response(status: u16, body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body.to_string()).with_status_code(StatusCode(status))
}

fn json_response(status: u16, value: serde_json::Value) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut response =
        Response::from_string(value.to_string()).with_status_code(StatusCode(status));
    if let Ok(header) =
        Header::from_bytes(b"Content-Type".as_slice(), b"application/json".as_slice())
    {
        response = response.with_header(header);
    }
    response
}

fn json_error_response(
    status: u16,
    error: &str,
    description: &str,
) -> Response<std::io::Cursor<Vec<u8>>> {
    json_response(
        status,
        serde_json::json!({
            "error": error,
            "error_description": description,
        }),
    )
}

fn redirect_response(location: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut response = Response::from_string(String::new()).with_status_code(StatusCode(302));
    if let Ok(header) = Header::from_bytes(b"Location".as_slice(), location.as_bytes()) {
        response = response.with_header(header);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::{build_id_token, parse_authorize_params};
    use std::collections::HashMap;

    #[test]
    fn build_id_token_emits_cli_claims() {
        let id_token = build_id_token(
            "cli-client",
            "123e4567-e89b-12d3-a456-426614174000",
            "sess_test",
            100,
            200,
            Some("http://localhost:48760"),
        )
        .expect("build id token");
        let claims = codexmanager_core::auth::parse_id_token_claims(id_token.as_str())
            .expect("parse claims");

        assert_eq!(claims.sub, "cli:123e4567-e89b-12d3-a456-426614174000");
        assert_eq!(
            claims
                .auth
                .as_ref()
                .and_then(|value| value.chatgpt_plan_type.as_deref()),
            Some("enterprise")
        );
    }

    #[test]
    fn parse_authorize_params_requires_pkce() {
        let mut params = HashMap::new();
        params.insert("response_type".to_string(), "code".to_string());
        params.insert("client_id".to_string(), "cli-client".to_string());
        params.insert(
            "redirect_uri".to_string(),
            "http://127.0.0.1:1455/callback".to_string(),
        );
        params.insert("state".to_string(), "state-1".to_string());
        params.insert("code_challenge".to_string(), "challenge".to_string());
        params.insert("code_challenge_method".to_string(), "plain".to_string());

        let err = parse_authorize_params(params).expect_err("reject non-S256 pkce");
        assert!(err.contains("unsupported code_challenge_method"));
    }

    #[test]
    fn parse_authorize_params_rejects_non_loopback_redirect_uri() {
        let mut params = HashMap::new();
        params.insert("response_type".to_string(), "code".to_string());
        params.insert("client_id".to_string(), "cli-client".to_string());
        params.insert(
            "redirect_uri".to_string(),
            "https://example.com/callback".to_string(),
        );
        params.insert("state".to_string(), "state-1".to_string());
        params.insert("code_challenge".to_string(), "challenge".to_string());
        params.insert("code_challenge_method".to_string(), "S256".to_string());

        let err = parse_authorize_params(params).expect_err("reject remote redirect");
        assert!(err.contains("loopback"));
    }
}
