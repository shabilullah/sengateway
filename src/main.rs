mod access;
mod auth;
mod config;
mod crypto;
mod manage;
mod model;
mod portal;
mod revoke;
mod unifi;

use axum::{
    Form, Router,
    extract::{ConnectInfo, DefaultBodyLimit, Query, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use config::Config;
use secrecy::SecretString;
use serde::Deserialize;
use sha2::{Digest, Sha256, Sha512};
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
use subtle::ConstantTimeEq;
use time::OffsetDateTime;
use tokio::sync::{Mutex, RwLock};
use tower_http::services::ServeDir;
use tower_sessions::{
    Expiry, Session, SessionManagerLayer,
    cookie::{Key, SameSite},
};
use tower_sessions_sqlx_store::SqliteStore;
use tracing::{error, info};

#[derive(Clone)]
struct AppState {
    config: Config,
    pool: SqlitePool,
    setup_done: Arc<RwLock<bool>>,
    attempts: Arc<Mutex<HashMap<IpAddr, Attempts>>>,
}
struct Attempts {
    window: i64,
    failures: u8,
}
#[derive(Deserialize)]
struct HomeQuery {
    id: Option<String>,
    ap: Option<String>,
    ssid: Option<String>,
    url: Option<String>,
}
#[derive(Deserialize)]
struct SetupForm {
    passcode: String,
    initial_admin_email: String,
    google_auth_client_id: String,
    google_oauth_client_secret: String,
    google_oauth_version: String,
    google_workspace_domain: String,
    unifi_network_api_url: String,
    unifi_api_key: String,
    #[serde(default)]
    trust_unifi_self_signed_certificate: bool,
    unifi_site_id: String,
}

type WebResult = Result<axum::response::Response, (StatusCode, &'static str)>;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let config = Config::load().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });
    let pool = SqlitePoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await
        .unwrap_or_else(|e| {
            eprintln!("database unavailable: {e}");
            std::process::exit(2)
        });
    sqlx::migrate!().run(&pool).await.unwrap_or_else(|e| {
        eprintln!("database migration failed: {e}");
        std::process::exit(2)
    });
    let done: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM settings WHERE id=1)")
        .fetch_one(&pool)
        .await
        .unwrap();
    let state = AppState {
        config: config.clone(),
        pool: pool.clone(),
        setup_done: Arc::new(RwLock::new(done)),
        attempts: Arc::new(Mutex::new(HashMap::new())),
    };
    let store = SqliteStore::new(pool.clone());
    store.migrate().await.expect("session migration");
    let key = Key::from(&Sha512::digest(&config.session_secret));
    let sessions = SessionManagerLayer::new(store)
        .with_secure(config.cookie_secure)
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_signed(key)
        .with_expiry(Expiry::OnInactivity(time::Duration::hours(8)));
    let app = Router::new()
        .route("/", get(home))
        .route("/guest/s/{site}", get(home))
        .route("/guest/s/{site}/", get(home))
        .route("/healthz", get(health))
        .route("/brand/logo", get(brand_logo))
        .route("/setup", get(setup_get).post(setup_post))
        .route("/portal", get(portal_get))
        .route("/portal/authorize", get(staff_authorize))
        .route("/coupon/redeem", post(coupon_redeem))
        .route("/auth/google/start", get(auth::start))
        .route("/auth/google/callback", get(auth::callback))
        .route("/logout", post(auth::logout))
        .route("/manage", get(manage::home))
        .route("/manage/coupons/issue", post(manage::issue))
        .route(
            "/admin/templates",
            get(manage::templates).post(manage::create_template),
        )
        .route("/admin/users", get(manage::users).post(manage::save_user))
        .route(
            "/admin/branding",
            get(manage::branding)
                .post(manage::save_branding)
                .layer(DefaultBodyLimit::max(1_100_000)),
        )
        .route("/admin/branding/remove", post(manage::remove_branding))
        .route("/admin/users/{id}", post(manage::delete_user))
        .route(
            "/admin/authorizations/{id}/revoke",
            post(revoke::authorization),
        )
        .route("/admin/coupons/{id}/revoke", post(revoke::coupon))
        .route("/admin/diagnostics", get(diagnostics))
        .route("/admin/{kind}", get(manage::simple))
        .nest_service("/static", ServeDir::new("static"))
        .fallback(fallback)
        .layer(middleware::from_fn_with_state(state.clone(), security_gate))
        .layer(sessions)
        .with_state(state.clone());

    tokio::spawn(reconcile(state));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080")
        .await
        .expect("bind");
    info!("listening on 8080");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await
    .expect("server");
}

async fn security_gate(State(s): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if path == "/healthz" {
        return next.run(request).await;
    }
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|p| p.0);
    let proto = request
        .headers()
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok());
    let forwarded = request
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok());
    let valid = peer.is_some_and(|p| p.ip() == s.config.trusted_proxy_ip)
        && proto == Some("https")
        && forwarded.is_some_and(|v| {
            v.split(',')
                .all(|part| part.trim().parse::<IpAddr>().is_ok())
        });
    if !valid {
        return (StatusCode::FORBIDDEN, "trusted HTTPS proxy required").into_response();
    }
    next.run(request).await
}
async fn health(State(s): State<AppState>) -> impl IntoResponse {
    if sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(&s.pool)
        .await
        .is_err()
    {
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    if *s.setup_done.read().await {
        match load_unifi(&s).await {
            Ok(client) if client.readiness_check().await.is_ok() => StatusCode::OK,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        }
    } else {
        StatusCode::OK
    }
}

fn scripts() -> &'static str {
    r#"<script defer src="/static/anime.umd.min.js"></script><script defer src="/static/app.js"></script>"#
}
fn page(title: &str, body: &str) -> Html<String> {
    Html(format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{title} · SEN Gateway</title><link rel="stylesheet" href="/static/app.css">{}</head><body><header><a class="brand" href="/"><span class="brand-mark">SEN</span> Gateway</a></header><main>{body}</main></body></html>"#,
        scripts()
    ))
}

