use codexmanager_core::storage::{
    now_ts, AffinityScopePromotion, AffinityTurnCommitOutcome, ClientBinding, ContextSnapshot,
    ConversationContextEvent, ConversationContextState, ConversationThread, Storage,
};
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_db_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("codexmanager-{name}-{}-{nanos}.db", process::id()))
}

#[test]
fn gateway_affinity_migration_creates_tables_and_indexes() {
    let db_path = temp_db_path("affinity-schema");
    let storage = Storage::open(&db_path).expect("open file db");
    storage.init().expect("init schema");

    let conn = Connection::open(&db_path).expect("open sqlite for inspection");

    let migration_count: i64 = conn
        .query_row(
            "SELECT COUNT(1) FROM schema_migrations WHERE version = ?1",
            ["040_gateway_affinity_routing"],
            |row| row.get(0),
        )
        .expect("count affinity migration");
    assert_eq!(migration_count, 1);

    for table_name in [
        "client_bindings",
        "conversation_threads",
        "conversation_context_state",
        "conversation_context_events",
        "context_snapshots",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(1)
                 FROM sqlite_master
                 WHERE type = 'table'
                   AND name = ?1",
                [table_name],
                |row| row.get(0),
            )
            .expect("table exists query");
        assert_eq!(exists, 1, "missing table {table_name}");
    }

    for index_name in [
        "idx_client_bindings_account_id",
        "idx_client_bindings_last_seen_at",
        "idx_conversation_threads_account_id",
        "idx_conversation_threads_last_seen_at",
        "idx_conversation_context_state_updated_at",
        "idx_conversation_context_events_scope_turn",
        "idx_conversation_context_events_created_at",
        "idx_context_snapshots_scope_turn",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(1)
                 FROM sqlite_master
                 WHERE type = 'index'
                   AND name = ?1",
                [index_name],
                |row| row.get(0),
            )
            .expect("index exists query");
        assert_eq!(exists, 1, "missing index {index_name}");
    }

    drop(conn);
    drop(storage);
    let _ = fs::remove_file(db_path);
}

