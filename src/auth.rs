use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

pub const RUN_TICKET_ISSUER: &str = "agent_control_saas";
pub const RUN_TICKET_AUDIENCE: &str = "ai_gateway_saas";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub user_id: Uuid,
    pub workspace_id: Uuid,
    pub run_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub is_run_ticket: bool,
    pub is_operator: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunTicketClaims {
    pub iss: Option<String>,
    pub aud: Option<serde_json::Value>,
    pub sub: String,
    #[serde(rename = "type")]
    pub ticket_type: Option<String>,
    pub workspace_id: String,
    pub run_id: Option<String>,
    pub agent_kind: Option<String>,
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub scope: Option<String>,
    pub exp: Option<i64>,
}

#[derive(Clone)]
pub struct TokenVerifier {
    http: reqwest::Client,
    jwks_url: Option<String>,
    static_keys: HashMap<String, DecodingKey>,
    cached_keys: Arc<RwLock<HashMap<String, DecodingKey>>>,
    last_fetch: Arc<tokio::sync::Mutex<Option<Instant>>>,
    allow_unverified_dev: bool,
}

impl TokenVerifier {
    pub fn new(jwks_url: Option<String>, allow_unverified_dev: bool) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            jwks_url,
            static_keys: HashMap::new(),
            cached_keys: Arc::new(RwLock::new(HashMap::new())),
            last_fetch: Arc::new(tokio::sync::Mutex::new(None)),
            allow_unverified_dev,
        }
    }

    pub fn with_static_rsa_pem(kid: &str, pem: &str) -> Result<Self, String> {
        let key = DecodingKey::from_rsa_pem(pem.as_bytes())
            .map_err(|e| format!("invalid RSA PEM: {e}"))?;
        let mut static_keys = HashMap::new();
        static_keys.insert(kid.to_string(), key);
        Ok(Self {
            http: reqwest::Client::new(),
            jwks_url: None,
            static_keys,
            cached_keys: Arc::new(RwLock::new(HashMap::new())),
            last_fetch: Arc::new(tokio::sync::Mutex::new(None)),
            allow_unverified_dev: false,
        })
    }

    pub fn permissive_for_test() -> Self {
        Self::new(None, true)
    }

    pub async fn verify(&self, token: &str) -> Result<Caller, String> {
        if self.allow_unverified_dev {
            // Decode claims without signature verification for test/dev mode
            return decode_insecure(token);
        }

        let header = jsonwebtoken::decode_header(token)
            .map_err(|e| format!("invalid token header: {e}"))?;
        let kid = header.kid.as_deref().unwrap_or("");

        let key = if let Some(key) = self.static_keys.get(kid) {
            key.clone()
        } else {
            let cached = self.cached_keys.read().await.get(kid).cloned();
            match cached {
                Some(key) => key,
                None => {
                    self.fetch_jwks().await?;
                    self.cached_keys
                        .read()
                        .await
                        .get(kid)
                        .cloned()
                        .ok_or_else(|| format!("unknown key id: {kid}"))?
                }
            }
        };

        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_exp = true;
        validation.set_issuer(&[RUN_TICKET_ISSUER]);
        validation.validate_aud = false; // Aud may be ai_gateway_saas or agent_control_saas

        let decoded = jsonwebtoken::decode::<RunTicketClaims>(token, &key, &validation)
            .map_err(|e| format!("token verification failed: {e}"))?;

        claims_to_caller(decoded.claims)
    }

    async fn fetch_jwks(&self) -> Result<(), String> {
        let Some(jwks_url) = &self.jwks_url else {
            return Err("no JWKS URL configured for token verification".into());
        };

        let mut last = self.last_fetch.lock().await;
        if last.map(|at| at.elapsed() < Duration::from_secs(10)).unwrap_or(false) {
            return Ok(());
        }
        *last = Some(Instant::now());

        let res = self
            .http
            .get(jwks_url)
            .send()
            .await
            .map_err(|e| format!("failed to fetch JWKS: {e}"))?;
        let set: JwkSet = res
            .json()
            .await
            .map_err(|e| format!("failed to parse JWKS: {e}"))?;

        let mut write = self.cached_keys.write().await;
        for jwk in set.keys {
            if let (Some(kid), Ok(key)) = (jwk.common.key_id.clone(), DecodingKey::from_jwk(&jwk)) {
                write.insert(kid, key);
            }
        }
        Ok(())
    }
}

