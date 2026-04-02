use rusqlite::{params, OptionalExtension, Result};

use super::{
    AffinityKeyMigration, AffinityScopePromotion, AffinityTurnCommitOutcome, ClientBinding,
    ContextSnapshot, ConversationContextEvent, ConversationContextState, ConversationThread,
    Storage,
};

fn map_client_binding(row: &rusqlite::Row<'_>) -> Result<ClientBinding> {
    Ok(ClientBinding {
        platform_key_hash: row.get(0)?,
        affinity_key: row.get(1)?,
        account_id: row.get(2)?,
        primary_scope_id: row.get(3)?,
        binding_version: row.get(4)?,
        status: row.get(5)?,
        last_supply_score: row.get(6)?,
        last_pressure_score: row.get(7)?,
        last_final_score: row.get(8)?,
        last_switch_reason: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
        last_seen_at: row.get(12)?,
    })
}

fn map_conversation_thread(row: &rusqlite::Row<'_>) -> Result<ConversationThread> {
    Ok(ConversationThread {
        platform_key_hash: row.get(0)?,
        affinity_key: row.get(1)?,
        conversation_scope_id: row.get(2)?,
        account_id: row.get(3)?,
        thread_epoch: row.get(4)?,
        thread_anchor: row.get(5)?,
        thread_version: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        last_seen_at: row.get(9)?,
    })
}

