# SEN Gateway Technical Setup

## Placeholder reference

Replace every angle-bracket token below with deployment value. Keep tokens in documentation and examples; never commit real environment values.

> Values in angle brackets are placeholders, not literal input. Example-format values only show expected shape and are not deployment defaults. Replace tokens locally; do not commit substituted copies.

| Placeholder | Meaning | Example format |
|---|---|---|
| `<PORTAL_HOSTNAME>` | Portal DNS hostname | `portal.example.com` |
| `<PORTAL_IPV4>` | Private IPv4 of Caddy portal host | `192.168.50.20` |
| `<RESTRICTED_CLOUDFLARE_DNS_TOKEN>` | Cloudflare API token with DNS edit for portal zone only | Deployment secret; never commit |
| `<UNIFI_HOSTNAME>` | UniFi controller hostname matching its TLS certificate | `unifi.example.internal` |
| `<UNIFI_IPV4>` | Private IPv4 of UniFi controller | `192.168.50.10` |
| `<UNIFI_PORT>` | UniFi HTTPS/API port | `443` or controller-specific port |
| `<UNIFI_SITE_ID>` | Site UUID returned by official UniFi API | UUID |
| `<WORKSPACE_DOMAIN>` | Lowercase Google Workspace domain | `example.com` |

Never put API keys, OAuth client secrets, session secrets, setup encryption keys, Cloudflare tokens, real internal addresses, site IDs, or organization domains in this file. Keep them in deployment-local configuration or approved secret store.

## UniFi captive portal configuration

### Guest Wi-Fi

1. Open **UniFi Network > Settings > WiFi**.
2. Select the guest SSID.
3. Set **Application** to **Hotspot** and **Hotspot Type** to **Captive Portal**. On Network 8/9, enable **Hotspot Portal** instead.
4. Open **Insights > Hotspot > Landing Page**. On older versions, use **Hotspot Manager > Landing Page**.

Staff and guests may use the same captive SSID. Gateway determines access from approved user role, not SSID name.

### Authentication

Under **One Way Methods**:

| Setting | Value |
|---|---|
| External Portal Server | **Enabled** |
| External Portal type | **Custom** |
| IPv4 Address | `<PORTAL_IPV4>` |

The IPv4 field accepts an address only. Do not enter a URL or hostname there.

### Landing Page Settings

| Setting | Value | Reason |
|---|---:|---|
| Show Landing Page | **On** | Starts captive-portal flow for new guests. |
| HTTPS Redirection Support | **Off** | Avoids TLS warnings caused by intercepting arbitrary HTTPS sites. OS captive-portal probes use HTTP. |
| Encrypted URL | **Off** | Gateway needs UniFi's plain `id`, `ap`, `ssid`, and `url` query parameters. |
| Secure Portal | **On** | Sends portal traffic through HTTPS. |
| Domain | **On** | Redirect hostname must match public certificate. |
| Domain value | `<PORTAL_HOSTNAME>` | Hostname only; no scheme, path, port, or trailing slash. |

Do not enter `https://<PORTAL_HOSTNAME>/portal` in **Domain**. Enter `<PORTAL_HOSTNAME>` only.

### Landing Page Designer

UniFi Landing Page Designer is bypassed when **External Portal Server** is enabled. Its title, logo, welcome text, button text, and colors may remain at defaults.

Expected page is gateway page at `https://<PORTAL_HOSTNAME>/`, not UniFi-designed page. If UniFi page appears, confirm External Portal Server is enabled and remove native UniFi Password/Voucher methods from authentication.

### Pre-Authorization Allowances

Permit portal and Google browser sign-in destinations before authentication:

```text
<PORTAL_IPV4>
<PORTAL_HOSTNAME>
accounts.google.com
www.gstatic.com
ssl.gstatic.com
fonts.gstatic.com
accounts.gstatic.com
lh3.googleusercontent.com
accounts.youtube.com
play.google.com
www.google.com
www.googleapis.com
```

