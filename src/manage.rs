use crate::{
    AppState, WebResult, audit,
    model::{generate_coupon, validity_minutes},
    page,
};
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
pub struct TemplateForm {
    csrf: String,
    name: String,
    device_limit: i64,
    validity_value: u32,
    unit: String,
}
#[derive(Deserialize)]
pub struct UserForm {
    csrf: String,
    email: String,
    role: String,
    approved: Option<String>,
    device_limit: i64,
}
#[derive(Deserialize)]
pub struct IssueForm {
    csrf: String,
    template_id: i64,
    note: Option<String>,
}

pub async fn home(State(s): State<AppState>, session: Session) -> WebResult {
    let (id, role, csrf) = guard(&session, &["ADMIN", "FRONT_DESK"]).await?;
    let rows=sqlx::query("SELECT id,name,device_limit,validity_minutes FROM coupon_templates WHERE active=1 ORDER BY name").fetch_all(&s.pool).await.map_err(db)?;
    let mut body = format!(
        "<section><h1>Coupon templates</h1><p>Signed in as {}</p>",
        crate::html(&role)
    );
    for r in rows {
        body.push_str(&format!(r#"<div class="card"><h2>{}</h2><p>{} devices · {} minutes</p><form method="post" action="/manage/coupons/issue"><input type="hidden" name="csrf" value="{}"><input type="hidden" name="template_id" value="{}"><label>Operator note<input maxlength="120" name="note"></label><button>Generate coupon</button></form></div>"#,crate::html(r.get("name")),r.get::<i64,_>("device_limit"),r.get::<i64,_>("validity_minutes"),csrf,r.get::<i64,_>("id")));
    }
    if role == "ADMIN" {
        body.push_str(r#"<nav><a href="/admin/users">Users</a><a href="/admin/templates">Templates</a><a href="/admin/coupons">Coupons</a><a href="/admin/authorizations">Authorizations</a><a href="/admin/audit">Audit</a><a href="/admin/diagnostics">Diagnostics</a></nav>"#)
    }
    body.push_str("</section>");
    let _ = id;
    Ok(page("Management", &body).into_response())
}
pub async fn templates(State(s): State<AppState>, session: Session) -> WebResult {
    let (_, _, csrf) = guard(&session, &["ADMIN"]).await?;
    let rows=sqlx::query("SELECT id,name,device_limit,validity_minutes,active FROM coupon_templates ORDER BY active DESC,name").fetch_all(&s.pool).await.map_err(db)?;
    let mut b = format!(
        r#"<section><h1>Templates</h1><form method="post"><input type="hidden" name="csrf" value="{csrf}"><label>Name<input maxlength="80" name="name" required></label><label>Devices<input type="number" min="1" max="100" name="device_limit" required></label><label>Validity<input type="number" min="1" max="52" name="validity_value" required></label><label>Unit<select name="unit"><option>HOURS</option><option>DAYS</option><option>WEEKS</option></select></label><button>Create template</button></form>"#
    );
    for r in rows {
        b.push_str(&format!(
            "<p>{} — {} devices, {} minutes, active={}</p>",
            crate::html(r.get("name")),
            r.get::<i64, _>("device_limit"),
            r.get::<i64, _>("validity_minutes"),
            r.get::<i64, _>("active")
        ));
    }
    b.push_str("</section>");
    Ok(page("Templates", &b).into_response())
}
pub async fn create_template(
    State(s): State<AppState>,
    session: Session,
    Form(f): Form<TemplateForm>,
) -> WebResult {
    let (id, _, _) = guard_csrf(&session, &f.csrf, &["ADMIN"]).await?;
    if f.name.trim().is_empty() || f.name.len() > 80 || !(1..=100).contains(&f.device_limit) {
        return Err((StatusCode::BAD_REQUEST, "invalid template"));
    }
    let minutes = validity_minutes(f.validity_value, &f.unit)
        .ok_or((StatusCode::BAD_REQUEST, "invalid validity"))?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let mut tx = s.pool.begin().await.map_err(db)?;
    let result=sqlx::query("INSERT INTO coupon_templates(name,device_limit,validity_minutes,active,created_at,updated_at) VALUES(?,?,?,1,?,?)").bind(f.name.trim()).bind(f.device_limit).bind(minutes).bind(now).bind(now).execute(&mut *tx).await.map_err(|_|(StatusCode::CONFLICT,"template name already exists"))?;
    audit(
        &mut tx,
        Some(id),
        "TEMPLATE_CREATED",
        "COUPON_TEMPLATE",
        Some(result.last_insert_rowid()),
        "{}",
    )
    .await
    .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(Redirect::to("/admin/templates").into_response())
}
pub async fn issue(
    State(s): State<AppState>,
    session: Session,
    Form(f): Form<IssueForm>,
) -> WebResult {
    let (id, _, _) = guard_csrf(&session, &f.csrf, &["ADMIN", "FRONT_DESK"]).await?;
    if f.note.as_deref().is_some_and(|n| n.chars().count() > 120) {
        return Err((StatusCode::BAD_REQUEST, "note too long"));
    }
    let t = sqlx::query(
        "SELECT name,device_limit,validity_minutes FROM coupon_templates WHERE id=? AND active=1",
    )
    .bind(f.template_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(db)?
    .ok_or((StatusCode::BAD_REQUEST, "template unavailable"))?;
    let (code, hash, suffix) = generate_coupon();
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let expires = now + t.get::<i64, _>("validity_minutes") * 60;
    let mut tx = s.pool.begin().await.map_err(db)?;
    let result=sqlx::query("INSERT INTO coupons(code_hash,code_suffix,template_id,template_name,device_limit,validity_minutes,issued_by_user_id,issued_at,expires_at,note) VALUES(?,?,?,?,?,?,?,?,?,?)").bind(hash.as_slice()).bind(suffix).bind(f.template_id).bind(t.get::<String,_>("name")).bind(t.get::<i64,_>("device_limit")).bind(t.get::<i64,_>("validity_minutes")).bind(id).bind(now).bind(expires).bind(f.note).execute(&mut *tx).await.map_err(db)?;
    audit(
        &mut tx,
        Some(id),
        "COUPON_ISSUED",
        "COUPON",
        Some(result.last_insert_rowid()),
        "{}",
    )
    .await
    .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(page("Coupon issued",&format!(r#"<section class="coupon"><h1>{}</h1><p class="code">{}</p><p>{} devices</p><p>Issued: {}</p><p>Expires: {}</p></section><button class="no-print" onclick="print()">Print</button>"#,crate::html(t.get("name")),code,t.get::<i64,_>("device_limit"),now,expires)).into_response())
}
pub async fn users(State(s): State<AppState>, session: Session) -> WebResult {
    let (_, _, csrf) = guard(&session, &["ADMIN"]).await?;
    let rows = sqlx::query("SELECT email,role,approved,device_limit FROM users ORDER BY email")
        .fetch_all(&s.pool)
        .await
        .map_err(db)?;
    let mut b = format!(
        r#"<section><h1>Users</h1><form method="post"><input type="hidden" name="csrf" value="{csrf}"><label>Email<input type="email" name="email" required></label><label>Role<select name="role"><option>ADMIN</option><option>FRONT_DESK</option><option>STAFF</option></select></label><label>Approved<input type="checkbox" name="approved" value="1"></label><label>Device limit<input type="number" min="1" max="100" name="device_limit" value="1"></label><button>Save user</button></form>"#
    );
    for r in rows {
        b.push_str(&format!(
            "<p>{} — {} approved={} limit={}</p>",
            crate::html(r.get("email")),
            r.get::<String, _>("role"),
            r.get::<i64, _>("approved"),
            r.get::<i64, _>("device_limit")
        ));
    }
    b.push_str("</section>");
    Ok(page("Users", &b).into_response())
}
pub async fn save_user(
    State(s): State<AppState>,
    session: Session,
    Form(f): Form<UserForm>,
) -> WebResult {
    let (id, _, _) = guard_csrf(&session, &f.csrf, &["ADMIN"]).await?;
    if !f.email.contains('@')
        || !matches!(f.role.as_str(), "ADMIN" | "FRONT_DESK" | "STAFF")
        || !(1..=100).contains(&f.device_limit)
    {
        return Err((StatusCode::BAD_REQUEST, "invalid user"));
    }
    let approved = i64::from(f.approved.is_some());
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let mut tx = s.pool.begin().await.map_err(db)?;
    sqlx::query("INSERT INTO users(email,role,approved,device_limit,created_at,updated_at) VALUES(?,?,?,?,?,?) ON CONFLICT(email) DO UPDATE SET role=excluded.role,approved=excluded.approved,device_limit=excluded.device_limit,updated_at=excluded.updated_at").bind(f.email.trim().to_lowercase()).bind(f.role).bind(approved).bind(f.device_limit).bind(now).bind(now).execute(&mut *tx).await.map_err(db)?;
    let admins: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role='ADMIN' AND approved=1")
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
    if admins < 1 {
        return Err((StatusCode::CONFLICT, "cannot remove last approved admin"));
    }
    audit(&mut tx, Some(id), "USER_POLICY_CHANGED", "USER", None, "{}")
        .await
        .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(Redirect::to("/admin/users").into_response())
}
pub async fn simple(
    State(s): State<AppState>,
    session: Session,
    Path(kind): Path<String>,
) -> WebResult {
    let (_, _, _) = guard(&session, &["ADMIN"]).await?;
    let body=match kind.as_str(){"audit"=>format!("<section><h1>Audit</h1><p>{} events</p></section>",sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM audit_events").fetch_one(&s.pool).await.map_err(db)?),"authorizations"=>format!("<section><h1>Active authorizations</h1><p>{} active</p></section>",sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM device_authorizations WHERE status='ACTIVE' AND expires_at>unixepoch()").fetch_one(&s.pool).await.map_err(db)?),"coupons"=>format!("<section><h1>Coupons</h1><p>{} issued</p></section>",sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM coupons").fetch_one(&s.pool).await.map_err(db)?),_=>return Err((StatusCode::NOT_FOUND,"not found"))};
    Ok(page("Admin", &body).into_response())
}
async fn guard(
    session: &Session,
    roles: &[&str],
) -> Result<(i64, String, String), (StatusCode, &'static str)> {
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
    if !roles.contains(&role.as_str()) {
        return Err((StatusCode::FORBIDDEN, "forbidden"));
    }
    let csrf = match session
        .get::<String>("csrf")
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "session failure"))?
    {
        Some(v) => v,
        None => {
            let v = uuid::Uuid::new_v4().to_string();
            session
                .insert("csrf", &v)
                .await
                .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "session failure"))?;
            v
        }
    };
    Ok((id, role, csrf))
}
async fn guard_csrf(
    session: &Session,
    csrf: &str,
    roles: &[&str],
) -> Result<(i64, String, String), (StatusCode, &'static str)> {
    let result = guard(session, roles).await?;
    if result.2 != csrf {
        return Err((StatusCode::FORBIDDEN, "invalid CSRF token"));
    }
    Ok(result)
}
fn db<E: std::fmt::Display>(_: E) -> (StatusCode, &'static str) {
    (StatusCode::INTERNAL_SERVER_ERROR, "database failure")
}
