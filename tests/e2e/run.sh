#!/usr/bin/env bash
# tests/e2e/run.sh — the FULL live end-to-end proof of cellarr's acquisition
# chain against REAL Docker services, driving the actual `cellarr run` daemon.
#
#   mock Torznab  ->  cellarr daemon (real binary)  ->  real qBittorrent (Docker)
#        ^ search received          ^ search/grab/track/import       ^ Completed
#
# DETERMINISTIC: the payload is pre-staged into qBittorrent's save dir before the
# grab, so adding the .torrent rechecks straight to Completed in seconds — no
# peers, no internet, no flaky real download. Every external wait is HARD-BOUNDED
# and a trap tears down every container + the daemon on any exit.
#
# This is Docker-gated and is NOT part of `just ci`. Run via `just e2e`.
set -uo pipefail

# --- configuration / bounds -------------------------------------------------
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
RUN_ID="${CELLARR_RUN_ID:-$(basename "$REPO" | tr -c 'a-zA-Z0-9_' '_' | sed 's/_*$//')}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/cellarr-e2e.XXXXXX")"
QBIT_NAME="qbittorrent-cellarr-${RUN_ID}-$$"
DAEMON_BIN="$REPO/target/debug/cellarr"
PY="${PYTHON:-python3}"
MOCK_PY="$HERE/torznab_mock.py"

READY_TIMEOUT=90      # container/daemon readiness ceiling (s)
HTTP_TIMEOUT=10       # any single HTTP/poll ceiling (s)
POLL_TIMEOUT=90       # bounded completion polling ceiling (s)

QBIT_PORT=18080       # host port for the qBittorrent WebUI (published)
MOCK_PORT=18099       # host port for the Torznab mock
API_PORT=0            # OS-assigned; read back from the daemon log

RELEASE_TITLE="The.Office.S01E01.1080p.BluRay.x264-CELLARR"
SERIES_TITLE="The Office"
PAYLOAD_NAME="The.Office.S01E01.1080p.BluRay.x264-CELLARR.mkv"
CATEGORY="cellarr-movies"

# Host save dir, bind-mounted into the container at /downloads so cellarr (host)
# can read what qBittorrent (container) writes.
SAVE_DIR="$WORK/downloads"
DATA_DIR="$WORK/data"
LIBRARY_ROOT="$WORK/library"
DAEMON_LOG="$WORK/daemon.log"
DAEMON_PID=""
MOCK_PID=""
INFOHASH=""
CONTENT_ID=""
FAIL_REASON=""

mkdir -p "$SAVE_DIR" "$DATA_DIR" "$LIBRARY_ROOT"

