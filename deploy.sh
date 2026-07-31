#!/bin/sh
set -eu

POD=${POD_NAME:-sengateway}
RESOURCE_PREFIX=${RESOURCE_PREFIX:-$POD}
APP_CONTAINER=${APP_CONTAINER_NAME:-sengateway-app}
CADDY_CONTAINER=${CADDY_CONTAINER_NAME:-sengateway-caddy}
APP_IMAGE=${APP_IMAGE:-ghcr.io/shabilullah/sengateway:latest}
CADDY_IMAGE=${CADDY_IMAGE:-ghcr.io/shabilullah/sengateway-caddy:latest}
HTTP_PORT=${HTTP_PORT:-80}
HTTPS_PORT=${HTTPS_PORT:-443}
APP_ONLY=false
BUILD_IMAGES=false
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
APP_VOLUME=${APP_VOLUME:-${RESOURCE_PREFIX}-app-data}
CADDY_DATA_VOLUME=${CADDY_DATA_VOLUME:-${RESOURCE_PREFIX}-caddy-data}
CADDY_CONFIG_VOLUME=${CADDY_CONFIG_VOLUME:-${RESOURCE_PREFIX}-caddy-config}
SESSION_SECRET_NAME=${SESSION_SECRET_NAME:-${RESOURCE_PREFIX}-session-secret}
ENCRYPTION_SECRET_NAME=${ENCRYPTION_SECRET_NAME:-${RESOURCE_PREFIX}-encryption-key}
CLOUDFLARE_SECRET_NAME=${CLOUDFLARE_SECRET_NAME:-${RESOURCE_PREFIX}-cloudflare-token}

usage() {
    cat <<'EOF'
Usage: ./deploy.sh [--build] [--app-only]

Creates one Podman pod containing Sen Gateway and Caddy. By default it pulls
published GHCR images. --build builds images from this checkout. Values may be
supplied through PORTAL_HOSTNAME, ORIGIN_BIND_IP, and CLOUDFLARE_API_TOKEN;
missing values are prompted. --app-only skips Caddy for local smoke tests.
EOF
}

for option in "$@"; do
    case $option in
        --build) BUILD_IMAGES=true ;;
        --app-only) APP_ONLY=true ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done

command -v podman >/dev/null 2>&1 || { echo "podman is required" >&2; exit 1; }
command -v ip >/dev/null 2>&1 || { echo "iproute2 is required" >&2; exit 1; }

if [ -z "${ORIGIN_BIND_IP:-}" ]; then
    detected_ip=$(ip -4 route get 1.1.1.1 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p' | head -n 1)
    [ -n "$detected_ip" ] || { echo "Unable to detect default-route IPv4; set ORIGIN_BIND_IP" >&2; exit 1; }
    if [ -t 0 ]; then
        printf 'Portal bind IP [%s]: ' "$detected_ip"
        read -r ORIGIN_BIND_IP
        ORIGIN_BIND_IP=${ORIGIN_BIND_IP:-$detected_ip}
    else
        ORIGIN_BIND_IP=$detected_ip
    fi
fi
case $ORIGIN_BIND_IP in
    0.0.0.0|127.*) [ "${ALLOW_LOOPBACK_BIND:-false}" = true ] || { echo "ORIGIN_BIND_IP must be a concrete non-loopback LAN address" >&2; exit 1; } ;;
esac

if [ -z "${PORTAL_HOSTNAME:-}" ]; then
    [ -t 0 ] || { echo "PORTAL_HOSTNAME is required without an interactive terminal" >&2; exit 1; }
    printf 'Portal hostname: '
    read -r PORTAL_HOSTNAME
fi
case $PORTAL_HOSTNAME in
    ""|*://*|*/*|*:*|*' '*) echo "PORTAL_HOSTNAME must be a DNS hostname without scheme, path, or port" >&2; exit 1 ;;
esac

if [ "$APP_ONLY" = false ] && [ -z "${CLOUDFLARE_API_TOKEN:-}" ]; then
    [ -t 0 ] || { echo "CLOUDFLARE_API_TOKEN is required without an interactive terminal" >&2; exit 1; }
    printf 'Cloudflare API token: '
    stty -echo
    trap 'stty echo' EXIT HUP INT TERM
    read -r CLOUDFLARE_API_TOKEN
    stty echo
    trap - EXIT HUP INT TERM
    printf '\n'
fi

random_base64() {
    bytes=$1
    head -c "$bytes" /dev/urandom | base64 | tr -d '\n'
}

