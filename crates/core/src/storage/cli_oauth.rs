use rusqlite::{params, OptionalExtension, Result};

use super::{
    now_ts, AccountQuotaExhaustion, ApiKeyOwnerContext, CliChildKey, CliOAuthSession, Storage,
};

fn map_cli_child_key(row: &rusqlite::Row<'_>) -> Result<CliChildKey> {
    Ok(CliChildKey {
        child_key_id: row.get(0)?,
        owner_key_id: row.get(1)?,
        cli_instance_uuid: row.get(2)?,
        status: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        last_seen_at: row.get(6)?,
    })
}

fn map_cli_oauth_session(row: &rusqlite::Row<'_>) -> Result<CliOAuthSession> {
    Ok(CliOAuthSession {
        session_id: row.get(0)?,
        child_key_id: row.get(1)?,
        owner_key_id: row.get(2)?,
        cli_instance_uuid: row.get(3)?,
        client_id: row.get(4)?,
        redirect_uri: row.get(5)?,
        pkce_challenge: row.get(6)?,
        pkce_method: row.get(7)?,
        state: row.get(8)?,
        authorization_code_hash: row.get(9)?,
        refresh_token_hash: row.get(10)?,
        status: row.get(11)?,
        id_token: row.get(12)?,
        expires_at: row.get(13)?,
        refresh_expires_at: row.get(14)?,
        created_at: row.get(15)?,
        updated_at: row.get(16)?,
        last_seen_at: row.get(17)?,
    })
}

fn map_account_quota_exhaustion(row: &rusqlite::Row<'_>) -> Result<AccountQuotaExhaustion> {
    Ok(AccountQuotaExhaustion {
        account_id: row.get(0)?,
        reason: row.get(1)?,
        exhausted_until: row.get(2)?,
        updated_at: row.get(3)?,
    })
}

impl Storage {
    pub fn save_cli_child_key(&self, child_key: &CliChildKey) -> Result<()> {
        self.conn.execute(
            "INSERT INTO cli_child_keys (
                child_key_id,
                owner_key_id,
                cli_instance_uuid,
                status,
                created_at,
                updated_at,
                last_seen_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(child_key_id) DO UPDATE SET
                owner_key_id = excluded.owner_key_id,
                cli_instance_uuid = excluded.cli_instance_uuid,
                status = excluded.status,
                updated_at = excluded.updated_at,
                last_seen_at = excluded.last_seen_at",
            params![
                &child_key.child_key_id,
                &child_key.owner_key_id,
                &child_key.cli_instance_uuid,
                &child_key.status,
                child_key.created_at,
                child_key.updated_at,
                child_key.last_seen_at,
            ],
        )?;
        Ok(())
    }

    pub fn find_cli_child_key(&self, child_key_id: &str) -> Result<Option<CliChildKey>> {
        self.conn
            .query_row(
                "SELECT
                    child_key_id,
                    owner_key_id,
                    cli_instance_uuid,
                    status,
                    created_at,
                    updated_at,
                    last_seen_at
                 FROM cli_child_keys
                 WHERE child_key_id = ?1
                 LIMIT 1",
                [child_key_id],
                map_cli_child_key,
            )
            .optional()
    }

    pub fn find_cli_child_key_by_instance(
        &self,
        owner_key_id: &str,
        cli_instance_uuid: &str,
    ) -> Result<Option<CliChildKey>> {
        self.conn
            .query_row(
                "SELECT
                    child_key_id,
                    owner_key_id,
                    cli_instance_uuid,
                    status,
                    created_at,
                    updated_at,
                    last_seen_at
                 FROM cli_child_keys
                 WHERE owner_key_id = ?1
                   AND cli_instance_uuid = ?2
                 LIMIT 1",
                params![owner_key_id, cli_instance_uuid],
                map_cli_child_key,
            )
            .optional()
    }

