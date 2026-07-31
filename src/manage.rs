use crate::{
    AppState, WebResult, audit, crypto,
    model::{generate_coupon, validity_minutes},
};
use axum::{
    Form,
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
};
use secrecy::{ExposeSecret, SecretString};
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
    unlimited_devices: Option<String>,
    never_expires: Option<String>,
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
    quantity: u8,
}
#[derive(Deserialize)]
pub struct DeleteUserForm {
    csrf: String,
}

const MAX_LOGO_BYTES: usize = 1024 * 1024;

fn logo_content_type(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

pub async fn home(State(s): State<AppState>, session: Session) -> WebResult {
    let (_, role, csrf) = guard(&session, &["ADMIN", "FRONT_DESK"]).await?;
    let rows = sqlx::query("SELECT id,name,device_limit,validity_minutes,unlimited_devices,never_expires FROM coupon_templates WHERE active=1 ORDER BY name")
        .fetch_all(&s.pool).await.map_err(db)?;
    let mut body = String::from(
        r#"<div class="page-head" data-motion><div><p class="eyebrow">Front desk</p><h1>Issue access.</h1><p>Create up to 20 guest Wi-Fi coupons at once.</p></div></div><section class="search-panel" data-motion><label for="template-search">Search templates<input id="template-search" type="search" data-template-search placeholder="Name, devices, or validity"></label></section><div class="stack" data-template-list>"#,
    );
    for r in rows {
        let name: String = r.get("name");
        let devices = if r.get::<i64, _>("unlimited_devices") == 1 {
            "Unlimited devices".into()
        } else {
            format!("{} devices", r.get::<i64, _>("device_limit"))
        };
        let validity = if r.get::<i64, _>("never_expires") == 1 {
            "Never expires".into()
        } else {
            format!("{} minutes", r.get::<i64, _>("validity_minutes"))
        };
        body.push_str(&format!(r#"<section class="card" data-motion data-template-card data-search="{} {} {}"><h2>{}</h2><p><strong>{devices}</strong> · {validity}</p><form class="form-grid" method="post" action="/manage/coupons/issue"><input type="hidden" name="csrf" value="{csrf}"><input type="hidden" name="template_id" value="{}"><label>Quantity<input type="number" name="quantity" min="1" max="20" value="1" required></label><label>Operator note<input maxlength="120" name="note" placeholder="Optional"></label><button>Generate coupons</button></form></section>"#, crate::html(&name.to_lowercase()), crate::html(&devices.to_lowercase()), crate::html(&validity.to_lowercase()), crate::html(&name), r.get::<i64,_>("id")));
    }
    body.push_str(r#"</div><div class="empty" data-template-empty hidden><h2>No matching templates.</h2></div>"#);
    Ok(if role == "ADMIN" {
        admin_page("Management", "manage", &csrf, &body)
    } else {
        operator_page("Management", &csrf, &body)
    }
    .into_response())
}

pub async fn templates(State(s): State<AppState>, session: Session) -> WebResult {
    let (_, _, csrf) = guard(&session, &["ADMIN"]).await?;
    let rows = sqlx::query("SELECT id,name,device_limit,validity_minutes,active,unlimited_devices,never_expires FROM coupon_templates ORDER BY active DESC,name")
        .fetch_all(&s.pool).await.map_err(db)?;
    let mut b = format!(
        r#"<div class="page-head" data-motion><div><p class="eyebrow">Access rules</p><h1>Templates.</h1></div></div><section data-motion><h2>New template</h2><p class="hint">Unlimited or never-expiring access stays valid until explicitly revoked.</p><form class="form-grid" method="post"><input type="hidden" name="csrf" value="{csrf}"><label>Name<input maxlength="80" name="name" required></label><label>Devices<input type="number" min="1" max="100" name="device_limit" value="1" required></label><label>Validity<input type="number" min="1" max="52" name="validity_value" value="1" required></label><label>Unit<select name="unit"><option>HOURS</option><option>DAYS</option><option>WEEKS</option></select></label><label class="check-label"><input type="checkbox" name="unlimited_devices" value="1"> Unlimited devices</label><label class="check-label"><input type="checkbox" name="never_expires" value="1"> Never expires</label><button>Create template</button></form></section><div class="table-wrap" data-motion><table><thead><tr><th>Name</th><th>Devices</th><th>Validity</th><th>Status</th></tr></thead><tbody>"#
    );
    for r in rows {
        let active = r.get::<i64, _>("active") == 1;
        let devices = if r.get::<i64, _>("unlimited_devices") == 1 {
            "Unlimited".into()
        } else {
            r.get::<i64, _>("device_limit").to_string()
        };
        let validity = if r.get::<i64, _>("never_expires") == 1 {
            "Never expires".into()
        } else {
            format!("{} minutes", r.get::<i64, _>("validity_minutes"))
        };
        b.push_str(&format!("<tr><td><strong>{}</strong></td><td>{devices}</td><td>{validity}</td><td><span class=\"badge{}\">{}</span></td></tr>", crate::html(r.get("name")), if active { "" } else { " off" }, if active { "Active" } else { "Inactive" }));
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
    let unlimited_devices = i64::from(f.unlimited_devices.is_some());
    let never_expires = i64::from(f.never_expires.is_some());
    let result = sqlx::query("INSERT INTO coupon_templates(name,device_limit,validity_minutes,active,created_at,updated_at,unlimited_devices,never_expires) VALUES(?,?,?,1,?,?,?,?)")
        .bind(f.name.trim()).bind(f.device_limit).bind(minutes).bind(now).bind(now).bind(unlimited_devices).bind(never_expires).execute(&mut *tx).await
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

fn valid_quantity(quantity: u8) -> bool {
    (1..=20).contains(&quantity)
}

pub async fn issue(
    State(s): State<AppState>,
    session: Session,
    Form(f): Form<IssueForm>,
) -> WebResult {
    let (id, role, _) = guard_csrf(&session, &f.csrf, &["ADMIN", "FRONT_DESK"]).await?;
    if f.note.as_deref().is_some_and(|n| n.chars().count() > 120) {
        return Err((StatusCode::BAD_REQUEST, "note too long"));
    }
    if !valid_quantity(f.quantity) {
        return Err((StatusCode::BAD_REQUEST, "quantity must be between 1 and 20"));
    }
    let t = sqlx::query(
        "SELECT name,device_limit,validity_minutes,unlimited_devices,never_expires FROM coupon_templates WHERE id=? AND active=1",
    )
    .bind(f.template_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(db)?
    .ok_or((StatusCode::BAD_REQUEST, "template unavailable"))?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let never_expires = t.get::<i64, _>("never_expires") == 1;
    let expires = if never_expires {
        i64::MAX
    } else {
        now + t.get::<i64, _>("validity_minutes") * 60
    };
    let unlimited_devices = t.get::<i64, _>("unlimited_devices");
    let mut tx = s.pool.begin().await.map_err(db)?;
    let mut codes = Vec::with_capacity(usize::from(f.quantity));
    for _ in 0..f.quantity {
        let (code, hash, suffix) = generate_coupon();
        let (ciphertext, nonce) =
            crypto::encrypt(&s.config.encryption_key, &SecretString::from(code.clone())).map_err(
                |_| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "coupon encryption failed",
                    )
                },
            )?;
        let result = sqlx::query("INSERT INTO coupons(code_hash,code_suffix,template_id,template_name,device_limit,validity_minutes,issued_by_user_id,issued_at,expires_at,note,code_ciphertext,code_nonce,unlimited_devices,never_expires) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(hash.as_slice()).bind(suffix).bind(f.template_id).bind(t.get::<String,_>("name")).bind(t.get::<i64,_>("device_limit")).bind(t.get::<i64,_>("validity_minutes")).bind(id).bind(now).bind(expires).bind(f.note.as_deref()).bind(ciphertext).bind(nonce.as_slice()).bind(unlimited_devices).bind(i64::from(never_expires)).execute(&mut *tx).await.map_err(db)?;
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
        codes.push(code);
    }
    tx.commit().await.map_err(db)?;
    let policy = format!(
        "{} · {}",
        if unlimited_devices == 1 {
            "Unlimited devices".into()
        } else {
            format!("{} devices", t.get::<i64, _>("device_limit"))
        },
        if never_expires {
            "Never expires".into()
        } else {
            format!("Expires <time data-unix=\"{expires}\">{expires}</time>")
        }
    );
    let cards = codes.iter().map(|code| format!(r#"<section class="coupon" data-motion><p class="eyebrow">Ready to use</p><h2>{}</h2><p class="code">{}</p><p>{policy}</p></section>"#, crate::html(t.get("name")), crate::html(code))).collect::<String>();
    let body = format!(
        r#"<div class="page-head no-print" data-motion><div><p class="eyebrow">Batch complete</p><h1>{} coupon{}.</h1><p>Codes remain available on Coupons page.</p></div><div class="actions"><a class="button secondary" href="/manage">Issue more</a><button type="button" data-print>Print</button></div></div><div class="coupon-grid">{cards}</div>"#,
        f.quantity,
        if f.quantity == 1 { "" } else { "s" }
    );
    Ok(if role == "ADMIN" {
        admin_page("Coupons issued", "manage", &f.csrf, &body)
    } else {
        operator_page("Coupons issued", &f.csrf, &body)
    }
    .into_response())
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

pub async fn branding(State(s): State<AppState>, session: Session) -> WebResult {
    let (_, _, csrf) = guard(&session, &["ADMIN"]).await?;
    let has_logo =
        sqlx::query_scalar::<_, i64>("SELECT logo_data IS NOT NULL FROM settings WHERE id=1")
            .fetch_one(&s.pool)
            .await
            .map_err(db)?
            != 0;
    let preview = if has_logo {
        r#"<img class="sen-logo brand-preview" src="/brand/logo" alt="Current organization logo">"#
    } else {
        r#"<div class="empty"><strong>No logo configured.</strong><p>Landing page uses text branding only.</p></div>"#
    };
    let remove = if has_logo {
        format!(
            r#"<form method="post" action="/admin/branding/remove" data-confirm="Remove current logo?"><input type="hidden" name="csrf" value="{csrf}"><button class="danger">Remove logo</button></form>"#
        )
    } else {
        String::new()
    };
    let body = format!(
        r#"<div class="page-head" data-motion><div><p class="eyebrow">Appearance</p><h1>Branding.</h1><p>Logo appears above guest Wi-Fi form.</p></div></div><section data-motion><h2>Current logo</h2>{preview}</section><section data-motion><h2>Upload logo</h2><p class="hint">PNG, JPEG, or WebP. Maximum 1 MiB.</p><form method="post" enctype="multipart/form-data"><input type="hidden" name="csrf" value="{csrf}"><label>Logo file<input type="file" name="logo" accept="image/png,image/jpeg,image/webp" required></label><div class="actions"><button>Upload logo</button>{remove}</div></form></section>"#
    );
    Ok(admin_page("Branding", "branding", &csrf, &body).into_response())
}

pub async fn save_branding(
    State(s): State<AppState>,
    session: Session,
    mut form: Multipart,
) -> WebResult {
    let mut csrf = None;
    let mut logo = None;
    while let Some(field) = form
        .next_field()
        .await
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid upload"))?
    {
        match field.name() {
            Some("csrf") => {
                csrf = Some(
                    field
                        .text()
                        .await
                        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid CSRF token"))?,
                )
            }
            Some("logo") => {
                logo = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid logo"))?
                        .to_vec(),
                )
            }
            _ => {}
        }
    }
    let csrf = csrf.ok_or((StatusCode::FORBIDDEN, "invalid CSRF token"))?;
    let (actor, _, _) = guard_csrf(&session, &csrf, &["ADMIN"]).await?;
    let logo = logo.ok_or((StatusCode::BAD_REQUEST, "logo required"))?;
    if logo.is_empty() || logo.len() > MAX_LOGO_BYTES {
        return Err((
            StatusCode::BAD_REQUEST,
            "logo must be between 1 byte and 1 MiB",
        ));
    }
    let content_type = logo_content_type(&logo)
        .ok_or((StatusCode::BAD_REQUEST, "logo must be PNG, JPEG, or WebP"))?;
    let mut tx = s.pool.begin().await.map_err(db)?;
    sqlx::query("UPDATE settings SET logo_content_type=?,logo_data=? WHERE id=1")
        .bind(content_type)
        .bind(logo)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    audit(
        &mut tx,
        Some(actor),
        "BRAND_LOGO_UPDATED",
        "SETTINGS",
        Some(1),
        "{}",
    )
    .await
    .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(Redirect::to("/admin/branding").into_response())
}

pub async fn remove_branding(
    State(s): State<AppState>,
    session: Session,
    Form(f): Form<DeleteUserForm>,
) -> WebResult {
    let (actor, _, _) = guard_csrf(&session, &f.csrf, &["ADMIN"]).await?;
    let mut tx = s.pool.begin().await.map_err(db)?;
    sqlx::query("UPDATE settings SET logo_content_type=NULL,logo_data=NULL WHERE id=1")
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    audit(
        &mut tx,
        Some(actor),
        "BRAND_LOGO_REMOVED",
        "SETTINGS",
        Some(1),
        "{}",
    )
    .await
    .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(Redirect::to("/admin/branding").into_response())
}

pub async fn simple(
    State(s): State<AppState>,
    session: Session,
    Path(kind): Path<String>,
) -> WebResult {
    let (_, _, csrf) = guard(&session, &["ADMIN"]).await?;
    let body = match kind.as_str() {
        "authorizations" => authorizations(&s, &csrf).await?,
        "coupons" => coupons(&s, &csrf).await?,
        "audit" => audit_log(&s).await?,
        _ => return Err((StatusCode::NOT_FOUND, "not found")),
    };
    Ok(admin_page("Admin", &kind, &csrf, &body).into_response())
}

async fn authorizations(s: &AppState, csrf: &str) -> Result<String, (StatusCode, &'static str)> {
    let rows = sqlx::query("SELECT a.id,a.kind,a.client_mac,a.authorized_at,a.expires_at,u.email,u.display_name,c.code_suffix,c.template_name,c.never_expires FROM device_authorizations a LEFT JOIN users u ON u.id=a.user_id LEFT JOIN coupons c ON c.id=a.coupon_id WHERE a.status='ACTIVE' AND a.expires_at>unixepoch() ORDER BY a.expires_at")
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
        let expiry = if r.try_get::<Option<i64>, _>("never_expires").ok().flatten() == Some(1) {
            "Never expires".into()
        } else {
            format!("<time data-unix=\"{expires}\">{expires}</time>")
        };
        b.push_str(&format!(r#"<tr><td><strong>{}</strong></td><td><code>{}</code></td><td><span class="badge">{}</span></td><td>{}</td><td>{expiry}</td><td><form method="post" action="/admin/authorizations/{id}/revoke" data-confirm="Disconnect this device now?"><input type="hidden" name="csrf" value="{csrf}"><button class="danger">Disconnect</button></form></td></tr>"#, crate::html(&identity), crate::html(r.get("client_mac")), crate::html(r.get("kind")), authorized.map(|v| format!("<time data-unix=\"{v}\">{v}</time>")).unwrap_or_else(|| "Pending".into())));
    }
    b.push_str("</tbody></table></div>");
    Ok(b)
}

async fn coupons(s: &AppState, csrf: &str) -> Result<String, (StatusCode, &'static str)> {
    let rows = sqlx::query("SELECT id,template_name,code_suffix,code_ciphertext,code_nonce,issued_at,expires_at,revoked_at,unlimited_devices,never_expires FROM coupons ORDER BY issued_at DESC LIMIT 100").fetch_all(&s.pool).await.map_err(db)?;
    let mut b = format!(
        "<div class=\"page-head\" data-motion><div><p class=\"eyebrow\">Guest access</p><h1>Coupons.</h1><p>Full codes available for coupons issued after secure storage upgrade.</p></div><span class=\"badge\">{} shown</span></div><div class=\"table-wrap\" data-motion><table><thead><tr><th>Template</th><th>Code</th><th>Policy</th><th>Issued</th><th>Status</th><th>Action</th></tr></thead><tbody>",
        rows.len()
    );
    for r in rows {
        let id: i64 = r.get("id");
        let revoked: Option<i64> = r.get("revoked_at");
        let issued: i64 = r.get("issued_at");
        let expires: i64 = r.get("expires_at");
        let ciphertext: Option<Vec<u8>> = r.get("code_ciphertext");
        let nonce: Option<Vec<u8>> = r.get("code_nonce");
        let code = match (ciphertext, nonce) {
            (Some(ciphertext), Some(nonce)) => {
                crypto::decrypt(&s.config.encryption_key, &ciphertext, &nonce)
                    .map(|value| value.expose_secret().to_owned())
                    .unwrap_or_else(|_| "Unavailable".into())
            }
            _ => format!("••••-{}", r.get::<String, _>("code_suffix")),
        };
        let policy = format!(
            "{} · {}",
            if r.get::<i64, _>("unlimited_devices") == 1 {
                String::from("Unlimited devices")
            } else {
                String::from("Limited devices")
            },
            if r.get::<i64, _>("never_expires") == 1 {
                String::from("Never expires")
            } else {
                format!("until <time data-unix=\"{expires}\">{expires}</time>")
            }
        );
        let action = if revoked.is_none() {
            format!(
                r#"<form method="post" action="/admin/coupons/{id}/revoke" data-confirm="Revoke this coupon and disconnect its active devices?"><input type="hidden" name="csrf" value="{csrf}"><button class="danger">Revoke</button></form>"#
            )
        } else {
            String::new()
        };
        b.push_str(&format!("<tr><td>{}</td><td><code>{}</code></td><td>{policy}</td><td><time data-unix=\"{issued}\">{issued}</time></td><td><span class=\"badge{}\">{}</span></td><td>{action}</td></tr>", crate::html(r.get("template_name")), crate::html(&code), if revoked.is_some() { " off" } else { "" }, if revoked.is_some() { "Revoked" } else { "Issued" }));
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
        ("branding", "/admin/branding", "Branding"),
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
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{} · SEN Gateway</title><link rel="stylesheet" href="/static/app.css">{}</head><body><header><a class="brand" href="/"><span class="brand-mark">SEN</span> Gateway</a><form class="header-logout" method="post" action="/logout"><input type="hidden" name="csrf" value="{csrf}"><button>Sign out</button></form></header><div class="admin-shell"><aside class="sidebar"><div class="sidebar-title">Control room</div><nav aria-label="Administration">{links}</nav></aside><main class="admin-main">{body}</main></div></body></html>"#,
        crate::html(title),
        crate::scripts()
    ))
}
fn operator_page(title: &str, csrf: &str, body: &str) -> axum::response::Html<String> {
    axum::response::Html(format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{} · SEN Gateway</title><link rel="stylesheet" href="/static/app.css">{}</head><body><header><a class="brand" href="/"><span class="brand-mark">SEN</span> Gateway</a><form class="header-logout" method="post" action="/logout"><input type="hidden" name="csrf" value="{csrf}"><button>Sign out</button></form></header><div class="admin-shell"><aside class="sidebar"><div class="sidebar-title">Front desk</div><nav aria-label="Management"><a href="/manage" aria-current="page">Issue coupon</a></nav></aside><main class="admin-main">{body}</main></div></body></html>"#,
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

    #[test]
    fn coupon_batch_bounds_and_admin_shell_contract() {
        assert!(valid_quantity(1));
        assert!(valid_quantity(20));
        assert!(!valid_quantity(0));
        assert!(!valid_quantity(21));
        let html = admin_page(
            "Issued",
            "manage",
            "token",
            r#"<button data-print>Print</button>"#,
        )
        .0;
        assert!(html.contains("aria-current=\"page\">Issue coupon"));
        assert!(html.contains("class=\"header-logout\""));
        assert!(html.contains("data-print"));
        assert!(!html.contains("onclick="));
    }

    #[test]
    fn logo_type_uses_file_signature() {
        assert_eq!(
            logo_content_type(b"\x89PNG\r\n\x1a\nrest"),
            Some("image/png")
        );
        assert_eq!(
            logo_content_type(&[0xff, 0xd8, 0xff, 0]),
            Some("image/jpeg")
        );
        assert_eq!(logo_content_type(b"RIFF1234WEBPrest"), Some("image/webp"));
        assert_eq!(logo_content_type(b"<svg></svg>"), None);
    }
}
