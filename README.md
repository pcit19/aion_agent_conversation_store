# aion_session_store

Persistent chat session and turn storage for AION Agent with workspace isolation (RLS) and Run Ticket authorization.

## Features
- **Row-Level Security (RLS)**: Enforces workspace isolation (`ass_caller_workspace()`).
- **Ticket Claims Authorization**: Enforces `conversation_id` claim in signed Run Tickets to prevent cross-conversation tampering.
- **Dynamic Settings in DB**: `max_history_characters` and `idle_timeout_secs` stored in a single-row `settings` table.
- **History Retrieval**: Trims history starting from oldest turns to keep within soft char threshold while preserving order.

## Database Roles & Security Architecture
- `aion_session_store_owner`: Owns tables, views, and functions. Exclusively used for running migrations via `AION_SESSION_STORE_MIGRATION_DATABASE_URL` (stored in `.env.owner` with mode `0600`, gitignored).
- `aion_session_store_app`: Least-privilege runtime role, subject to RLS (`conversations_own`, `turns_own`). Configured via `AION_SESSION_STORE_DATABASE_URL` (stored in `.env.app` with mode `0600`).
- The service **does not** run migrations automatically on normal server boot. Server boot uses only `aion_session_store_app` to prevent accidental privilege escalation or ownership corruption.

## Migrations
Migrations are applied exclusively using the CLI subcommand `migrate`:
```bash
# From container environment:
set -a; . .env.owner; set +a
aion_session_store migrate

# From host (connecting via host port 25432):
AION_SESSION_STORE_MIGRATION_DATABASE_URL="$AION_SESSION_STORE_HOST_MIGRATION_DATABASE_URL" aion_session_store migrate
```

## Deployment
1. Ensure `.env.app` exists with mode `0600` containing `AION_SESSION_STORE_DATABASE_URL`.
2. Build image via `deploy/build_image.sh` (built under memory limit `run-limited --mem 3G`).
3. Rotate application password when needed via `python3 deploy/rotate_secret.py` (atomic, updates database and writes `.env.app` mode `0600`).
4. Run container via `docker compose up -d`.
