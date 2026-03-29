CREATE TABLE IF NOT EXISTS client_bindings (
    platform_key_hash TEXT NOT NULL,
    affinity_key TEXT NOT NULL,
    account_id TEXT NOT NULL,
    primary_scope_id TEXT,
    binding_version INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL DEFAULT 'active',
    last_supply_score REAL,
    last_pressure_score REAL,
    last_final_score REAL,
    last_switch_reason TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    PRIMARY KEY (platform_key_hash, affinity_key)
);

CREATE INDEX IF NOT EXISTS idx_client_bindings_account_id
ON client_bindings (account_id, last_seen_at DESC);

CREATE INDEX IF NOT EXISTS idx_client_bindings_last_seen_at
ON client_bindings (last_seen_at DESC);

CREATE TABLE IF NOT EXISTS conversation_threads (
    platform_key_hash TEXT NOT NULL,
    affinity_key TEXT NOT NULL,
    conversation_scope_id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    thread_epoch INTEGER NOT NULL,
    thread_anchor TEXT NOT NULL,
    thread_version INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    PRIMARY KEY (platform_key_hash, affinity_key, conversation_scope_id)
);

CREATE INDEX IF NOT EXISTS idx_conversation_threads_account_id
ON conversation_threads (account_id, last_seen_at DESC);

CREATE INDEX IF NOT EXISTS idx_conversation_threads_last_seen_at
ON conversation_threads (last_seen_at DESC);

CREATE TABLE IF NOT EXISTS conversation_context_state (
    platform_key_hash TEXT NOT NULL,
    affinity_key TEXT NOT NULL,
    conversation_scope_id TEXT NOT NULL,
    model TEXT,
    instructions_text TEXT,
    tools_json TEXT,
    tool_choice_json TEXT,
    parallel_tool_calls INTEGER,
    reasoning_json TEXT,
    text_format_json TEXT,
    service_tier TEXT,
    metadata_json TEXT,
    encrypted_content TEXT,
    protocol_type TEXT,
    response_adapter TEXT,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (platform_key_hash, affinity_key, conversation_scope_id)
);

CREATE INDEX IF NOT EXISTS idx_conversation_context_state_updated_at
ON conversation_context_state (updated_at DESC);

CREATE TABLE IF NOT EXISTS conversation_context_events (
    platform_key_hash TEXT NOT NULL,
    affinity_key TEXT NOT NULL,
    conversation_scope_id TEXT NOT NULL,
    turn_index INTEGER NOT NULL,
    item_seq INTEGER NOT NULL,
    role TEXT,
    pair_group_id TEXT,
    capture_complete INTEGER NOT NULL DEFAULT 1,
    item_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (
        platform_key_hash,
        affinity_key,
        conversation_scope_id,
        turn_index,
        item_seq
    )
);

CREATE INDEX IF NOT EXISTS idx_conversation_context_events_scope_turn
ON conversation_context_events (
    platform_key_hash,
    affinity_key,
    conversation_scope_id,
    turn_index DESC,
    item_seq ASC
);

CREATE INDEX IF NOT EXISTS idx_conversation_context_events_created_at
ON conversation_context_events (created_at DESC);

CREATE TABLE IF NOT EXISTS context_snapshots (
    platform_key_hash TEXT NOT NULL,
    affinity_key TEXT NOT NULL,
    conversation_scope_id TEXT NOT NULL,
    upto_turn_index INTEGER NOT NULL,
    summary_text TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (platform_key_hash, affinity_key, conversation_scope_id, upto_turn_index)
);

CREATE INDEX IF NOT EXISTS idx_context_snapshots_scope_turn
ON context_snapshots (
    platform_key_hash,
    affinity_key,
    conversation_scope_id,
    upto_turn_index DESC
);