log()  { printf '[e2e %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
fail() { FAIL_REASON="$*"; log "FAIL: $*"; capture_evidence; cleanup; exit 1; }

CLEANED=""
cleanup() {
  [ -n "$CLEANED" ] && return; CLEANED=1
  log "tearing down…"
  [ -n "${WATCHDOG:-}" ] && kill "$WATCHDOG" 2>/dev/null
  [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null
  [ -n "$MOCK_PID" ]   && kill "$MOCK_PID"   2>/dev/null
  docker rm -f "$QBIT_NAME" >/dev/null 2>&1
  # Keep $WORK on failure for forensics; remove on success.
  if [ -z "$FAIL_REASON" ]; then rm -rf "$WORK"; else log "evidence kept under $WORK"; fi
}
trap cleanup EXIT INT TERM

capture_evidence() {
  log "----- EVIDENCE -----"
  if [ -f "$DAEMON_LOG" ]; then log "daemon log (tail):"; tail -n 40 "$DAEMON_LOG" >&2; fi
  local rl="$WORK/release.torrent.requests.log"
  if [ -f "$rl" ]; then log "mock received requests:"; cat "$rl" >&2; fi
  log "qBittorrent torrents/info:"; qbit_torrents_info >&2 2>/dev/null || true
  log "docker logs (tail):"; docker logs "$QBIT_NAME" 2>&1 | tail -n 20 >&2 || true
  log "--------------------"
}

# A curl with a hard per-call timeout so nothing can hang.
hc() { curl -fsS --max-time "$HTTP_TIMEOUT" "$@"; }

api()  { echo "http://127.0.0.1:${API_PORT}$1"; }
mock() { echo "http://127.0.0.1:${MOCK_PORT}$1"; }

# ===========================================================================
# 1. Build the payload + a real .torrent matching its bytes; pre-stage the data.
# ===========================================================================
prepare_payload() {
  log "building dummy payload + .torrent"
  local payload="$SAVE_DIR/$PAYLOAD_NAME"
  # A few KB of deterministic bytes standing in for a media file.
  head -c 4096 /dev/zero | tr '\0' 'A' > "$payload"
  printf 'CELLARR-E2E-PAYLOAD' >> "$payload"
  INFOHASH="$("$PY" "$MOCK_PY" make-torrent "$payload" "$WORK/release.torrent")" \
    || fail "could not build .torrent"
  [ -n "$INFOHASH" ] || fail "empty infohash from torrent builder"
  log "payload at $payload ; infohash=$INFOHASH"
}

# ===========================================================================
# 2. Real Torznab mock (its enclosure points the container at host.docker.internal).
# ===========================================================================
start_mock() {
  log "starting Torznab mock on 127.0.0.1:${MOCK_PORT}"
  # The container reaches the host via host.docker.internal (OrbStack/Docker
  # Desktop). The enclosure URL handed to cellarr -> qBittorrent must therefore
  # use that host so the *container* can fetch the .torrent.
  local public_base="http://host.docker.internal:${MOCK_PORT}"
  "$PY" "$MOCK_PY" serve 0.0.0.0 "$MOCK_PORT" "$WORK/release.torrent" \
        "$RELEASE_TITLE" "$INFOHASH" "$public_base" &
  MOCK_PID=$!
  # Bounded wait for caps to answer.
  local deadline=$(( SECONDS + READY_TIMEOUT ))
  until hc "$(mock "/api?t=caps")" >/dev/null 2>&1; do
    [ $SECONDS -ge $deadline ] && fail "mock did not become ready in ${READY_TIMEOUT}s"
    kill -0 "$MOCK_PID" 2>/dev/null || fail "mock process died on startup"
    sleep 0.5
  done
  log "mock ready (t=caps answers)"
}

# ===========================================================================
# 3. Real qBittorrent (Docker), tmpfs /config, ephemeral admin password.
# ===========================================================================
start_qbit() {
  log "starting qBittorrent container $QBIT_NAME"
  docker rm -f "$QBIT_NAME" >/dev/null 2>&1
  docker run -d --name "$QBIT_NAME" \
    --tmpfs /config \
    -e PUID=0 -e PGID=0 -e WEBUI_PORT="$QBIT_PORT" \
    -p "127.0.0.1:${QBIT_PORT}:${QBIT_PORT}" \
    -v "$SAVE_DIR:/downloads" \
    --add-host host.docker.internal:host-gateway \
    lscr.io/linuxserver/qbittorrent:latest >/dev/null \
    || fail "docker run qbittorrent failed"

  # Discover the temporary admin password from the container logs (bounded).
  local deadline=$(( SECONDS + READY_TIMEOUT )) pw=""
  while :; do
    pw="$(docker logs "$QBIT_NAME" 2>&1 \
          | sed -n 's/.*temporary password is provided for this session: //p' \
          | tail -n1 | tr -d '\r')"
    [ -n "$pw" ] && break
    [ $SECONDS -ge $deadline ] && fail "qBittorrent temp password not found in ${READY_TIMEOUT}s"
    docker ps -q --filter "name=$QBIT_NAME" | grep -q . || fail "qBittorrent container exited early"
    sleep 1
  done
  QBIT_PASS="$pw"
  log "qBittorrent temp admin password discovered"

  # Bounded wait for the WebUI to answer + log in.
  deadline=$(( SECONDS + READY_TIMEOUT ))
  until qbit_login; do
    [ $SECONDS -ge $deadline ] && fail "qBittorrent WebUI login failed within ${READY_TIMEOUT}s"
    sleep 1
  done
  log "qBittorrent WebUI reachable + authenticated"

  # Deterministic save path + category so cellarr's category-scoped add lands in
  # /downloads (the bind-mount) under $CATEGORY.
  qbit_api POST "/api/v2/app/setPreferences" \
    --data-urlencode 'json={"save_path":"/downloads","auto_tmm_enabled":false}' >/dev/null \
    || fail "could not set qBittorrent save_path"
  qbit_api POST "/api/v2/torrents/createCategory" \
    --data-urlencode "category=$CATEGORY" --data-urlencode "savePath=/downloads" >/dev/null \
    || log "category create returned non-2xx (may already exist) — continuing"
  log "qBittorrent save_path=/downloads, category=$CATEGORY ready"
}

QBIT_COOKIE=""
qbit_login() {
  local hdrs
  hdrs="$(curl -sS --max-time "$HTTP_TIMEOUT" -D - -o /dev/null \
      --data-urlencode "username=admin" --data-urlencode "password=$QBIT_PASS" \
      -H "Referer: http://127.0.0.1:${QBIT_PORT}" \
      "http://127.0.0.1:${QBIT_PORT}/api/v2/auth/login" 2>/dev/null)" || return 1
  QBIT_COOKIE="$(printf '%s' "$hdrs" | sed -n 's/.*[Ss]et-[Cc]ookie: \(SID=[^;]*\).*/\1/p' | tail -n1)"
  if [ -z "$QBIT_COOKIE" ]; then
    QBIT_COOKIE="$(printf '%s' "$hdrs" | sed -n 's/.*[Ss]et-[Cc]ookie: \(QBT_SID[^;]*\).*/\1/p' | tail -n1)"
  fi
  [ -n "$QBIT_COOKIE" ]
}

qbit_api() {
  local method="$1" path="$2"; shift 2
  curl -sS --max-time "$HTTP_TIMEOUT" -X "$method" \
    -H "Referer: http://127.0.0.1:${QBIT_PORT}" \
    -H "Cookie: $QBIT_COOKIE" \
    "http://127.0.0.1:${QBIT_PORT}${path}" "$@"
}

qbit_torrents_info() { qbit_api GET "/api/v2/torrents/info"; }

# ===========================================================================
# 4. The real cellarr daemon (the `cellarr run` binary), zero-config tempdir.
# ===========================================================================
start_daemon() {
  [ -x "$DAEMON_BIN" ] || fail "daemon binary missing: $DAEMON_BIN (build cellarr-cli first)"
  log "booting real cellarr daemon"
  CELLARR_DATA_DIR="$DATA_DIR" CELLARR_API__BIND="127.0.0.1" CELLARR_API__PORT="0" \
    CELLARR_LOG__FILTER="info,cellarr=debug" \
    "$DAEMON_BIN" run >"$DAEMON_LOG" 2>&1 &
  DAEMON_PID=$!

  # The daemon logs "API listener bound" with the OS-assigned addr; read it back.
  # tracing colorizes the output, so strip ANSI escapes before matching.
  local deadline=$(( SECONDS + READY_TIMEOUT ))
  while :; do
    API_PORT="$(sed $'s/\x1b\\[[0-9;]*m//g' "$DAEMON_LOG" \
      | sed -n 's/.*API listener bound.*addr=127\.0\.0\.1:\([0-9]\{1,\}\).*/\1/p' | tail -n1)"
    [ -n "$API_PORT" ] && [ "$API_PORT" != "0" ] && break
    [ $SECONDS -ge $deadline ] && fail "daemon did not bind an API port in ${READY_TIMEOUT}s"
    kill -0 "$DAEMON_PID" 2>/dev/null || fail "daemon process exited during boot"
    sleep 0.5
  done
  # Bounded wait for the API to actually answer.
  deadline=$(( SECONDS + READY_TIMEOUT ))
  until hc "$(api "/api/v1/system/status")" >/dev/null 2>&1; do
    [ $SECONDS -ge $deadline ] && fail "daemon API not answering in ${READY_TIMEOUT}s"
    sleep 0.5
  done
  log "daemon API up on 127.0.0.1:${API_PORT}"
}

# ===========================================================================
# 5. Seed config: profile + library + monitored episode + FTS + remote-path map
#    (direct SQLite — there is no public route for these), then register the
#    indexer + qBittorrent client via the real HTTP API.
# ===========================================================================
DB="$DATA_DIR/cellarr.sqlite"

sq() { sqlite3 "$DB" "$1"; }
# Retry sqlite writes briefly in case the daemon's pool holds a momentary lock.
sq_w() {
  local q="$1" i
  for i in 1 2 3 4 5 6 7 8 9 10; do
    if sqlite3 "$DB" "$q" 2>/dev/null; then return 0; fi
    sleep 0.3
  done
  sqlite3 "$DB" "$q"  # final attempt surfaces the real error
}

seed_db() {
  [ -f "$DB" ] || fail "daemon DB not found at $DB"
  log "seeding profile/library/content/FTS/remote-path-map into the DB"

  local prof_id lib_id ep_id
  prof_id="$("$PY" -c 'import uuid;print(uuid.uuid4())')"
  lib_id="$("$PY" -c 'import uuid;print(uuid.uuid4())')"
  ep_id="$("$PY" -c 'import uuid;print(uuid.uuid4())')"
  CONTENT_ID="$ep_id"

  # Permissive profile: every rank allowed, no CF floor -> a clean Grab verdict.
  local prof_body
  prof_body="$("$PY" - "$prof_id" <<'PYEOF'
import json, sys
pid = sys.argv[1]
print(json.dumps({
    "id": pid, "name": "e2e-permissive",
    "allowed_qualities": list(range(0, 30)),
    "upgrades_allowed": True, "cutoff_quality": 29,
    "min_custom_format_score": 0, "upgrade_until_custom_format_score": 1000,
    "required_languages": [],
}))
PYEOF
)" || fail "could not build profile body"
  [ -n "$prof_body" ] || fail "empty profile body"
  sq_w "INSERT INTO quality_profile (id,name,body) VALUES ('$prof_id','e2e-permissive','$(echo "$prof_body" | sed "s/'/''/g")');" \
    || fail "seed quality_profile failed"

  sq_w "INSERT INTO library (id,media_type,name,root_folders,default_quality_profile)
        VALUES ('$lib_id','tv','E2E TV','[\"$LIBRARY_ROOT\"]','$prof_id');" \
    || fail "seed library failed"

  # The monitored, file-less episode leaf (a root node: a "series" container has
  # no Coordinates variant, and indexing it in FTS would make the content lookup
  # choke deserializing its coords — so the grabbable episode IS the FTS-indexed
  # node). monitored_missing keys on kind, not on having a parent.
  sq_w "INSERT INTO content (id,library_id,media_type,parent_id,kind,coords,monitored,title_id)
        VALUES ('$ep_id','$lib_id','tv',NULL,'episode','{\"type\":\"episode\",\"season\":1,\"episode\":1}',1,NULL);" \
    || fail "seed episode node failed"

  # FTS title: Identify's content lookup searches FTS with the parsed release
  # title; the episode node's title is the series title the TV module matches on,
  # then keys on the episode coordinates (S01E01).
  sq_w "INSERT INTO content_fts (content_id,title) VALUES ('$ep_id','$SERIES_TITLE');" \
    || fail "seed episode FTS failed"

  # Remote-path mapping: qBittorrent reports /downloads/<file>; cellarr sees it at
  # the host save dir. Empty host matches any client (the daemon leaves client_host empty).
  local rpm_id rpm_body
  rpm_id="$("$PY" -c 'import uuid;print(uuid.uuid4())')"
  rpm_body="$("$PY" - "$rpm_id" "$SAVE_DIR" <<'PYEOF'
