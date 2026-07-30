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
    id: Option<i64>,
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
#[derive(Deserialize)]
pub struct DeleteUserForm {
    csrf: String,
}

pub async fn home(State(s): State<AppState>, session: Session) -> WebResult {
    let (_, role, csrf) = guard(&session, &["ADMIN", "FRONT_DESK"]).await?;
    let rows = sqlx::query("SELECT id,name,device_limit,validity_minutes FROM coupon_templates WHERE active=1 ORDER BY name")
        .fetch_all(&s.pool).await.map_err(db)?;
    let mut body = String::from(
        "<div class=\"page-head\" data-motion><div><p class=\"eyebrow\">Front desk</p><h1>Issue access.</h1><p>Create one-time guest Wi-Fi vouchers.</p></div></div><div class=\"stack\">",
    );
    for r in rows {
        body.push_str(&format!(r#"<section class="card" data-motion><h2>{}</h2><p><strong>{} devices</strong> · {} minutes</p><form method="post" action="/manage/coupons/issue"><input type="hidden" name="csrf" value="{}"><input type="hidden" name="template_id" value="{}"><label>Operator note<input maxlength="120" name="note" placeholder="Optional"></label><button>Generate coupon</button></form></section>"#, crate::html(r.get("name")), r.get::<i64,_>("device_limit"), r.get::<i64,_>("validity_minutes"), csrf, r.get::<i64,_>("id")));
    }
    body.push_str("</div>");
    Ok(if role == "ADMIN" {
        admin_page("Management", "manage", &csrf, &body)
    } else {
        page("Management", &body)
    }
    .into_response())
}

pub async fn templates(State(s): State<AppState>, session: Session) -> WebResult {
    let (_, _, csrf) = guard(&session, &["ADMIN"]).await?;
    let rows = sqlx::query("SELECT id,name,device_limit,validity_minutes,active FROM coupon_templates ORDER BY active DESC,name")
        .fetch_all(&s.pool).await.map_err(db)?;
    let mut b = format!(
        r#"<div class="page-head" data-motion><div><p class="eyebrow">Access rules</p><h1>Templates.</h1></div></div><section data-motion><h2>New template</h2><form class="form-grid" method="post"><input type="hidden" name="csrf" value="{csrf}"><label>Name<input maxlength="80" name="name" required></label><label>Devices<input type="number" min="1" max="100" name="device_limit" required></label><label>Validity<input type="number" min="1" max="52" name="validity_value" required></label><label>Unit<select name="unit"><option>HOURS</option><option>DAYS</option><option>WEEKS</option></select></label><button>Create template</button></form></section><div class="table-wrap" data-motion><table><thead><tr><th>Name</th><th>Devices</th><th>Validity</th><th>Status</th></tr></thead><tbody>"#
    );
    for r in rows {
        let active = r.get::<i64, _>("active") == 1;
        b.push_str(&format!("<tr><td><strong>{}</strong></td><td>{}</td><td>{} minutes</td><td><span class=\"badge{}\">{}</span></td></tr>", crate::html(r.get("name")), r.get::<i64,_>("device_limit"), r.get::<i64,_>("validity_minutes"), if active { "" } else { " off" }, if active { "Active" } else { "Inactive" }));
    }
    b.push_str("</tbody></table></div>");
    Ok(admin_page("Templates", "templates", &csrf, &b).into_response())
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
    let result = sqlx::query("INSERT INTO coupon_templates(name,device_limit,validity_minutes,active,created_at,updated_at) VALUES(?,?,?,1,?,?)")
        .bind(f.name.trim()).bind(f.device_limit).bind(minutes).bind(now).bind(now).execute(&mut *tx).await
        .map_err(|_| (StatusCode::CONFLICT, "template name already exists"))?;
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
    let result = sqlx::query("INSERT INTO coupons(code_hash,code_suffix,template_id,template_name,device_limit,validity_minutes,issued_by_user_id,issued_at,expires_at,note) VALUES(?,?,?,?,?,?,?,?,?,?)")
        .bind(hash.as_slice()).bind(suffix).bind(f.template_id).bind(t.get::<String,_>("name")).bind(t.get::<i64,_>("device_limit")).bind(t.get::<i64,_>("validity_minutes")).bind(id).bind(now).bind(expires).bind(f.note).execute(&mut *tx).await.map_err(db)?;
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
    Ok(page("Coupon issued", &format!(r#"<section class="coupon" data-motion><p class="eyebrow">Ready to use</p><h1>{}</h1><p class="code">{}</p><p>{} devices</p><p>Issued: <time data-unix="{}">{}</time></p><p>Expires: <time data-unix="{}">{}</time></p></section><button class="no-print" onclick="print()">Print</button>"#, crate::html(t.get("name")), code, t.get::<i64,_>("device_limit"), now, now, expires, expires)).into_response())
}

pub async fn users(State(s): State<AppState>, session: Session) -> WebResult {
    let (_, _, csrf) = guard(&session, &["ADMIN"]).await?;
    let rows = sqlx::query("SELECT id,email,role,approved,device_limit FROM users ORDER BY email")
        .fetch_all(&s.pool)
        .await
        .map_err(db)?;
    let mut b = format!(
        r#"<div class="page-head" data-motion><div><p class="eyebrow">Identity policy</p><h1>Users.</h1><p>Add, approve, edit, or remove access.</p></div></div><section data-motion><h2>Add user</h2>{}</section><div class="stack">"#,
        user_form(None, "", "STAFF", false, 1, &csrf)
    );
    for r in rows {
        let id: i64 = r.get("id");
        let email: String = r.get("email");
        let role: String = r.get("role");
        let approved = r.get::<i64, _>("approved") == 1;
        let limit: i64 = r.get("device_limit");
        b.push_str(&format!(r#"<section class="user-card" data-motion><div class="user-title"><h2>{}</h2><span class="badge{}">{}</span></div>{}<form class="actions" method="post" action="/admin/users/{}" data-confirm="Delete {}? Existing history prevents deletion."><input type="hidden" name="csrf" value="{}"><button class="danger">Delete user</button></form></section>"#, crate::html(&email), if approved { "" } else { " off" }, if approved { "Approved" } else { "Blocked" }, user_form(Some(id), &email, &role, approved, limit, &csrf), id, crate::html(&email), csrf));
    }
    b.push_str("</div>");
    Ok(admin_page("Users", "users", &csrf, &b).into_response())
}

fn user_form(
    id: Option<i64>,
    email: &str,
    role: &str,
    approved: bool,
    limit: i64,
    csrf: &str,
) -> String {
    let id_field = id
        .map(|v| format!("<input type=\"hidden\" name=\"id\" value=\"{v}\">"))
        .unwrap_or_default();
    let options = ["ADMIN", "FRONT_DESK", "STAFF"]
        .map(|value| {
            format!(
                "<option{}>{value}</option>",
                if value == role { " selected" } else { "" }
            )
        })
        .join("");
    format!(
        r#"<form class="form-grid" method="post" action="/admin/users"><input type="hidden" name="csrf" value="{csrf}">{id_field}<label>Email<input type="email" maxlength="254" name="email" value="{}" required></label><label>Role<select name="role">{options}</select></label><label>Approved<input type="checkbox" name="approved" value="1" {}></label><label>Device limit<input type="number" min="1" max="100" name="device_limit" value="{limit}" required></label><button>{}</button></form>"#,
        crate::html(email),
        if approved { "checked" } else { "" },
        if id.is_some() {
            "Update user"
        } else {
            "Add user"
        }
    )
}

pub async fn save_user(
    State(s): State<AppState>,
    session: Session,
    Form(f): Form<UserForm>,
) -> WebResult {
    let (actor, _, _) = guard_csrf(&session, &f.csrf, &["ADMIN"]).await?;
    let email = f.email.trim().to_lowercase();
    if email.len() > 254
        || !email.contains('@')
        || !matches!(f.role.as_str(), "ADMIN" | "FRONT_DESK" | "STAFF")
        || !(1..=100).contains(&f.device_limit)
    {
        return Err((StatusCode::BAD_REQUEST, "invalid user"));
    }
    let approved = i64::from(f.approved.is_some());
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let mut tx = s.pool.begin().await.map_err(db)?;
    let target = if let Some(target) = f.id {
        let result = sqlx::query(
            "UPDATE users SET email=?,role=?,approved=?,device_limit=?,updated_at=? WHERE id=?",
        )
        .bind(email)
        .bind(f.role)
        .bind(approved)
        .bind(f.device_limit)
        .bind(now)
        .bind(target)
        .execute(&mut *tx)
        .await
        .map_err(|_| (StatusCode::CONFLICT, "email already exists"))?;
        if result.rows_affected() != 1 {
            return Err((StatusCode::NOT_FOUND, "user not found"));
        }
        target
    } else {
        sqlx::query("INSERT INTO users(email,role,approved,device_limit,created_at,updated_at) VALUES(?,?,?,?,?,?)")
            .bind(email).bind(f.role).bind(approved).bind(f.device_limit).bind(now).bind(now).execute(&mut *tx).await
            .map_err(|_| (StatusCode::CONFLICT, "email already exists"))?.last_insert_rowid()
    };
    ensure_admin_exists(&mut tx).await?;
    audit(
        &mut tx,
        Some(actor),
        "USER_POLICY_CHANGED",
        "USER",
        Some(target),
        "{}",
    )
    .await
    .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(Redirect::to("/admin/users").into_response())
}

pub async fn delete_user(
    State(s): State<AppState>,
    session: Session,
    Path(target): Path<i64>,
    Form(f): Form<DeleteUserForm>,
) -> WebResult {
    let (actor, _, _) = guard_csrf(&session, &f.csrf, &["ADMIN"]).await?;
    if actor == target {
        return Err((StatusCode::CONFLICT, "cannot delete your own account"));
    }
    let mut tx = s.pool.begin().await.map_err(db)?;
    let history: i64 = sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM coupons WHERE issued_by_user_id=?) + (SELECT COUNT(*) FROM device_authorizations WHERE user_id=?) + (SELECT COUNT(*) FROM audit_events WHERE actor_user_id=?)")
        .bind(target).bind(target).bind(target).fetch_one(&mut *tx).await.map_err(db)?;
    if history > 0 {
        return Err((
            StatusCode::CONFLICT,
            "user has retained authorization or audit history",
        ));
    }
    let result = sqlx::query("DELETE FROM users WHERE id=?")
        .bind(target)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    if result.rows_affected() != 1 {
        return Err((StatusCode::NOT_FOUND, "user not found"));
    }
    ensure_admin_exists(&mut tx).await?;
    audit(
        &mut tx,
        Some(actor),
        "USER_DELETED",
        "USER",
        Some(target),
        "{}",
    )
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
    let (_, _, csrf) = guard(&session, &["ADMIN"]).await?;
    let body = match kind.as_str() {
        "authorizations" => authorizations(&s, &csrf).await?,
        "coupons" => coupons(&s).await?,
        "audit" => audit_log(&s).await?,
        _ => return Err((StatusCode::NOT_FOUND, "not found")),
    };
    Ok(admin_page("Admin", &kind, &csrf, &body).into_response())
}

async fn authorizations(s: &AppState, csrf: &str) -> Result<String, (StatusCode, &'static str)> {
    let rows = sqlx::query("SELECT a.id,a.kind,a.client_mac,a.authorized_at,a.expires_at,u.email,u.display_name,c.code_suffix,c.template_name FROM device_authorizations a LEFT JOIN users u ON u.id=a.user_id LEFT JOIN coupons c ON c.id=a.coupon_id WHERE a.status='ACTIVE' AND a.expires_at>unixepoch() ORDER BY a.expires_at")
        .fetch_all(&s.pool).await.map_err(db)?;
    let mut b = format!(
        r#"<div class="page-head" data-motion><div><p class="eyebrow">Live network</p><h1>Active access.</h1><p>People and devices online now.</p></div><span class="badge">{} active</span></div>"#,
        rows.len()
    );
    if rows.is_empty() {
        b.push_str("<div class=\"empty\" data-motion><h2>No active authorizations.</h2></div>");
        return Ok(b);
    }
    b.push_str("<div class=\"table-wrap\" data-motion><table><thead><tr><th>User / voucher</th><th>Device</th><th>Type</th><th>Authorized</th><th>Expires</th><th>Action</th></tr></thead><tbody>");
    for r in rows {
        let identity = r
            .try_get::<Option<String>, _>("display_name")
            .ok()
            .flatten()
            .filter(|v| !v.is_empty())
            .or_else(|| r.try_get::<Option<String>, _>("email").ok().flatten())
            .unwrap_or_else(|| {
                format!(
                    "{} · ending {}",
                    r.try_get::<Option<String>, _>("template_name")
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| "Voucher".into()),
                    r.try_get::<Option<String>, _>("code_suffix")
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| "—".into())
                )
            });
        let id: i64 = r.get("id");
        let authorized: Option<i64> = r.get("authorized_at");
        let expires: i64 = r.get("expires_at");
        b.push_str(&format!(r#"<tr><td><strong>{}</strong></td><td><code>{}</code></td><td><span class="badge">{}</span></td><td>{}</td><td><time data-unix="{expires}">{expires}</time></td><td><form method="post" action="/admin/authorizations/{id}/revoke" data-confirm="Disconnect this device now?"><input type="hidden" name="csrf" value="{csrf}"><button class="danger">Disconnect</button></form></td></tr>"#, crate::html(&identity), crate::html(r.get("client_mac")), crate::html(r.get("kind")), authorized.map(|v| format!("<time data-unix=\"{v}\">{v}</time>")).unwrap_or_else(|| "Pending".into())));
    }
    b.push_str("</tbody></table></div>");
    Ok(b)
}

async fn coupons(s: &AppState) -> Result<String, (StatusCode, &'static str)> {
    let rows = sqlx::query("SELECT template_name,code_suffix,issued_at,expires_at,revoked_at FROM coupons ORDER BY issued_at DESC LIMIT 100").fetch_all(&s.pool).await.map_err(db)?;
    let mut b = format!(
        "<div class=\"page-head\" data-motion><div><p class=\"eyebrow\">Guest access</p><h1>Coupons.</h1></div><span class=\"badge\">{} shown</span></div><div class=\"table-wrap\" data-motion><table><thead><tr><th>Template</th><th>Code</th><th>Issued</th><th>Expires</th><th>Status</th></tr></thead><tbody>",
        rows.len()
    );
    for r in rows {
        let revoked: Option<i64> = r.get("revoked_at");
        let issued: i64 = r.get("issued_at");
        let expires: i64 = r.get("expires_at");
        b.push_str(&format!("<tr><td>{}</td><td>••••-{}</td><td><time data-unix=\"{issued}\">{issued}</time></td><td><time data-unix=\"{expires}\">{expires}</time></td><td><span class=\"badge{}\">{}</span></td></tr>", crate::html(r.get("template_name")), crate::html(r.get("code_suffix")), if revoked.is_some() { " off" } else { "" }, if revoked.is_some() { "Revoked" } else { "Issued" }));
    }
    b.push_str("</tbody></table></div>");
    Ok(b)
}

async fn audit_log(s: &AppState) -> Result<String, (StatusCode, &'static str)> {
    let rows = sqlx::query("SELECT event_type,target_type,target_id,created_at FROM audit_events ORDER BY created_at DESC LIMIT 100").fetch_all(&s.pool).await.map_err(db)?;
    let mut b = String::from(
        "<div class=\"page-head\" data-motion><div><p class=\"eyebrow\">Accountability</p><h1>Audit log.</h1></div></div><div class=\"table-wrap\" data-motion><table><thead><tr><th>Event</th><th>Target</th><th>Time</th></tr></thead><tbody>",
    );
    for r in rows {
        let at: i64 = r.get("created_at");
        let id: Option<i64> = r.get("target_id");
        b.push_str(&format!("<tr><td><strong>{}</strong></td><td>{} {}</td><td><time data-unix=\"{at}\">{at}</time></td></tr>", crate::html(r.get("event_type")), crate::html(r.get("target_type")), id.map(|v| format!("#{v}")).unwrap_or_default()));
    }
    b.push_str("</tbody></table></div>");
    Ok(b)
}

pub(crate) fn admin_page(
    title: &str,
    active: &str,
    csrf: &str,
    body: &str,
) -> axum::response::Html<String> {
    let links = [
        ("manage", "/manage", "Issue coupon"),
        ("users", "/admin/users", "Users"),
        ("templates", "/admin/templates", "Templates"),
        ("coupons", "/admin/coupons", "Coupons"),
        ("authorizations", "/admin/authorizations", "Active access"),
        ("audit", "/admin/audit", "Audit"),
        ("diagnostics", "/admin/diagnostics", "Diagnostics"),
    ]
    .map(|(key, href, label)| {
        format!(
            r#"<a href="{href}"{}>{label}</a>"#,
            if key == active {
                " aria-current=\"page\""
            } else {
                ""
            }
        )
    })
    .join("");
    axum::response::Html(format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{} · SEN Gateway</title><link rel="stylesheet" href="/static/app.css">{}</head><body><header><a class="brand" href="/"><span class="brand-mark">SEN</span> Gateway</a></header><div class="admin-shell"><aside class="sidebar"><div class="sidebar-title">Control room</div><nav aria-label="Administration">{links}</nav><div class="sidebar-footer"><small>Approved administrator</small><form method="post" action="/logout"><input type="hidden" name="csrf" value="{csrf}"><button>Sign out</button></form></div></aside><main class="admin-main">{body}</main></div></body></html>"#,
        crate::html(title),
        crate::scripts()
    ))
}

async fn ensure_admin_exists(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<(), (StatusCode, &'static str)> {
    let admins: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role='ADMIN' AND approved=1")
            .fetch_one(&mut **tx)
            .await
            .map_err(db)?;
    if admins < 1 {
        Err((StatusCode::CONFLICT, "cannot remove last approved admin"))
    } else {
        Ok(())
    }
}

pub(crate) async fn guard(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_form_targets_stable_user_id() {
        let html = user_form(Some(42), "admin@example.com", "ADMIN", true, 3, "token");
        assert!(html.contains("name=\"id\" value=\"42\""));
        assert!(html.contains("<option selected>ADMIN</option>"));
        assert!(html.contains("Update user"));
    }

    #[test]
    fn active_authorization_page_has_disconnect_control() {
        let html = admin_page(
            "Access",
            "authorizations",
            "token",
            "<button>Disconnect</button>",
        )
        .0;
        assert!(html.contains("aria-current=\"page\">Active access"));
        assert!(html.contains("<button>Disconnect</button>"));
    }
}
