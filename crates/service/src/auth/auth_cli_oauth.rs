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
    let issuer_base = issuer_base_url(&request);
    let password_required = crate::auth::web_access_password_configured();
    if password_required {
        let provided_password = form
            .get("web_password")
            .map(String::as_str)
            .unwrap_or_default();
        if !crate::auth::verify_web_access_password(provided_password) {
            let body = build_authorize_page(
                &params,
                issuer_base.as_deref(),
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

    let Some(storage) = open_storage() else {
        let _ = request.respond(text_response(503, "storage unavailable"));
        return Ok(());
    };
    let binding = match resolve_parent_key_binding(&storage, employee_api_key) {
        Ok(binding) => binding,
        Err(err) => {
            let body =
                build_authorize_page(&params, issuer_base.as_deref(), password_required, Some(&err));
            let _ = request.respond(html_response(400, body));
            return Ok(());
        }
    };
    let now = now_ts();
    let authorization_code = generate_platform_key();
    let refresh_token = generate_platform_key();
    let session_id = format!("sess_{}", generate_platform_key());
    let id_token = match build_id_token(
        params.client_id.as_str(),
        binding.cli_instance_uuid.as_str(),
        session_id.as_str(),
        now,
        now + AUTHORIZATION_CODE_EXPIRES_SECS,
        issuer_base.as_deref(),
    ) {
        Ok(id_token) => id_token,
        Err(err) => {
            let message = format!("build id token failed: {err}");
            let _ = request.respond(text_response(500, &message));
            return Ok(());
        }
    };
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
    if let Err(err) = storage.save_cli_oauth_session(&session) {
        let message = format!("save cli oauth session failed: {err}");
        let _ = request.respond(text_response(500, &message));
        return Ok(());
    }

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
    let translations_json = r#"{
  "en": {
    "page_title": "Authorize API Key",
    "eyebrow_cli_oauth": "CodexManager CLI OAuth",
    "hero_title": "Authorize API Key",
    "hero_copy": "Issue a CLI child key for this session.",
    "hero_signal_parent": "Parent key",
    "hero_signal_child": "CLI child key",
    "hero_signal_loopback": "Local callback",
    "hero_note": "Only the child key is returned to the container.",
    "step1_title": "Paste an active parent API key",
    "step1_desc": "Only parent keys are accepted here. Existing CLI child keys are rejected to prevent nested delegation.",
    "step2_title": "Approve this pending CLI session",
    "step2_desc": "CodexManager binds the new child key to this one OAuth session and preserves the parent ownership chain for logging and routing.",
    "step3_title": "Return to the waiting callback",
    "step3_desc": "The browser redirects back to the loopback callback on your machine so the container can finish the token exchange locally.",
    "summary_issuer_label": "Issuer",
    "summary_issuer_desc": "Approval target",
    "summary_callback_label": "Callback",
    "summary_callback_desc": "Waiting callback",
    "eyebrow_parent_key_approval": "Parent Key Approval",
    "form_title": "Authorize API Key",
    "form_copy": "Issue a CLI child key for this session.",
    "status_pkce_loopback": "PKCE + Loopback",
    "meta_client_id": "Client ID",
    "meta_callback_host": "Callback Host",
    "meta_callback_path": "Callback Path",
    "meta_challenge_method": "Challenge Method",
    "meta_scopes": "Scopes",
    "meta_toggle_label": "Session details",
    "meta_toggle_hint": "Client, callback, scopes",
    "alert_error_title": "Authorization failed",
    "alert_password_title": "Web password required",
    "alert_password_desc": "This instance also requires the Web Access Password.",
    "alert_parent_only_title": "Ready",
    "alert_parent_only_desc": "This instance only needs a parent API key.",
    "label_parent_api_key": "Parent API key",
    "placeholder_parent_api_key": "Paste parent API key",
    "hint_parent_api_key": "A CLI child key is issued for this login. The parent key is not returned to the CLI.",
    "label_web_password": "Web access password",
    "placeholder_web_password": "Required by this CodexManager instance",
    "hint_web_password": "Used only for browser access. It is never stored here.",
    "remember_key_label": "Remember in this browser",
    "action_authorize": "Authorize",
    "action_authorizing": "Authorizing...",
    "action_clear_saved_key": "Clear saved",
    "action_paste": "Paste",
    "action_show": "Show",
    "action_hide": "Hide",
    "key_status_opt_in": "",
    "key_status_none_saved": "",
    "key_status_will_remember": "Will be remembered ({count})",
    "key_status_loaded_unsaved": "",
    "footer_note_html": "",
    "lang_switch_label": "Language",
    "server_errors": {
      "Invalid web access password.": "Invalid web access password.",
      "invalid employee API key": "invalid employee API key",
      "employee API key is disabled": "employee API key is disabled",
      "employee API key must be a parent key, not a CLI child key": "employee API key must be a parent key, not a CLI child key",
      "storage unavailable": "storage unavailable"
    }
  },
  "zh": {
    "page_title": "授权 API Key",
    "eyebrow_cli_oauth": "CodexManager CLI OAuth",
    "hero_title": "授权 API Key",
    "hero_copy": "为本次会话签发 CLI 子 Key。",
    "hero_signal_parent": "父级 Key",
    "hero_signal_child": "CLI 子 Key",
    "hero_signal_loopback": "本地回调",
    "hero_note": "只有子 Key 会返回给容器。",
    "step1_title": "粘贴一个有效的父级 API Key",
    "step1_desc": "这里只接受父级 Key。现有的 CLI 子 Key 会被拒绝，以防止继续嵌套委派。",
    "step2_title": "批准当前等待中的 CLI 会话",
    "step2_desc": "CodexManager 会把新的子 Key 绑定到这一次 OAuth 会话，并保留父级归属链用于日志和路由。",
    "step3_title": "回到等待中的本地回调",
    "step3_desc": "浏览器会跳回你机器上的 loopback 回调地址，容器随后会在本地完成 token exchange。",
    "summary_issuer_label": "Issuer",
    "summary_issuer_desc": "授权提交目标",
    "summary_callback_label": "回调",
    "summary_callback_desc": "等待中的回调",
    "eyebrow_parent_key_approval": "父级 Key 授权",
    "form_title": "授权 API Key",
    "form_copy": "为本次会话签发 CLI 子 Key。",
    "status_pkce_loopback": "PKCE + Loopback",
    "meta_client_id": "Client ID",
    "meta_callback_host": "回调主机",
    "meta_callback_path": "回调路径",
    "meta_challenge_method": "Challenge Method",
    "meta_scopes": "Scopes",
    "meta_toggle_label": "会话详情",
    "meta_toggle_hint": "客户端、回调、Scopes",
    "alert_error_title": "授权失败",
    "alert_password_title": "需要 Web 密码",
    "alert_password_desc": "当前实例还需要 Web 访问密码。",
    "alert_parent_only_title": "可以直接授权",
    "alert_parent_only_desc": "当前实例只需要父级 API Key。",
    "label_parent_api_key": "父级 API Key",
    "placeholder_parent_api_key": "粘贴父级 API Key",
    "hint_parent_api_key": "系统会为本次登录签发 CLI 子 Key，父级 Key 不会返回给 CLI。",
    "label_web_password": "Web 访问密码",
    "placeholder_web_password": "当前 CodexManager 实例要求填写",
    "hint_web_password": "仅用于浏览器访问控制，不会在这里保存。",
    "remember_key_label": "仅在当前浏览器记住",
    "action_authorize": "授权",
    "action_authorizing": "授权中...",
    "action_clear_saved_key": "清除已保存",
    "action_paste": "粘贴",
    "action_show": "显示",
    "action_hide": "隐藏",
    "key_status_opt_in": "",
    "key_status_none_saved": "",
    "key_status_will_remember": "将记住（{count}）",
    "key_status_loaded_unsaved": "",
    "footer_note_html": "",
    "lang_switch_label": "语言",
    "server_errors": {
      "Invalid web access password.": "Web 访问密码错误。",
      "invalid employee API key": "父级 API Key 无效。",
      "employee API key is disabled": "父级 API Key 已被禁用。",
      "employee API key must be a parent key, not a CLI child key": "这里只能使用父级 API Key，不能使用 CLI 子 Key。",
      "storage unavailable": "存储服务暂时不可用。"
    }
  }
}"#;
    let error_html = error
        .map(html_escape)
        .map(|message| {
            format!(
                r#"<div class="alert alert-error" role="alert">
      <div class="alert-title" data-i18n="alert_error_title">Authorization failed</div>
      <p class="server-error-message" data-server-error="{message}">{message}</p>
    </div>"#
            )
        })
        .unwrap_or_default();
    let password_html = if password_required {
        r#"<div class="field">
        <div class="field-head">
          <label for="web_password" data-i18n="label_web_password">Web access password</label>
          <div class="field-actions">
            <button type="button" class="ghost-button" data-toggle-target="web_password" data-toggle-show-key="action_show" data-toggle-hide-key="action_hide">Show</button>
          </div>
        </div>
        <input
          id="web_password"
          type="password"
          name="web_password"
          autocomplete="current-password"
          required
          data-i18n-placeholder="placeholder_web_password"
          placeholder="Required by this CodexManager instance"
        >
      </div>"#
            .to_string()
    } else {
        String::new()
    };
    let scope_value = params.scope.as_deref().unwrap_or(OAUTH_SCOPE);
    let scope_badges = scope_value
        .split_whitespace()
        .map(html_escape)
        .map(|scope| format!(r#"<span class="scope-chip">{scope}</span>"#))
        .collect::<Vec<_>>()
        .join("");
    let issuer_label = issuer_base
        .map(html_escape)
        .unwrap_or_else(|| "Resolved from current request".to_string());
    let redirect = Url::parse(&params.redirect_uri).ok();
    let callback_host = redirect
        .as_ref()
        .and_then(|url| url.host_str())
        .unwrap_or("localhost")
        .to_string();
    let callback_port = redirect
        .as_ref()
        .and_then(|url| url.port_or_known_default())
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    let callback_path = redirect
        .as_ref()
        .map(|url| url.path().to_string())
        .unwrap_or_else(|| "/callback".to_string());
    let password_notice = if password_required {
        r#"<div class="alert alert-warn">
      <div class="alert-title" data-i18n="alert_password_title">Extra confirmation required</div>
      <p data-i18n="alert_password_desc">This CodexManager instance is protected by a Web Access Password. Enter it below after the parent API key.</p>
    </div>"#
            .to_string()
    } else {
        String::new()
    };
    let storage_key = html_escape(&format!(
        "codexmanager.oauth.parent-key::{issuer_label}::{}",
        params.client_id
    ));
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>Authorize API Key</title>
  <style>
    :root {{
      color-scheme: light;
      --bg: #07111f;
      --bg-2: #0f2138;
      --card: rgba(255, 255, 255, 0.9);
      --card-border: rgba(148, 163, 184, 0.28);
      --ink: #0f172a;
      --muted: #526072;
      --muted-2: #6b7a8f;
      --accent: #0f766e;
      --accent-2: #f97316;
      --accent-3: #2563eb;
      --surface: rgba(255, 255, 255, 0.72);
      --shadow: 0 28px 90px rgba(2, 12, 27, 0.24);
      --radius-xl: 28px;
      --radius-lg: 20px;
      --radius-md: 14px;
      --radius-sm: 12px;
    }}

    * {{ box-sizing: border-box; }}

    body {{
      margin: 0;
      min-height: 100vh;
      font-family: "Avenir Next", "Segoe UI", "Helvetica Neue", sans-serif;
      color: var(--ink);
      background:
        radial-gradient(circle at top left, rgba(56, 189, 248, 0.28), transparent 34%),
        radial-gradient(circle at top right, rgba(249, 115, 22, 0.22), transparent 28%),
        linear-gradient(145deg, var(--bg) 0%, var(--bg-2) 55%, #10223b 100%);
      padding: 32px;
    }}

    body::before,
    body::after {{
      content: "";
      position: fixed;
      inset: auto;
      border-radius: 999px;
      pointer-events: none;
      filter: blur(10px);
      opacity: 0.8;
    }}

    body::before {{
      width: 280px;
      height: 280px;
      top: 52px;
      right: 8%;
      background: rgba(45, 212, 191, 0.18);
    }}

    body::after {{
      width: 340px;
      height: 340px;
      left: 6%;
      bottom: 8%;
      background: rgba(249, 115, 22, 0.14);
    }}

    .layout {{
      position: relative;
      z-index: 1;
      max-width: 760px;
      margin: 0 auto;
    }}

    .panel {{
      border: 1px solid var(--card-border);
      border-radius: var(--radius-xl);
      background: var(--card);
      box-shadow: var(--shadow);
      backdrop-filter: blur(18px);
      overflow: hidden;
    }}

    .hero {{
      position: relative;
      padding: 34px;
      background:
        linear-gradient(160deg, rgba(255, 255, 255, 0.86), rgba(233, 248, 247, 0.82)),
        linear-gradient(135deg, rgba(14, 116, 144, 0.08), rgba(249, 115, 22, 0.04));
    }}

    .hero::after {{
      content: "";
      position: absolute;
      inset: auto -70px -90px auto;
      width: 250px;
      height: 250px;
      border-radius: 999px;
      background: radial-gradient(circle, rgba(37, 99, 235, 0.18), transparent 70%);
      pointer-events: none;
    }}

    .hero-top {{
      display: flex;
      align-items: start;
      justify-content: space-between;
      gap: 16px;
    }}

    .eyebrow {{
      display: inline-flex;
      align-items: center;
      gap: 8px;
      padding: 7px 12px;
      border-radius: 999px;
      background: rgba(15, 118, 110, 0.1);
      color: var(--accent);
      font-size: 12px;
      font-weight: 700;
      letter-spacing: 0.08em;
      text-transform: uppercase;
    }}

    .eyebrow::before {{
      content: "";
      width: 8px;
      height: 8px;
      border-radius: 999px;
      background: currentColor;
      box-shadow: 0 0 0 6px rgba(15, 118, 110, 0.12);
    }}

    .lang-switch {{
      display: inline-flex;
      align-items: center;
      gap: 6px;
      padding: 6px;
      border-radius: 999px;
      background: rgba(255, 255, 255, 0.72);
      border: 1px solid rgba(148, 163, 184, 0.24);
      box-shadow: 0 12px 30px rgba(15, 23, 42, 0.08);
    }}

    .lang-button {{
      appearance: none;
      border: 0;
      border-radius: 999px;
      padding: 9px 14px;
      min-width: 58px;
      background: transparent;
      color: var(--muted);
      font: inherit;
      font-size: 13px;
      font-weight: 800;
      cursor: pointer;
      transition: background .18s ease, color .18s ease, transform .18s ease, box-shadow .18s ease;
    }}

    .lang-button:hover {{
      transform: translateY(-1px);
      color: var(--ink);
    }}

    .lang-button.is-active {{
      background: linear-gradient(135deg, #0f172a, #1e293b);
      color: #fff;
      box-shadow: 0 10px 22px rgba(15, 23, 42, 0.2);
    }}

    h1 {{
      margin: 22px 0 14px;
      font-size: clamp(34px, 5vw, 54px);
      line-height: 1.02;
      letter-spacing: -0.04em;
    }}

    .hero-copy {{
      max-width: 520px;
      margin: 0;
      font-size: 16px;
      line-height: 1.75;
      color: var(--muted);
    }}

    .signal-row {{
      display: flex;
      flex-wrap: wrap;
      gap: 10px;
      margin: 22px 0 14px;
    }}

    .signal-chip {{
      display: inline-flex;
      align-items: center;
      padding: 9px 12px;
      border-radius: 999px;
      background: rgba(255, 255, 255, 0.68);
      border: 1px solid rgba(148, 163, 184, 0.22);
      color: var(--ink);
      font-size: 12px;
      font-weight: 700;
      letter-spacing: 0.04em;
    }}

    .hero-note {{
      margin-top: 2px;
    }}

    .summary-card p,
    .alert p,
    .hint,
    .micro {{
      margin: 0;
      color: var(--muted);
      line-height: 1.65;
    }}

    .summary-grid {{
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: 14px;
    }}

    .summary-card {{
      padding: 18px;
      border-radius: var(--radius-lg);
      background: linear-gradient(180deg, rgba(8, 15, 31, 0.94), rgba(15, 23, 42, 0.88));
      color: #f8fafc;
      border: 1px solid rgba(255, 255, 255, 0.08);
      box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.04);
    }}

    .summary-card .label {{
      display: block;
      margin-bottom: 10px;
      color: rgba(226, 232, 240, 0.72);
      font-size: 12px;
      font-weight: 700;
      letter-spacing: 0.08em;
      text-transform: uppercase;
    }}

    .summary-card strong {{
      display: block;
      margin-bottom: 8px;
      font-size: 16px;
      line-height: 1.3;
    }}

    .summary-card code,
    .meta code {{
      display: inline-block;
      max-width: 100%;
      overflow-wrap: anywhere;
      padding: 4px 8px;
      border-radius: 10px;
      background: rgba(148, 163, 184, 0.16);
      color: inherit;
      font-size: 13px;
      font-family: "SFMono-Regular", Consolas, "Liberation Mono", monospace;
    }}

    .form-panel {{
      padding: 30px;
      background:
        linear-gradient(180deg, rgba(255, 255, 255, 0.96), rgba(248, 250, 252, 0.94));
    }}

    .form-top {{
      display: flex;
      align-items: start;
      justify-content: space-between;
      gap: 16px;
      margin-bottom: 18px;
    }}

    .top-actions {{
      display: flex;
      flex-wrap: wrap;
      justify-content: end;
      align-items: center;
      gap: 10px;
    }}

    .form-top h2 {{
      margin: 0 0 10px;
      font-size: 30px;
      line-height: 1.1;
      letter-spacing: -0.03em;
    }}

    .form-top p {{
      margin: 0;
      color: var(--muted);
      line-height: 1.6;
      max-width: 420px;
    }}

    .status-pill {{
      flex: 0 0 auto;
      display: inline-flex;
      align-items: center;
      gap: 8px;
      padding: 10px 13px;
      border-radius: 999px;
      background: rgba(37, 99, 235, 0.1);
      color: var(--accent-3);
      font-size: 12px;
      font-weight: 800;
      letter-spacing: 0.08em;
      text-transform: uppercase;
    }}

    .status-pill::before {{
      content: "";
      width: 8px;
      height: 8px;
      border-radius: 999px;
      background: currentColor;
    }}

    details.meta {{
      margin-bottom: 20px;
      border-radius: var(--radius-lg);
      background: rgba(241, 245, 249, 0.72);
      border: 1px solid rgba(203, 213, 225, 0.65);
      overflow: hidden;
    }}

    .meta-summary {{
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 16px;
      padding: 16px 18px;
      cursor: pointer;
      list-style: none;
    }}

    .meta-summary::-webkit-details-marker {{
      display: none;
    }}

    .meta-summary strong {{
      font-size: 14px;
      font-weight: 700;
    }}

    .meta-chevron {{
      width: 11px;
      height: 11px;
      border-right: 2px solid rgba(15, 23, 42, 0.46);
      border-bottom: 2px solid rgba(15, 23, 42, 0.46);
      transform: rotate(45deg);
      transition: transform .18s ease;
      flex: 0 0 auto;
      margin-right: 4px;
    }}

    details.meta[open] .meta-chevron {{
      transform: rotate(225deg);
      margin-top: 6px;
    }}

    .meta-body {{
      display: grid;
      gap: 12px;
      padding: 0 18px 18px;
      border-top: 1px solid rgba(203, 213, 225, 0.62);
    }}

    .meta-row {{
      display: flex;
      align-items: baseline;
      justify-content: space-between;
      gap: 16px;
    }}

    .meta-row span {{
      font-size: 12px;
      font-weight: 700;
      letter-spacing: 0.08em;
      text-transform: uppercase;
      color: var(--muted-2);
    }}

    .scope-grid {{
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
    }}

    .scope-chip {{
      display: inline-flex;
      align-items: center;
      padding: 8px 10px;
      border-radius: 999px;
      background: rgba(14, 165, 233, 0.1);
      color: #0f172a;
      border: 1px solid rgba(14, 165, 233, 0.16);
      font-size: 12px;
      font-weight: 700;
    }}

    .alert {{
      margin-bottom: 18px;
      padding: 12px 14px;
      border-radius: var(--radius-lg);
      border: 1px solid transparent;
    }}

    .alert-title {{
      margin-bottom: 4px;
      font-size: 13px;
      font-weight: 800;
      letter-spacing: 0.08em;
      text-transform: uppercase;
    }}

    .alert-error {{
      background: #fff1f2;
      border-color: rgba(244, 63, 94, 0.18);
      color: #9f1239;
    }}

    .alert-warn {{
      background: #fff7ed;
      border-color: rgba(249, 115, 22, 0.18);
      color: #9a3412;
    }}

    .alert-info {{
      background: #ecfeff;
      border-color: rgba(8, 145, 178, 0.18);
      color: #155e75;
    }}

    form {{
      display: grid;
      gap: 16px;
    }}

    .field {{
      display: grid;
      gap: 10px;
    }}

    .field-head {{
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 12px;
    }}

    label {{
      font-size: 14px;
      font-weight: 700;
    }}

    .field-actions,
    .button-row {{
      display: flex;
      flex-wrap: wrap;
      gap: 10px;
    }}

    input[type="password"],
    input[type="text"] {{
      width: 100%;
      padding: 14px 16px;
      border-radius: var(--radius-md);
      border: 1px solid #cbd5e1;
      background: #fff;
      font: inherit;
      color: var(--ink);
      transition: border-color .18s ease, box-shadow .18s ease, transform .18s ease;
    }}

    input[type="password"]:focus,
    input[type="text"]:focus {{
      outline: none;
      border-color: rgba(15, 118, 110, 0.58);
      box-shadow: 0 0 0 4px rgba(20, 184, 166, 0.12);
      transform: translateY(-1px);
    }}

    .remember-row {{
      display: flex;
      align-items: center;
      gap: 12px;
      padding: 13px 15px;
      border-radius: var(--radius-md);
      background: rgba(241, 245, 249, 0.88);
      border: 1px solid rgba(203, 213, 225, 0.75);
      cursor: pointer;
    }}

    .remember-row input {{
      width: 18px;
      height: 18px;
      margin: 0;
      accent-color: var(--accent);
    }}

    .hint {{
      font-size: 13px;
    }}

    .micro {{
      font-size: 12px;
      color: var(--muted-2);
    }}

    #key_status:empty,
    .micro:empty {{
      display: none;
    }}

    .primary-button,
    .secondary-button,
    .ghost-button {{
      appearance: none;
      border: 0;
      border-radius: 999px;
      font: inherit;
      font-weight: 700;
      cursor: pointer;
      transition: transform .18s ease, box-shadow .18s ease, background .18s ease, opacity .18s ease;
    }}

    .primary-button {{
      padding: 14px 18px;
      background: linear-gradient(135deg, var(--accent), #0f766e 55%, #0ea5e9);
      color: #fff;
      box-shadow: 0 18px 36px rgba(15, 118, 110, 0.22);
    }}

    .secondary-button {{
      padding: 14px 18px;
      background: #e2e8f0;
      color: var(--ink);
    }}

    .ghost-button {{
      padding: 9px 12px;
      background: rgba(148, 163, 184, 0.12);
      color: var(--ink);
      border: 1px solid rgba(148, 163, 184, 0.2);
    }}

    .primary-button:hover,
    .secondary-button:hover,
    .ghost-button:hover {{
      transform: translateY(-1px);
    }}

    .primary-button:disabled {{
      opacity: 0.72;
      cursor: progress;
      transform: none;
    }}

    .footer-note {{
      margin-top: 2px;
    }}

    @media (max-width: 980px) {{
      body {{ padding: 18px; }}
      .form-panel {{ padding: 24px; }}
      .form-top {{ flex-direction: column; }}
      .top-actions {{
        width: 100%;
        justify-content: space-between;
      }}
    }}

    @media (max-width: 640px) {{
      h1 {{ font-size: 34px; }}
      .form-top h2 {{ font-size: 24px; }}
      .meta-row {{ display: grid; gap: 6px; }}
      .meta-summary {{
        align-items: start;
      }}
      .top-actions {{
        align-items: stretch;
      }}
      .lang-switch {{
        width: 100%;
        justify-content: space-between;
      }}
    }}
  </style>
</head>
<body>
  <main class="layout">
    <section class="panel form-panel">
      <div class="form-top">
        <div>
          <span class="eyebrow" data-i18n="eyebrow_parent_key_approval">Parent Key Approval</span>
          <h1 data-i18n="hero_title">Authorize API Key</h1>
          <p class="hero-copy" data-i18n="hero_copy">Issue a CLI child key for this session.</p>
        </div>
        <div class="top-actions">
          <span class="status-pill" data-i18n="status_pkce_loopback">PKCE + Loopback</span>
          <div class="lang-switch" role="group" aria-label="Language switch">
            <button type="button" class="lang-button" data-lang-option="en">EN</button>
            <button type="button" class="lang-button" data-lang-option="zh">中文</button>
          </div>
        </div>
      </div>

      <details class="meta">
        <summary class="meta-summary">
          <strong data-i18n="meta_toggle_label">Session details</strong>
          <span class="meta-chevron" aria-hidden="true"></span>
        </summary>
        <div class="meta-body">
          <div class="meta-row">
            <span data-i18n="meta_client_id">Client ID</span>
            <code>{client_id}</code>
          </div>
          <div class="meta-row">
            <span data-i18n="meta_callback_host">Callback Host</span>
            <code>{callback_host}:{callback_port}</code>
          </div>
          <div class="meta-row">
            <span data-i18n="meta_callback_path">Callback Path</span>
            <code>{callback_path}</code>
          </div>
          <div class="meta-row">
            <span data-i18n="meta_challenge_method">Challenge Method</span>
            <code>{code_challenge_method}</code>
          </div>
          <div>
            <span class="micro" data-i18n="meta_scopes">Scopes</span>
            <div class="scope-grid">{scope_badges}</div>
          </div>
        </div>
      </details>

      {error_html}
      {password_notice}

      <form
        id="authorize-form"
        method="post"
        action="/oauth/authorize/approve"
        data-storage-key="{storage_key}"
      >
      <input type="hidden" name="response_type" value="code">
      <input type="hidden" name="client_id" value="{client_id}">
      <input type="hidden" name="redirect_uri" value="{redirect_uri}">
      <input type="hidden" name="state" value="{state}">
      <input type="hidden" name="code_challenge" value="{code_challenge}">
      <input type="hidden" name="code_challenge_method" value="{code_challenge_method}">
      <input type="hidden" name="scope" value="{scope}">

      <div class="field">
        <div class="field-head">
          <label for="employee_api_key" data-i18n="label_parent_api_key">Parent API key</label>
          <div class="field-actions">
            <button type="button" class="ghost-button" data-paste-target="employee_api_key" data-i18n="action_paste">Paste</button>
            <button type="button" class="ghost-button" data-toggle-target="employee_api_key" data-toggle-show-key="action_show" data-toggle-hide-key="action_hide">Show</button>
          </div>
        </div>
        <input
          id="employee_api_key"
          type="password"
          name="employee_api_key"
          autocomplete="off"
          autocapitalize="off"
          autocorrect="off"
          spellcheck="false"
          required
          data-i18n-placeholder="placeholder_parent_api_key"
          placeholder="Paste parent API key"
        >
      </div>

      {password_html}

      <label class="remember-row" for="remember_key">
        <input id="remember_key" type="checkbox">
        <span data-i18n="remember_key_label">Remember in this browser</span>
      </label>

      <div class="button-row">
        <button id="authorize_button" class="primary-button" type="submit" data-i18n="action_authorize">Authorize</button>
        <button id="clear_saved_key" class="secondary-button" type="button" data-i18n="action_clear_saved_key">Clear saved</button>
      </div>

      <p id="key_status" class="hint footer-note"></p>
      <p class="micro" data-i18n-html="footer_note_html"></p>
    </form>

    </section>
  </main>

  <script>
    (() => {{
      const form = document.getElementById("authorize-form");
      const apiKeyInput = document.getElementById("employee_api_key");
      const rememberKey = document.getElementById("remember_key");
      const clearSavedKey = document.getElementById("clear_saved_key");
      const authorizeButton = document.getElementById("authorize_button");
      const keyStatus = document.getElementById("key_status");
      const storageKey = form?.dataset.storageKey || "";
      const languageStorageKey = "codexmanager.oauth.ui-language";
      const translations = {translations_json};

      const safeStorage = {{
        get(key) {{
          try {{
            return window.localStorage.getItem(key);
          }} catch (_err) {{
            return null;
          }}
        }},
        set(key, value) {{
          try {{
            window.localStorage.setItem(key, value);
          }} catch (_err) {{
            return;
          }}
        }},
        remove(key) {{
          try {{
            window.localStorage.removeItem(key);
          }} catch (_err) {{
            return;
          }}
        }},
      }};

      const normalizeLanguage = (value) => {{
        if (!value) {{
          return "en";
        }}
        return String(value).toLowerCase().startsWith("zh") ? "zh" : "en";
      }};

      const resolveLanguage = () => {{
        const stored = normalizeLanguage(safeStorage.get(languageStorageKey));
        if (translations[stored]) {{
          return stored;
        }}
        return normalizeLanguage(window.navigator?.language);
      }};

      let currentLanguage = resolveLanguage();

      const interpolate = (template, replacements = {{}}) =>
        String(template).replace(/\{{(\w+)\}}/g, (_match, key) =>
          Object.prototype.hasOwnProperty.call(replacements, key) ? replacements[key] : ""
        );

      const t = (key, replacements = {{}}) => {{
        const bundle = translations[currentLanguage] || translations.en || {{}};
        const fallback = translations.en || {{}};
        const template = bundle[key] || fallback[key] || key;
        return interpolate(template, replacements);
      }};

      const translateServerError = (message) => {{
        if (!message) {{
          return "";
        }}
        const bundle = translations[currentLanguage] || translations.en || {{}};
        const serverErrors = bundle.server_errors || {{}};
        return serverErrors[message] || message;
      }};

      const updateKeyStatus = () => {{
        const value = apiKeyInput?.value.trim() || "";
        if (!keyStatus) {{
          return;
        }}
        if (!value) {{
          keyStatus.textContent = "";
          return;
        }}
        keyStatus.textContent = rememberKey?.checked
          ? t("key_status_will_remember", {{ count: String(value.length) }})
          : "";
      }};

      if (storageKey && apiKeyInput && rememberKey) {{
        const savedKey = safeStorage.get(storageKey);
        if (savedKey) {{
          apiKeyInput.value = savedKey;
          rememberKey.checked = true;
        }}
      }}

      const applyTranslations = () => {{
        document.documentElement.lang = currentLanguage === "zh" ? "zh-CN" : "en";
        document.title = t("page_title");

        document.querySelectorAll("[data-i18n]").forEach((node) => {{
          const key = node.dataset.i18n;
          if (key) {{
            node.textContent = t(key);
          }}
        }});

        document.querySelectorAll("[data-i18n-html]").forEach((node) => {{
          const key = node.dataset.i18nHtml;
          if (key) {{
            node.innerHTML = t(key);
          }}
        }});

        document.querySelectorAll("[data-i18n-placeholder]").forEach((node) => {{
          const key = node.dataset.i18nPlaceholder;
          if (key) {{
            node.setAttribute("placeholder", t(key));
          }}
        }});

        document.querySelectorAll(".server-error-message[data-server-error]").forEach((node) => {{
          node.textContent = translateServerError(node.dataset.serverError || "");
        }});

        document.querySelectorAll("[data-lang-option]").forEach((button) => {{
          const isActive = button.dataset.langOption === currentLanguage;
          button.classList.toggle("is-active", isActive);
          button.setAttribute("aria-pressed", isActive ? "true" : "false");
        }});

        document.querySelectorAll("[data-toggle-target]").forEach((button) => {{
          const input = document.getElementById(button.dataset.toggleTarget || "");
          const translationKey =
            input?.type === "text"
              ? button.dataset.toggleHideKey || "action_hide"
              : button.dataset.toggleShowKey || "action_show";
          button.textContent = t(translationKey);
        }});

        updateKeyStatus();
      }};

      document.querySelectorAll("[data-toggle-target]").forEach((button) => {{
        button.addEventListener("click", () => {{
          const input = document.getElementById(button.dataset.toggleTarget || "");
          if (!input) {{
            return;
          }}
          const nextType = input.type === "password" ? "text" : "password";
          input.type = nextType;
          button.textContent = t(
            nextType === "password"
              ? button.dataset.toggleShowKey || "action_show"
              : button.dataset.toggleHideKey || "action_hide"
          );
        }});
      }});

      document.querySelectorAll("[data-paste-target]").forEach((button) => {{
        button.addEventListener("click", async () => {{
          const input = document.getElementById(button.dataset.pasteTarget || "");
          if (!input || !navigator.clipboard?.readText) {{
            return;
          }}
          try {{
            const text = await navigator.clipboard.readText();
            if (text.trim()) {{
              input.value = text.trim();
              input.dispatchEvent(new Event("input", {{ bubbles: true }}));
              input.focus();
            }}
          }} catch (_err) {{
            return;
          }}
        }});
      }});

      clearSavedKey?.addEventListener("click", () => {{
        if (storageKey) {{
          safeStorage.remove(storageKey);
        }}
        if (apiKeyInput) {{
          apiKeyInput.value = "";
          apiKeyInput.focus();
        }}
        if (rememberKey) {{
          rememberKey.checked = false;
        }}
        updateKeyStatus();
      }});

      document.querySelectorAll("[data-lang-option]").forEach((button) => {{
        button.addEventListener("click", () => {{
          const nextLanguage = button.dataset.langOption;
          if (!translations[nextLanguage] || nextLanguage === currentLanguage) {{
            return;
          }}
          currentLanguage = nextLanguage;
          safeStorage.set(languageStorageKey, currentLanguage);
          applyTranslations();
        }});
      }});

      apiKeyInput?.addEventListener("input", updateKeyStatus);
      rememberKey?.addEventListener("change", updateKeyStatus);
      applyTranslations();

      form?.addEventListener("submit", () => {{
        const value = apiKeyInput?.value.trim() || "";
        if (storageKey) {{
          if (rememberKey?.checked && value) {{
            safeStorage.set(storageKey, value);
          }} else {{
            safeStorage.remove(storageKey);
          }}
        }}
        if (authorizeButton) {{
          authorizeButton.disabled = true;
          authorizeButton.textContent = t("action_authorizing");
        }}
      }});
    }})();
  </script>
</body>
</html>"#,
        callback_host = html_escape(&callback_host),
        callback_port = html_escape(&callback_port),
        callback_path = html_escape(&callback_path),
        scope_badges = scope_badges,
        client_id = html_escape(&params.client_id),
        redirect_uri = html_escape(&params.redirect_uri),
        state = html_escape(&params.state),
        code_challenge = html_escape(&params.code_challenge),
        code_challenge_method = html_escape(&params.code_challenge_method),
        scope = html_escape(scope_value),
        password_html = password_html,
        password_notice = password_notice,
        error_html = error_html,
        storage_key = storage_key,
        translations_json = translations_json,
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