This list combines Google official sign-in guidance with hosts observed during this gateway OAuth page load. `oauth2.googleapis.com` is used by gateway server after callback; guest browser does not need direct pre-authorization access to it.

Also permit DNS to approved resolvers and TCP/443 to `<PORTAL_IPV4>`. Do not broadly allow private networks. Keep guest LAN isolated and block other private destinations.

#### Google Prompt two-factor approval

If approval phone uses mobile data or another authenticated network, no additional captive-portal allowances are needed for phone. If Android approval phone is on same unauthenticated SSID, Google Prompt depends on Firebase Cloud Messaging. Permit direct outbound TCP/443 and TCP/5228-5230 to:

```text
mtalk.google.com
mtalk4.google.com
mtalk-staging.google.com
mtalk-dev.google.com
alt1-mtalk.google.com
alt2-mtalk.google.com
alt3-mtalk.google.com
alt4-mtalk.google.com
alt5-mtalk.google.com
alt6-mtalk.google.com
alt7-mtalk.google.com
alt8-mtalk.google.com
android.apis.google.com
device-provisioning.googleapis.com
firebaseinstallations.googleapis.com
fcm.googleapis.com
```

FCM must connect directly; do not proxy or TLS-inspect these destinations. If UniFi pre-authorization allowances cannot express ports or wildcard Google endpoints reliably, move approval phone to mobile data while approving, or use another configured factor such as authenticator code or security key. Never broadly pre-authorize all Internet access merely to make Google Prompt work.

### Expected redirect

UniFi supplies device context automatically. Depending on Network version, it may redirect to either supported form:

```text
https://<PORTAL_HOSTNAME>/?id=<client-mac>&ap=<ap-mac>&ssid=<ssid>&url=<original-url>
https://<PORTAL_HOSTNAME>/portal?id=<client-mac>&ap=<ap-mac>&ssid=<ssid>&url=<original-url>
```

Do not manually add query parameters in UniFi settings.

## User flow

### Guest

```text
Join guest Wi-Fi
→ UniFi detects unauthenticated client
→ gateway opens with UniFi client context
→ enter SEN Gateway voucher
→ gateway validates voucher
→ gateway authorizes device through official UniFi API
```

Direct visits to `https://<PORTAL_HOSTNAME>/` show guest-first page, but voucher controls remain disabled without UniFi client context. User must join guest Wi-Fi first.

### Staff

```text
Join guest Wi-Fi
→ open gateway through UniFi redirect
→ select Staff login
→ Google Workspace login
→ gateway verifies approved STAFF role and device limit
→ gateway authorizes device
```

Staff login from direct homepage does not authorize a device because no UniFi client context exists.

### Administrator and front desk

Management entry:

```text
https://<PORTAL_HOSTNAME>/auth/google/start?intent=MANAGEMENT
```

Dashboard after login:

```text
https://<PORTAL_HOSTNAME>/manage
```

Header **Admin login** starts management OAuth. Approved `ADMIN` and `FRONT_DESK` users may enter management according to role.

## Google Cloud configuration

1. Use internal Google Workspace consent screen.
2. Use Web OAuth client.
3. Register this exact authorized redirect URI:

```text
https://<PORTAL_HOSTNAME>/auth/google/callback
```

4. Workspace domain must be exactly lowercase `<WORKSPACE_DOMAIN>`.
5. No Admin SDK, offline access, or refresh-token scope is required.
6. Gateway requests `openid`, `email`, and `profile` and verifies issuer, audience, signature, expiry, nonce, verified email, and Workspace `hd` claim.

## DNS, TLS, and firewall

