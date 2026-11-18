# aion_session_store

Persistent chat session and turn storage for AION Agent with workspace isolation (RLS) and Run Ticket authorization.

## Features
- **Row-Level Security (RLS)**: Enforces workspace isolation (`ass_caller_workspace()`).
- **Ticket Claims Authorization**: Enforces `conversation_id` claim in signed Run Tickets to prevent cross-conversation tampering.
- **Dynamic Settings in DB**: `MAX_HISTORY_CHARACTERS` and `idle_timeout_secs` stored in a single-row `settings` table.
- **History Retrieval**: Trims history starting from oldest turns to keep within soft char threshold while preserving order.