import json, sys
rid, local = sys.argv[1], sys.argv[2]
print(json.dumps({"id": rid, "host": "", "remote_path": "/downloads", "local_path": local}))
PYEOF
)"
  sq_w "INSERT INTO remote_path_mapping (id,host,remote_path,local_path,body)
        VALUES ('$rpm_id','','/downloads','$(echo "$SAVE_DIR" | sed "s/'/''/g")','$(echo "$rpm_body" | sed "s/'/''/g")');" \
    || fail "seed remote_path_mapping failed"

  log "seeded content episode id=$CONTENT_ID (library=$lib_id, profile=$prof_id)"
}

register_indexer() {
  log "POST /api/v1/indexers (real Torznab pointing at the mock)"
  local id; id="$("$PY" -c 'import uuid;print(uuid.uuid4())')"
  local base; base="$(mock "/api")"
  hc -X POST "$(api "/api/v1/indexers")" -H 'Content-Type: application/json' \
     -d "$("$PY" - "$id" "$base" <<'PYEOF'
import json, sys
iid, base = sys.argv[1], sys.argv[2]
print(json.dumps({
    "id": iid, "name": "e2e-torznab", "kind": "torznab", "protocol": "torrent",
    "enabled": True, "priority": 1,
    "settings": {"baseUrl": base, "apiKey": ""},
}))
PYEOF
)" >/dev/null || fail "indexer registration failed"
}