- `<PORTAL_HOSTNAME>` resolves to private on-site IP `<PORTAL_IPV4>`.
- Cloudflare record is DNS-only, not proxied.
- Caddy obtains public certificate using Cloudflare DNS-01.
- No Cloudflare Tunnel and no WAN port forwarding.
- Local DNS rebinding protection must allow `<PORTAL_HOSTNAME>` to resolve to `<PORTAL_IPV4>`, or local DNS must provide matching override.
- Permit TCP/80 and TCP/443 to `<PORTAL_IPV4>` only from organization and guest LANs. Port 80 only redirects to HTTPS.
- Do not expose app port 8080.

## Dockge deployment

### 1. Create stack

In Dockge, select **Compose > New Stack**, name it `sengateway`, then paste this complete Compose YAML. No repository checkout, external `Caddyfile`, bind mount, session secret, or encryption key is required.

```yaml
services:
  app:
    image: ghcr.io/shabilullah/sengateway:latest
    pull_policy: always
    restart: unless-stopped
    init: true
    environment:
      PUBLIC_BASE_URL: "https://${PORTAL_HOSTNAME:?Set PORTAL_HOSTNAME}"
      DATABASE_URL: "sqlite:/data/gateway.db?mode=rwc"
      SENGATEWAY_SECRET_DIR: /data
      COOKIE_SECURE: "true"
      TRUSTED_PROXY_IP: 172.28.0.3
      SETUP: ${SETUP:?Set SETUP true or false}
      SETUP_PASSCODE: ${SETUP_PASSCODE:?Set setup passcode with at least 16 bytes}
      RUST_LOG: sengateway=info
    volumes:
      - app-data:/data
    expose:
      - "8080"
    healthcheck:
      test: ["CMD", "curl", "-fsS", "http://127.0.0.1:8080/healthz"]
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 10s
    networks:
      portal:
        ipv4_address: 172.28.0.2
    logging:
      options:
        max-size: "10m"
        max-file: "3"

  caddy:
    image: ghcr.io/shabilullah/sengateway-caddy:latest
    pull_policy: always
    restart: unless-stopped
    init: true
    entrypoint: ["/bin/sh", "-c"]
    command:
      - |
        cat >/tmp/Caddyfile <<'EOF'
        {$$PORTAL_HOSTNAME} {
          tls {
            dns cloudflare {env.CLOUDFLARE_API_TOKEN}
          }
          encode zstd gzip
          header {
            Content-Security-Policy "default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; form-action 'self' https://accounts.google.com; frame-ancestors 'none'; base-uri 'none'"
            X-Content-Type-Options nosniff
            Referrer-Policy no-referrer
          }
          reverse_proxy app:8080
        }
        EOF
        exec caddy run --config /tmp/Caddyfile --adapter caddyfile
    depends_on:
      app:
        condition: service_healthy
    environment:
      PORTAL_HOSTNAME: ${PORTAL_HOSTNAME:?Set PORTAL_HOSTNAME}
      CLOUDFLARE_API_TOKEN: ${CLOUDFLARE_API_TOKEN:?Set restricted Cloudflare DNS token}
    ports:
      - "${ORIGIN_BIND_IP:?Set private LAN ORIGIN_BIND_IP}:80:80"
      - "${ORIGIN_BIND_IP:?Set private LAN ORIGIN_BIND_IP}:443:443"
    volumes:
      - caddy-data:/data
      - caddy-config:/config
    networks:
      portal:
        ipv4_address: 172.28.0.3
    logging:
      options:
        max-size: "10m"
        max-file: "3"

networks:
  portal:
    driver: bridge
    ipam:
      config:
        - subnet: 172.28.0.0/24

volumes:
  app-data:
  caddy-data:
  caddy-config:
```

### 2. Set stack environment

In Dockge stack **Environment**, add these deployment values:

```dotenv
PORTAL_HOSTNAME=<PORTAL_HOSTNAME>
ORIGIN_BIND_IP=<PORTAL_IPV4>
CLOUDFLARE_API_TOKEN=<RESTRICTED_CLOUDFLARE_DNS_TOKEN>
SETUP=true
SETUP_PASSCODE=<SETUP_PASSCODE>
```

