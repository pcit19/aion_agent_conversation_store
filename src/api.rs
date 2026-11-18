use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{check_conversation_access, Caller, TokenVerifier};
use crate::db::StoreDb;
use crate::history::{trim_turns, Turn, TurnView};

#[derive(Clone)]
pub struct AppState {
    pub db: StoreDb,
    pub verifier: TokenVerifier,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Settings {
    pub max_history_characters: i32,
    pub idle_timeout_secs: i32,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSettingsRequest {
    pub max_history_characters: Option<i32>,
    pub idle_timeout_secs: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub max_chars: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct HistoryResponse {
    pub conversation_id: Uuid,
    pub turns: Vec<TurnView>,
}

#[derive(Debug, Deserialize)]
pub struct PostTurnRequest {
    pub role: String,
    pub content: String,
}

pub fn app_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/settings", get(get_settings).put(put_settings))
        .route(
            "/v1/conversations/{id}/history",
            get(get_conversation_history),
        )
        .route("/v1/conversations/{id}/turns", post(post_turn))
        .route("/v1/conversations/{id}/close", post(close_conversation))
        .with_state(state)
}

async fn healthz(State(state): State<AppState>) -> Response {
    match state.db.ping().await {
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "ok", "db": "ok" })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "status": "error", "db": err.to_string() })),
        )
            .into_response(),
    }
}

async fn extract_caller(headers: &HeaderMap, verifier: &TokenVerifier) -> Result<Caller, (StatusCode, String)> {
    let header_val = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "missing Authorization header".to_string()))?;

    let token = header_val
        .strip_prefix("Bearer ")
        .or_else(|| header_val.strip_prefix("bearer "))
        .map(|t| t.trim())
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "malformed Authorization header: must be Bearer <token>".to_string()))?;

    verifier
        .verify(token)
        .await
        .map_err(|err| (StatusCode::UNAUTHORIZED, format!("invalid token: {err}")))
}

async fn get_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Settings>, (StatusCode, String)> {
    extract_caller(&headers, &state.verifier).await?;
    let settings = sqlx::query_as::<_, Settings>(
        "SELECT max_history_characters, idle_timeout_secs, updated_at, updated_by FROM settings WHERE id = 1",
    )
    .fetch_one(state.db.pool())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")))?;

    Ok(Json(settings))
}

async fn put_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<UpdateSettingsRequest>,
) -> Result<Json<Settings>, (StatusCode, String)> {
    let caller = extract_caller(&headers, &state.verifier).await?;
    if !caller.is_operator {
        return Err((
            StatusCode::FORBIDDEN,
            "operator authorization required to update settings".to_string(),
        ));
    }
    let updated_by = caller.user_id.to_string();

    let mut current = sqlx::query_as::<_, Settings>(
        "SELECT max_history_characters, idle_timeout_secs, updated_at, updated_by FROM settings WHERE id = 1",
    )
    .fetch_one(state.db.pool())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")))?;

    if let Some(max_chars) = body.max_history_characters {
        if !(100..=2_000_000).contains(&max_chars) {
            return Err((StatusCode::BAD_REQUEST, "max_history_characters must be between 100 and 2,000,000".into()));
        }
        current.max_history_characters = max_chars;
    }
    if let Some(idle) = body.idle_timeout_secs {
        if !(10..=86400 * 30).contains(&idle) {
            return Err((StatusCode::BAD_REQUEST, "idle_timeout_secs must be between 10 and 2,592,000".into()));
        }
        current.idle_timeout_secs = idle;
    }

    let updated = sqlx::query_as::<_, Settings>(
        "UPDATE settings SET max_history_characters = $1, idle_timeout_secs = $2, updated_at = now(), updated_by = $3 WHERE id = 1 RETURNING max_history_characters, idle_timeout_secs, updated_at, updated_by",
    )
    .bind(current.max_history_characters)
    .bind(current.idle_timeout_secs)
    .bind(updated_by)
    .fetch_one(state.db.pool())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")))?;

    Ok(Json(updated))
}

async fn get_conversation_history(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<HistoryQuery>,
    headers: HeaderMap,
) -> Result<Json<HistoryResponse>, (StatusCode, String)> {
    let caller = extract_caller(&headers, &state.verifier).await?;
    check_conversation_access(&caller, id).map_err(|(sc, msg)| (sc, msg.to_string()))?;

    // Determine max_chars limit (fallback to DB settings)
    let max_chars = match query.max_chars {
        Some(limit) => limit,
        None => {
            let row = sqlx::query_scalar::<_, i32>("SELECT max_history_characters FROM settings WHERE id = 1")
                .fetch_optional(state.db.pool())
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db error: {e}")))?;
            row.unwrap_or(60_000) as usize
        }
    };

    let mut tx = state
        .db
        .for_workspace(caller.workspace_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("tx begin error: {e}")))?;

    let raw_turns = sqlx::query_as::<_, Turn>(
        "SELECT id, conversation_id, workspace_id, role, content, created_at FROM turns WHERE conversation_id = $1 ORDER BY id ASC",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("fetch turns error: {e}")))?;

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("commit error: {e}")))?;

    let trimmed = trim_turns(raw_turns, max_chars);
    let views = trimmed.into_iter().map(TurnView::from).collect();

    Ok(Json(HistoryResponse {
        conversation_id: id,
        turns: views,
    }))
}

async fn post_turn(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<PostTurnRequest>,
) -> Result<(StatusCode, Json<TurnView>), (StatusCode, String)> {
    let caller = extract_caller(&headers, &state.verifier).await?;
    check_conversation_access(&caller, id).map_err(|(sc, msg)| (sc, msg.to_string()))?;

    let role = body.role.trim().to_lowercase();
    if role != "user" && role != "assistant" && role != "system" {
        return Err((StatusCode::BAD_REQUEST, "role must be 'user', 'assistant', or 'system'".into()));
    }
    if body.content.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "content cannot be empty".into()));
    }

    let mut tx = state
        .db
        .for_workspace(caller.workspace_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("tx begin error: {e}")))?;

    // Upsert conversation
    sqlx::query(
        "INSERT INTO conversations (id, workspace_id, user_id, title, status, last_active_at)
         VALUES ($1, $2, $3, '', 'active', now())
         ON CONFLICT (id) DO UPDATE SET last_active_at = now()",
    )
    .bind(id)
    .bind(caller.workspace_id)
    .bind(caller.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("upsert conversation error: {e}")))?;

    // Insert turn
    let turn = sqlx::query_as::<_, Turn>(
        "INSERT INTO turns (conversation_id, workspace_id, role, content, created_at)
         VALUES ($1, $2, $3, $4, now())
         RETURNING id, conversation_id, workspace_id, role, content, created_at",
    )
    .bind(id)
    .bind(caller.workspace_id)
    .bind(role)
    .bind(body.content)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("insert turn error: {e}")))?;

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("commit error: {e}")))?;

    Ok((StatusCode::CREATED, Json(TurnView::from(turn))))
}

async fn close_conversation(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let caller = extract_caller(&headers, &state.verifier).await?;
    check_conversation_access(&caller, id).map_err(|(sc, msg)| (sc, msg.to_string()))?;

    let mut tx = state
        .db
        .for_workspace(caller.workspace_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("tx begin error: {e}")))?;

    let res = sqlx::query("UPDATE conversations SET status = 'closed', last_active_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("update error: {e}")))?;

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("commit error: {e}")))?;

    if res.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, "conversation not found".into()));
    }

    Ok(Json(serde_json::json!({ "conversation_id": id, "status": "closed" })))
}
