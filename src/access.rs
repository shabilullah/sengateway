use crate::{
    audit,
    model::{INVALID_COUPON, coupon_hash},
    unifi::{UnifiClient, UnifiError},
};
use sqlx::{Sqlite, SqlitePool, Transaction};
use time::OffsetDateTime;

pub async fn redeem_coupon(
    pool: &SqlitePool,
    unifi: &UnifiClient,
    code: &str,
    mac: &str,
) -> Result<(), &'static str> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let hash = coupon_hash(code);
    let mut tx = pool.begin().await.map_err(|_| "internal error")?;
    begin_immediate(&mut tx).await?;
    let coupon = sqlx::query_as::<_, (i64, i64, i64, Option<i64>, i64, i64)>(
        "SELECT id,device_limit,expires_at,revoked_at,unlimited_devices,never_expires FROM coupons WHERE code_hash=?",
    )
    .bind(hash.as_slice())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| "internal error")?
    .ok_or(INVALID_COUPON)?;
    if coupon.2 <= now || coupon.3.is_some() {
        return Err(INVALID_COUPON);
    }
    if sqlx::query_scalar::<_,i64>("SELECT EXISTS(SELECT 1 FROM device_authorizations WHERE coupon_id=? AND client_mac=? AND status='ACTIVE' AND expires_at>?)").bind(coupon.0).bind(mac).bind(now).fetch_one(&mut *tx).await.map_err(|_|"internal error")?!=0{return Ok(())}
    let count=sqlx::query_scalar::<_,i64>("SELECT COUNT(DISTINCT client_mac) FROM device_authorizations WHERE coupon_id=? AND status IN ('PENDING','ACTIVE') AND expires_at>?").bind(coupon.0).bind(now).fetch_one(&mut *tx).await.map_err(|_|"internal error")?;
    if coupon.4 == 0 && count >= coupon.1 {
        return Err(INVALID_COUPON);
    }
    let client = unifi
        .resolve_mac(mac)
        .await
        .map_err(|_| "Unable to contact network controller")?;
    let id=sqlx::query("INSERT INTO device_authorizations(kind,coupon_id,client_mac,unifi_client_id,status,expires_at,created_at) VALUES('COUPON',?,?,?,'PENDING',?,?)").bind(coupon.0).bind(mac).bind(&client.id).bind(coupon.2).bind(now).execute(&mut *tx).await.map_err(|_|INVALID_COUPON)?.last_insert_rowid();
    tx.commit().await.map_err(|_| "internal error")?;
    let minutes = if coupon.5 == 1 {
        None
    } else {
        Some(((coupon.2 - now + 59) / 60).max(1))
    };
    finalize(
        pool,
        unifi.authorize(&client.id, minutes).await,
        id,
        "COUPON_REDEEMED",
        coupon.0,
    )
    .await
}

pub async fn authorize_staff(
    pool: &SqlitePool,
    unifi: &UnifiClient,
    user_id: i64,
    limit: i64,
    minutes: i64,
    mac: &str,
) -> Result<(), &'static str> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let client = unifi
        .resolve_mac(mac)
        .await
        .map_err(|_| "Unable to contact network controller")?;
    let expires = now + minutes * 60;
    let mut tx = pool.begin().await.map_err(|_| "internal error")?;
    begin_immediate(&mut tx).await?;
    sqlx::query("UPDATE device_authorizations SET status='REVOKED',revoked_at=?,revoke_reason='EXPIRED_LOCAL' WHERE user_id=? AND status='ACTIVE' AND expires_at<=?").bind(now).bind(user_id).bind(now).execute(&mut *tx).await.map_err(|_|"internal error")?;
    if sqlx::query_scalar::<_,i64>("SELECT EXISTS(SELECT 1 FROM device_authorizations WHERE user_id=? AND client_mac=? AND status='ACTIVE' AND expires_at>?)").bind(user_id).bind(mac).bind(now).fetch_one(&mut *tx).await.map_err(|_|"internal error")?!=0{return Ok(())}
    let count=sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM device_authorizations WHERE user_id=? AND status IN ('PENDING','ACTIVE') AND expires_at>?").bind(user_id).bind(now).fetch_one(&mut *tx).await.map_err(|_|"internal error")?;
    let oldest = if count >= limit {
        sqlx::query_as::<_,(i64,String)>("SELECT id,unifi_client_id FROM device_authorizations WHERE user_id=? AND status='ACTIVE' ORDER BY authorized_at LIMIT 1").bind(user_id).fetch_optional(&mut *tx).await.map_err(|_|"internal error")?
    } else {
        None
    };
    let new_id=sqlx::query("INSERT INTO device_authorizations(kind,user_id,client_mac,unifi_client_id,status,expires_at,created_at) VALUES('STAFF',?,?,?,'PENDING',?,?)").bind(user_id).bind(mac).bind(&client.id).bind(expires).bind(now).execute(&mut *tx).await.map_err(|_|"Authorization in progress; retry shortly")?.last_insert_rowid();
    tx.commit().await.map_err(|_| "internal error")?;
    if let Some((old_id, old_client)) = oldest {
        if unifi.unauthorize(&old_client).await.is_err() {
            fail(pool, new_id, "replacement revoke failed").await;
            return Err("Unable to replace existing device");
        };
        let mut tx = pool.begin().await.map_err(|_| "internal error")?;
        sqlx::query("UPDATE device_authorizations SET status='REVOKED',revoked_at=?,revoke_reason='DEVICE_LIMIT_REPLACED' WHERE id=?").bind(now).bind(old_id).execute(&mut *tx).await.map_err(|_|"internal error")?;
        audit(
            &mut tx,
            Some(user_id),
            "DEVICE_LIMIT_REPLACED",
            "AUTHORIZATION",
            Some(old_id),
            "{}",
        )
        .await
        .map_err(|_| "internal error")?;
        tx.commit().await.map_err(|_| "internal error")?;
    }
    finalize(
        pool,
        unifi.authorize(&client.id, Some(minutes)).await,
        new_id,
        "STAFF_DEVICE_AUTHORIZED",
        user_id,
    )
    .await
}
async fn finalize(
    pool: &SqlitePool,
    result: Result<(), UnifiError>,
    id: i64,
    event: &str,
    target: i64,
) -> Result<(), &'static str> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    match result {
        Ok(()) => {
            let mut tx = pool.begin().await.map_err(|_| "internal error")?;
            sqlx::query(
                "UPDATE device_authorizations SET status='ACTIVE',authorized_at=? WHERE id=?",
            )
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|_| "internal error")?;
            audit(&mut tx, None, event, "AUTHORIZATION", Some(target), "{}")
                .await
                .map_err(|_| "internal error")?;
            tx.commit().await.map_err(|_| "internal error")?;
            Ok(())
        }
        Err(_) => {
            fail(pool, id, "UniFi authorization failed").await;
            Err("Network authorization failed; retry")
        }
    }
}
async fn fail(pool: &SqlitePool, id: i64, message: &str) {
    let _ = sqlx::query(
        "UPDATE device_authorizations SET status='FAILED',failure_message=? WHERE id=?",
    )
    .bind(message)
    .bind(id)
    .execute(pool)
    .await;
}
async fn begin_immediate(tx: &mut Transaction<'_, Sqlite>) -> Result<(), &'static str> {
    sqlx::query("PRAGMA busy_timeout=5000")
        .execute(&mut **tx)
        .await
        .map_err(|_| "internal error")?;
    Ok(())
}