`CLOUDFLARE_API_TOKEN` must be a Cloudflare API token restricted to DNS edit access for portal zone. `SETUP_PASSCODE` must be a random value containing at least 16 bytes. Do not use Global API Key. Do not add quotes around values.

Internal subnet `172.28.0.0/24` must be unused on deployment host. If it conflicts, change all four values together before deployment: subnet `172.28.0.0/24`, app address `172.28.0.2`, proxy address `172.28.0.3`, and `TRUSTED_PROXY_IP` `172.28.0.3`.

### 3. Deploy and complete setup

1. Select **Deploy** in Dockge.
2. Wait until `app` reports `healthy` and `caddy` remains running.
3. Open `https://<PORTAL_HOSTNAME>/setup` from trusted on-site network.
4. Enter `SETUP_PASSCODE`, administrator email, Google OAuth, Workspace, and UniFi API values.
5. For independently verified self-signed UniFi certificate, enable **Trust certificate currently presented by this UniFi server**. Leave disabled for publicly trusted certificate.
6. Submit setup. Successful setup redirects to Google management login.
7. In Dockge Environment, set `SETUP=false`, then redeploy. `/setup` now returns `404`.

To reconfigure later without deleting data, set `SETUP=true`, redeploy, open `/setup`, save settings with same passcode, then restore `SETUP=false` and redeploy. Reconfiguration preserves users, coupons, authorizations, sessions, and audit history.

App creates `.session-secret`, `.setup-encryption-key`, and SQLite database inside persistent `app-data` volume. Caddy stores ACME account and certificate state in `caddy-data`. Redeploying same Dockge stack preserves these volumes. Deleting stack volumes destroys gateway configuration, coupons, sessions, audit history, certificate pin, and generated secrets.

## UniFi API configuration

Generate API key under **UniFi Network > Control Plane > Integrations**.

Gateway setup values:

```text
UniFi controller URL:
https://<UNIFI_HOSTNAME>:<UNIFI_PORT>

UniFi API key:
<UNIFI_API_KEY>
```

Gateway appends `/proxy/network/integration/v1`, fetches every accessible site from `GET /sites`, and displays site name plus ID for selection. Do not enter API path or site ID manually.

### Private UniFi hostname and certificate

`<UNIFI_HOSTNAME>` is a local name chosen for TLS-safe controller access. It must satisfy both conditions:

1. Gateway container resolves `<UNIFI_HOSTNAME>` to `<UNIFI_IPV4>`.
2. UniFi HTTPS certificate contains `<UNIFI_HOSTNAME>` in its Subject Alternative Name (SAN).

Changing `/etc/hosts` or DNS alone does not fix TLS. Hostname and certificate SAN must match. Do not use controller IP in API URL when certificate covers only hostname.

#### 1. Confirm certificate name

From deployment host:

```sh
openssl s_client \
  -connect <UNIFI_IPV4>:<UNIFI_PORT> \
  -servername <UNIFI_HOSTNAME> \
  -showcerts </dev/null 2>/dev/null \
  | openssl x509 -noout -subject -issuer -ext subjectAltName
```

Output must list:

```text
DNS:<UNIFI_HOSTNAME>
```

If missing, create/install controller certificate containing that SAN before continuing. Do not disable certificate verification.

#### 2. Choose trust mode during setup

Setup offers **Trust certificate currently presented by this UniFi server**.

- Leave disabled when UniFi OS has certificate trusted by standard OS CA store. Gateway performs normal CA and hostname verification.
- Enable only for confirmed self-signed UniFi endpoint such as `unifi.local`. Gateway first captures presented certificate without sending API key, then creates strict TLS client pinned to that certificate. Setup saves pin only after hostname/SAN validation and official site API check succeed.

Before enabling, independently compare certificate shown by UniFi administrator interface or from deployment host:

