PRAGMA foreign_keys = ON;

CREATE TABLE settings (
 id INTEGER PRIMARY KEY CHECK (id = 1),
 public_base_url TEXT NOT NULL,
 google_client_id TEXT NOT NULL,
 google_client_secret_ciphertext BLOB NOT NULL,
 google_client_secret_nonce BLOB NOT NULL CHECK(length(google_client_secret_nonce) = 12),
 google_oauth_version TEXT NOT NULL CHECK(google_oauth_version = 'v2'),
 google_workspace_domain TEXT NOT NULL,
 unifi_network_api_url TEXT NOT NULL,
 unifi_api_key_ciphertext BLOB NOT NULL,
 unifi_api_key_nonce BLOB NOT NULL CHECK(length(unifi_api_key_nonce) = 12),
 unifi_site_id TEXT NOT NULL,
 staff_session_minutes INTEGER NOT NULL DEFAULT 480 CHECK(staff_session_minutes >= 1),
 setup_completed_at INTEGER NOT NULL
);
CREATE TABLE users (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 google_sub TEXT UNIQUE,
 email TEXT NOT NULL UNIQUE COLLATE NOCASE,
 display_name TEXT,
 role TEXT NOT NULL CHECK(role IN ('ADMIN','FRONT_DESK','STAFF')),
 approved INTEGER NOT NULL CHECK(approved IN (0,1)),
 device_limit INTEGER NOT NULL CHECK(device_limit >= 1),
 created_at INTEGER NOT NULL,
 updated_at INTEGER NOT NULL
);
CREATE TABLE coupon_templates (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 name TEXT NOT NULL UNIQUE,
 device_limit INTEGER NOT NULL CHECK(device_limit BETWEEN 1 AND 100),
 validity_minutes INTEGER NOT NULL CHECK(validity_minutes BETWEEN 60 AND 524160),
 active INTEGER NOT NULL CHECK(active IN (0,1)),
 created_at INTEGER NOT NULL,
 updated_at INTEGER NOT NULL
);
CREATE TABLE coupons (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 code_hash BLOB NOT NULL UNIQUE CHECK(length(code_hash)=32),
 code_suffix TEXT NOT NULL,
 template_id INTEGER NOT NULL REFERENCES coupon_templates(id),
 template_name TEXT NOT NULL,
 device_limit INTEGER NOT NULL,
 validity_minutes INTEGER NOT NULL,
 issued_by_user_id INTEGER NOT NULL REFERENCES users(id),
 issued_at INTEGER NOT NULL,
 expires_at INTEGER NOT NULL,
 revoked_at INTEGER,
 note TEXT CHECK(note IS NULL OR length(note) <= 120)
);
CREATE TABLE device_authorizations (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 kind TEXT NOT NULL CHECK(kind IN ('STAFF','COUPON')),
 user_id INTEGER REFERENCES users(id),
 coupon_id INTEGER REFERENCES coupons(id),
 client_mac TEXT NOT NULL,
 unifi_client_id TEXT NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('PENDING','ACTIVE','REVOKED','FAILED')),
 authorized_at INTEGER,
 expires_at INTEGER NOT NULL,
 revoked_at INTEGER,
 revoke_reason TEXT,
 failure_message TEXT,
 created_at INTEGER NOT NULL,
 CHECK((kind='STAFF' AND user_id IS NOT NULL AND coupon_id IS NULL) OR (kind='COUPON' AND coupon_id IS NOT NULL AND user_id IS NULL))
);
CREATE UNIQUE INDEX active_staff_device ON device_authorizations(user_id,client_mac) WHERE kind='STAFF' AND status IN ('PENDING','ACTIVE');
CREATE UNIQUE INDEX active_coupon_device ON device_authorizations(coupon_id,client_mac) WHERE kind='COUPON' AND status IN ('PENDING','ACTIVE');
CREATE INDEX authorizations_expiry ON device_authorizations(status,expires_at);
CREATE TABLE audit_events (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 actor_user_id INTEGER REFERENCES users(id),
 event_type TEXT NOT NULL,
 target_type TEXT NOT NULL,
 target_id INTEGER,
 details_json TEXT NOT NULL CHECK(json_valid(details_json)),
 created_at INTEGER NOT NULL
);
CREATE INDEX audit_created ON audit_events(created_at DESC);