ensure_generated_secret() {
    name=$1
    bytes=$2
    if ! podman secret inspect "$name" >/dev/null 2>&1; then
        random_base64 "$bytes" | podman secret create "$name" - >/dev/null
    fi
}

ensure_generated_secret "$SESSION_SECRET_NAME" 48
ensure_generated_secret "$ENCRYPTION_SECRET_NAME" 32
if [ "$APP_ONLY" = false ]; then
    printf '%s' "$CLOUDFLARE_API_TOKEN" | podman secret create --replace "$CLOUDFLARE_SECRET_NAME" - >/dev/null
fi
unset CLOUDFLARE_API_TOKEN

if [ "$BUILD_IMAGES" = true ]; then
    podman build -t "$APP_IMAGE" -f "$SCRIPT_DIR/Dockerfile" "$SCRIPT_DIR"
    if [ "$APP_ONLY" = false ]; then
        podman build -t "$CADDY_IMAGE" -f "$SCRIPT_DIR/Dockerfile.caddy" "$SCRIPT_DIR"
    fi
else
    podman pull "$APP_IMAGE"
    if [ "$APP_ONLY" = false ]; then
        podman pull "$CADDY_IMAGE"
    fi
fi

ensure_volume() {
    podman volume inspect "$1" >/dev/null 2>&1 || podman volume create "$1" >/dev/null
}
ensure_volume "$APP_VOLUME"
if [ "$APP_ONLY" = false ]; then
    ensure_volume "$CADDY_DATA_VOLUME"
    ensure_volume "$CADDY_CONFIG_VOLUME"
fi

if podman pod exists "$POD"; then
    podman pod rm -f "$POD" >/dev/null
fi

podman pod create \
    --name "$POD" \
    --restart unless-stopped \
    --publish "${ORIGIN_BIND_IP}:${HTTP_PORT}:80" \
    --publish "${ORIGIN_BIND_IP}:${HTTPS_PORT}:443" >/dev/null

podman run -d \
    --name "$APP_CONTAINER" \
    --pod "$POD" \
    --restart unless-stopped \
    --volume "$APP_VOLUME:/data" \
    --secret "$SESSION_SECRET_NAME,type=env,target=SESSION_SECRET" \
    --secret "$ENCRYPTION_SECRET_NAME,type=env,target=SETUP_ENCRYPTION_KEY" \
    --secret "$CLOUDFLARE_SECRET_NAME,type=env,target=CLOUDFLARE_API_TOKEN" \
    --health-cmd 'curl -fsS http://127.0.0.1:8080/healthz' \
    --health-interval 30s \
    --health-timeout 5s \
    --health-retries 3 \
    --env "PUBLIC_BASE_URL=https://${PORTAL_HOSTNAME}" \
    --env 'DATABASE_URL=sqlite:/data/gateway.db?mode=rwc' \
    --env 'COOKIE_SECURE=true' \
    --env 'TRUSTED_PROXY_IP=127.0.0.1' \
    "$APP_IMAGE" >/dev/null

if [ "$APP_ONLY" = false ]; then
    podman run -d \
        --name "$CADDY_CONTAINER" \
        --pod "$POD" \
        --restart unless-stopped \
        --volume "$SCRIPT_DIR/Caddyfile:/etc/caddy/Caddyfile:ro" \
        --volume "$CADDY_DATA_VOLUME:/data" \
        --volume "$CADDY_CONFIG_VOLUME:/config" \
        --secret "$CLOUDFLARE_SECRET_NAME,type=env,target=CLOUDFLARE_API_TOKEN" \
        --env "PORTAL_HOSTNAME=${PORTAL_HOSTNAME}" \
        --env 'CADDY_UPSTREAM=127.0.0.1:8080' \
        "$CADDY_IMAGE" >/dev/null
fi

printf 'Pod %s started.\n' "$POD"
podman ps --pod --filter "pod=${POD}"
printf '\nOne-time setup URL:\n'
attempt=0
while [ "$attempt" -lt 30 ]; do
    setup_url=$(podman logs "$APP_CONTAINER" 2>&1 | sed -n '/\/setup?token=/p' | head -n 1)
    if [ -n "$setup_url" ]; then
        printf '%s\n' "$setup_url"
        exit 0
    fi
    attempt=$((attempt + 1))
    sleep 1
done
echo "Setup URL not found; inspect podman logs $APP_CONTAINER" >&2
exit 1
