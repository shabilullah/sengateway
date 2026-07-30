CREATE TABLE oauth_attempts (
 state TEXT PRIMARY KEY,
 pkce TEXT NOT NULL,
 nonce TEXT NOT NULL,
 intent TEXT NOT NULL CHECK(intent IN ('PORTAL','MANAGEMENT')),
 portal_context_json TEXT CHECK(portal_context_json IS NULL OR json_valid(portal_context_json)),
 expires_at INTEGER NOT NULL
);
CREATE INDEX oauth_attempts_expiry ON oauth_attempts(expires_at);
