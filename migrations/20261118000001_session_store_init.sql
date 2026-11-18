-- aion_session_store: conversations and turns with workspace isolation (RLS)
-- and dynamic configuration settings.

DO $$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'aion_session_store_app') THEN
        CREATE ROLE aion_session_store_app;
    END IF;
END
$$;

CREATE OR REPLACE FUNCTION ass_caller_workspace() RETURNS UUID
    LANGUAGE sql STABLE AS $$ SELECT nullif(current_setting('app.workspace_id', true), '')::uuid $$;

CREATE TABLE IF NOT EXISTS settings (
    id INT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    max_history_characters INT NOT NULL DEFAULT 60000,
    idle_timeout_secs INT NOT NULL DEFAULT 1800,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by TEXT
);

INSERT INTO settings (id, max_history_characters, idle_timeout_secs, updated_by)
VALUES (1, 60000, 1800, 'system')
ON CONFLICT (id) DO NOTHING;

CREATE TABLE IF NOT EXISTS conversations (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL,
    user_id UUID NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'closed', 'archived')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_active_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS conversations_by_workspace ON conversations (workspace_id, last_active_at DESC);

CREATE TABLE IF NOT EXISTS turns (
    id BIGSERIAL PRIMARY KEY,
    conversation_id UUID NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system')),
    content TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS turns_by_conversation ON turns (conversation_id, id ASC);

ALTER TABLE settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE conversations ENABLE ROW LEVEL SECURITY;
ALTER TABLE turns ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS settings_read ON settings;
CREATE POLICY settings_read ON settings FOR SELECT USING (true);

DROP POLICY IF EXISTS settings_write ON settings;
CREATE POLICY settings_write ON settings FOR ALL USING (true) WITH CHECK (true);

DROP POLICY IF EXISTS conversations_own ON conversations;
CREATE POLICY conversations_own ON conversations FOR ALL
    USING (workspace_id = ass_caller_workspace())
    WITH CHECK (workspace_id = ass_caller_workspace());

DROP POLICY IF EXISTS turns_own ON turns;
CREATE POLICY turns_own ON turns FOR ALL
    USING (workspace_id = ass_caller_workspace())
    WITH CHECK (workspace_id = ass_caller_workspace());

GRANT USAGE ON SCHEMA public TO aion_session_store_app;
GRANT SELECT, UPDATE ON settings TO aion_session_store_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON conversations, turns TO aion_session_store_app;
GRANT USAGE, SELECT ON SEQUENCE turns_id_seq TO aion_session_store_app;
