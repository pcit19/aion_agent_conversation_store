-- Global conversation ownership lookup bypassing workspace RLS
CREATE OR REPLACE FUNCTION ass_conversation_owner(conv UUID)
    RETURNS TABLE (workspace_id UUID, user_id UUID)
    LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog, public
    AS $$ SELECT workspace_id, user_id FROM conversations WHERE id = conv LIMIT 1 $$;

GRANT EXECUTE ON FUNCTION ass_conversation_owner(UUID) TO aion_session_store_app;