fn landing(portal_ready: bool, has_logo: bool) -> Html<String> {
    let form = if portal_ready {
        r#"<form method="post" action="/coupon/redeem"><label for="code">Voucher code</label><input id="code" required name="code" autocomplete="one-time-code" placeholder="XXXX-XXXX-XXXX"><button>Connect to internet</button></form>"#
    } else {
        r#"<form><label for="code">Voucher code</label><input id="code" disabled placeholder="XXXX-XXXX-XXXX"><button disabled>Connect to internet</button></form><p class="hint">Connect to guest Wi-Fi first. This page will reopen with your device details.</p>"#
    };
    let logo = if has_logo {
        r#"<img class="sen-logo" src="/brand/logo" alt="Organization logo">"#
    } else {
        ""
    };
    Html(format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Internet access · SEN Gateway</title><link rel="stylesheet" href="/static/app.css">{}</head><body><header><a class="brand" href="/"><span class="brand-mark">SEN</span> Gateway</a><nav><a href="/auth/google/start?intent=PORTAL">Staff login</a><a href="/auth/google/start?intent=MANAGEMENT">Admin login</a></nav></header><main class="landing"><section data-motion>{logo}<p class="eyebrow">Guest Wi-Fi</p><h1>Get online. Stay connected.</h1><p>Enter voucher provided by front desk.</p>{form}</section><aside id="staff-help" class="card" data-motion><p class="kicker">Team access</p><h2>Staff sign-in</h2><p>Connect to guest Wi-Fi, then use approved Google Workspace account.</p><a class="button secondary" href="/auth/google/start?intent=PORTAL">Continue with Google</a></aside></main></body></html>"#,
        scripts(),
    ))
}
async fn setup_get(
    State(s): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> WebResult {
    trusted(
        &s,
        peer,
        headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok()),
    )?;
    if !s.config.setup_enabled {
        return Err((StatusCode::NOT_FOUND, "not found"));
    }
    let heading = if *s.setup_done.read().await {
        "Reconfigure gateway"
    } else {
        "Initial setup"
    };
    Ok(page("Setup", &format!(r#"<section><h1>{heading}</h1><p class="hint">Setup remains available while SETUP=true. Set SETUP=false and redeploy after saving.</p><form method="post"><label>Setup passcode<input required type="password" name="passcode" autocomplete="current-password"></label><label>Administrator email<input required type="email" name="initial_admin_email"></label><label>Google client ID<input required name="google_auth_client_id"></label><label>Google client secret<input required type="password" name="google_oauth_client_secret"></label><label>Google OAuth version<input required name="google_oauth_version" value="v2"></label><label>Workspace domain<input required name="google_workspace_domain"></label><label>UniFi Network API URL<input required type="url" name="unifi_network_api_url" placeholder="https://unifi.local:11443/proxy/network/integration/v1"></label><label class="check"><input type="checkbox" name="trust_unifi_self_signed_certificate" value="true"> Trust certificate currently presented by this UniFi server</label><p class="hint">Enable only after independently confirming this URL reaches your UniFi server. Leave disabled when UniFi uses a certificate trusted by the operating system.</p><label>UniFi API key<input required type="password" name="unifi_api_key"></label><label>UniFi site ID<input required name="unifi_site_id"></label><button>Save setup</button></form></section>"#)).into_response())
}
async fn setup_post(
    State(s): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(f): Form<SetupForm>,
) -> WebResult {
    trusted(
        &s,
        peer,
        headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok()),
    )?;
    if !s.config.setup_enabled {
        return Err((StatusCode::NOT_FOUND, "not found"));
    }
    if !check_passcode(&s, &f.passcode, peer.ip()).await {
        return Err((StatusCode::UNAUTHORIZED, "invalid setup passcode"));
    }
    validate_setup(&f)?;
    let unifi_certificate = if f.trust_unifi_self_signed_certificate {
        Some(
            unifi::UnifiClient::capture_certificate(&f.unifi_network_api_url)
                .map_err(|_| (StatusCode::BAD_REQUEST, "cannot read UniFi TLS certificate"))?,
        )
    } else {
        None
    };
    let probe = unifi::UnifiClient::new(
        f.unifi_network_api_url.clone(),
        f.unifi_site_id.clone(),
        SecretString::from(f.unifi_api_key.clone()),
        unifi_certificate.as_deref(),
    )
    .map_err(|_| (StatusCode::BAD_REQUEST, "invalid UniFi settings"))?;
    probe.site_check().await.map_err(unifi_setup_error)?;
    let (gc, gn) = crypto::encrypt(
        &s.config.encryption_key,
        &SecretString::from(f.google_oauth_client_secret),
    )
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "encryption failed"))?;
    let (uc, un) = crypto::encrypt(
        &s.config.encryption_key,
        &SecretString::from(f.unifi_api_key),
    )
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "encryption failed"))?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let mut tx = s.pool.begin().await.map_err(internal)?;
    let reconfiguring = *s.setup_done.read().await;
    sqlx::query("INSERT INTO settings(id,public_base_url,google_client_id,google_client_secret_ciphertext,google_client_secret_nonce,google_oauth_version,google_workspace_domain,unifi_network_api_url,unifi_api_key_ciphertext,unifi_api_key_nonce,unifi_site_id,staff_session_minutes,setup_completed_at,unifi_certificate_pem) VALUES(1,?,?,?,?,?,?,?,?,?,?,480,?,?) ON CONFLICT(id) DO UPDATE SET public_base_url=excluded.public_base_url,google_client_id=excluded.google_client_id,google_client_secret_ciphertext=excluded.google_client_secret_ciphertext,google_client_secret_nonce=excluded.google_client_secret_nonce,google_oauth_version=excluded.google_oauth_version,google_workspace_domain=excluded.google_workspace_domain,unifi_network_api_url=excluded.unifi_network_api_url,unifi_api_key_ciphertext=excluded.unifi_api_key_ciphertext,unifi_api_key_nonce=excluded.unifi_api_key_nonce,unifi_site_id=excluded.unifi_site_id,setup_completed_at=excluded.setup_completed_at,unifi_certificate_pem=excluded.unifi_certificate_pem").bind(s.config.public_base_url.as_str()).bind(f.google_auth_client_id.trim()).bind(gc).bind(gn.as_slice()).bind("v2").bind(f.google_workspace_domain.trim()).bind(f.unifi_network_api_url.trim_end_matches('/')).bind(uc).bind(un.as_slice()).bind(f.unifi_site_id.trim()).bind(now).bind(unifi_certificate).execute(&mut *tx).await.map_err(internal)?;
    sqlx::query("INSERT INTO users(email,role,approved,device_limit,created_at,updated_at) VALUES(?,'ADMIN',1,1,?,?) ON CONFLICT(email) DO UPDATE SET role='ADMIN',approved=1,updated_at=excluded.updated_at").bind(f.initial_admin_email.trim().to_lowercase()).bind(now).bind(now).execute(&mut *tx).await.map_err(internal)?;
    audit(
        &mut tx,
        None,
        if reconfiguring {
            "SETUP_RECONFIGURED"
        } else {
            "SETUP_COMPLETED"
        },
        "SETTINGS",
        Some(1),
        "{}",
    )
    .await
    .map_err(internal)?;
    tx.commit().await.map_err(internal)?;
    *s.setup_done.write().await = true;
    Ok(Redirect::to("/auth/google/start?intent=MANAGEMENT").into_response())
}
async fn portal_get(
    State(s): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    session: Session,
    Query(q): Query<portal::PortalQuery>,
) -> WebResult {
    trusted(
        &s,
        peer,
        headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok()),
    )?;
    if !*s.setup_done.read().await {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "setup required"));
    }
    let ctx = portal::PortalContext::try_from(q)
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid portal request"))?;
    session
        .insert("portal_context", ctx)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "session failure"))?;
    let has_logo =
        sqlx::query_scalar::<_, i64>("SELECT logo_data IS NOT NULL FROM settings WHERE id=1")
            .fetch_one(&s.pool)
            .await
            .map_err(internal)?
            != 0;
    Ok(landing(true, has_logo).into_response())
}
#[derive(Deserialize)]
struct CouponForm {
    code: String,
}
async fn coupon_redeem(
    State(s): State<AppState>,
    session: Session,
    Form(f): Form<CouponForm>,
) -> WebResult {
    let mut ctx: portal::PortalContext = session
        .get("portal_context")
        .await
        .map_err(|_| (StatusCode::BAD_REQUEST, "fresh portal required"))?
        .ok_or((StatusCode::BAD_REQUEST, "fresh portal required"))?;
    ctx.consume()
        .map_err(|_| (StatusCode::BAD_REQUEST, "fresh portal required"))?;
    session
        .insert("portal_context", &ctx)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "session failure"))?;
    let unifi = load_unifi(&s).await?;
    access::redeem_coupon(&s.pool, &unifi, &f.code, &ctx.mac)
        .await
        .map_err(|m| (StatusCode::FORBIDDEN, m))?;
    Ok(page(
        "Access granted",
        "<section><h1>Internet access granted</h1></section>",
    )
    .into_response())
}
fn can_authorize_device(role: &str) -> bool {
    matches!(role, "ADMIN" | "FRONT_DESK" | "STAFF")
}
async fn staff_authorize(State(s): State<AppState>, session: Session) -> WebResult {
    let user_id = session
        .get::<i64>("user_id")
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "login required"))?
        .ok_or((StatusCode::UNAUTHORIZED, "login required"))?;
    let role = session
        .get::<String>("role")
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "login required"))?
        .ok_or((StatusCode::UNAUTHORIZED, "login required"))?;
    if !can_authorize_device(&role) {
        return Err((StatusCode::FORBIDDEN, "forbidden"));
    }
    let mut ctx: portal::PortalContext = session
        .get("portal_context")
        .await
        .map_err(|_| (StatusCode::BAD_REQUEST, "fresh portal required"))?
        .ok_or((StatusCode::BAD_REQUEST, "fresh portal required"))?;
    ctx.consume()
        .map_err(|_| (StatusCode::BAD_REQUEST, "fresh portal required"))?;
    session
        .insert("portal_context", &ctx)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "session failure"))?;
    let row=sqlx::query_as::<_,(i64,i64)>("SELECT u.device_limit,s.staff_session_minutes FROM users u CROSS JOIN settings s WHERE u.id=?").bind(user_id).fetch_one(&s.pool).await.map_err(internal)?;
    let unifi = load_unifi(&s).await?;
    access::authorize_staff(&s.pool, &unifi, user_id, row.0, row.1, &ctx.mac)
        .await
        .map_err(|m| (StatusCode::BAD_GATEWAY, m))?;
    Ok(page(
        "Access granted",
        "<section><h1>Internet access granted</h1></section>",
    )
    .into_response())
}
async fn diagnostics(State(s): State<AppState>, session: Session) -> WebResult {
    let (_, _, csrf) = manage::guard(&session, &["ADMIN"]).await?;
    let client = load_unifi(&s).await?;
    let status = match client.readiness_check().await {
        Ok(()) => "healthy",
        Err(_) => "unavailable",
    };
    let last = client
        .last_success()
        .await
        .map(|t| t.to_string())
        .unwrap_or_else(|| "never".into());
    let body = format!(
        r#"<div class="page-head" data-motion><div><p class="eyebrow">Controller health</p><h1>Diagnostics.</h1></div><span class="badge{}">{status}</span></div><section data-motion><h2>UniFi Network</h2><p><strong>Endpoint</strong><br>{}</p><p><strong>Site</strong><br>{}</p><p><strong>Last successful request</strong><br>{last}</p></section>"#,
        if status == "healthy" { "" } else { " off" },
        html(&client.redacted_endpoint()),
        html(client.site())
    );
    Ok(manage::admin_page("Diagnostics", "diagnostics", &csrf, &body).into_response())
}
async fn load_unifi(s: &AppState) -> Result<unifi::UnifiClient, (StatusCode, &'static str)> {
    use secrecy::ExposeSecret;
    let row=sqlx::query("SELECT unifi_network_api_url,unifi_api_key_ciphertext,unifi_api_key_nonce,unifi_site_id,unifi_certificate_pem FROM settings WHERE id=1").fetch_one(&s.pool).await.map_err(internal)?;
    let secret = crypto::decrypt(
        &s.config.encryption_key,
        sqlx::Row::get(&row, "unifi_api_key_ciphertext"),
        sqlx::Row::get(&row, "unifi_api_key_nonce"),
    )
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "credential decryption failed",
        )
    })?;
    let certificate: Option<Vec<u8>> = sqlx::Row::get(&row, "unifi_certificate_pem");
    unifi::UnifiClient::new(
        sqlx::Row::get(&row, "unifi_network_api_url"),
        sqlx::Row::get(&row, "unifi_site_id"),
        SecretString::from(secret.expose_secret().to_owned()),
        certificate.as_deref(),
    )
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "UniFi client failed"))
}
async fn brand_logo(State(s): State<AppState>) -> WebResult {
    let row = sqlx::query("SELECT logo_content_type,logo_data FROM settings WHERE id=1")
        .fetch_optional(&s.pool)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "logo not configured"))?;
    let content_type: Option<String> = sqlx::Row::get(&row, "logo_content_type");
    let data: Option<Vec<u8>> = sqlx::Row::get(&row, "logo_data");
    let (content_type, data) = content_type
        .zip(data)
        .ok_or((StatusCode::NOT_FOUND, "logo not configured"))?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache".into()),
        ],
        data,
    )
        .into_response())
}

