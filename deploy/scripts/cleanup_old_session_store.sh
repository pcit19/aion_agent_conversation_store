#!/usr/bin/env bash
# Cleanup script: drop old database aion_session_store and old roles
#
# DO NOT RUN WITHOUT AUTHORIZATION. Run by Opus ONLY after full cutover verification.
set -euo pipefail

CONTAINER="aion_master_postgres_team"
OLD_DB="aion_session_store"
NEW_DB="aion_agent_conversation_store"
OLD_APP_ROLE="aion_session_store_app"
OLD_OWNER_ROLE="aion_session_store_owner"

echo "=== [1/4] Safety check: verifying new database ${NEW_DB} ==="
NEW_EXISTS=$(docker exec -i "${CONTAINER}" psql -U root -d postgres -t -A -c "SELECT 1 FROM pg_database WHERE datname='${NEW_DB}';")
if [ "${NEW_EXISTS}" != "1" ]; then
    echo "ERROR: New database ${NEW_DB} does not exist! Refusing to cleanup old database." >&2
    exit 1
fi

NEW_CONV=$(docker exec -i "${CONTAINER}" psql -U root -d "${NEW_DB}" -t -A -c "SELECT count(*) FROM conversations;")
echo "Confirmed new database ${NEW_DB} has ${NEW_CONV} conversations."

echo "=== [2/4] Terminating connections to ${OLD_DB} ==="
docker exec -i "${CONTAINER}" psql -U root -d postgres <<EOF
SELECT pg_terminate_backend(pid)
FROM pg_stat_activity
WHERE datname = '${OLD_DB}' AND pid <> pg_backend_pid();
EOF

echo "=== [3/4] Dropping old database ${OLD_DB} ==="
docker exec -i "${CONTAINER}" psql -U root -d postgres -c "DROP DATABASE IF EXISTS ${OLD_DB};"

echo "=== [4/4] Dropping old roles ==="
docker exec -i "${CONTAINER}" psql -U root -d postgres <<EOF
DROP ROLE IF EXISTS ${OLD_APP_ROLE};
DROP ROLE IF EXISTS ${OLD_OWNER_ROLE};
EOF

echo "=========================================================="
echo "Cleanup of ${OLD_DB} and old roles completed."
echo "=========================================================="