#[test]
fn conversation_affinity_storage_roundtrip_and_delete_for_account() {
    let storage = Storage::open_in_memory().expect("open in memory");
    storage.init().expect("init schema");
    let now = now_ts();

    let binding = ClientBinding {
        platform_key_hash: "platform-1".to_string(),
        affinity_key: "sid:test-1".to_string(),
        account_id: "acc-1".to_string(),
        primary_scope_id: Some("scope-1".to_string()),
        binding_version: 1,
        status: "active".to_string(),
        last_supply_score: Some(0.9),
        last_pressure_score: Some(0.2),
        last_final_score: Some(0.8),
        last_switch_reason: Some("initial_bind".to_string()),
        created_at: now,
        updated_at: now,
        last_seen_at: now,
    };
    assert!(
        storage
            .save_client_binding(&binding, None)
            .expect("insert client binding")
    );

    let thread = ConversationThread {
        platform_key_hash: "platform-1".to_string(),
        affinity_key: "sid:test-1".to_string(),
        conversation_scope_id: "scope-1".to_string(),
        account_id: "acc-1".to_string(),
        thread_epoch: 1,
        thread_anchor: "resp_123".to_string(),
        thread_version: 1,
        created_at: now,
        updated_at: now,
        last_seen_at: now,
    };
    assert!(
        storage
            .save_conversation_thread(&thread, None)
            .expect("insert thread")
    );

    storage
        .save_conversation_context_state(&ConversationContextState {
            platform_key_hash: "platform-1".to_string(),
            affinity_key: "sid:test-1".to_string(),
            conversation_scope_id: "scope-1".to_string(),
            model: Some("gpt-5.4".to_string()),
            instructions_text: Some("be precise".to_string()),
            tools_json: Some("[{\"type\":\"function\",\"name\":\"lookup\"}]".to_string()),
            tool_choice_json: Some("{\"type\":\"auto\"}".to_string()),
            parallel_tool_calls: Some(true),
            reasoning_json: Some("{\"effort\":\"high\"}".to_string()),
            text_format_json: Some("{\"type\":\"text\"}".to_string()),
            service_tier: Some("default".to_string()),
            metadata_json: Some("{\"source\":\"test\"}".to_string()),
            encrypted_content: Some("ciphertext".to_string()),
            protocol_type: Some("openai_compat".to_string()),
            response_adapter: Some("Passthrough".to_string()),
            updated_at: now,
        })
        .expect("save context state");

    storage
        .replace_conversation_context_turn(
            "platform-1",
            "sid:test-1",
            "scope-1",
            0,
            &[
                ConversationContextEvent {
                    platform_key_hash: "platform-1".to_string(),
                    affinity_key: "sid:test-1".to_string(),
                    conversation_scope_id: "scope-1".to_string(),
                    turn_index: 0,
                    item_seq: 0,
                    role: Some("user".to_string()),
                    pair_group_id: None,
                    capture_complete: true,
                    item_json: "{\"type\":\"message\",\"role\":\"user\",\"content\":\"hi\"}"
                        .to_string(),
                    created_at: now,
                },
                ConversationContextEvent {
                    platform_key_hash: "platform-1".to_string(),
                    affinity_key: "sid:test-1".to_string(),
                    conversation_scope_id: "scope-1".to_string(),
                    turn_index: 0,
                    item_seq: 1,
                    role: Some("assistant".to_string()),
                    pair_group_id: Some("pair-1".to_string()),
                    capture_complete: true,
                    item_json:
                        "{\"type\":\"message\",\"role\":\"assistant\",\"content\":\"hello\"}"
                            .to_string(),
                    created_at: now,
                },
            ],
        )
        .expect("replace turn");

    storage
        .save_context_snapshot(&ContextSnapshot {
            platform_key_hash: "platform-1".to_string(),
            affinity_key: "sid:test-1".to_string(),
            conversation_scope_id: "scope-1".to_string(),
            upto_turn_index: 0,
            summary_text: "summary".to_string(),
            created_at: now,
            updated_at: now,
        })
        .expect("save snapshot");

    let loaded_binding = storage
        .get_client_binding("platform-1", "sid:test-1")
        .expect("get binding")
        .expect("binding exists");
    assert_eq!(loaded_binding.account_id, "acc-1");
    assert_eq!(loaded_binding.primary_scope_id.as_deref(), Some("scope-1"));

    let loaded_thread = storage
        .get_conversation_thread("platform-1", "sid:test-1", "scope-1")
        .expect("get thread")
        .expect("thread exists");
    assert_eq!(loaded_thread.thread_anchor, "resp_123");

    let loaded_state = storage
        .get_conversation_context_state("platform-1", "sid:test-1", "scope-1")
        .expect("get context state")
        .expect("context state exists");
    assert_eq!(loaded_state.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(loaded_state.protocol_type.as_deref(), Some("openai_compat"));

    let loaded_events = storage
        .list_conversation_context_events("platform-1", "sid:test-1", "scope-1")
        .expect("list context events");
    assert_eq!(loaded_events.len(), 2);
    assert_eq!(loaded_events[0].role.as_deref(), Some("user"));
    assert_eq!(loaded_events[1].pair_group_id.as_deref(), Some("pair-1"));

    let loaded_snapshots = storage
        .list_context_snapshots("platform-1", "sid:test-1", "scope-1")
        .expect("list context snapshots");
    assert_eq!(loaded_snapshots.len(), 1);
    assert_eq!(loaded_snapshots[0].summary_text, "summary");

    storage
        .delete_affinity_state_for_account("acc-1")
        .expect("delete affinity state");

    assert!(
        storage
            .get_client_binding("platform-1", "sid:test-1")
            .expect("binding after delete")
            .is_none()
    );
    assert!(
        storage
            .get_conversation_thread("platform-1", "sid:test-1", "scope-1")
            .expect("thread after delete")
            .is_none()
    );
    assert!(
        storage
            .get_conversation_context_state("platform-1", "sid:test-1", "scope-1")
            .expect("state after delete")
            .is_none()
    );
    assert!(
        storage
            .list_conversation_context_events("platform-1", "sid:test-1", "scope-1")
            .expect("events after delete")
            .is_empty()
    );
    assert!(
        storage
            .list_context_snapshots("platform-1", "sid:test-1", "scope-1")
            .expect("snapshots after delete")
            .is_empty()
    );
}

#[test]
fn commit_affinity_turn_success_rolls_back_on_cas_miss() {
    let storage = Storage::open_in_memory().expect("open in memory");
    storage.init().expect("init schema");
    let now = now_ts();

    storage
        .save_client_binding(
            &ClientBinding {
                platform_key_hash: "platform-1".to_string(),
                affinity_key: "sid:test-1".to_string(),
                account_id: "acc-1".to_string(),
                primary_scope_id: Some("affinity::synthetic".to_string()),
                binding_version: 1,
                status: "active".to_string(),
                last_supply_score: Some(0.7),
                last_pressure_score: Some(0.8),
                last_final_score: Some(0.6),
                last_switch_reason: Some("initial_bind".to_string()),
                created_at: now,
                updated_at: now,
                last_seen_at: now,
            },
            None,
        )
        .expect("insert binding");
    storage
        .save_conversation_thread(
            &ConversationThread {
                platform_key_hash: "platform-1".to_string(),
                affinity_key: "sid:test-1".to_string(),
                conversation_scope_id: "affinity::synthetic".to_string(),
                account_id: "acc-1".to_string(),
                thread_epoch: 1,
                thread_anchor: "thread-synth".to_string(),
                thread_version: 1,
                created_at: now,
                updated_at: now,
                last_seen_at: now,
            },
            None,
        )
        .expect("insert thread");
    storage
        .save_conversation_context_state(&ConversationContextState {
            platform_key_hash: "platform-1".to_string(),
            affinity_key: "sid:test-1".to_string(),
            conversation_scope_id: "affinity::synthetic".to_string(),
            model: Some("gpt-5.4".to_string()),
            instructions_text: Some("old".to_string()),
            tools_json: None,
            tool_choice_json: None,
            parallel_tool_calls: Some(false),
            reasoning_json: None,
            text_format_json: None,
            service_tier: None,
            metadata_json: None,
            encrypted_content: None,
            protocol_type: Some("openai_compat".to_string()),
            response_adapter: Some("Passthrough".to_string()),
            updated_at: now,
        })
        .expect("insert state");

    let committed = storage
        .commit_affinity_turn_success(
            &ClientBinding {
                platform_key_hash: "platform-1".to_string(),
                affinity_key: "sid:test-1".to_string(),
                account_id: "acc-2".to_string(),
                primary_scope_id: Some("scope-real".to_string()),
                binding_version: 2,
                status: "active".to_string(),
                last_supply_score: Some(0.9),
                last_pressure_score: Some(0.5),
                last_final_score: Some(0.8),
                last_switch_reason: Some("affinity_rebind".to_string()),
                created_at: now,
                updated_at: now + 1,
                last_seen_at: now + 1,
            },
            Some(9),
            &ConversationThread {
                platform_key_hash: "platform-1".to_string(),
                affinity_key: "sid:test-1".to_string(),
                conversation_scope_id: "scope-real".to_string(),
                account_id: "acc-2".to_string(),
                thread_epoch: 2,
                thread_anchor: "thread-real".to_string(),
                thread_version: 2,
                created_at: now,
                updated_at: now + 1,
                last_seen_at: now + 1,
            },
            Some(1),
            Some(&AffinityScopePromotion {
                platform_key_hash: "platform-1".to_string(),
                affinity_key: "sid:test-1".to_string(),
                from_scope_id: "affinity::synthetic".to_string(),
                to_scope_id: "scope-real".to_string(),
            }),
            None,
            &ConversationContextState {
                platform_key_hash: "platform-1".to_string(),
                affinity_key: "sid:test-1".to_string(),
                conversation_scope_id: "scope-real".to_string(),
                model: Some("gpt-5.4".to_string()),
                instructions_text: Some("new".to_string()),
                tools_json: None,
                tool_choice_json: None,
                parallel_tool_calls: Some(false),
                reasoning_json: None,
                text_format_json: None,
                service_tier: None,
                metadata_json: None,
                encrypted_content: None,
                protocol_type: Some("openai_compat".to_string()),
                response_adapter: Some("Passthrough".to_string()),
                updated_at: now + 1,
            },
            1,
            &[ConversationContextEvent {
                platform_key_hash: "platform-1".to_string(),
                affinity_key: "sid:test-1".to_string(),
                conversation_scope_id: "scope-real".to_string(),
                turn_index: 1,
                item_seq: 0,
                role: Some("user".to_string()),
                pair_group_id: None,
                capture_complete: true,
                item_json: "{\"type\":\"message\",\"role\":\"user\",\"content\":\"hello\"}"
                    .to_string(),
                created_at: now + 1,
            }],
        )
        .expect("commit result");
    assert_eq!(
        committed,
        AffinityTurnCommitOutcome::Conflict,
        "cas miss should abort commit"
    );

    let binding = storage
        .get_client_binding("platform-1", "sid:test-1")
        .expect("load binding")
        .expect("binding exists");
    assert_eq!(binding.account_id, "acc-1");
    assert_eq!(
        binding.primary_scope_id.as_deref(),
        Some("affinity::synthetic")
    );

    assert!(
        storage
            .get_conversation_thread("platform-1", "sid:test-1", "scope-real")
            .expect("load promoted thread")
            .is_none()
    );
    let original_thread = storage
        .get_conversation_thread("platform-1", "sid:test-1", "affinity::synthetic")
        .expect("load original thread")
        .expect("original thread exists");
    assert_eq!(original_thread.account_id, "acc-1");
    assert_eq!(original_thread.thread_anchor, "thread-synth");

    assert!(
        storage
            .get_conversation_context_state("platform-1", "sid:test-1", "scope-real")
            .expect("load promoted state")
            .is_none()
    );
    let original_state = storage
        .get_conversation_context_state("platform-1", "sid:test-1", "affinity::synthetic")
        .expect("load original state")
        .expect("original state exists");
    assert_eq!(original_state.instructions_text.as_deref(), Some("old"));
}