    pub fn list_cli_child_keys_for_owner(&self, owner_key_id: &str) -> Result<Vec<CliChildKey>> {
        let mut stmt = self.conn.prepare(
            "SELECT
                child_key_id,
                owner_key_id,
                cli_instance_uuid,
                status,
                created_at,
                updated_at,
                last_seen_at
             FROM cli_child_keys
             WHERE owner_key_id = ?1
             ORDER BY created_at ASC, child_key_id ASC",
        )?;
        let mut rows = stmt.query([owner_key_id])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(map_cli_child_key(row)?);
        }
        Ok(out)
    }

    pub fn delete_cli_child_key(&self, child_key_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM cli_child_keys WHERE child_key_id = ?1",
            [child_key_id],
        )?;
        Ok(())
    }

    pub fn update_cli_child_key_status_by_owner(
        &self,
        owner_key_id: &str,
        status: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE cli_child_keys
             SET status = ?1, updated_at = ?2
             WHERE owner_key_id = ?3",
            params![status, now_ts(), owner_key_id],
        )?;
        Ok(())
    }

    pub fn lookup_api_key_owner_context(&self, key_id: &str) -> Result<Option<ApiKeyOwnerContext>> {
        self.conn
            .query_row(
                "SELECT owner_key_id, cli_instance_uuid
                 FROM cli_child_keys
                 WHERE child_key_id = ?1
                 LIMIT 1",
                [key_id],
                |row| {
                    Ok(ApiKeyOwnerContext {
                        owner_key_id: row.get(0)?,
                        cli_instance_uuid: row.get(1)?,
                    })
                },
            )
            .optional()
    }

    pub fn save_cli_oauth_session(&self, session: &CliOAuthSession) -> Result<()> {
        self.conn.execute(
            "INSERT INTO cli_oauth_sessions (
                session_id,
                child_key_id,
                owner_key_id,
                cli_instance_uuid,
                client_id,
                redirect_uri,
                pkce_challenge,
                pkce_method,
                state,
                authorization_code_hash,
                refresh_token_hash,
                status,
                id_token,
                expires_at,
                refresh_expires_at,
                created_at,
                updated_at,
                last_seen_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18
             )
             ON CONFLICT(session_id) DO UPDATE SET
                child_key_id = excluded.child_key_id,
                owner_key_id = excluded.owner_key_id,
                cli_instance_uuid = excluded.cli_instance_uuid,
                client_id = excluded.client_id,
                redirect_uri = excluded.redirect_uri,
                pkce_challenge = excluded.pkce_challenge,
                pkce_method = excluded.pkce_method,
                state = excluded.state,
                authorization_code_hash = excluded.authorization_code_hash,
                refresh_token_hash = excluded.refresh_token_hash,
                status = excluded.status,
                id_token = excluded.id_token,
                expires_at = excluded.expires_at,
                refresh_expires_at = excluded.refresh_expires_at,
                updated_at = excluded.updated_at,
                last_seen_at = excluded.last_seen_at",
            params![
                &session.session_id,
                &session.child_key_id,
                &session.owner_key_id,
                &session.cli_instance_uuid,
                &session.client_id,
                &session.redirect_uri,
                &session.pkce_challenge,
                &session.pkce_method,
                &session.state,
                &session.authorization_code_hash,
                &session.refresh_token_hash,
                &session.status,
                &session.id_token,
                session.expires_at,
                session.refresh_expires_at,
                session.created_at,
                session.updated_at,
                session.last_seen_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_cli_oauth_session(&self, session_id: &str) -> Result<Option<CliOAuthSession>> {
        self.conn
            .query_row(
                "SELECT
                    session_id,
                    child_key_id,
                    owner_key_id,
                    cli_instance_uuid,
                    client_id,
                    redirect_uri,
                    pkce_challenge,
                    pkce_method,
                    state,
                    authorization_code_hash,
                    refresh_token_hash,
                    status,
                    id_token,
                    expires_at,
                    refresh_expires_at,
                    created_at,
                    updated_at,
                    last_seen_at
                 FROM cli_oauth_sessions
                 WHERE session_id = ?1
                 LIMIT 1",
                [session_id],
                map_cli_oauth_session,
            )
            .optional()
    }

    pub fn find_cli_oauth_session_by_authorization_code_hash(
        &self,
        authorization_code_hash: &str,
    ) -> Result<Option<CliOAuthSession>> {
        self.conn
            .query_row(
                "SELECT
                    session_id,
                    child_key_id,
                    owner_key_id,
                    cli_instance_uuid,
                    client_id,
                    redirect_uri,
                    pkce_challenge,
                    pkce_method,
                    state,
                    authorization_code_hash,
                    refresh_token_hash,
                    status,
                    id_token,
                    expires_at,
                    refresh_expires_at,
                    created_at,
                    updated_at,
                    last_seen_at
                 FROM cli_oauth_sessions
                 WHERE authorization_code_hash = ?1
                 LIMIT 1",
                [authorization_code_hash],
                map_cli_oauth_session,
            )
            .optional()
    }

    pub fn find_cli_oauth_session_by_refresh_token_hash(
        &self,
        refresh_token_hash: &str,
    ) -> Result<Option<CliOAuthSession>> {
        self.conn
            .query_row(
                "SELECT
                    session_id,
                    child_key_id,
                    owner_key_id,
                    cli_instance_uuid,
                    client_id,
                    redirect_uri,
                    pkce_challenge,
                    pkce_method,
                    state,
                    authorization_code_hash,
                    refresh_token_hash,
                    status,
                    id_token,
                    expires_at,
                    refresh_expires_at,
                    created_at,
                    updated_at,
                    last_seen_at
                 FROM cli_oauth_sessions
                 WHERE refresh_token_hash = ?1
                 LIMIT 1",
                [refresh_token_hash],
                map_cli_oauth_session,
            )
            .optional()
    }

    pub fn find_cli_oauth_session_by_id_token(
        &self,
        id_token: &str,
    ) -> Result<Option<CliOAuthSession>> {
        self.conn
            .query_row(
                "SELECT
                    session_id,
                    child_key_id,
                    owner_key_id,
                    cli_instance_uuid,
                    client_id,
                    redirect_uri,
                    pkce_challenge,
                    pkce_method,
                    state,
                    authorization_code_hash,
                    refresh_token_hash,
                    status,
                    id_token,
                    expires_at,
                    refresh_expires_at,
                    created_at,
                    updated_at,
                    last_seen_at
                 FROM cli_oauth_sessions
                 WHERE id_token = ?1
                 LIMIT 1",
                [id_token],
                map_cli_oauth_session,
            )
            .optional()
    }

    pub fn invalidate_cli_oauth_sessions_for_owner(
        &self,
        owner_key_id: &str,
        status: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE cli_oauth_sessions
             SET status = ?1,
                 authorization_code_hash = NULL,
                 refresh_token_hash = '',
                 updated_at = ?2
             WHERE owner_key_id = ?3",
            params![status, now_ts(), owner_key_id],
        )?;
        Ok(())
    }

    pub fn invalidate_cli_oauth_sessions_for_child_key(
        &self,
        child_key_id: &str,
        status: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE cli_oauth_sessions
             SET status = ?1,
                 authorization_code_hash = NULL,
                 refresh_token_hash = '',
                 updated_at = ?2
             WHERE child_key_id = ?3",
            params![status, now_ts(), child_key_id],
        )?;
        Ok(())
    }

    pub fn delete_cli_oauth_sessions_for_owner(&self, owner_key_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM cli_oauth_sessions WHERE owner_key_id = ?1",
            [owner_key_id],
        )?;
        Ok(())
    }

    pub fn delete_cli_oauth_sessions_for_child_key(&self, child_key_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM cli_oauth_sessions WHERE child_key_id = ?1",
            [child_key_id],
        )?;
        Ok(())
    }

    pub fn upsert_account_quota_exhaustion(
        &self,
        exhaustion: &AccountQuotaExhaustion,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO account_quota_exhaustion (
                account_id,
                reason,
                exhausted_until,
                updated_at
             ) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id) DO UPDATE SET
                reason = excluded.reason,
                exhausted_until = excluded.exhausted_until,
                updated_at = excluded.updated_at",
            params![
                &exhaustion.account_id,
                &exhaustion.reason,
                exhaustion.exhausted_until,
                exhaustion.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_account_quota_exhaustion(
        &self,
        account_id: &str,
    ) -> Result<Option<AccountQuotaExhaustion>> {
        self.conn
            .query_row(
                "SELECT account_id, reason, exhausted_until, updated_at
                 FROM account_quota_exhaustion
                 WHERE account_id = ?1
                 LIMIT 1",
                [account_id],
                map_account_quota_exhaustion,
            )
            .optional()
    }

    pub fn delete_account_quota_exhaustion(&self, account_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM account_quota_exhaustion WHERE account_id = ?1",
            [account_id],
        )?;
        Ok(())
    }

    pub(super) fn ensure_cli_child_keys_table(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS cli_child_keys (
                child_key_id TEXT PRIMARY KEY,
                owner_key_id TEXT NOT NULL,
                cli_instance_uuid TEXT NOT NULL UNIQUE,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_cli_child_keys_owner_key_id
             ON cli_child_keys(owner_key_id, created_at DESC)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_cli_child_keys_last_seen_at
             ON cli_child_keys(last_seen_at DESC)",
            [],
        )?;
        Ok(())
    }

    pub(super) fn ensure_cli_oauth_sessions_table(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS cli_oauth_sessions (
                session_id TEXT PRIMARY KEY,
                child_key_id TEXT NOT NULL,
                owner_key_id TEXT NOT NULL,
                cli_instance_uuid TEXT NOT NULL,
                client_id TEXT NOT NULL,
                redirect_uri TEXT NOT NULL,
                pkce_challenge TEXT NOT NULL,
                pkce_method TEXT NOT NULL,
                state TEXT NOT NULL,
                authorization_code_hash TEXT,
                refresh_token_hash TEXT NOT NULL,
                status TEXT NOT NULL,
                id_token TEXT NOT NULL,
                expires_at INTEGER NOT NULL,
                refresh_expires_at INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_cli_oauth_sessions_authorization_code_hash
             ON cli_oauth_sessions(authorization_code_hash)",
            [],
        )?;
        self.conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_cli_oauth_sessions_refresh_token_hash
             ON cli_oauth_sessions(refresh_token_hash)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_cli_oauth_sessions_child_key_id
             ON cli_oauth_sessions(child_key_id, updated_at DESC)",
            [],
        )?;
        Ok(())
    }

    pub(super) fn ensure_account_quota_exhaustion_table(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS account_quota_exhaustion (
                account_id TEXT PRIMARY KEY,
                reason TEXT NOT NULL,
                exhausted_until INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_account_quota_exhaustion_until
             ON account_quota_exhaustion(exhausted_until DESC)",
            [],
        )?;
        self.conn.execute(
            "DELETE FROM account_quota_exhaustion WHERE exhausted_until <= ?1",
            [now_ts()],
        )?;
        Ok(())
    }
}