```sh
openssl s_client \
  -connect <UNIFI_IPV4>:<UNIFI_PORT> \
  -servername <UNIFI_HOSTNAME> </dev/null 2>/dev/null \
  | openssl x509 -noout -fingerprint -sha256 -subject -issuer -ext subjectAltName
```

Do not enable based only on certificate captured across untrusted network. Alternative: configure UniFi OS with certificate from trusted CA and leave checkbox disabled.

#### 3. Provide hostname inside app container

Preferred: organization DNS resolves `<UNIFI_HOSTNAME>` to `<UNIFI_IPV4>` from app container network. Host resolution alone is insufficient: `<UNIFI_HOSTNAME>` must exactly match controller certificate SAN.

Podman/netavark custom networks may give container embedded DNS such as `172.28.0.1`. This resolver can resolve public names while failing private split-horizon records available through LAN resolver. Typical setup errors are:

```text
Could not establish a trusted TLS connection to UniFi
Could not capture UniFi TLS certificate
```

Confirm root cause inside Dockge `app` service console:

```sh
cat /etc/resolv.conf
getent hosts <UNIFI_HOSTNAME>
curl -v https://<UNIFI_HOSTNAME>:<UNIFI_PORT>/proxy/network/integration/v1/sites
```

Expected `getent` output contains `<UNIFI_IPV4>`. Expected unauthenticated `curl` response is HTTP `401`; that proves DNS, TCP, hostname verification, certificate trust, TLS, and API path work. `Could not resolve host` or empty `getent` output is DNS failure, not certificate failure.

For Dockge stack where embedded Podman DNS does not return private record, add exact certificate hostname mapping to `app` service:

```yaml
services:
  app:
    extra_hosts:
      - <UNIFI_HOSTNAME>:<UNIFI_IPV4>
```

Example:

```yaml
services:
  app:
    extra_hosts:
      - unifi.example.com:192.168.50.10
```

Do not map only alias such as `unifi.local` when setup URL and certificate use `unifi.example.com`; mappings are exact hostname matches. Do not use controller IP in setup URL when certificate covers hostname.

Save and redeploy through Dockge so Compose recreates `app` with persistent `/etc/hosts` entry. Do not edit container `/etc/hosts` directly except temporary diagnosis; change disappears on recreation. Verify after redeploy:

```sh
getent hosts <UNIFI_HOSTNAME>
curl -v https://<UNIFI_HOSTNAME>:<UNIFI_PORT>/proxy/network/integration/v1/sites
```

If controller uses publicly trusted certificate, leave **Trust certificate currently presented by this UniFi server** disabled. Enable it only for independently verified self-signed certificate; DNS must work in either mode because certificate capture also resolves hostname.

If `/healthz` remains HTTP `503` after DNS repair, database may still contain old UniFi hostname from prior setup. Temporarily set `SETUP=true`, redeploy through Dockge, re-run setup with working controller base URL, select discovered site, save, then restore `SETUP=false` and redeploy. Do not edit SQLite directly.

For host-only mapping with Podman pod deployment outside Dockge, add mapping when creating pod because containers share pod network namespace:

```sh
podman pod create \
  --name sengateway \
  --add-host <UNIFI_HOSTNAME>:<UNIFI_IPV4> \
  --publish <PORTAL_IPV4>:80:80 \
  --publish <PORTAL_IPV4>:443:443
```

During WebUI setup enter only controller base URL:

```text
https://<UNIFI_HOSTNAME>:<UNIFI_PORT>
```

Publicly trusted certificate uses image OS CA store automatically.

#### 4. Optional administrator-supplied CA

