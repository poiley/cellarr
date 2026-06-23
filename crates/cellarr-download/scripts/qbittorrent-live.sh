#!/usr/bin/env bash
# Live smoke test for the cellarr-download qBittorrent adapter.
#
# Stands up an ephemeral qBittorrent (lscr.io/linuxserver/qbittorrent) on an
# OS-allocated port with a tmpfs /config, discovers the WebUI credentials within
# a BOUNDED loop, runs the real adapter against it (cargo example), then tears
# the container down. The torrent download is NEVER waited on to completion —
# the example only asserts add + category + presence, then deletes.
#
# Hard bounds (anti-wedge):
#   - container readiness / credential discovery: <= 60s, FAIL FAST otherwise
#   - the cargo live example (which itself bounds every HTTP call to 10s and the
#     status poll to ~30s) is wrapped in a 120s hard kill
#   - the whole script is wrapped so the container is always torn down (trap)
#
# Usage: crates/cellarr-download/scripts/qbittorrent-live.sh
set -uo pipefail

DOCKER="${DOCKER:-/usr/local/bin/docker}"
IMAGE="${QBIT_IMAGE:-lscr.io/linuxserver/qbittorrent:latest}"
CATEGORY="${CELLARR_QBIT_CATEGORY:-cellarr-tv}"
# A well-known PUBLIC torrent: Debian 12 netinst (the .torrent is served from
# cdimage.debian.org). Magnet for the same is hard to keep current, so we use a
# magnet to a public Linux distro that resolves metadata fast; fall back is fine
# since we only need the torrent to APPEAR.
MAGNET="${CELLARR_QBIT_MAGNET:-magnet:?xt=urn:btih:dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c&dn=ubuntu&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce}"

# Unique, ephemeral container name (per-run; torn down in trap).
CTR="qbittorrent-cellarr-live-$$-$RANDOM"

