#!/usr/bin/env bash
# Migration script: aion_session_store -> aion_agent_conversation_store
# Standard: SAO LƯU & PHỤC HỒI / ADR-020
#
# DO NOT RUN WITHOUT AUTHORIZATION. Run by Opus on cutover.
# This script:
# 1. Checks PostgreSQL container and source database.
# 2. Generates cryptographic passwords for new roles via Python (no CLI leaking).
# 3. Creates roles aion_agent_conversation_store_owner and aion_agent_conversation_store_app.
# 4. Creates database aion_agent_conversation_store owned by aion_agent_conversation_store_owner.
# 5. Writes secrets to .env.app and .env.owner (mode 0600) atomically.
# 6. Safety check: verifies target database is clean before dumping (safe re-run guard).
# 7. Stops old container aion_session_store to prevent in-flight writes.
# 8. Dumps data from aion_session_store and restores into aion_agent_conversation_store.
# 9. Reassigns ownership of tables/sequences/functions via pg_proc + regprocedure (NO REASSIGN OWNED).
# 10. Grants runtime privileges to aion_agent_conversation_store_app.
# 11. Verifies row counts against source database (conversations, turns, settings).
# 12. Verifies runtime connection with aion_agent_conversation_store_app.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
CONTAINER="aion_master_postgres_team"
OLD_CONTAINER="aion_session_store"
OLD_DB="aion_session_store"
NEW_DB="aion_agent_conversation_store"
OWNER_ROLE="aion_agent_conversation_store_owner"
APP_ROLE="aion_agent_conversation_store_app"

echo "=== [1/9] Checking PostgreSQL container and source database ==="
if ! docker ps --format '{{.Names}}' | grep -q "^${CONTAINER}\$"; then
    echo "ERROR: Container ${CONTAINER} is not running." >&2
    exit 1
fi
echo "Container ${CONTAINER} is running."

OLD_DB_EXISTS=$(docker exec -i "${CONTAINER}" psql -U root -d postgres -t -A -c "SELECT 1 FROM pg_database WHERE datname='${OLD_DB}';")
if [ "${OLD_DB_EXISTS}" != "1" ]; then
    echo "ERROR: Source database ${OLD_DB} does not exist!" >&2
    exit 1
fi

echo "=== [2/9] Generating secure credentials ==="
# Generates passwords in memory without printing to terminal or logging
read -r OWNER_PASS APP_PASS < <(python3 -c "
import secrets
print(f'{secrets.token_hex(24)} {secrets.token_hex(24)}')
")

echo "=== [3/9] Creating roles and target database ==="
# Execute SQL via stdin to prevent password exposure in process table
docker exec -i "${CONTAINER}" psql -U root -d postgres <<EOF
DO \$\$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '${OWNER_ROLE}') THEN
        CREATE ROLE ${OWNER_ROLE} WITH LOGIN PASSWORD '${OWNER_PASS}';
    ELSE
        ALTER ROLE ${OWNER_ROLE} WITH PASSWORD '${OWNER_PASS}';
    END IF;

    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '${APP_ROLE}') THEN
        CREATE ROLE ${APP_ROLE} WITH LOGIN PASSWORD '${APP_PASS}';
    ELSE
        ALTER ROLE ${APP_ROLE} WITH PASSWORD '${APP_PASS}';
    END IF;
END
\$\$;

SELECT 'CREATE DATABASE ${NEW_DB} OWNER ${OWNER_ROLE}'
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = '${NEW_DB}')\gexec
EOF

echo "=== [4/9] Writing secret files (.env.app and .env.owner) mode 0600 ==="
python3 - <<EOF
import os, urllib.parse

repo_dir = "${REPO_DIR}"
owner_pass = "${OWNER_PASS}"
app_pass = "${APP_PASS}"

owner_escaped = urllib.parse.quote(owner_pass)
app_escaped = urllib.parse.quote(app_pass)