async fn home(
    State(s): State<AppState>,
    session: Session,
    Query(q): Query<HomeQuery>,
) -> WebResult {
    if !*s.setup_done.read().await {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "setup required"));
    }
    let has_logo =
        sqlx::query_scalar::<_, i64>("SELECT logo_data IS NOT NULL FROM settings WHERE id=1")
            .fetch_one(&s.pool)
            .await
            .map_err(internal)?
            != 0;
    let Some(id) = q.id else {
        return Ok(landing(false, has_logo).into_response());
    };
    let ctx = portal::PortalContext::try_from(portal::PortalQuery {
        id,
        ap: q.ap,
        ssid: q.ssid,
        url: q.url,
    })
    .map_err(|_| (StatusCode::BAD_REQUEST, "invalid portal request"))?;
    session
        .insert("portal_context", ctx)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "session failure"))?;
    Ok(landing(true, has_logo).into_response())
}

async fn fallback(State(s): State<AppState>) -> impl IntoResponse {
    if !*s.setup_done.read().await {
        (StatusCode::SERVICE_UNAVAILABLE, "setup required")
    } else {
        (StatusCode::NOT_FOUND, "not found")
    }
}
fn unifi_setup_error(error: unifi::UnifiError) -> (StatusCode, &'static str) {
    match error {
        unifi::UnifiError::Http(StatusCode::UNAUTHORIZED) => {
            (StatusCode::BAD_GATEWAY, "UniFi API key was rejected")
        }
        unifi::UnifiError::Http(StatusCode::FORBIDDEN) => {
            (StatusCode::BAD_GATEWAY, "UniFi API key lacks site access")
        }
        unifi::UnifiError::Http(StatusCode::NOT_FOUND) => (
            StatusCode::BAD_GATEWAY,
            "UniFi API URL or site ID was not found",
        ),
        unifi::UnifiError::Http(_) => (
            StatusCode::BAD_GATEWAY,
            "UniFi API returned an unexpected response",
        ),
        unifi::UnifiError::Request => (
            StatusCode::BAD_GATEWAY,
            "Could not connect securely to UniFi",
        ),
        unifi::UnifiError::NotFound => (StatusCode::BAD_GATEWAY, "UniFi site was not found"),
    }
}

