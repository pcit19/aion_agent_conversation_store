use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use tower::ServiceExt;
use uuid::Uuid;

use aion_agent_conversation_store::api::{app_router, AppState};
use aion_agent_conversation_store::auth::TokenVerifier;
use aion_agent_conversation_store::db::StoreDb;

fn get_test_db_url() -> Option<String> {
    match std::env::var("TEST_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => None,
    }
}

fn make_run_token(user_id: Uuid, workspace_id: Uuid, run_id: Uuid, conv_id: Uuid) -> String {
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::json!({
            "sub": user_id.to_string(),
            "workspace_id": workspace_id.to_string(),
            "run_id": run_id.to_string(),
            "conversation_id": conv_id.to_string(),
            "type": "run"
        })
        .to_string(),
    );
    format!("{header}.{payload}.sig")
}

#[tokio::test]
async fn test_db_real_store_user_a_cannot_access_user_b_conversation() {
    let Some(db_url) = get_test_db_url() else {
        println!("Skipping integration test: TEST_DATABASE_URL is not set (will not run against production database)");
        return;
    };

    let db = match StoreDb::connect_lazy(&db_url, 5) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("Skipping test: cannot connect to db: {e}");
            return;
        }
    };

    if let Err(e) = db.ping().await {
        eprintln!("Skipping test: ping db failed: {e}");
        return;
    }

    let verifier = TokenVerifier::permissive_for_test();
    let state = AppState {
        db: db.clone(),
        verifier,
    };
    let app = app_router(state);

    // Identity for User B (the legitimate owner)
    let ws_b = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    let conv_b = Uuid::new_v4();

    // Identity for User A (the attacker / separate tenant)
    let ws_a = Uuid::new_v4();
    let user_a = Uuid::new_v4();
    let run_a = Uuid::new_v4();
    let conv_a = Uuid::new_v4();

    // 1. Insert conversation owned by User B under Workspace B
    {
        let mut tx = db
            .for_workspace(ws_b)
            .await
            .expect("begin tx for ws_b");
        sqlx::query("INSERT INTO conversations (id, workspace_id, user_id, title) VALUES ($1, $2, $3, $4)")
            .bind(conv_b)
            .bind(ws_b)
            .bind(user_b)
            .bind("User B Confidential Conversation")
            .execute(&mut *tx)
            .await
            .expect("insert conv_b");
        tx.commit().await.expect("commit conv_b insert");
    }

    // SCENARIO 1: User A calls with their own run ticket (claiming conv_a) against conv_b
    // Fails at check_conversation_access (ticket mismatch)
    {
        let token_a = make_run_token(user_a, ws_a, run_a, conv_a);
        let req = Request::builder()
            .method("GET")
            .uri(format!("/v1/conversations/{conv_b}/history"))
            .header("Authorization", format!("Bearer {token_a}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "User A's run ticket for conv_a must be 403 Forbidden when accessing conv_b"
        );
    }

    // SCENARIO 2: User A tampers/crafts a run ticket claiming conv_b, but sub=user_a, ws=ws_a
    // check_conversation_access passes, but check_conversation_owner queries ass_conversation_owner
    // and rejects due to cross-workspace mismatch!
    {
        let forged_token = make_run_token(user_a, ws_a, run_a, conv_b);
        let req = Request::builder()
            .method("GET")
            .uri(format!("/v1/conversations/{conv_b}/history"))
            .header("Authorization", format!("Bearer {forged_token}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "User A cross-workspace forged run ticket must be 403 Forbidden via ass_conversation_owner"
        );
    }

    // SCENARIO 3: User A is in same workspace ws_b, but sub=user_a (cross-user in same workspace)
    // check_conversation_owner queries ass_conversation_owner and rejects due to cross-user mismatch!
    {
        let cross_user_token = make_run_token(user_a, ws_b, run_a, conv_b);
        let req = Request::builder()
            .method("GET")
            .uri(format!("/v1/conversations/{conv_b}/history"))
            .header("Authorization", format!("Bearer {cross_user_token}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "User A cross-user in same workspace must be 403 Forbidden via ass_conversation_owner"
        );
    }

    // SCENARIO 4: User A attempts to append turn to User B's conversation
    {
        let forged_token = make_run_token(user_a, ws_a, run_a, conv_b);
        let req = Request::builder()
            .method("POST")
            .uri(format!("/v1/conversations/{conv_b}/turns"))
            .header("Authorization", format!("Bearer {forged_token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"role":"user","content":"unauthorized turn"}"#))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "User A appending turn to User B conversation must be 403 Forbidden"
        );
    }

    // SCENARIO 5: Legitimate owner User B accesses conv_b -> 200 OK
    {
        let run_b = Uuid::new_v4();
        let token_b = make_run_token(user_b, ws_b, run_b, conv_b);
        let req = Request::builder()
            .method("GET")
            .uri(format!("/v1/conversations/{conv_b}/history"))
            .header("Authorization", format!("Bearer {token_b}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "User B with legitimate run ticket must be 200 OK"
        );
    }

    // Clean up test conversation
    {
        let mut tx = db.for_workspace(ws_b).await.expect("begin tx cleanup");
        let _ = sqlx::query("DELETE FROM conversations WHERE id = $1")
            .bind(conv_b)
            .execute(&mut *tx)
            .await;
        let _ = tx.commit().await;
    }
}