fn map_context_state(row: &rusqlite::Row<'_>) -> Result<ConversationContextState> {
    Ok(ConversationContextState {
        platform_key_hash: row.get(0)?,
        affinity_key: row.get(1)?,
        conversation_scope_id: row.get(2)?,
        model: row.get(3)?,
        instructions_text: row.get(4)?,
        tools_json: row.get(5)?,
        tool_choice_json: row.get(6)?,
        parallel_tool_calls: row.get::<_, Option<i64>>(7)?.map(|value| value != 0),
        reasoning_json: row.get(8)?,
        text_format_json: row.get(9)?,
        service_tier: row.get(10)?,
        metadata_json: row.get(11)?,
        encrypted_content: row.get(12)?,
        protocol_type: row.get(13)?,
        response_adapter: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

fn map_context_event(row: &rusqlite::Row<'_>) -> Result<ConversationContextEvent> {
    Ok(ConversationContextEvent {
        platform_key_hash: row.get(0)?,
        affinity_key: row.get(1)?,
        conversation_scope_id: row.get(2)?,
        turn_index: row.get(3)?,
        item_seq: row.get(4)?,
        role: row.get(5)?,
        pair_group_id: row.get(6)?,
        capture_complete: row.get::<_, i64>(7)? != 0,
        item_json: row.get(8)?,
        created_at: row.get(9)?,
    })
}

fn map_context_snapshot(row: &rusqlite::Row<'_>) -> Result<ContextSnapshot> {
    Ok(ContextSnapshot {
        platform_key_hash: row.get(0)?,
        affinity_key: row.get(1)?,
        conversation_scope_id: row.get(2)?,
        upto_turn_index: row.get(3)?,
        summary_text: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn affinity_key_exists(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    platform_key_hash: &str,
    affinity_key: &str,
) -> Result<bool> {
    let sql =
        format!("SELECT COUNT(1) FROM {table} WHERE platform_key_hash = ?1 AND affinity_key = ?2");
    let count: i64 = tx.query_row(
        sql.as_str(),
        params![platform_key_hash, affinity_key],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn affinity_key_migration_conflicts(
    tx: &rusqlite::Transaction<'_>,
    migration: &AffinityKeyMigration,
) -> Result<bool> {
    let tables = [
        "client_bindings",
        "conversation_threads",
        "conversation_context_state",
        "conversation_context_events",
        "context_snapshots",
    ];
    for table in tables {
        if affinity_key_exists(
            tx,
            table,
            migration.platform_key_hash.as_str(),
            migration.to_affinity_key.as_str(),
        )? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn rewrite_affinity_key(
    tx: &rusqlite::Transaction<'_>,
    migration: &AffinityKeyMigration,
) -> Result<()> {
    let tables = [
        "client_bindings",
        "conversation_threads",
        "conversation_context_state",
        "conversation_context_events",
        "context_snapshots",
    ];
    for table in tables {
        let sql = format!(
            "UPDATE {table}
             SET affinity_key = ?3
             WHERE platform_key_hash = ?1
               AND affinity_key = ?2"
        );
        tx.execute(
            sql.as_str(),
            params![
                migration.platform_key_hash.as_str(),
                migration.from_affinity_key.as_str(),
                migration.to_affinity_key.as_str(),
            ],
        )?;
    }
    Ok(())
}

impl Storage {
    pub fn get_client_binding(
        &self,
        platform_key_hash: &str,
        affinity_key: &str,
    ) -> Result<Option<ClientBinding>> {
        self.conn
            .query_row(
                "SELECT
                    platform_key_hash,
                    affinity_key,
                    account_id,
                    primary_scope_id,
                    binding_version,
                    status,
                    last_supply_score,
                    last_pressure_score,
                    last_final_score,
                    last_switch_reason,
                    created_at,
                    updated_at,
                    last_seen_at
                 FROM client_bindings
                 WHERE platform_key_hash = ?1
                   AND affinity_key = ?2
                 LIMIT 1",
                params![platform_key_hash, affinity_key],
                map_client_binding,
            )
            .optional()
    }

    pub fn count_recent_client_bindings_for_account(
        &self,
        account_id: &str,
        last_seen_at_gte: i64,
        exclude_affinity_key: Option<(&str, &str)>,
    ) -> Result<i64> {
        match exclude_affinity_key {
            Some((platform_key_hash, affinity_key)) => self.conn.query_row(
                "SELECT COUNT(1)
                 FROM client_bindings
                 WHERE account_id = ?1
                   AND last_seen_at >= ?2
                   AND NOT (platform_key_hash = ?3 AND affinity_key = ?4)",
                params![
                    account_id,
                    last_seen_at_gte,
                    platform_key_hash,
                    affinity_key
                ],
                |row| row.get(0),
            ),
            None => self.conn.query_row(
                "SELECT COUNT(1)
                 FROM client_bindings
                 WHERE account_id = ?1
                   AND last_seen_at >= ?2",
                params![account_id, last_seen_at_gte],
                |row| row.get(0),
            ),
        }
    }

    pub fn save_client_binding(
        &self,
        binding: &ClientBinding,
        expected_binding_version: Option<i64>,
    ) -> Result<bool> {
        match expected_binding_version {
            Some(expected_version) => {
                let updated = self.conn.execute(
                    "UPDATE client_bindings
                     SET account_id = ?3,
                         primary_scope_id = ?4,
                         binding_version = ?5,
                         status = ?6,
                         last_supply_score = ?7,
                         last_pressure_score = ?8,
                         last_final_score = ?9,
                         last_switch_reason = ?10,
                         updated_at = ?11,
                         last_seen_at = ?12
                     WHERE platform_key_hash = ?1
                       AND affinity_key = ?2
                       AND binding_version = ?13",
                    params![
                        &binding.platform_key_hash,
                        &binding.affinity_key,
                        &binding.account_id,
                        &binding.primary_scope_id,
                        binding.binding_version,
                        &binding.status,
                        binding.last_supply_score,
                        binding.last_pressure_score,
                        binding.last_final_score,
                        &binding.last_switch_reason,
                        binding.updated_at,
                        binding.last_seen_at,
                        expected_version,
                    ],
                )?;
                Ok(updated > 0)
            }
            None => {
                let inserted = self.conn.execute(
                    "INSERT OR IGNORE INTO client_bindings (
                        platform_key_hash,
                        affinity_key,
                        account_id,
                        primary_scope_id,
                        binding_version,
                        status,
                        last_supply_score,
                        last_pressure_score,
                        last_final_score,
                        last_switch_reason,
                        created_at,
                        updated_at,
                        last_seen_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13
                     )",
                    params![
                        &binding.platform_key_hash,
                        &binding.affinity_key,
                        &binding.account_id,
                        &binding.primary_scope_id,
                        binding.binding_version,
                        &binding.status,
                        binding.last_supply_score,
                        binding.last_pressure_score,
                        binding.last_final_score,
                        &binding.last_switch_reason,
                        binding.created_at,
                        binding.updated_at,
                        binding.last_seen_at,
                    ],
                )?;
                Ok(inserted > 0)
            }
        }
    }

    pub fn touch_client_binding(
        &self,
        platform_key_hash: &str,
        affinity_key: &str,
        account_id: &str,
        touched_at: i64,
    ) -> Result<bool> {
        let updated = self.conn.execute(
            "UPDATE client_bindings
             SET last_seen_at = ?4,
                 updated_at = ?4
             WHERE platform_key_hash = ?1
               AND affinity_key = ?2
               AND account_id = ?3",
            params![platform_key_hash, affinity_key, account_id, touched_at],
        )?;
        Ok(updated > 0)
    }

    pub fn get_conversation_thread(
        &self,
        platform_key_hash: &str,
        affinity_key: &str,
        conversation_scope_id: &str,
    ) -> Result<Option<ConversationThread>> {
        self.conn
            .query_row(
                "SELECT
                    platform_key_hash,
                    affinity_key,
                    conversation_scope_id,
                    account_id,
                    thread_epoch,
                    thread_anchor,
                    thread_version,
                    created_at,
                    updated_at,
                    last_seen_at
                 FROM conversation_threads
                 WHERE platform_key_hash = ?1
                   AND affinity_key = ?2
                   AND conversation_scope_id = ?3
                 LIMIT 1",
                params![platform_key_hash, affinity_key, conversation_scope_id],
                map_conversation_thread,
            )
            .optional()
    }

    pub fn latest_conversation_turn_index(
        &self,
        platform_key_hash: &str,
        affinity_key: &str,
        conversation_scope_id: &str,
    ) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT turn_index
                 FROM conversation_context_events
                 WHERE platform_key_hash = ?1
                   AND affinity_key = ?2
                   AND conversation_scope_id = ?3
                 ORDER BY turn_index DESC, item_seq ASC
                 LIMIT 1",
                params![platform_key_hash, affinity_key, conversation_scope_id],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn list_conversation_threads_for_affinity(
        &self,
        platform_key_hash: &str,
        affinity_key: &str,
    ) -> Result<Vec<ConversationThread>> {
        let mut stmt = self.conn.prepare(
            "SELECT
                platform_key_hash,
                affinity_key,
                conversation_scope_id,
                account_id,
                thread_epoch,
                thread_anchor,
                thread_version,
                created_at,
                updated_at,
                last_seen_at
             FROM conversation_threads
             WHERE platform_key_hash = ?1
               AND affinity_key = ?2
             ORDER BY conversation_scope_id ASC",
        )?;
        let mut rows = stmt.query(params![platform_key_hash, affinity_key])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(map_conversation_thread(row)?);
        }
        Ok(out)
    }

    pub fn save_conversation_thread(
        &self,
        thread: &ConversationThread,
        expected_thread_version: Option<i64>,
    ) -> Result<bool> {
        match expected_thread_version {
            Some(expected_version) => {
                let updated = self.conn.execute(
                    "UPDATE conversation_threads
                     SET account_id = ?4,
                         thread_epoch = ?5,
                         thread_anchor = ?6,
                         thread_version = ?7,
                         updated_at = ?8,
                         last_seen_at = ?9
                     WHERE platform_key_hash = ?1
                       AND affinity_key = ?2
                       AND conversation_scope_id = ?3
                       AND thread_version = ?10",
                    params![
                        &thread.platform_key_hash,
                        &thread.affinity_key,
                        &thread.conversation_scope_id,
                        &thread.account_id,
                        thread.thread_epoch,
                        &thread.thread_anchor,
                        thread.thread_version,
                        thread.updated_at,
                        thread.last_seen_at,
                        expected_version,
                    ],
                )?;
                Ok(updated > 0)
            }
            None => {
                let inserted = self.conn.execute(
                    "INSERT OR IGNORE INTO conversation_threads (
                        platform_key_hash,
                        affinity_key,
                        conversation_scope_id,
                        account_id,
                        thread_epoch,
                        thread_anchor,
                        thread_version,
                        created_at,
                        updated_at,
                        last_seen_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10
                     )",
                    params![
                        &thread.platform_key_hash,
                        &thread.affinity_key,
                        &thread.conversation_scope_id,
                        &thread.account_id,
                        thread.thread_epoch,
                        &thread.thread_anchor,
                        thread.thread_version,
                        thread.created_at,
                        thread.updated_at,
                        thread.last_seen_at,
                    ],
                )?;
                Ok(inserted > 0)
            }
        }
    }

    pub fn touch_conversation_thread(
        &self,
        platform_key_hash: &str,
        affinity_key: &str,
        conversation_scope_id: &str,
        account_id: &str,
        touched_at: i64,
    ) -> Result<bool> {
        let updated = self.conn.execute(
            "UPDATE conversation_threads
             SET last_seen_at = ?5,
                 updated_at = ?5
             WHERE platform_key_hash = ?1
               AND affinity_key = ?2
               AND conversation_scope_id = ?3
               AND account_id = ?4",
            params![
                platform_key_hash,
                affinity_key,
                conversation_scope_id,
                account_id,
                touched_at,
            ],
        )?;
        Ok(updated > 0)
    }

    pub fn get_conversation_context_state(
        &self,
        platform_key_hash: &str,
        affinity_key: &str,
        conversation_scope_id: &str,
    ) -> Result<Option<ConversationContextState>> {
        self.conn
            .query_row(
                "SELECT
                    platform_key_hash,
                    affinity_key,
                    conversation_scope_id,
                    model,
                    instructions_text,
                    tools_json,
                    tool_choice_json,
                    parallel_tool_calls,
                    reasoning_json,
                    text_format_json,
                    service_tier,
                    metadata_json,
                    encrypted_content,
                    protocol_type,
                    response_adapter,
                    updated_at
                 FROM conversation_context_state
                 WHERE platform_key_hash = ?1
                   AND affinity_key = ?2
                   AND conversation_scope_id = ?3
                 LIMIT 1",
                params![platform_key_hash, affinity_key, conversation_scope_id],
                map_context_state,
            )
            .optional()
    }

    pub fn save_conversation_context_state(&self, state: &ConversationContextState) -> Result<()> {
        self.conn.execute(
            "INSERT INTO conversation_context_state (
                platform_key_hash,
                affinity_key,
                conversation_scope_id,
                model,
                instructions_text,
                tools_json,
                tool_choice_json,
                parallel_tool_calls,
                reasoning_json,
                text_format_json,
                service_tier,
                metadata_json,
                encrypted_content,
                protocol_type,
                response_adapter,
                updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
             )
             ON CONFLICT(platform_key_hash, affinity_key, conversation_scope_id) DO UPDATE SET
                model = excluded.model,
                instructions_text = excluded.instructions_text,
                tools_json = excluded.tools_json,
                tool_choice_json = excluded.tool_choice_json,
                parallel_tool_calls = excluded.parallel_tool_calls,
                reasoning_json = excluded.reasoning_json,
                text_format_json = excluded.text_format_json,
                service_tier = excluded.service_tier,
                metadata_json = excluded.metadata_json,
                encrypted_content = excluded.encrypted_content,
                protocol_type = excluded.protocol_type,
                response_adapter = excluded.response_adapter,
                updated_at = excluded.updated_at",
            params![
                &state.platform_key_hash,
                &state.affinity_key,
                &state.conversation_scope_id,
                &state.model,
                &state.instructions_text,
                &state.tools_json,
                &state.tool_choice_json,
                state
                    .parallel_tool_calls
                    .map(|value| if value { 1 } else { 0 }),
                &state.reasoning_json,
                &state.text_format_json,
                &state.service_tier,
                &state.metadata_json,
                &state.encrypted_content,
                &state.protocol_type,
                &state.response_adapter,
                state.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn list_conversation_context_events(
        &self,
        platform_key_hash: &str,
        affinity_key: &str,
        conversation_scope_id: &str,
    ) -> Result<Vec<ConversationContextEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT
                platform_key_hash,
                affinity_key,
                conversation_scope_id,
                turn_index,
                item_seq,
                role,
                pair_group_id,
                capture_complete,
                item_json,
                created_at
             FROM conversation_context_events
             WHERE platform_key_hash = ?1
               AND affinity_key = ?2
               AND conversation_scope_id = ?3
             ORDER BY turn_index ASC, item_seq ASC",
        )?;
        let mut rows = stmt.query(params![
            platform_key_hash,
            affinity_key,
            conversation_scope_id
        ])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(map_context_event(row)?);
        }
        Ok(out)
    }

    pub fn replace_conversation_context_turn(
        &self,
        platform_key_hash: &str,
        affinity_key: &str,
        conversation_scope_id: &str,
        turn_index: i64,
        events: &[ConversationContextEvent],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM conversation_context_events
             WHERE platform_key_hash = ?1
               AND affinity_key = ?2
               AND conversation_scope_id = ?3
               AND turn_index = ?4",
            params![
                platform_key_hash,
                affinity_key,
                conversation_scope_id,
                turn_index
            ],
        )?;
        for event in events {
            tx.execute(
                "INSERT INTO conversation_context_events (
                    platform_key_hash,
                    affinity_key,
                    conversation_scope_id,
                    turn_index,
                    item_seq,
                    role,
                    pair_group_id,
                    capture_complete,
                    item_json,
                    created_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10
                 )",
                params![
                    &event.platform_key_hash,
                    &event.affinity_key,
                    &event.conversation_scope_id,
                    event.turn_index,
                    event.item_seq,
                    &event.role,
                    &event.pair_group_id,
                    if event.capture_complete { 1 } else { 0 },
                    &event.item_json,
                    event.created_at,
                ],
            )?;
        }
        tx.commit()
    }

    pub fn list_context_snapshots(
        &self,
        platform_key_hash: &str,
        affinity_key: &str,
        conversation_scope_id: &str,
    ) -> Result<Vec<ContextSnapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT
                platform_key_hash,
                affinity_key,
                conversation_scope_id,
                upto_turn_index,
                summary_text,
                created_at,
                updated_at
             FROM context_snapshots
             WHERE platform_key_hash = ?1
               AND affinity_key = ?2
               AND conversation_scope_id = ?3
             ORDER BY upto_turn_index DESC",
        )?;
        let mut rows = stmt.query(params![
            platform_key_hash,
            affinity_key,
            conversation_scope_id
        ])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(map_context_snapshot(row)?);
        }
        Ok(out)
    }

    pub fn save_context_snapshot(&self, snapshot: &ContextSnapshot) -> Result<()> {
        self.conn.execute(
            "INSERT INTO context_snapshots (
                platform_key_hash,
                affinity_key,
                conversation_scope_id,
                upto_turn_index,
                summary_text,
                created_at,
                updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7
             )
             ON CONFLICT(platform_key_hash, affinity_key, conversation_scope_id, upto_turn_index)
             DO UPDATE SET
                summary_text = excluded.summary_text,
                updated_at = excluded.updated_at",
            params![
                &snapshot.platform_key_hash,
                &snapshot.affinity_key,
                &snapshot.conversation_scope_id,
                snapshot.upto_turn_index,
                &snapshot.summary_text,
                snapshot.created_at,
                snapshot.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn commit_affinity_turn_success(
        &self,
        binding: &ClientBinding,
        expected_binding_version: Option<i64>,
        thread: &ConversationThread,
        expected_thread_version: Option<i64>,
        scope_promotion: Option<&AffinityScopePromotion>,
        key_migration: Option<&AffinityKeyMigration>,
        context_state: &ConversationContextState,
        turn_index: i64,
        events: &[ConversationContextEvent],
        reset_existing_context: bool,
    ) -> Result<AffinityTurnCommitOutcome> {
        let tx = self.conn.unchecked_transaction()?;

        if let Some(migration) = key_migration {
            if migration.from_affinity_key != migration.to_affinity_key {
                if affinity_key_migration_conflicts(&tx, migration)? {
                    tx.rollback()?;
                    return Ok(AffinityTurnCommitOutcome::MigrationConflict);
                }
                rewrite_affinity_key(&tx, migration)?;
            }
        }

        if let Some(promotion) = scope_promotion {
            if promotion.from_scope_id != promotion.to_scope_id {
                let target_thread_exists: i64 = tx.query_row(
                    "SELECT COUNT(1)
                     FROM conversation_threads
                     WHERE platform_key_hash = ?1
                       AND affinity_key = ?2
                       AND conversation_scope_id = ?3",
                    params![
                        &promotion.platform_key_hash,
                        &promotion.affinity_key,
                        &promotion.to_scope_id,
                    ],
                    |row| row.get(0),
                )?;
                let target_state_exists: i64 = tx.query_row(
                    "SELECT COUNT(1)
                     FROM conversation_context_state
                     WHERE platform_key_hash = ?1
                       AND affinity_key = ?2
                       AND conversation_scope_id = ?3",
                    params![
                        &promotion.platform_key_hash,
                        &promotion.affinity_key,
                        &promotion.to_scope_id,
                    ],
                    |row| row.get(0),
                )?;
                let target_snapshot_exists: i64 = tx.query_row(
                    "SELECT COUNT(1)
                     FROM context_snapshots
                     WHERE platform_key_hash = ?1
                       AND affinity_key = ?2
                       AND conversation_scope_id = ?3",
                    params![
                        &promotion.platform_key_hash,
                        &promotion.affinity_key,
                        &promotion.to_scope_id,
                    ],
                    |row| row.get(0),
                )?;
                let target_event_exists: i64 = tx.query_row(
                    "SELECT COUNT(1)
                     FROM conversation_context_events
                     WHERE platform_key_hash = ?1
                       AND affinity_key = ?2
                       AND conversation_scope_id = ?3",
                    params![
                        &promotion.platform_key_hash,
                        &promotion.affinity_key,
                        &promotion.to_scope_id,
                    ],
                    |row| row.get(0),
                )?;
                if target_thread_exists > 0
                    || target_state_exists > 0
                    || target_snapshot_exists > 0
                    || target_event_exists > 0
                {
                    tx.rollback()?;
                    return Ok(AffinityTurnCommitOutcome::Conflict);
                }

                tx.execute(
                    "UPDATE conversation_threads
                     SET conversation_scope_id = ?4
                     WHERE platform_key_hash = ?1
                       AND affinity_key = ?2
                       AND conversation_scope_id = ?3",
                    params![
                        &promotion.platform_key_hash,
                        &promotion.affinity_key,
                        &promotion.from_scope_id,
                        &promotion.to_scope_id,
                    ],
                )?;
                tx.execute(
                    "UPDATE conversation_context_state
                     SET conversation_scope_id = ?4
                     WHERE platform_key_hash = ?1
                       AND affinity_key = ?2
                       AND conversation_scope_id = ?3",
                    params![
                        &promotion.platform_key_hash,
                        &promotion.affinity_key,
                        &promotion.from_scope_id,
                        &promotion.to_scope_id,
                    ],
                )?;
                tx.execute(
                    "UPDATE conversation_context_events
                     SET conversation_scope_id = ?4
                     WHERE platform_key_hash = ?1
                       AND affinity_key = ?2
                       AND conversation_scope_id = ?3",
                    params![
                        &promotion.platform_key_hash,
                        &promotion.affinity_key,
                        &promotion.from_scope_id,
                        &promotion.to_scope_id,
                    ],
                )?;
                tx.execute(
                    "UPDATE context_snapshots
                     SET conversation_scope_id = ?4
                     WHERE platform_key_hash = ?1
                       AND affinity_key = ?2
                       AND conversation_scope_id = ?3",
                    params![
                        &promotion.platform_key_hash,
                        &promotion.affinity_key,
                        &promotion.from_scope_id,
                        &promotion.to_scope_id,
                    ],
                )?;
            }
        }

        let binding_saved = match expected_binding_version {
            Some(expected_version) => {
                let updated = tx.execute(
                    "UPDATE client_bindings
                     SET account_id = ?3,
                         primary_scope_id = ?4,
                         binding_version = ?5,
                         status = ?6,
                         last_supply_score = ?7,
                         last_pressure_score = ?8,
                         last_final_score = ?9,
                         last_switch_reason = ?10,
                         updated_at = ?11,
                         last_seen_at = ?12
                     WHERE platform_key_hash = ?1
                       AND affinity_key = ?2
                       AND binding_version = ?13",
                    params![
                        &binding.platform_key_hash,
                        &binding.affinity_key,
                        &binding.account_id,
                        &binding.primary_scope_id,
                        binding.binding_version,
                        &binding.status,
                        binding.last_supply_score,
                        binding.last_pressure_score,
                        binding.last_final_score,
                        &binding.last_switch_reason,
                        binding.updated_at,
                        binding.last_seen_at,
                        expected_version,
                    ],
                )?;
                updated > 0
            }
            None => {
                let inserted = tx.execute(
                    "INSERT OR IGNORE INTO client_bindings (
                        platform_key_hash,
                        affinity_key,
                        account_id,
                        primary_scope_id,
                        binding_version,
                        status,
                        last_supply_score,
                        last_pressure_score,
                        last_final_score,
                        last_switch_reason,
                        created_at,
                        updated_at,
                        last_seen_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13
                     )",
                    params![
                        &binding.platform_key_hash,
                        &binding.affinity_key,
                        &binding.account_id,
                        &binding.primary_scope_id,
                        binding.binding_version,
                        &binding.status,
                        binding.last_supply_score,
                        binding.last_pressure_score,
                        binding.last_final_score,
                        &binding.last_switch_reason,
                        binding.created_at,
                        binding.updated_at,
                        binding.last_seen_at,
                    ],
                )?;
                inserted > 0
            }
        };
        if !binding_saved {
            tx.rollback()?;
            return Ok(AffinityTurnCommitOutcome::Conflict);
        }

        let thread_saved = match expected_thread_version {
            Some(expected_version) => {
                let updated = tx.execute(
                    "UPDATE conversation_threads
                     SET account_id = ?4,
                         thread_epoch = ?5,
                         thread_anchor = ?6,
                         thread_version = ?7,
                         updated_at = ?8,
                         last_seen_at = ?9
                     WHERE platform_key_hash = ?1
                       AND affinity_key = ?2
                       AND conversation_scope_id = ?3
                       AND thread_version = ?10",
                    params![
                        &thread.platform_key_hash,
                        &thread.affinity_key,
                        &thread.conversation_scope_id,
                        &thread.account_id,
                        thread.thread_epoch,
                        &thread.thread_anchor,
                        thread.thread_version,
                        thread.updated_at,
                        thread.last_seen_at,
                        expected_version,
                    ],
                )?;
                updated > 0
            }
            None => {
                let inserted = tx.execute(
                    "INSERT OR IGNORE INTO conversation_threads (
                        platform_key_hash,
                        affinity_key,
                        conversation_scope_id,
                        account_id,
                        thread_epoch,
                        thread_anchor,
                        thread_version,
                        created_at,
                        updated_at,
                        last_seen_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10
                     )",
                    params![
                        &thread.platform_key_hash,
                        &thread.affinity_key,
                        &thread.conversation_scope_id,
                        &thread.account_id,
                        thread.thread_epoch,
                        &thread.thread_anchor,
                        thread.thread_version,
                        thread.created_at,
                        thread.updated_at,
                        thread.last_seen_at,
                    ],
                )?;
                inserted > 0
            }
        };
        if !thread_saved {
            tx.rollback()?;
            return Ok(AffinityTurnCommitOutcome::Conflict);
        }

        if reset_existing_context {
            tx.execute(
                "DELETE FROM context_snapshots
                 WHERE platform_key_hash = ?1
                   AND affinity_key = ?2
                   AND conversation_scope_id = ?3",
                params![
                    &thread.platform_key_hash,
                    &thread.affinity_key,
                    &thread.conversation_scope_id,
                ],
            )?;
            tx.execute(
                "DELETE FROM conversation_context_events
                 WHERE platform_key_hash = ?1
                   AND affinity_key = ?2
                   AND conversation_scope_id = ?3",
                params![
                    &thread.platform_key_hash,
                    &thread.affinity_key,
                    &thread.conversation_scope_id,
                ],
            )?;
            tx.execute(
                "DELETE FROM conversation_context_state
                 WHERE platform_key_hash = ?1
                   AND affinity_key = ?2
                   AND conversation_scope_id = ?3",
                params![
                    &thread.platform_key_hash,
                    &thread.affinity_key,
                    &thread.conversation_scope_id,
                ],
            )?;
        }

        tx.execute(
            "INSERT INTO conversation_context_state (
                platform_key_hash,
                affinity_key,
                conversation_scope_id,
                model,
                instructions_text,
                tools_json,
                tool_choice_json,
                parallel_tool_calls,
                reasoning_json,
                text_format_json,
                service_tier,
                metadata_json,
                encrypted_content,
                protocol_type,
                response_adapter,
                updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
             )
             ON CONFLICT(platform_key_hash, affinity_key, conversation_scope_id) DO UPDATE SET
                model = excluded.model,
                instructions_text = excluded.instructions_text,
                tools_json = excluded.tools_json,
                tool_choice_json = excluded.tool_choice_json,
                parallel_tool_calls = excluded.parallel_tool_calls,
                reasoning_json = excluded.reasoning_json,
                text_format_json = excluded.text_format_json,
                service_tier = excluded.service_tier,
                metadata_json = excluded.metadata_json,
                encrypted_content = excluded.encrypted_content,
                protocol_type = excluded.protocol_type,
                response_adapter = excluded.response_adapter,
                updated_at = excluded.updated_at",
            params![
                &context_state.platform_key_hash,
                &context_state.affinity_key,
                &context_state.conversation_scope_id,
                &context_state.model,
                &context_state.instructions_text,
                &context_state.tools_json,
                &context_state.tool_choice_json,
                context_state
                    .parallel_tool_calls
                    .map(|value| if value { 1 } else { 0 }),
                &context_state.reasoning_json,
                &context_state.text_format_json,
                &context_state.service_tier,
                &context_state.metadata_json,
                &context_state.encrypted_content,
                &context_state.protocol_type,
                &context_state.response_adapter,
                context_state.updated_at,
            ],
        )?;

        tx.execute(
            "DELETE FROM conversation_context_events
             WHERE platform_key_hash = ?1
               AND affinity_key = ?2
               AND conversation_scope_id = ?3
               AND turn_index = ?4",
            params![
                &thread.platform_key_hash,
                &thread.affinity_key,
                &thread.conversation_scope_id,
                turn_index,
            ],
        )?;
        for event in events {
            tx.execute(
                "INSERT INTO conversation_context_events (
                    platform_key_hash,
                    affinity_key,
                    conversation_scope_id,
                    turn_index,
                    item_seq,
                    role,
                    pair_group_id,
                    capture_complete,
                    item_json,
                    created_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10
                 )",
                params![
                    &event.platform_key_hash,
                    &event.affinity_key,
                    &event.conversation_scope_id,
                    event.turn_index,
                    event.item_seq,
                    &event.role,
                    &event.pair_group_id,
                    if event.capture_complete { 1 } else { 0 },
                    &event.item_json,
                    event.created_at,
                ],
            )?;
        }

        tx.commit()?;
        Ok(AffinityTurnCommitOutcome::Committed)
    }

    pub fn promote_affinity_primary_scope(
        &self,
        platform_key_hash: &str,
        affinity_key: &str,
        from_scope_id: &str,
        to_scope_id: &str,
        expected_binding_version: i64,
        next_binding_version: i64,
        updated_at: i64,
    ) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let updated = tx.execute(
            "UPDATE client_bindings
             SET primary_scope_id = ?3,
                 binding_version = ?4,
                 updated_at = ?5,
                 last_seen_at = ?5
             WHERE platform_key_hash = ?1
               AND affinity_key = ?2
               AND binding_version = ?6",
            params![
                platform_key_hash,
                affinity_key,
                to_scope_id,
                next_binding_version,
                updated_at,
                expected_binding_version,
            ],
        )?;
        if updated == 0 {
            tx.rollback()?;
            return Ok(false);
        }
        tx.execute(
            "UPDATE conversation_threads
             SET conversation_scope_id = ?4
             WHERE platform_key_hash = ?1
               AND affinity_key = ?2
               AND conversation_scope_id = ?3",
            params![platform_key_hash, affinity_key, from_scope_id, to_scope_id],
        )?;
        tx.execute(
            "UPDATE conversation_context_state
             SET conversation_scope_id = ?4
             WHERE platform_key_hash = ?1
               AND affinity_key = ?2
               AND conversation_scope_id = ?3",
            params![platform_key_hash, affinity_key, from_scope_id, to_scope_id],
        )?;
        tx.execute(
            "UPDATE conversation_context_events
             SET conversation_scope_id = ?4
             WHERE platform_key_hash = ?1
               AND affinity_key = ?2
               AND conversation_scope_id = ?3",
            params![platform_key_hash, affinity_key, from_scope_id, to_scope_id],
        )?;
        tx.execute(
            "UPDATE context_snapshots
             SET conversation_scope_id = ?4
             WHERE platform_key_hash = ?1
               AND affinity_key = ?2
               AND conversation_scope_id = ?3",
            params![platform_key_hash, affinity_key, from_scope_id, to_scope_id],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn delete_affinity_state_for_account(&self, account_id: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM conversation_context_events
             WHERE EXISTS (
                 SELECT 1
                 FROM conversation_threads threads
                 WHERE threads.platform_key_hash = conversation_context_events.platform_key_hash
                   AND threads.affinity_key = conversation_context_events.affinity_key
                   AND threads.conversation_scope_id = conversation_context_events.conversation_scope_id
                   AND threads.account_id = ?1
             )",
            [account_id],
        )?;
        tx.execute(
            "DELETE FROM conversation_context_state
             WHERE EXISTS (
                 SELECT 1
                 FROM conversation_threads threads
                 WHERE threads.platform_key_hash = conversation_context_state.platform_key_hash
                   AND threads.affinity_key = conversation_context_state.affinity_key
                   AND threads.conversation_scope_id = conversation_context_state.conversation_scope_id
                   AND threads.account_id = ?1
             )",
            [account_id],
        )?;
        tx.execute(
            "DELETE FROM context_snapshots
             WHERE EXISTS (
                 SELECT 1
                 FROM conversation_threads threads
                 WHERE threads.platform_key_hash = context_snapshots.platform_key_hash
                   AND threads.affinity_key = context_snapshots.affinity_key
                   AND threads.conversation_scope_id = context_snapshots.conversation_scope_id
                   AND threads.account_id = ?1
             )",
            [account_id],
        )?;
        tx.execute(
            "DELETE FROM client_bindings WHERE account_id = ?1",
            [account_id],
        )?;
        tx.execute(
            "DELETE FROM conversation_threads WHERE account_id = ?1",
            [account_id],
        )?;
        tx.commit()
    }

    pub fn delete_stale_affinity_state(
        &self,
        stale_binding_before: i64,
        stale_context_before: i64,
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM client_bindings WHERE last_seen_at < ?1",
            [stale_binding_before],
        )?;
        tx.execute(
            "DELETE FROM conversation_threads WHERE last_seen_at < ?1",
            [stale_binding_before],
        )?;
        tx.execute(
            "DELETE FROM conversation_context_events WHERE created_at < ?1",
            [stale_context_before],
        )?;
        tx.execute(
            "DELETE FROM conversation_context_state WHERE updated_at < ?1",
            [stale_context_before],
        )?;
        tx.execute(
            "DELETE FROM context_snapshots WHERE updated_at < ?1",
            [stale_context_before],
        )?;
        tx.commit()
    }
}