register_client() {
  log "POST /api/v1/downloadclients (real qBittorrent)"
  local id; id="$("$PY" -c 'import uuid;print(uuid.uuid4())')"
  hc -X POST "$(api "/api/v1/downloadclients")" -H 'Content-Type: application/json' \
     -d "$("$PY" - "$id" "$QBIT_PORT" "$QBIT_PASS" "$CATEGORY" <<'PYEOF'
import json, sys
cid, port, pw, cat = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
print(json.dumps({
    "id": cid, "name": "e2e-qbit", "kind": "qbittorrent", "protocol": "torrent",
    "enabled": True, "priority": 1, "category": cat,
    "settings": {"base_url": f"http://127.0.0.1:{port}", "username": "admin", "password": pw},
}))
PYEOF
)" >/dev/null || fail "download client registration failed"
}

# ===========================================================================
# 6. Trigger the search via /api/v3/command, then bounded-poll the chain.
# ===========================================================================
trigger_search() {
  log "POST /api/v3/command ManualSearch for content $CONTENT_ID"
  hc -X POST "$(api "/api/v3/command")" -H 'Content-Type: application/json' \
     -d "{\"name\":\"ManualSearch\",\"seriesId\":\"$CONTENT_ID\"}" >/dev/null \
     || fail "command trigger failed"
}