fn validate_setup(f: &SetupForm) -> Result<(), (StatusCode, &'static str)> {
    if f.google_oauth_version != "v2" {
        return Err((StatusCode::BAD_REQUEST, "Google OAuth version must be v2"));
    }
    if f.google_workspace_domain != f.google_workspace_domain.to_lowercase() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Workspace domain must be lowercase",
        ));
    }
    if !f.initial_admin_email.contains('@') {
        return Err((StatusCode::BAD_REQUEST, "Initial admin email is invalid"));
    }
    if f.unifi_site_id.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "UniFi site ID is required"));
    }
    let u = url::Url::parse(&f.unifi_network_api_url)
        .map_err(|_| (StatusCode::BAD_REQUEST, "UniFi API URL is invalid"))?;
    if u.scheme() != "https" {
        return Err((StatusCode::BAD_REQUEST, "UniFi API URL must use HTTPS"));
    }
    Ok(())
}
async fn check_passcode(s: &AppState, passcode: &str, ip: IpAddr) -> bool {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let mut attempts = s.attempts.lock().await;
    let a = attempts.entry(ip).or_insert(Attempts {
        window: now,
        failures: 0,
    });
    if now - a.window > 60 {
        a.window = now;
        a.failures = 0
    }
    if a.failures >= 5 {
        return false;
    }
    let got: [u8; 32] = Sha256::digest(passcode.as_bytes()).into();
    let ok = bool::from(s.config.setup_passcode_hash.ct_eq(&got));
    if !ok {
        a.failures += 1
    }
    ok
}
fn trusted(
    s: &AppState,
    peer: SocketAddr,
    proto: Option<&str>,
) -> Result<(), (StatusCode, &'static str)> {
    if peer.ip() != s.config.trusted_proxy_ip || proto != Some("https") {
        Err((StatusCode::FORBIDDEN, "trusted HTTPS proxy required"))
    } else {
        Ok(())
    }
}
fn html(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, &'static str) {
    error!("database operation failed: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}
async fn audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    actor: Option<i64>,
    event: &str,
    target: &str,
    id: Option<i64>,
    details: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO audit_events(actor_user_id,event_type,target_type,target_id,details_json,created_at) VALUES(?,?,?,?,?,?)").bind(actor).bind(event).bind(target).bind(id).bind(details).bind(OffsetDateTime::now_utc().unix_timestamp()).execute(&mut **tx).await?;
    Ok(())
}
async fn reconcile(s: AppState) {
    let mut timer = tokio::time::interval(std::time::Duration::from_secs(60));
    loop {
        timer.tick().await;
        if !*s.setup_done.read().await {
            continue;
        }
        let now = OffsetDateTime::now_utc().unix_timestamp();
        if let Err(e) = sqlx::query("UPDATE device_authorizations SET status='FAILED',failure_message='reservation timed out' WHERE status='PENDING' AND created_at<?")
            .bind(now - 120)
            .execute(&s.pool)
            .await
        {
            error!("pending reconciliation failed: {e}");
        }
        let Ok(client) = load_unifi(&s).await else {
            continue;
        };
        let rows = match sqlx::query_as::<_, (i64, String)>("SELECT id,unifi_client_id FROM device_authorizations WHERE status='ACTIVE' AND expires_at<=?")
            .bind(now)
            .fetch_all(&s.pool)
            .await
        {
            Ok(rows) => rows,
            Err(e) => { error!("expiry query failed: {e}"); continue; }
        };
        for (id, unifi_id) in rows {
            if client.unauthorize(&unifi_id).await.is_ok()
                && let Err(e) = sqlx::query("UPDATE device_authorizations SET status='REVOKED',revoked_at=?,revoke_reason='EXPIRED' WHERE id=? AND status='ACTIVE'")
                    .bind(now).bind(id).execute(&s.pool).await
            {
                error!("expiry finalization failed: {e}");
            }
        }
    }
}
async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_form() -> SetupForm {
        SetupForm {
            passcode: "0123456789abcdef".into(),
            initial_admin_email: "admin@example.com".into(),
            google_auth_client_id: "client".into(),
            google_oauth_client_secret: "secret".into(),
            google_oauth_version: "v2".into(),
            google_workspace_domain: "example.com".into(),
            unifi_network_api_url: "https://unifi.example.com/integration/v1".into(),
            unifi_api_key: "key".into(),
            trust_unifi_self_signed_certificate: false,
            unifi_site_id: "site-id".into(),
        }
    }

    #[test]
    fn setup_validation_identifies_invalid_field() {
        let mut form = setup_form();
        assert!(validate_setup(&form).is_ok());
        form.google_workspace_domain = "EXAMPLE.com".into();
        assert_eq!(
            validate_setup(&form),
            Err((
                StatusCode::BAD_REQUEST,
                "Workspace domain must be lowercase"
            ))
        );
    }

    #[test]
    fn setup_passcode_comparison_is_exact() {
        let expected: [u8; 32] = Sha256::digest(b"0123456789abcdef").into();
        let matching: [u8; 32] = Sha256::digest(b"0123456789abcdef").into();
        let different: [u8; 32] = Sha256::digest(b"0123456789abcdeg").into();
        assert!(bool::from(expected.ct_eq(&matching)));
        assert!(!bool::from(expected.ct_eq(&different)));
    }

    #[test]
    fn all_approved_roles_can_authorize_devices() {
        assert!(can_authorize_device("ADMIN"));
        assert!(can_authorize_device("FRONT_DESK"));
        assert!(can_authorize_device("STAFF"));
        assert!(!can_authorize_device("UNKNOWN"));
    }

    #[test]
    fn staff_login_always_starts_portal_oauth() {
        for portal_ready in [false, true] {
            let html = landing(portal_ready, false).0;
            assert_eq!(
                html.matches("href=\"/auth/google/start?intent=PORTAL\"")
                    .count(),
                2
            );
            assert!(!html.contains("href=\"#staff-help\""));
        }
    }
    #[test]
    fn landing_only_renders_configured_logo() {
        assert!(!landing(false, false).0.contains("/brand/logo"));
        assert!(landing(false, true).0.contains("src=\"/brand/logo\""));
        assert!(!landing(false, true).0.contains("seameo-sen-logo.png"));
    }
}