Certificate checkbox is portable default for verified self-signed UniFi. Administrator-supplied private CA remains optional deployment override through `UNIFI_CA_CERT_PATH`; see app runtime configuration when centralized CA management is required.
Default `compose.yaml` needs no CA file. Setup persists optional pinned certificate in SQLite after explicit consent and successful strict verification. Gateway never disables TLS verification. It uses official `AUTHORIZE_GUEST_ACCESS` and `UNAUTHORIZE_GUEST_ACCESS` actions; do not use legacy `/api/s/{site}/cmd/stamgr` endpoints.

## Operations

### Health

From on-site network:

```sh
curl --fail https://<PORTAL_HOSTNAME>/healthz
```

Expected HTTP status: `200`.

In Dockge, open `sengateway`, confirm both `app` and `caddy` services are running, then open each service log. `app` must report `healthy`; `caddy` must show successful certificate loading or issuance for `<PORTAL_HOSTNAME>`. Generated container names differ between Docker Compose and Podman Compose, so operate by Dockge service name rather than hard-coded container name.

### Safe checks

```sh
getent hosts <PORTAL_HOSTNAME>
getent hosts <UNIFI_HOSTNAME>
curl --fail https://<PORTAL_HOSTNAME>/healthz
```

Expected addresses:

```text
<PORTAL_IPV4> <PORTAL_HOSTNAME>
<UNIFI_IPV4> <UNIFI_HOSTNAME>
```

## Troubleshooting

### UniFi says “Please enter a valid IPv4 address”

Enter `<PORTAL_IPV4>` in **External Portal > IPv4 Address**. Put `<PORTAL_HOSTNAME>` in **Domain**. Do not put full URL into IPv4 field.

### UniFi designer page appears

- Enable **External Portal Server**.
- Set custom IPv4 to `<PORTAL_IPV4>`.
- Remove native UniFi Password/Voucher methods if they are selected.
- Keep **Show Landing Page** on.

### Gateway opens but voucher field is disabled

Page lacks UniFi client context. Connect device to guest SSID and let UniFi redirect it. Do not browse directly to homepage for voucher redemption.

### Portal does not open

- Confirm SSID uses Hotspot/Captive Portal.
- Confirm `<PORTAL_IPV4>` is External Portal IPv4 and pre-authorization allowance.
- Confirm Domain is `<PORTAL_HOSTNAME>`.
- Confirm guest DNS resolves hostname to `<PORTAL_IPV4>`.
- Confirm guest VLAN reaches `<PORTAL_IPV4>:443`.
- Test with a new client or forget/rejoin SSID.
- Open `http://example.com` to trigger HTTP captive-portal detection.

### Google login fails before account selection

Allow browser sign-in hosts listed under **Pre-Authorization Allowances**. Confirm callback URI exactly matches Google Cloud configuration. `oauth2.googleapis.com` is server-side and does not fix browser page loading.

### Google Prompt approval completes but browser does not continue

First move approval phone to mobile data. If browser then completes, phone was blocked from FCM on unauthenticated SSID; add Google Prompt FCM destinations and TCP/5228-5230 listed above. If phone receives prompt and approval succeeds but login browser still stalls, inspect blocked destinations from browser device and confirm `accounts.google.com`, `www.gstatic.com`, `accounts.youtube.com`, `play.google.com`, and `lh3.googleusercontent.com` are pre-authorized. Google changes sign-in assets over time; allow only observed Google destinations, then retest.

### Login says account is not enabled

User must exist in gateway management, be approved, and have role matching intent:

- `STAFF` for captive portal.
- `ADMIN` or `FRONT_DESK` for management.

### Certificate warning

- Confirm **Secure Portal** on.
- Confirm **Domain** is `<PORTAL_HOSTNAME>`.
- Confirm **HTTPS Redirection Support** off.
- Confirm hostname resolves to `<PORTAL_IPV4>` and Caddy serves valid certificate.

### Setup returns 404

`/setup` exists only while app environment has `SETUP=true`. Temporarily enable it in Dockge and redeploy, reconfigure with `SETUP_PASSCODE`, then restore `SETUP=false` and redeploy. Never delete database merely to reopen setup.
