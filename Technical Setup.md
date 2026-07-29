# SEN Gateway Technical Setup

## Placeholder reference

Replace every angle-bracket token below with deployment value. Keep tokens in documentation and examples; never commit real environment values.

> Values in angle brackets are placeholders, not literal input. Example-format values only show expected shape and are not deployment defaults. Replace tokens locally; do not commit substituted copies.

| Placeholder | Meaning | Example format |
|---|---|---|
| `<PORTAL_HOSTNAME>` | Portal DNS hostname | `portal.example.com` |
| `<PORTAL_IPV4>` | Private IPv4 of Caddy portal host | `192.168.50.20` |
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

Permit these before authentication:

```text
<PORTAL_IPV4>
<PORTAL_HOSTNAME>
accounts.google.com
oauth2.googleapis.com
```

Also permit:

- DNS to approved resolvers.
- TCP/443 to `<PORTAL_IPV4>`.
- Google static hosts observed during real Workspace login if guest policy blocks them.

Do not broadly allow private networks. Keep guest LAN isolated and block other private destinations.

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

## UniFi API configuration

Generate API key under **UniFi Network > Control Plane > Integrations**.

Gateway setup values:

```text
UniFi Network API URL:
https://<UNIFI_HOSTNAME>:<UNIFI_PORT>/proxy/network/integration/v1

UniFi site ID:
<UNIFI_SITE_ID>
```

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

#### 2. Export trusted certificate

If controller uses private or self-signed certificate, export trusted PEM chain to deployment host:

```sh
sudo install -d -m 0755 /opt/sengateway
openssl s_client \
  -connect <UNIFI_IPV4>:<UNIFI_PORT> \
  -servername <UNIFI_HOSTNAME> \
  -showcerts </dev/null 2>/dev/null \
  | openssl x509 -outform PEM \
  | sudo tee /opt/sengateway/unifi-ca.pem >/dev/null
sudo chmod 0644 /opt/sengateway/unifi-ca.pem
```

For CA-issued controller certificate, use CA root/intermediate PEM supplied by administrator instead of copying leaf certificate. Leaf export above is suitable only when controller certificate itself is self-signed and trusted as root.

Verify saved PEM:

```sh
openssl x509 -in /opt/sengateway/unifi-ca.pem -noout -subject -issuer -dates
```

#### 3. Provide hostname inside container

Preferred: organization DNS resolves `<UNIFI_HOSTNAME>` to `<UNIFI_IPV4>` from container network.

For host-only mapping with Podman pod, add mapping when creating pod because containers share pod network namespace:

```sh
podman pod create \
  --name sengateway \
  --add-host <UNIFI_HOSTNAME>:<UNIFI_IPV4> \
  --publish <PORTAL_IPV4>:80:80 \
  --publish <PORTAL_IPV4>:443:443
```

For Compose, add to `app` service:

```yaml
services:
  app:
    extra_hosts:
      - "<UNIFI_HOSTNAME>:<UNIFI_IPV4>"
```

Do not add mapping when normal DNS already supplies correct address.

#### 4. Mount CA and enable app trust

Podman app container options:

```sh
--volume /opt/sengateway/unifi-ca.pem:/run/secrets/unifi-ca.pem:ro \
--env UNIFI_CA_CERT_PATH=/run/secrets/unifi-ca.pem
```

Compose app service:

```yaml
services:
  app:
    environment:
      UNIFI_CA_CERT_PATH: /run/secrets/unifi-ca.pem
    volumes:
      - /opt/sengateway/unifi-ca.pem:/run/secrets/unifi-ca.pem:ro
```

Then configure gateway:

```text
UniFi Network API URL:
https://<UNIFI_HOSTNAME>:<UNIFI_PORT>/proxy/network/integration/v1
```

Current `deploy.sh` and `compose.yaml` templates do not infer private hostname or CA. Add options above to local deployment configuration when controller does not use publicly trusted DNS/certificate.

Gateway loads PEM from `UNIFI_CA_CERT_PATH` into HTTP client trust store. Never disable TLS verification. Gateway uses official `AUTHORIZE_GUEST_ACCESS` and `UNAUTHORIZE_GUEST_ACCESS` actions; do not use legacy `/api/s/{site}/cmd/stamgr` endpoints.

## Operations

### Health

From on-site network:

```sh
curl --fail https://<PORTAL_HOSTNAME>/healthz
```

Expected HTTP status: `200`.

On deployment host:

```sh
podman pod ps --filter name=sengateway
podman ps --pod --filter pod=sengateway
podman inspect sengateway-app --format '{{.State.Health.Status}}'
podman logs --tail=100 sengateway-app
podman logs --tail=100 sengateway-caddy
```

Expected containers:

```text
sengateway-app
sengateway-caddy
```

Expected app health: `healthy`.

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

Allow `accounts.google.com`, `oauth2.googleapis.com`, and required Google static hosts in pre-authorization allowances. Confirm callback URI exactly matches Google Cloud configuration.

### Login says account is not enabled

User must exist in gateway management, be approved, and have role matching intent:

- `STAFF` for captive portal.
- `ADMIN` or `FRONT_DESK` for management.

### Certificate warning

- Confirm **Secure Portal** on.
- Confirm **Domain** is `<PORTAL_HOSTNAME>`.
- Confirm **HTTPS Redirection Support** off.
- Confirm hostname resolves to `<PORTAL_IPV4>` and Caddy serves valid certificate.

### Setup is already complete

`/setup` returns `404` after one-time setup commits. This is expected. Use management login instead. Never delete database to reopen setup.