assert_mock_received_search() {
  log "verifying the mock received a search request"
  local rl="$WORK/release.torrent.requests.log"
  local deadline=$(( SECONDS + POLL_TIMEOUT ))
  until [ -f "$rl" ] && grep -Eq 't=(search|tvsearch|movie)' "$rl"; do
    [ $SECONDS -ge $deadline ] && fail "mock never received a search (caps-only or no traffic)"
    sleep 1
  done
  EV_MOCK="$(grep -Eo 't=(search|tvsearch|movie)[^ ]*' "$rl" | head -n1)"
  log "OK mock-search-received: $EV_MOCK"
}

assert_qbit_completed() {
  log "verifying qBittorrent has the torrent in category=$CATEGORY and it Completes"
  local deadline=$(( SECONDS + POLL_TIMEOUT )) info state cat prog rechecked=""
  local first_seen=0
  while :; do
    info="$(qbit_api GET "/api/v2/torrents/info?hashes=$INFOHASH" 2>/dev/null)"
    if [ -n "$info" ] && [ "$info" != "[]" ]; then
      cat="$("$PY" -c 'import json,sys;d=json.load(sys.stdin);print(d[0].get("category","")) if d else print("")' <<<"$info" 2>/dev/null)"
      state="$("$PY" -c 'import json,sys;d=json.load(sys.stdin);print(d[0].get("state","")) if d else print("")' <<<"$info" 2>/dev/null)"
      prog="$("$PY" -c 'import json,sys;d=json.load(sys.stdin);print(d[0].get("progress",0)) if d else print(0)' <<<"$info" 2>/dev/null)"
      if [ -z "${EV_QBIT_SEEN:-}" ]; then EV_QBIT_SEEN="state=$state category=$cat"; first_seen=$SECONDS; log "qbit-has-torrent: $EV_QBIT_SEEN"; fi
      # Complete = a seeding/up state OR 100% progress (matches the qbit adapter's
      # own progress->Completed mapping in cellarr-download/qbittorrent.rs).
      local done100; done100="$("$PY" -c "import sys;print('1' if float('$prog' or 0)>=1.0 else '0')" 2>/dev/null)"
      case "$state" in
        uploading|stalledUP|pausedUP|forcedUP|queuedUP|checkingUP)
          EV_QBIT_DONE="state=$state category=$cat progress=$prog"; log "OK qbit-completed: $EV_QBIT_DONE"; return 0 ;;
        error|missingFiles)
          fail "qBittorrent torrent entered terminal failure state: $state" ;;
      esac
      if [ "$done100" = "1" ]; then
        EV_QBIT_DONE="state=$state category=$cat progress=$prog"; log "OK qbit-completed: $EV_QBIT_DONE"; return 0
      fi
    fi
    [ $SECONDS -ge $deadline ] && fail "qBittorrent torrent did not complete in ${POLL_TIMEOUT}s"
    # ONE recheck nudge, fired only AFTER the add-time auto-check has had a grace
    # period to settle (rechecking *during* checkingResumeData/checkingDL just
    # restarts the check and never finishes — the failure we debugged). With the
    # data pre-staged, the post-grace recheck validates the single piece in ~1s.
    if [ -z "$rechecked" ] && [ "$first_seen" -gt 0 ] && [ $(( SECONDS - first_seen )) -ge 12 ]; then
      rechecked=1
      log "nudging a one-shot recheck (state=$state, grace elapsed)"
      qbit_api POST "/api/v2/torrents/recheck" --data-urlencode "hashes=$INFOHASH" >/dev/null 2>&1 || true
    fi
    sleep 3
  done
}

