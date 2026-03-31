#[path = "auth_account.rs"]
pub(crate) mod account;
#[path = "auth_callback.rs"]
pub(crate) mod callback;
#[path = "auth_cli_oauth.rs"]
pub(crate) mod cli_oauth;
#[path = "auth_login.rs"]
pub(crate) mod login;
pub(crate) mod rpc;
#[path = "auth_tokens.rs"]
pub(crate) mod tokens;
pub(crate) mod web_access;

pub use rpc::{rpc_auth_token, rpc_auth_token_matches};
pub use web_access::{
    build_web_access_session_token, current_web_access_password_hash, set_web_access_password,
    verify_web_access_password, web_access_password_configured, web_auth_status_value,
};