cleanup() {
  "$DOCKER" rm -f "$CTR" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

fail() { echo "LIVE-FAIL: $*" >&2; exit 1; }

# Portable hard-bound runner (this machine has no `timeout`/`gtimeout`): run a
# command in the background, poll a deadline, and SIGKILL the whole process group
# if it overruns. Returns 124 on timeout (matching GNU `timeout`), else the cmd rc.
bound() {
  local secs="$1"; shift
  "$@" &
  local pid=$!
  local end=$(( $(date +%s) + secs ))
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$(date +%s)" -ge "$end" ]; then
      kill -KILL "$pid" 2>/dev/null
      wait "$pid" 2>/dev/null
      return 124
    fi
    sleep 1
  done
  wait "$pid"
  return $?
}

echo "==> ensuring image present (bounded 120s): $IMAGE"
if ! "$DOCKER" image inspect "$IMAGE" >/dev/null 2>&1; then
  bound 120 "$DOCKER" pull "$IMAGE" >/dev/null 2>&1
  "$DOCKER" image inspect "$IMAGE" >/dev/null 2>&1 || fail "could not pull or find image $IMAGE"
fi

echo "==> starting ephemeral container $CTR (tmpfs /config, OS-allocated port)"
# -P with an EXPOSE-less image won't map, so publish 8080 to an ephemeral host
# port explicitly via 127.0.0.1::8080.
if ! "$DOCKER" run -d --name "$CTR" \
    --tmpfs /config \
    -e PUID=1000 -e PGID=1000 -e TZ=Etc/UTC -e WEBUI_PORT=8080 \
    -p 127.0.0.1::8080 \
    "$IMAGE" >/dev/null 2>&1; then
  fail "docker run failed"
fi

# Resolve the host port qBittorrent's 8080 was mapped to.
HOSTPORT="$("$DOCKER" port "$CTR" 8080/tcp 2>/dev/null | head -1 | sed 's/.*://')"
[ -n "$HOSTPORT" ] || fail "could not resolve mapped host port"
BASEURL="http://127.0.0.1:$HOSTPORT"
echo "==> WebUI at $BASEURL"

# --- Bounded credential setup (<=60s) -------------------------------------
# qBittorrent 5.x quirk: the linuxserver image logs a *temporary* admin password
# on first boot, but that temp credential is NOT accepted by the WebUI login over
# the published (host-mapped) port — 5.x also enforces Host-header validation and
# rejects a Host whose port differs from the internal WebUI port (the mapped host
# port always differs), answering 401. To get a deterministic, mapped-port login
# we instead SEED a known config before the WebUI accepts traffic:
#   - a known password (admin / adminadmin) as a PBKDF2 hash, and
#   - WebUI\HostHeaderValidation=false so the adapter (whose Host is the mapped
#     host:port) is accepted — the realistic LAN/reverse-proxy operator setting.
# This still discovers/confirms the working credential within the 60s bound and
# FAILS FAST otherwise. We first wait for the temp-password log line as the
# readiness signal (it only prints once the WebUI is up).
USER="admin"
PASS="adminadmin"
# PBKDF2-HMAC-SHA512, 100000 iters, 16-byte salt, base64(salt):base64(hash) — the
# qBittorrent.conf format. Precomputed for "adminadmin" (fixed salt is fine for a
# throwaway ephemeral container).
PWHASH='@ByteArray(KYsadd/flM4AzMCpgDEvhw==:rL/28RWHcENEwoeRFCBHZXnTuqSeQbDRRMavQbxjWRck3fWQd6JuWotLCfUhnOsT86QSqGNfHiDNL8Tfzdelpg==)'
CONF_PATH="/config/qBittorrent/qBittorrent.conf"

echo "==> waiting for WebUI readiness within 60s"
deadline=$(( $(date +%s) + 60 ))
ready=0
while [ "$(date +%s)" -lt "$deadline" ]; do
  # The temp-password log line is the readiness signal: it is printed only after
  # qBittorrent has started the WebUI and written its initial config.
  if "$DOCKER" logs "$CTR" 2>&1 | grep -qi 'temporary password'; then
    # Confirm the config file actually exists before we try to rewrite it.
    if "$DOCKER" exec "$CTR" test -f "$CONF_PATH" 2>/dev/null; then
      ready=1
      break
    fi
  fi
  sleep 2
done
[ "$ready" -eq 1 ] || { echo "----- container logs -----" >&2; "$DOCKER" logs "$CTR" 2>&1 | tail -40 >&2; fail "WebUI not ready within 60s"; }

echo "==> seeding known credential + disabling Host-header validation"
# Stop the qbittorrent-nox process (via s6) before writing, so it doesn't rewrite
# the config out from under us; bounded wait for it to actually exit.
"$DOCKER" exec "$CTR" s6-svc -d /run/service/svc-qbittorrent >/dev/null 2>&1 || true
stopdl=$(( $(date +%s) + 15 ))
while [ "$(date +%s)" -lt "$stopdl" ]; do
  "$DOCKER" exec "$CTR" pgrep -x qbittorrent-nox >/dev/null 2>&1 || break
  sleep 1
done

# Write the seeded config via `tee` over stdin (reliable inside the container).
printf '%s\n' \
  '[BitTorrent]' \
  'Session\QueueingSystemEnabled=false' \
  '' \
  '[Meta]' \
  'MigrationVersion=8' \
  '' \
  '[Preferences]' \
  'WebUI\Address=*' \
  'WebUI\ServerDomains=*' \
  "WebUI\\Username=$USER" \
  "WebUI\\Password_PBKDF2=\"$PWHASH\"" \
  'WebUI\HostHeaderValidation=false' \
  'WebUI\CSRFProtection=false' \
  | "$DOCKER" exec -i "$CTR" tee "$CONF_PATH" >/dev/null \
  || fail "could not write seeded config"
"$DOCKER" exec "$CTR" chown abc:abc "$CONF_PATH" >/dev/null 2>&1 || true

# Restart qbittorrent and confirm login works over the MAPPED host port within the
# remaining budget. This is the real success-check: the adapter's path.
"$DOCKER" exec "$CTR" s6-svc -u /run/service/svc-qbittorrent >/dev/null 2>&1 || true
logindl=$(( $(date +%s) + 30 ))
login_ok=0
while [ "$(date +%s)" -lt "$logindl" ]; do
  code="$(curl -s -o /dev/null -w '%{http_code}' -m 5 -X POST "$BASEURL/api/v2/auth/login" \
    -H "Referer: $BASEURL" \
    --data-urlencode "username=$USER" --data-urlencode "password=$PASS" 2>/dev/null || echo 000)"
  # 200 (Ok.) or 204 (5.x) both mean the credential + Host config are accepted.
  if [ "$code" = "200" ] || [ "$code" = "204" ]; then
    login_ok=1
    break
  fi
  sleep 2
done
[ "$login_ok" -eq 1 ] || { echo "----- container logs -----" >&2; "$DOCKER" logs "$CTR" 2>&1 | tail -40 >&2; fail "seeded credential did not authenticate over mapped port within budget"; }

echo "==> using user=$USER (seeded credential confirmed over mapped port)"

# --- Run the live adapter driver (hard 120s kill) -------------------------
echo "==> running cargo live example (hard-bounded 120s)"
export CELLARR_QBIT_URL="$BASEURL"
export CELLARR_QBIT_USER="$USER"
export CELLARR_QBIT_PASS="$PASS"
export CELLARR_QBIT_MAGNET="$MAGNET"
export CELLARR_QBIT_CATEGORY="$CATEGORY"
export CELLARR_QBIT_POLL_BUDGET_SECS="${CELLARR_QBIT_POLL_BUDGET_SECS:-30}"

MISE="${MISE:-/opt/homebrew/bin/mise}"
REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"

# Build first (outside the 120s budget) so the timeout only bounds the live run.
( cd "$REPO_ROOT" && "$MISE" exec -- cargo build -p cellarr-download --example qbittorrent_live >/dev/null 2>&1 ) \
  || fail "failed to build live example"

runlive() { ( cd "$REPO_ROOT" && "$MISE" exec -- cargo run -q -p cellarr-download --example qbittorrent_live ); }
bound 120 runlive
rc=$?

if [ "$rc" -eq 124 ]; then
  fail "live example exceeded the 120s hard bound (killed)"
elif [ "$rc" -ne 0 ]; then
  fail "live example exited non-zero ($rc)"
fi

echo "LIVE-OK"
