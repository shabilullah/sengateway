# Sen Gateway

Rust captive-portal gateway for UniFi Network. Front-desk users issue guest coupons, approved staff authenticate through Google Workspace, and administrators manage access policy and audit records.

## Requirements

### Development

- Rust stable with Cargo
- SQLite support supplied by `sqlx`
- Caddy for browser testing through HTTPS
- Reachable UniFi Network Application 10.4.57 or newer for setup and end-to-end access tests
- Google Workspace Web OAuth client for login tests

### Deployment

- Linux host with Podman and a Compose provider (`podman-compose` or Docker Compose plugin)
- Private on-site IPv4 address for portal
- Cloudflare-managed DNS zone
- Google Workspace Web OAuth client
- Self-hosted UniFi Network Application 10.4.57 or newer

## Development

### 1. Create local environment

Generate secrets:

```sh
openssl rand -base64 48
openssl rand -base64 32
```

First output becomes `SESSION_SECRET`. Second output becomes `SETUP_ENCRYPTION_KEY`; it must decode to exactly 32 bytes.

Create local database directory and export environment variables. Bash example:

```sh
mkdir -p .data
export PUBLIC_BASE_URL=https://localhost:8443
export DATABASE_URL='sqlite:.data/gateway.db?mode=rwc'
export SESSION_SECRET='<first-generated-value>'
export SETUP_ENCRYPTION_KEY='<second-generated-value>'
export COOKIE_SECURE=true
export TRUSTED_PROXY_IP=127.0.0.1
export RUST_LOG=sengateway=info
```

PowerShell example:

```powershell
New-Item -ItemType Directory -Force .data | Out-Null
$env:PUBLIC_BASE_URL = 'https://localhost:8443'
$env:DATABASE_URL = 'sqlite:.data/gateway.db?mode=rwc'
$env:SESSION_SECRET = '<first-generated-value>'
$env:SETUP_ENCRYPTION_KEY = '<second-generated-value>'
$env:COOKIE_SECURE = 'true'
$env:TRUSTED_PROXY_IP = '127.0.0.1'
$env:RUST_LOG = 'sengateway=info'
```

`PUBLIC_BASE_URL` must be an HTTPS origin. App intentionally rejects direct requests except `GET /healthz`; browser traffic must arrive through trusted HTTPS proxy.

### 2. Start app

```sh
cargo run
```

App listens on `0.0.0.0:8080`, applies embedded migrations, and prints one setup URL when database has no completed setup record.

In another terminal, start local HTTPS proxy:

```sh
caddy reverse-proxy --from https://localhost:8443 --to http://127.0.0.1:8080
```

Trust Caddy local CA when browser prompts. Open setup URL printed by app, replacing nothing; `PUBLIC_BASE_URL` already points to local proxy.

Setup validates UniFi site immediately. Use real development UniFi endpoint, API key, and site ID. Google callback URI must be registered exactly as:

```text
https://localhost:8443/auth/google/callback
```

Delete `.data/gateway.db` to reset local setup. This destroys all local settings, users, coupons, audit events, and authorizations.

### 3. Health and checks

Health endpoint bypasses proxy gate for Podman and local process monitoring:

```sh
curl --fail http://127.0.0.1:8080/healthz
```

Run required quality gates:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

Tests do not require live Google or UniFi credentials.

## Deployment

### 1. Configure network services

#### Cloudflare

1. Create DNS-only `A` record for portal hostname pointing to private `ORIGIN_BIND_IP`.
2. Do not configure Cloudflare Tunnel or public WAN forwarding.
3. Create dedicated API token restricted to portal zone with only `Zone:DNS:Edit` and `Zone:Zone:Read`.
4. Permit private DNS answer through DNS-rebinding protection, or create matching local DNS override.
5. Do not add `AAAA` unless local IPv6 routing and firewall are configured.

#### Google Cloud

1. Configure internal Google Workspace consent screen.
2. Create Web OAuth client.
3. Register exact callback URI:

```text
https://<PORTAL_HOSTNAME>/auth/google/callback
```

No Admin SDK, offline access, or refresh-token scope is needed.

#### UniFi

1. Upgrade Network Application to 10.4.57 or newer.
2. Generate API key under **Network > Control Plane > Integrations**.
3. Put captive SSID/network in Hotspot zone.
4. Set External Portal URL to:

```text
https://<PORTAL_HOSTNAME>/portal
```

5. Permit pre-authorization DNS, portal private IP on TCP/443, `accounts.google.com`, `oauth2.googleapis.com`, and Google static hosts observed during OAuth.
6. Block other LAN/private destinations from guest network.

Staff and guests may share one captive SSID. Separate SSIDs may point to same `/portal` endpoint; authorization never trusts SSID for role.

### 2. Configure and start with Compose

Deployment uses released multi-architecture GHCR images. No Rust toolchain or local image compilation is required:

```text
ghcr.io/shabilullah/sengateway:latest
ghcr.io/shabilullah/sengateway-caddy:latest
```

Create deployment environment from template:

```sh
cp .env.example .env
```

Set `PORTAL_HOSTNAME`, concrete private LAN `ORIGIN_BIND_IP`, restricted Cloudflare token, and generated secrets in `.env`. Generate secrets with:

```sh
openssl rand -base64 48
openssl rand -base64 32
```

First output becomes `SESSION_SECRET`. Second becomes `SETUP_ENCRYPTION_KEY` and must decode to exactly 32 bytes. Keep `.env` private; it contains credentials required to decrypt persisted settings and sign sessions.

Pull released images and start services from repository root:

```sh
docker compose pull
docker compose up -d
```

With Podman Compose, use equivalent commands:

```sh
podman compose pull
podman compose up -d
```

Compose creates app, Caddy, private `portal` network, and persistent `app-data`, `caddy-data`, and `caddy-config` volumes. App port 8080 remains internal. Caddy binds only `ORIGIN_BIND_IP` on TCP/80 and TCP/443, performs Cloudflare DNS-01, and proxies app traffic.

`ORIGIN_BIND_IP` must be concrete private LAN interface, never `0.0.0.0`. Host firewall must allow TCP/80 and TCP/443 only from organization and guest LANs. Do not expose TCP/8080 or create WAN NAT forwarding. Port 80 exists only for HTTPS redirect; ACME uses DNS-01.

If UniFi controller uses private CA, install CA certificate into app image trust store. Never disable TLS verification.

Expected startup:

- Caddy creates temporary `_acme-challenge` TXT record and obtains trusted certificate.
- App logs one setup URL.
- Port 8080 has no host listener.

Check status and obtain setup URL:

```sh
docker compose ps
docker compose logs app
docker compose logs caddy
curl --fail https://<PORTAL_HOSTNAME>/healthz
```

Open one-time setup URL from app logs. Enter initial administrator, Google v2 credentials, exact lowercase Workspace domain, full UniFi Network API URL, UniFi API key, and site ID. Token expires after 30 minutes and cannot be reused after setup commit.

### 3. Operational verification

From organization or guest LAN:

1. Confirm portal hostname resolves to private `ORIGIN_BIND_IP`.
2. Confirm browser trusts certificate.
3. Confirm `https://<PORTAL_HOSTNAME>/portal` loads from captive SSID.
4. Confirm portal is unreachable from Internet/off-site network.
5. Confirm first coupon device authorizes, over-limit device is denied, and admin revoke unauthorizes client.
6. Confirm second staff device replaces oldest device when staff limit is one.
7. Restart services and confirm setup does not reopen:

```sh
docker compose restart
docker compose logs app
```

### Updates and backups

SQLite state lives in Compose `app-data` volume. Back up volume before upgrades. Preserve `SETUP_ENCRYPTION_KEY`; replacing it makes encrypted Google and UniFi credentials unreadable.

Pull current released images and replace containers while preserving volumes:

```sh
docker compose pull
docker compose up -d
```

Pin deployment to immutable commit images by setting these values in `.env`:

```dotenv
APP_IMAGE=ghcr.io/shabilullah/sengateway:sha-<commit>
CADDY_IMAGE=ghcr.io/shabilullah/sengateway-caddy:sha-<commit>
```

Then apply pinned images:

```sh
docker compose pull
docker compose up -d
```

Tags matching `vMAJOR.MINOR.PATCH` publish semantic release images. Every push to `master` also publishes `latest` and immutable `sha-<commit>` tags for `linux/amd64` and `linux/arm64`.

Inspect failures:

```sh
docker compose logs --tail=200 app
docker compose logs --tail=200 caddy
```