# .env.app
app_content = (
    f"AION_AGENT_CONVERSATION_STORE_DATABASE_URL=postgres://${APP_ROLE}:{app_escaped}@${CONTAINER}:5432/${NEW_DB}\n"
    f"AION_SESSION_STORE_DATABASE_URL=postgres://${APP_ROLE}:{app_escaped}@${CONTAINER}:5432/${NEW_DB}\n"
)
app_path = os.path.join(repo_dir, ".env.app")
fd_app = os.open(app_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
with os.fdopen(fd_app, "w") as f:
    f.write(app_content)

# .env.owner
owner_content = (
    f"AION_AGENT_CONVERSATION_STORE_MIGRATION_DATABASE_URL=postgres://${OWNER_ROLE}:{owner_escaped}@${CONTAINER}:5432/${NEW_DB}\n"
    f"AION_AGENT_CONVERSATION_STORE_HOST_MIGRATION_DATABASE_URL=postgres://${OWNER_ROLE}:{owner_escaped}@127.0.0.1:25432/${NEW_DB}\n"
    f"AION_SESSION_STORE_MIGRATION_DATABASE_URL=postgres://${OWNER_ROLE}:{owner_escaped}@${CONTAINER}:5432/${NEW_DB}\n"
    f"AION_SESSION_STORE_HOST_MIGRATION_DATABASE_URL=postgres://${OWNER_ROLE}:{owner_escaped}@127.0.0.1:25432/${NEW_DB}\n"
)
owner_path = os.path.join(repo_dir, ".env.owner")
fd_owner = os.open(owner_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
with os.fdopen(fd_owner, "w") as f:
    f.write(owner_content)
EOF
chmod 0600 "${REPO_DIR}/.env.app" "${REPO_DIR}/.env.owner"
echo "Secret files written with mode 0600."

echo "=== [5/9] Safety check: verifying ${NEW_DB} has no pre-existing tables ==="
EXISTING_TABLES=$(docker exec -i "${CONTAINER}" psql -U root -d "${NEW_DB}" -t -A -c "
    SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public' AND table_type = 'BASE TABLE';
")
if [ "${EXISTING_TABLES}" -gt 0 ]; then
    echo "ERROR: Database ${NEW_DB} already contains ${EXISTING_TABLES} table(s)!" >&2
    echo "Refusing to restore on top of an existing schema." >&2
    echo "If re-running after an aborted run, drop and recreate ${NEW_DB} manually before running this script." >&2
    exit 1
fi
echo "Verified ${NEW_DB} is clean (0 tables)."

echo "=== [6/9] Stopping old container ${OLD_CONTAINER} before dump ==="
if docker ps --format '{{.Names}}' | grep -q "^${OLD_CONTAINER}\$"; then
    echo "Stopping container ${OLD_CONTAINER} to ensure zero write divergence..."
    docker stop "${OLD_CONTAINER}"
else
    echo "Container ${OLD_CONTAINER} is not running."
fi

echo "=== [7/9] Dumping ${OLD_DB} and restoring into ${NEW_DB} ==="
# Stream dump directly into new database
docker exec -i "${CONTAINER}" pg_dump -U root --no-owner --no-privileges -d "${OLD_DB}" | \
    docker exec -i "${CONTAINER}" psql -U root -d "${NEW_DB}"

echo "=== [8/9] Reassigning ownership & granting privileges ==="
docker exec -i "${CONTAINER}" psql -U root -d "${NEW_DB}" <<EOF
ALTER DATABASE ${NEW_DB} OWNER TO ${OWNER_ROLE};
ALTER SCHEMA public OWNER TO ${OWNER_ROLE};

DO \$\$
DECLARE
    r RECORD;
BEGIN
    -- Reassign table ownership
    FOR r IN (SELECT tablename FROM pg_tables WHERE schemaname = 'public') LOOP
        EXECUTE 'ALTER TABLE public.' || quote_ident(r.tablename) || ' OWNER TO ${OWNER_ROLE};';
    END LOOP;

    -- Reassign sequence ownership
    FOR r IN (SELECT sequence_name FROM information_schema.sequences WHERE sequence_schema = 'public') LOOP
        EXECUTE 'ALTER SEQUENCE public.' || quote_ident(r.sequence_name) || ' OWNER TO ${OWNER_ROLE};';
    END LOOP;

    -- Reassign function ownership with full signature using pg_proc + regprocedure
    FOR r IN (
        SELECT p.oid::regprocedure AS proc_sig
        FROM pg_proc p
        JOIN pg_namespace n ON p.pronamespace = n.oid
        WHERE n.nspname = 'public'
    ) LOOP
        EXECUTE 'ALTER FUNCTION ' || r.proc_sig || ' OWNER TO ${OWNER_ROLE};';
    END LOOP;
END
\$\$;

-- Grant runtime privileges to app role
GRANT USAGE ON SCHEMA public TO ${APP_ROLE};
GRANT SELECT, UPDATE ON settings TO ${APP_ROLE};
GRANT SELECT, INSERT, UPDATE, DELETE ON conversations, turns TO ${APP_ROLE};
GRANT USAGE, SELECT ON SEQUENCE turns_id_seq TO ${APP_ROLE};
GRANT EXECUTE ON FUNCTION ass_conversation_owner(UUID) TO ${APP_ROLE};
EOF

echo "=== [9/9] Verifying row counts & app role connectivity ==="
OLD_CONV=$(docker exec -i "${CONTAINER}" psql -U root -d "${OLD_DB}" -t -A -c "SELECT count(*) FROM conversations;")
NEW_CONV=$(docker exec -i "${CONTAINER}" psql -U root -d "${NEW_DB}" -t -A -c "SELECT count(*) FROM conversations;")

OLD_TURNS=$(docker exec -i "${CONTAINER}" psql -U root -d "${OLD_DB}" -t -A -c "SELECT count(*) FROM turns;")
NEW_TURNS=$(docker exec -i "${CONTAINER}" psql -U root -d "${NEW_DB}" -t -A -c "SELECT count(*) FROM turns;")

OLD_SETT=$(docker exec -i "${CONTAINER}" psql -U root -d "${OLD_DB}" -t -A -c "SELECT count(*) FROM settings;")
NEW_SETT=$(docker exec -i "${CONTAINER}" psql -U root -d "${NEW_DB}" -t -A -c "SELECT count(*) FROM settings;")

echo "Source (${OLD_DB}): conversations=${OLD_CONV}, turns=${OLD_TURNS}, settings=${OLD_SETT}"
echo "Target (${NEW_DB}): conversations=${NEW_CONV}, turns=${NEW_TURNS}, settings=${NEW_SETT}"

if [ "${OLD_CONV}" != "${NEW_CONV}" ] || [ "${OLD_TURNS}" != "${NEW_TURNS}" ] || [ "${OLD_SETT}" != "${NEW_SETT}" ]; then
    echo "ERROR: Row count mismatch! Migration verification failed." >&2
    exit 1
fi
echo "[OK] Row counts match perfectly."

# Test least-privilege connection
APP_CHECK=$(docker exec -i -e PGPASSWORD="${APP_PASS}" "${CONTAINER}" psql -U "${APP_ROLE}" -d "${NEW_DB}" -t -A -c "SELECT count(*) FROM settings;")
if [ "${APP_CHECK}" != "1" ]; then
    echo "ERROR: App role connection test failed!" >&2
    exit 1
fi
echo "[OK] App role connection and least-privilege query succeeded."

echo "=========================================================="
echo "Migration to ${NEW_DB} completed successfully!"
echo "Target database is ready for cutover."
echo "=========================================================="