assert_cellarr_imported() {
  log "verifying cellarr imported the file + recorded Imported/history/decision-log"
  local deadline=$(( SECONDS + POLL_TIMEOUT )) imported=""
  # The grab reaches Imported, recorded in this content's history.
  while :; do
    local hist; hist="$(hc "$(api "/api/v1/content/$CONTENT_ID/history")" 2>/dev/null || echo '[]')"
    # HistoryEvent serializes its tag snake_case: the import event is "imported".
    if echo "$hist" | grep -q '"event":"imported"'; then break; fi
    [ $SECONDS -ge $deadline ] && { capture_daemon_pipeline; fail "cellarr did not record an imported history event in ${POLL_TIMEOUT}s"; }
    sleep 2
  done

  # The grab row's terminal status is Imported (queried via the v3 queue/native).
  # We read the grab via the daemon log + DB to capture the exact landed path.
  imported="$(find "$LIBRARY_ROOT" -type f -print 2>/dev/null | head -n1)"
  [ -n "$imported" ] || fail "no imported file found under the library root $LIBRARY_ROOT"
  EV_IMPORT_PATH="$imported"
  log "OK imported-file-path: $EV_IMPORT_PATH"

  # Content is the payload (proves it is the same bytes qBittorrent completed).
  grep -q 'CELLARR-E2E-PAYLOAD' "$imported" || fail "imported file is not the staged payload"

  # The on-disk media_file is linked (node no longer monitored-missing) and the
  # grab status is Imported, read straight from the DB the daemon wrote.
  local grab_status decision_count
  grab_status="$(sq "SELECT status FROM grab ORDER BY created_at DESC LIMIT 1;")"
  # GrabStatus is stored snake_case; Imported -> "imported".
  [ "$grab_status" = "imported" ] || fail "grab status is '$grab_status', expected imported"
  EV_GRAB_STATUS="$grab_status"
  decision_count="$(sq "SELECT COUNT(*) FROM decision_log;")"
  [ "${decision_count:-0}" -gt 0 ] || fail "no decision_log rows were written"
  log "OK grab Imported; decision_log rows=$decision_count"
}

capture_daemon_pipeline() {
  log "pipeline diagnostics:"
  grep -Ei 'pipeline|grab|import|reject|held|decision|error|warn' "$DAEMON_LOG" | tail -n 30 >&2 || true
  log "grab rows:"; sq "SELECT status, download_id FROM grab;" >&2 || true
}

# ===========================================================================
# Drive it.
# ===========================================================================
main() {
  log "=== cellarr live e2e (RUN_ID=$RUN_ID) — work dir $WORK ==="
  prepare_payload
  start_mock
  start_qbit
  start_daemon
  seed_db
  register_indexer
  register_client
  trigger_search
  assert_mock_received_search
  assert_qbit_completed
  assert_cellarr_imported

  log "================= E2E CHAIN PROVEN ================="
  log "  mock-search-received : ${EV_MOCK}"
  log "  qbit-has-torrent     : ${EV_QBIT_SEEN}"
  log "  qbit-completed       : ${EV_QBIT_DONE}"
  log "  imported-file-path   : ${EV_IMPORT_PATH}"
  log "  grab status          : ${EV_GRAB_STATUS}"
  log "==================================================="
  # Machine-readable summary line for the harness.
  echo "E2E_OK mock=[${EV_MOCK}] qbit=[${EV_QBIT_DONE}] import=[${EV_IMPORT_PATH}] grab=[${EV_GRAB_STATUS}]"
}

# The whole live e2e is itself wall-clock bounded: a watchdog hard-kills us.
( sleep 480; log "WATCHDOG: 8min wall-clock exceeded — killing"; kill -TERM $$ 2>/dev/null ) &
WATCHDOG=$!
main
kill "$WATCHDOG" 2>/dev/null
