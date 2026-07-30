use crate::{AppState, WebResult, audit, crypto};
use axum::{
    Form,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use openidconnect::core::{CoreClient, CoreProviderMetadata, CoreResponseType};
use openidconnect::{
    AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};
use openidconnect::{EndpointMaybeSet, EndpointNotSet, EndpointSet};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use subtle::ConstantTimeEq;
use tower_sessions::Session;

#[derive(Deserialize)]
pub struct Start {
    pub intent: String,
}
#[derive(Deserialize)]
pub struct Callback {
    pub code: String,
    pub state: String,
}
#[derive(Serialize, Deserialize)]
struct Pending {
    state: String,
    nonce: String,
    pkce: String,
    intent: String,
}
#[derive(Deserialize)]
struct RawClaims {
    hd: Option<String>,
}

pub async fn start(
    State(s): State<AppState>,
    session: Session,
    Query(q): Query<Start>,
) -> WebResult {
    if q.intent != "PORTAL" && q.intent != "MANAGEMENT" {
        return Err((StatusCode::BAD_REQUEST, "invalid login intent"));
    }
    let (client, _domain) = client(&s).await?;
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, state, nonce) = client
        .authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("openid".into()))
        .add_scope(Scope::new("email".into()))
        .add_scope(Scope::new("profile".into()))
        .set_pkce_challenge(challenge)
        .url();
    session
        .insert(
            "oidc",
            Pending {
                state: state.secret().into(),
                nonce: nonce.secret().into(),
                pkce: verifier.secret().into(),
                intent: q.intent,
            },
        )
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "session failure"))?;
    Ok(Redirect::to(url.as_str()).into_response())
}
pub async fn callback(
    State(s): State<AppState>,
    session: Session,
    Query(q): Query<Callback>,
) -> WebResult {
    let pending: Pending = session
        .remove("oidc")
        .await
        .map_err(|_| (StatusCode::BAD_REQUEST, "fresh login required"))?
        .ok_or((StatusCode::BAD_REQUEST, "fresh login required"))?;
    if !bool::from(pending.state.as_bytes().ct_eq(q.state.as_bytes())) {
        return Err((StatusCode::BAD_REQUEST, "fresh login required"));
    }
    let (client, domain) = client(&s).await?;
    let http = openidconnect::reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| (StatusCode::BAD_GATEWAY, "Google unavailable"))?;
    let tokens = client
        .exchange_code(AuthorizationCode::new(q.code))
        .map_err(|_| (StatusCode::BAD_REQUEST, "fresh login required"))?
        .set_pkce_verifier(PkceCodeVerifier::new(pending.pkce))
        .request_async(&http)
        .await
        .map_err(|_| (StatusCode::BAD_GATEWAY, "Google unavailable"))?;
    let id = tokens
        .id_token()
        .ok_or((StatusCode::UNAUTHORIZED, "Google identity token missing"))?;
    let claims = id
        .claims(&client.id_token_verifier(), &Nonce::new(pending.nonce))
        .map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                "Google identity validation failed",
            )
        })?;
    let email = claims
        .email()
        .filter(|_| claims.email_verified() == Some(true))
        .map(|e| e.as_str().to_lowercase())
        .ok_or((StatusCode::UNAUTHORIZED, "verified email required"))?;
    let payload = id
        .to_string()
        .split('.')
        .nth(1)
        .and_then(|p| URL_SAFE_NO_PAD.decode(p).ok())
        .and_then(|p| serde_json::from_slice::<RawClaims>(&p).ok())
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "Google identity validation failed",
        ))?;
    if payload.hd.as_deref() != Some(domain.as_str()) {
        return deny(&s, None, "wrong workspace domain").await;
    }
    let sub = claims.subject().as_str();
    let display = claims.name().and_then(|n| n.get(None)).map(|n| n.as_str());
    let row=sqlx::query("SELECT id,google_sub,role,approved FROM users WHERE google_sub=? OR (google_sub IS NULL AND email=? COLLATE NOCASE) ORDER BY google_sub IS NOT NULL DESC LIMIT 1").bind(sub).bind(&email).fetch_optional(&s.pool).await.map_err(|e| { tracing::error!("login user lookup failed: {e}"); (StatusCode::INTERNAL_SERVER_ERROR,"database failure") })?;
    let Some(row) = row else {
        return deny(&s, None, "account not approved").await;
    };
    let id: i64 = row.get("id");
    let existing: Option<String> = row.get("google_sub");
    let role: String = row.get("role");
    let approved: i64 = row.get("approved");
    if approved != 1
        || (!matches!(
            (pending.intent.as_str(), role.as_str()),
            ("PORTAL", "ADMIN" | "FRONT_DESK" | "STAFF") | ("MANAGEMENT", "ADMIN" | "FRONT_DESK")
        ))
    {
        return deny(&s, Some(id), "role denied").await;
    }
    let mut tx = s.pool.begin().await.map_err(|e| {
        tracing::error!("login transaction failed: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, "database failure")
    })?;
    if existing.is_none() {
        let conflict: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE google_sub=? AND id<>?)")
                .bind(sub)
                .bind(id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database failure"))?;
        if conflict {
            tx.rollback().await.ok();
            return deny(&s, Some(id), "identity already bound").await;
        }
    }
    sqlx::query("UPDATE users SET google_sub=COALESCE(google_sub,?),email=?,display_name=?,updated_at=unixepoch() WHERE id=?").bind(sub).bind(&email).bind(display).bind(id).execute(&mut *tx).await.map_err(|_|(StatusCode::CONFLICT,"identity binding conflict"))?;
    audit(&mut tx, Some(id), "LOGIN_SUCCEEDED", "USER", Some(id), "{}")
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database failure"))?;
    tx.commit()
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database failure"))?;
    session
        .cycle_id()
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "session failure"))?;
    session
        .insert("user_id", id)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "session failure"))?;
    session
        .insert("role", role)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "session failure"))?;
    Ok(Redirect::to(if pending.intent == "MANAGEMENT" {
        "/manage"
    } else {
        "/portal/authorize"
    })
    .into_response())
}
pub async fn logout(
    session: Session,
    Form(_): Form<std::collections::HashMap<String, String>>,
) -> WebResult {
    session
        .flush()
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "session failure"))?;
    Ok(Redirect::to("/").into_response())
}
async fn client(
    s: &AppState,
) -> Result<
    (
        CoreClient<
            EndpointSet,
            EndpointNotSet,
            EndpointNotSet,
            EndpointNotSet,
            EndpointMaybeSet,
            EndpointMaybeSet,
        >,
        String,
    ),
    (StatusCode, &'static str),
> {
    let r=sqlx::query("SELECT google_client_id,google_client_secret_ciphertext,google_client_secret_nonce,google_workspace_domain FROM settings WHERE id=1").fetch_one(&s.pool).await.map_err(|_|(StatusCode::SERVICE_UNAVAILABLE,"setup required"))?;
    let secret = crypto::decrypt(
        &s.config.encryption_key,
        r.get("google_client_secret_ciphertext"),
        r.get("google_client_secret_nonce"),
    )
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "credential decryption failed",
        )
    })?;
    let http = openidconnect::reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| (StatusCode::BAD_GATEWAY, "Google unavailable"))?;
    let provider = CoreProviderMetadata::discover_async(
        IssuerUrl::new("https://accounts.google.com".into()).unwrap(),
        &http,
    )
    .await
    .map_err(|_| (StatusCode::BAD_GATEWAY, "Google unavailable"))?;
    let redirect = RedirectUrl::new(
        s.config
            .public_base_url
            .join("auth/google/callback")
            .unwrap()
            .into(),
    )
    .unwrap();
    Ok((
        CoreClient::from_provider_metadata(
            provider,
            ClientId::new(r.get("google_client_id")),
            Some(ClientSecret::new(secret.expose_secret().into())),
        )
        .set_redirect_uri(redirect),
        r.get("google_workspace_domain"),
    ))
}
async fn deny(s: &AppState, id: Option<i64>, reason: &str) -> WebResult {
    let mut tx = s
        .pool
        .begin()
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database failure"))?;
    let details = serde_json::json!({"reason":reason}).to_string();
    audit(&mut tx, id, "LOGIN_DENIED", "USER", id, &details)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database failure"))?;
    tx.commit()
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database failure"))?;
    Err((
        StatusCode::FORBIDDEN,
        "Internet access not enabled; contact administrator",
    ))
}