fn claims_to_caller(claims: RunTicketClaims) -> Result<Caller, String> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| "claim 'sub' is not a valid UUID".to_string())?;
    let workspace_id = Uuid::parse_str(&claims.workspace_id)
        .map_err(|_| "claim 'workspace_id' is not a valid UUID".to_string())?;
    let run_id = claims
        .run_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| "claim 'run_id' is not a valid UUID".to_string())?;
    let conversation_id = claims
        .conversation_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| "claim 'conversation_id' is not a valid UUID".to_string())?;

    let is_run_ticket = claims.ticket_type.as_deref() == Some("run") || run_id.is_some();

    let mut is_operator = false;
    for s in &claims.scopes {
        if s == "operator" || s == "admin" || s == "agent_control:operator" || s == "agent_control:write" {
            is_operator = true;
        }
    }
    if let Some(scope_str) = &claims.scope {
        for s in scope_str.split_whitespace() {
            if s == "operator" || s == "admin" || s == "agent_control:operator" || s == "agent_control:write" {
                is_operator = true;
            }
        }
    }
    if let Ok(owners_env) = std::env::var("VIVIOS_OWNER_SUBJECTS") {
        for owner in owners_env.split(',') {
            if let Ok(owner_uuid) = Uuid::parse_str(owner.trim()) {
                if owner_uuid == user_id {
                    is_operator = true;
                }
            }
        }
    }
    if is_run_ticket {
        is_operator = false;
    }

    Ok(Caller {
        user_id,
        workspace_id,
        run_id,
        conversation_id,
        is_run_ticket,
        is_operator,
    })
}

fn decode_insecure(token: &str) -> Result<Caller, String> {
    use base64::Engine;
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| "malformed JWT: missing payload".to_string())?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|e| format!("base64 decode error: {e}"))?;
    let claims: RunTicketClaims = serde_json::from_slice(&bytes)
        .map_err(|e| format!("json claims parse error: {e}"))?;
    claims_to_caller(claims)
}

/// Enforces that if the caller is an agent run, it can only access the conversation
/// bound in its ticket claims (mandate 1).
pub fn check_conversation_access(caller: &Caller, requested_id: Uuid) -> Result<(), (axum::http::StatusCode, &'static str)> {
    if caller.is_run_ticket {
        match caller.conversation_id {
            Some(bound_id) if bound_id == requested_id => Ok(()),
            Some(_) => Err((
                axum::http::StatusCode::FORBIDDEN,
                "ticket does not match this conversation_id",
            )),
            None => Err((
                axum::http::StatusCode::FORBIDDEN,
                "ticket is missing required conversation_id claim",
            )),
        }
    } else {
        // User/IAM tickets are bound to workspace via RLS, not restricted to a single conversation
        Ok(())
    }
}

/// Ensures a caller cannot access a conversation owned by another workspace or another user.
pub fn validate_conversation_owner(
    caller: &Caller,
    existing_owner: Option<(Uuid, Uuid)>,
) -> Result<(), (axum::http::StatusCode, &'static str)> {
    if caller.is_operator {
        return Ok(());
    }
    if let Some((owner_ws, owner_user)) = existing_owner {
        if owner_ws != caller.workspace_id {
            return Err((
                axum::http::StatusCode::FORBIDDEN,
                "conversation belongs to another workspace",
            ));
        }
        if owner_user != caller.user_id {
            return Err((
                axum::http::StatusCode::FORBIDDEN,
                "conversation belongs to another user",
            ));
        }
    }
    Ok(())
}
