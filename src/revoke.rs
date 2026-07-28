use crate::{AppState, WebResult, audit, load_unifi};
use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
};
use serde::Deserialize;
use sqlx::Row;
use time::OffsetDateTime;
use tower_sessions::Session;

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf: String,
}
async fn admin(session: &Session, csrf: &str) -> Result<i64, (StatusCode, &'static str)> {
    let id = session
        .get::<i64>("user_id")
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "login required"))?
        .ok_or((StatusCode::UNAUTHORIZED, "login required"))?;
    let role = session
        .get::<String>("role")
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "login required"))?
        .ok_or((StatusCode::UNAUTHORIZED, "login required"))?;
    let expected = session
        .get::<String>("csrf")
        .await
        .map_err(|_| (StatusCode::FORBIDDEN, "invalid CSRF token"))?
        .ok_or((StatusCode::FORBIDDEN, "invalid CSRF token"))?;
    if role != "ADMIN" {
        return Err((StatusCode::FORBIDDEN, "forbidden"));
    }
    if expected != csrf {
        return Err((StatusCode::FORBIDDEN, "invalid CSRF token"));
    }
    Ok(id)
}
pub async fn authorization(
    State(s): State<AppState>,
    session: Session,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> WebResult {
    let actor = admin(&session, &f.csrf).await?;
    let row = sqlx::query(
        "SELECT unifi_client_id FROM device_authorizations WHERE id=? AND status='ACTIVE'",
    )
    .bind(id)
    .fetch_optional(&s.pool)
    .await
    .map_err(db)?
    .ok_or((StatusCode::NOT_FOUND, "active authorization not found"))?;
    let client = load_unifi(&s).await?;
    client
        .unauthorize(row.get("unifi_client_id"))
        .await
        .map_err(|_| {
            (
                StatusCode::BAD_GATEWAY,
                "UniFi revoke failed; authorization remains active",
            )
        })?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let mut tx = s.pool.begin().await.map_err(db)?;
    sqlx::query("UPDATE device_authorizations SET status='REVOKED',revoked_at=?,revoke_reason='ADMIN_REVOKED' WHERE id=? AND status='ACTIVE'").bind(now).bind(id).execute(&mut *tx).await.map_err(db)?;
    audit(
        &mut tx,
        Some(actor),
        "AUTHORIZATION_REVOKED",
        "AUTHORIZATION",
        Some(id),
        "{}",
    )
    .await
    .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(Redirect::to("/admin/authorizations").into_response())
}
pub async fn coupon(
    State(s): State<AppState>,
    session: Session,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> WebResult {
    let actor = admin(&session, &f.csrf).await?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    sqlx::query("UPDATE coupons SET revoked_at=COALESCE(revoked_at,?) WHERE id=?")
        .bind(now)
        .bind(id)
        .execute(&s.pool)
        .await
        .map_err(db)?;
    let rows=sqlx::query_as::<_,(i64,String)>("SELECT id,unifi_client_id FROM device_authorizations WHERE coupon_id=? AND status='ACTIVE'").bind(id).fetch_all(&s.pool).await.map_err(db)?;
    let client = load_unifi(&s).await?;
    let mut failed = 0;
    for (auth_id, client_id) in rows {
        if client.unauthorize(&client_id).await.is_ok() {
            sqlx::query("UPDATE device_authorizations SET status='REVOKED',revoked_at=?,revoke_reason='COUPON_REVOKED' WHERE id=?").bind(now).bind(auth_id).execute(&s.pool).await.map_err(db)?;
        } else {
            failed += 1
        }
    }
    let mut tx = s.pool.begin().await.map_err(db)?;
    let details = serde_json::json!({"failed":failed}).to_string();
    audit(
        &mut tx,
        Some(actor),
        "COUPON_REVOKED",
        "COUPON",
        Some(id),
        &details,
    )
    .await
    .map_err(db)?;
    tx.commit().await.map_err(db)?;
    if failed > 0 {
        return Err((
            StatusCode::BAD_GATEWAY,
            "Coupon revoked; some active devices could not be disconnected. Retry revoke.",
        ));
    }
    Ok(Redirect::to("/admin/coupons").into_response())
}
fn db<E>(_: E) -> (StatusCode, &'static str) {
    (StatusCode::INTERNAL_SERVER_ERROR, "database failure")
}
