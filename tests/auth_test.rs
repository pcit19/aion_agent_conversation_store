use aion_agent_conversation_store::auth::{check_conversation_access, Caller, TokenVerifier};
use axum::http::StatusCode;
use base64::Engine;
use uuid::Uuid;

#[test]
fn test_run_ticket_access_matches_conversation() {
    let conv_id = Uuid::new_v4();
    let caller = Caller {
        user_id: Uuid::new_v4(),
        workspace_id: Uuid::new_v4(),
        run_id: Some(Uuid::new_v4()),
        conversation_id: Some(conv_id),
        is_run_ticket: true,
        is_operator: false,
    };

    // Matching conversation -> OK
    assert!(check_conversation_access(&caller, conv_id).is_ok());

    // Mismatched conversation -> 403 Forbidden
    let other_id = Uuid::new_v4();
    let err = check_conversation_access(&caller, other_id).unwrap_err();
    assert_eq!(err.0, StatusCode::FORBIDDEN);
    assert_eq!(err.1, "ticket does not match this conversation_id");
}

#[test]
fn test_run_ticket_missing_conversation_id_rejected() {
    let caller = Caller {
        user_id: Uuid::new_v4(),
        workspace_id: Uuid::new_v4(),
        run_id: Some(Uuid::new_v4()),
        conversation_id: None,
        is_run_ticket: true,
        is_operator: false,
    };

    let err = check_conversation_access(&caller, Uuid::new_v4()).unwrap_err();
    assert_eq!(err.0, StatusCode::FORBIDDEN);
    assert_eq!(err.1, "ticket is missing required conversation_id claim");
}

#[test]
fn test_user_ticket_not_restricted_to_single_conversation() {
    let caller = Caller {
        user_id: Uuid::new_v4(),
        workspace_id: Uuid::new_v4(),
        run_id: None,
        conversation_id: None,
        is_run_ticket: false,
        is_operator: false,
    };

    // Any conversation in workspace is allowed for user ticket
    assert!(check_conversation_access(&caller, Uuid::new_v4()).is_ok());
}

#[tokio::test]
async fn test_permissive_verifier_decodes_claims() {
    use base64::Engine;
    let verifier = TokenVerifier::permissive_for_test();

    let user_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    let conv_id = Uuid::new_v4();

    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::json!({
        "sub": user_id.to_string(),
        "workspace_id": workspace_id.to_string(),
        "run_id": run_id.to_string(),
        "conversation_id": conv_id.to_string(),
        "type": "run"
    }).to_string());

    let token = format!("{header}.{payload}.sig");
    let caller = verifier.verify(&token).await.unwrap();

    assert_eq!(caller.user_id, user_id);
    assert_eq!(caller.workspace_id, workspace_id);
    assert_eq!(caller.run_id, Some(run_id));
    assert_eq!(caller.conversation_id, Some(conv_id));
    assert!(caller.is_run_ticket);
    assert!(!caller.is_operator);
}

#[tokio::test]
async fn test_operator_claims_detection() {
    use base64::Engine;
    let verifier = TokenVerifier::permissive_for_test();

    let user_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();

    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);

    // 1. Regular user token without operator scopes
    let payload1 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::json!({
        "sub": user_id.to_string(),
        "workspace_id": workspace_id.to_string(),
        "type": "user",
        "scopes": ["read", "write"]
    }).to_string());
    let caller1 = verifier.verify(&format!("{header}.{payload1}.sig")).await.unwrap();
    assert!(!caller1.is_operator);

    // 2. User token with operator scope
    let payload2 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::json!({
        "sub": user_id.to_string(),
        "workspace_id": workspace_id.to_string(),
        "type": "user",
        "scopes": ["operator"]
    }).to_string());
    let caller2 = verifier.verify(&format!("{header}.{payload2}.sig")).await.unwrap();
    assert!(caller2.is_operator);

    // 3. Run ticket with operator scope is NEVER operator
    let payload3 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::json!({
        "sub": user_id.to_string(),
        "workspace_id": workspace_id.to_string(),
        "run_id": Uuid::new_v4().to_string(),
        "conversation_id": Uuid::new_v4().to_string(),
        "type": "run",
        "scopes": ["operator"]
    }).to_string());
    let caller3 = verifier.verify(&format!("{header}.{payload3}.sig")).await.unwrap();
    assert!(!caller3.is_operator);
}

#[test]
fn test_conversation_ownership_cross_workspace_and_cross_user_rejected() {
    use aion_agent_conversation_store::auth::validate_conversation_owner;

    let ws1 = Uuid::new_v4();
    let ws2 = Uuid::new_v4();
    let user1 = Uuid::new_v4();
    let user2 = Uuid::new_v4();

    let caller1 = Caller {
        user_id: user1,
        workspace_id: ws1,
        run_id: None,
        conversation_id: None,
        is_run_ticket: false,
        is_operator: false,
    };

    // 1. Matching owner -> OK
    assert!(validate_conversation_owner(&caller1, Some((ws1, user1))).is_ok());

    // 2. Cross workspace -> 403 Forbidden
    let err_ws = validate_conversation_owner(&caller1, Some((ws2, user1))).unwrap_err();
    assert_eq!(err_ws.0, StatusCode::FORBIDDEN);
    assert_eq!(err_ws.1, "conversation belongs to another workspace");

    // 3. Same workspace but cross user -> 403 Forbidden
    let err_user = validate_conversation_owner(&caller1, Some((ws1, user2))).unwrap_err();
    assert_eq!(err_user.0, StatusCode::FORBIDDEN);
    assert_eq!(err_user.1, "conversation belongs to another user");

    // 4. Operator bypasses ownership checks
    let operator_caller = Caller {
        user_id: user1,
        workspace_id: ws1,
        run_id: None,
        conversation_id: None,
        is_run_ticket: false,
        is_operator: true,
    };
    assert!(validate_conversation_owner(&operator_caller, Some((ws2, user2))).is_ok());
}

#[tokio::test]
async fn test_owner_subjects_dual_read() {
    let verifier = TokenVerifier::new(None, true);
    let user_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::json!({
        "sub": user_id.to_string(),
        "workspace_id": workspace_id.to_string(),
        "type": "user"
    }).to_string());
    let token = format!("{header}.{payload}.sig");

    // Case 1: VIVIOS_OWNER_SUBJECTS set
    std::env::remove_var("AION_OWNER_SUBJECTS");
    std::env::set_var("VIVIOS_OWNER_SUBJECTS", format!("{},{}", Uuid::new_v4(), user_id));
    let caller = verifier.verify(&token).await.unwrap();
    assert!(caller.is_operator);

    // Case 2: AION_OWNER_SUBJECTS set (takes precedence)
    std::env::set_var("AION_OWNER_SUBJECTS", user_id.to_string());
    std::env::set_var("VIVIOS_OWNER_SUBJECTS", Uuid::new_v4().to_string());
    let caller2 = verifier.verify(&token).await.unwrap();
    assert!(caller2.is_operator);

    // Clean up
    std::env::remove_var("AION_OWNER_SUBJECTS");
    std::env::remove_var("VIVIOS_OWNER_SUBJECTS");
}
