# cellarr task runner. `just` to list recipes.
#
# ISOLATION MODEL (see docs/16-local-dev-and-testing.md):
#   Every workflow/worktree gets a unique RUN_ID (its folder name by default). All ephemeral
#   resources are namespaced by RUN_ID and all long-lived servers use a per-run PORT BLOCK, so
#   many workflows can test their own versions on this machine at once without colliding.

set shell := ["bash", "-uc"]

# Toolchains are mise-managed (.mise.toml) and not globally installed; put mise's shims on PATH
# so `cargo`/`npm`/`node` resolve inside recipes (mise selects the pinned version per .mise.toml).
export PATH := env_var('HOME') + "/.local/share/mise/shims:" + env_var('PATH')

# RUN_ID: explicit env wins, else the git worktree's folder name (sanitized).
run_id := env_var_or_default("CELLARR_RUN_ID", `basename "$(git rev-parse --show-toplevel 2>/dev/null || pwd)" | tr -c 'a-zA-Z0-9_' '_' | sed 's/_*$//'`)

# Deterministic per-run base port in [20000, 59990], stepped by 10. Derived from RUN_ID so a
# given worktree always gets the same block (stable across restarts), but different worktrees differ.
port_base := `python3 -c "import hashlib,sys; print(20000 + (int(hashlib.sha1(sys.argv[1].encode()).hexdigest(),16)%4000)*10)" "{{run_id}}"`

# Per-run scratch dir for manual `just dev` runs (gitignored).
run_dir := "./.run/" + run_id

_default:
    @just --list

# --- one-time setup -------------------------------------------------------------------------

# Install toolchains and web deps.
setup:
    mise install
    @# cargo-nextest: prebuilt tarball into ~/.cargo/bin (shared by all worktrees). See .mise.toml.
    @if ! command -v cargo-nextest >/dev/null 2>&1; then \
        echo "installing cargo-nextest..."; \
        curl -LsSf https://get.nexte.st/latest/mac | tar zxf - -C "$HOME/.cargo/bin"; \
    fi
    @if [ -d web ]; then cd web && npm install; else echo "web/ not present yet (pre Phase 6)"; fi
    @echo "RUN_ID={{run_id}}  PORT_BASE={{port_base}}"

# Show the ports/IDs allocated to THIS run/worktree.
ports:
    @echo "RUN_ID    = {{run_id}}"
    @echo "PORT_BASE = {{port_base}}"
    @echo "  api     = $(({{port_base}}+0))"
    @echo "  meta    = $(({{port_base}}+1))"
    @echo "  web     = $(({{port_base}}+2))"
    @echo "  mock    = $(({{port_base}}+3))"
    @echo "pg/oracle containers use ephemeral host ports + per-run names (see docs/16)."

# --- tests ----------------------------------------------------------------------------------

# Fast, hermetic: full workspace test suite (SQLite + record/replay; NO Docker, NO ports).
# Safe to run in many worktrees simultaneously: each has its own target/ and tests use tempdirs.
test *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ ! -f Cargo.toml ]; then echo "no crates yet (pre Phase 0) — nothing to test"; exit 0; fi
    if command -v cargo-nextest >/dev/null 2>&1; then
        cargo nextest run --workspace {{ARGS}}
        cargo test --workspace --doc {{ARGS}}   # nextest skips doctests; run them separately
    else
        echo "note: cargo-nextest not found — falling back to 'cargo test' (run 'just setup' for the faster runner)"
        cargo test --workspace {{ARGS}}
    fi

# Lint + format gates.
lint:
    @if [ ! -f Cargo.toml ]; then echo "no crates yet (pre Phase 0)"; exit 0; fi
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings

# Repository tests against an EPHEMERAL, per-run Postgres (unique container + ephemeral host port).
# Multiple runs never collide: container is named cellarr-pg-<RUN_ID>.
#
# DEFERRED in v1: Postgres is a post-v1 opt-in (SQLite is the v1 default — docs/08-database.md,
# docs/14-roadmap.md). The `postgres` cargo feature currently only enables the sqlx driver; the
# repository layer is not yet PG-dialect complete, so this harness is gated off by default. Set
# CELLARR_ENABLE_PG_TESTS=1 once the PG repositories land. The full harness below is kept ready.
test-pg *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "${CELLARR_ENABLE_PG_TESTS:-0}" != "1" ]; then
        echo "test-pg: Postgres backend is deferred to post-v1 (SQLite is the v1 default)."
        echo "         The repository layer is SQLite-only for now; skipping."
        echo "         Set CELLARR_ENABLE_PG_TESTS=1 to run this once PG repos land."
        exit 0
    fi
    if [ ! -f Cargo.toml ]; then echo "no crates yet (pre Phase 0)"; exit 0; fi
    name="cellarr-pg-{{run_id}}"
    trap 'docker rm -f "$name" >/dev/null 2>&1 || true' EXIT
    docker rm -f "$name" >/dev/null 2>&1 || true
    docker run -d --name "$name" -e POSTGRES_PASSWORD=cellarr -e POSTGRES_DB=cellarr \
        -p 0:5432 postgres:17-alpine >/dev/null
    # discover the OS-allocated host port
    for _ in $(seq 1 30); do
        hostport="$(docker port "$name" 5432/tcp 2>/dev/null | head -1 | sed 's/.*://')" || true
        [ -n "${hostport:-}" ] && break; sleep 0.5
    done
    until docker exec "$name" pg_isready -U postgres >/dev/null 2>&1; do sleep 0.5; done
    export CELLARR_TEST_DATABASE_URL="postgres://postgres:cellarr@127.0.0.1:${hostport}/cellarr"
    echo "Postgres for run {{run_id}} at 127.0.0.1:${hostport}"
    cargo test --workspace --features postgres {{ARGS}}

# --- differential oracle (pinned Sonarr/Radarr in Docker) -----------------------------------

# Run the parser parity oracle: bring up pinned Sonarr/Radarr, extract their API
# keys, point the harness at them, and diff cellarr's parser. Per-run compose
# project + ephemeral ports → many runs coexist. Results: target/parity/ +
# docs/parity/. See docs/parity/methodology.md.
oracle *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    compose="tests/oracle/docker-compose.yml"
    if [ ! -f "$compose" ]; then echo "oracle compose not present"; exit 0; fi
    proj="cellarr-oracle-{{run_id}}"
    trap 'docker compose -p "$proj" -f "$compose" down -v >/dev/null 2>&1 || true' EXIT
    docker compose -p "$proj" -f "$compose" up -d
    sonarr="$(docker compose -p "$proj" -f "$compose" port sonarr 8989 | sed 's/.*://')"
    radarr="$(docker compose -p "$proj" -f "$compose" port radarr 7878 | sed 's/.*://')"
    sc=$(docker compose -p "$proj" -f "$compose" ps -q sonarr)
    rc=$(docker compose -p "$proj" -f "$compose" ps -q radarr)
    echo "waiting for Sonarr/Radarr first boot + API keys..."
    SK=""; RK=""
    for _ in $(seq 1 60); do
      SK=$(docker exec "$sc" sed -n 's:.*<ApiKey>\(.*\)</ApiKey>.*:\1:p' /config/config.xml 2>/dev/null || true)
      RK=$(docker exec "$rc" sed -n 's:.*<ApiKey>\(.*\)</ApiKey>.*:\1:p' /config/config.xml 2>/dev/null || true)
      [ -n "$SK" ] && [ -n "$RK" ] && break; sleep 2
    done
    [ -z "$SK" ] || [ -z "$RK" ] && { echo "API keys not ready"; exit 1; }
    export CELLARR_ORACLE_SONARR="http://127.0.0.1:${sonarr}" CELLARR_ORACLE_SONARR_KEY="$SK"
    export CELLARR_ORACLE_RADARR="http://127.0.0.1:${radarr}" CELLARR_ORACLE_RADARR_KEY="$RK"
    # wait until both APIs answer
    for _ in $(seq 1 60); do
      a=$(curl -s -o /dev/null -w '%{http_code}' -H "X-Api-Key: $SK" "$CELLARR_ORACLE_SONARR/api/v3/system/status" || true)
      b=$(curl -s -o /dev/null -w '%{http_code}' -H "X-Api-Key: $RK" "$CELLARR_ORACLE_RADARR/api/v3/system/status" || true)
      [ "$a" = "200" ] && [ "$b" = "200" ] && break; sleep 2
    done
    echo "oracle {{run_id}}: sonarr=${sonarr} radarr=${radarr}"
    cargo test -p cellarr-parse --test oracle -- --ignored --nocapture {{ARGS}}

# Custom-format MATCHING oracle: configure a CF set in a live Sonarr, import the same
# into cellarr, and diff matched-CF sets over the corpus. See docs/parity/decision-gaps.md.
oracle-cf *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    name="cellarr-oracle-cf-{{run_id}}"
    trap 'docker rm -f "$name" >/dev/null 2>&1 || true' EXIT
    docker rm -f "$name" >/dev/null 2>&1 || true
    docker run -d --name "$name" -p 0:8989 --tmpfs /config \
        lscr.io/linuxserver/sonarr@sha256:02bc962946fef994e67a38152446df25c10a52f8583aefeeb6467f9dd44cab99 >/dev/null
    port=$(docker port "$name" 8989/tcp | head -1 | sed 's/.*://')
    for _ in $(seq 1 60); do
      SK=$(docker exec "$name" sed -n 's:.*<ApiKey>\(.*\)</ApiKey>.*:\1:p' /config/config.xml 2>/dev/null || true)
      [ -n "${SK:-}" ] && break; sleep 2
    done
    for _ in $(seq 1 60); do
      a=$(curl -s -o /dev/null -w '%{http_code}' -H "X-Api-Key: $SK" "http://127.0.0.1:${port}/api/v3/system/status" || true)
      [ "$a" = "200" ] && break; sleep 2
    done
    export CELLARR_ORACLE_SONARR="http://127.0.0.1:${port}" CELLARR_ORACLE_SONARR_KEY="$SK"
    cargo test -p cellarr-decide --test oracle_cf -- --ignored --nocapture {{ARGS}}

# Custom-format SCORE oracle: configure a CF set + a scored quality profile in a live Sonarr,
# import the same CFs+scores into cellarr, and diff per-title CF scores over the corpus.
# Sonarr's score = Σ(profile formatItems score of each CF its /api/v3/parse matched).
oracle-cf-score *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    name="cellarr-oracle-cfscore-{{run_id}}"
    trap 'docker rm -f "$name" >/dev/null 2>&1 || true' EXIT
    docker rm -f "$name" >/dev/null 2>&1 || true
    docker run -d --name "$name" -p 0:8989 --tmpfs /config \
        lscr.io/linuxserver/sonarr@sha256:02bc962946fef994e67a38152446df25c10a52f8583aefeeb6467f9dd44cab99 >/dev/null
    port=$(docker port "$name" 8989/tcp | head -1 | sed 's/.*://')
    for _ in $(seq 1 60); do
      SK=$(docker exec "$name" sed -n 's:.*<ApiKey>\(.*\)</ApiKey>.*:\1:p' /config/config.xml 2>/dev/null || true)
      [ -n "${SK:-}" ] && break; sleep 2
    done
    for _ in $(seq 1 60); do
      a=$(curl -s -o /dev/null -w '%{http_code}' -H "X-Api-Key: $SK" "http://127.0.0.1:${port}/api/v3/system/status" || true)
      [ "$a" = "200" ] && break; sleep 2
    done
    export CELLARR_ORACLE_SONARR="http://127.0.0.1:${port}" CELLARR_ORACLE_SONARR_KEY="$SK"
    cargo test -p cellarr-decide --test oracle_cf_score -- --ignored --nocapture {{ARGS}}

# Full-corpus REAL-TRaSH-set CF MATCH + SCORE oracle: POST the entire TRaSH Sonarr CF
# set into a live Sonarr and the Radarr set into a live Radarr, import the same sets
# into cellarr, and diff matched-CF sets + scores over the whole corpus (routed by path).
# Results: target/parity/trash-cf-*. See docs/parity/decision-gaps.md.
oracle-trash-cf *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    sname="cellarr-sonarr-gt-{{run_id}}"
    rname="cellarr-radarr-gt-{{run_id}}"
    trap 'docker rm -f "$sname" "$rname" >/dev/null 2>&1 || true' EXIT
    docker rm -f "$sname" "$rname" >/dev/null 2>&1 || true
    docker run -d --name "$sname" -p 0:8989 --tmpfs /config \
        lscr.io/linuxserver/sonarr@sha256:02bc962946fef994e67a38152446df25c10a52f8583aefeeb6467f9dd44cab99 >/dev/null
    docker run -d --name "$rname" -p 0:7878 --tmpfs /config \
        lscr.io/linuxserver/radarr:latest >/dev/null
    sport=$(docker port "$sname" 8989/tcp | head -1 | sed 's/.*://')
    rport=$(docker port "$rname" 7878/tcp | head -1 | sed 's/.*://')
    SK=""; RK=""
    for _ in $(seq 1 30); do
      SK=$(docker exec "$sname" sed -n 's:.*<ApiKey>\(.*\)</ApiKey>.*:\1:p' /config/config.xml 2>/dev/null || true)
      RK=$(docker exec "$rname" sed -n 's:.*<ApiKey>\(.*\)</ApiKey>.*:\1:p' /config/config.xml 2>/dev/null || true)
      [ -n "${SK:-}" ] && [ -n "${RK:-}" ] && break; sleep 2
    done
    [ -z "$SK" ] || [ -z "$RK" ] && { echo "API keys not ready"; exit 1; }
    for _ in $(seq 1 30); do
      a=$(curl -s -o /dev/null -w '%{http_code}' -H "X-Api-Key: $SK" "http://127.0.0.1:${sport}/api/v3/system/status" || true)
      b=$(curl -s -o /dev/null -w '%{http_code}' -H "X-Api-Key: $RK" "http://127.0.0.1:${rport}/api/v3/system/status" || true)
      [ "$a" = "200" ] && [ "$b" = "200" ] && break; sleep 2
    done
    export CELLARR_ORACLE_SONARR="http://127.0.0.1:${sport}" CELLARR_ORACLE_SONARR_KEY="$SK"
    export CELLARR_ORACLE_RADARR="http://127.0.0.1:${rport}" CELLARR_ORACLE_RADARR_KEY="$RK"
    echo "oracle-trash-cf {{run_id}}: sonarr=${sport} radarr=${rport}"
    cargo test -p cellarr-decide --test oracle_trash_cf -- --ignored --nocapture {{ARGS}}

# --- live end-to-end (Docker-gated; NOT part of `just ci`) ----------------------------------

# Full live e2e: a real Torznab mock + a real qBittorrent (Docker) + the real
# `cellarr run` daemon, proving the whole chain search->grab->track->import.
# DETERMINISTIC (the payload is pre-staged so the torrent rechecks to Completed
# in seconds) and HARD-BOUNDED (every wait is capped; an 8-min watchdog kills it).
# Tears down all qbittorrent-cellarr-* containers + the daemon on any exit.
# Requires Docker; gated out of `just ci`.
e2e: (_build-cli)
    tests/e2e/run.sh

# Build just the daemon binary the e2e drives (debug).
_build-cli:
    cargo build -p cellarr-cli

# --- web ------------------------------------------------------------------------------------

# Typecheck + component tests + the SRCL-only lint.
web-test:
    @if [ ! -d web ]; then echo "no web/ yet (pre Phase 6)"; exit 0; fi
    cd web && npm run typecheck && npm run test && npm run lint:srcl-only

# --- full stack (manual) --------------------------------------------------------------------

# Run the daemon + web for THIS run on its own port block, data under ./.run/<RUN_ID> (gitignored).
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ ! -f Cargo.toml ]; then echo "no crates yet (pre Phase 0)"; exit 0; fi
    mkdir -p "{{run_dir}}"
    export CELLARR_DATA_DIR="{{run_dir}}"
    export CELLARR_API_PORT="$(({{port_base}}+0))"
    export CELLARR_META_PORT="$(({{port_base}}+1))"
    export CELLARR_WEB_PORT="$(({{port_base}}+2))"
    echo "cellarr dev [{{run_id}}] api=$CELLARR_API_PORT meta=$CELLARR_META_PORT web=$CELLARR_WEB_PORT data={{run_dir}}"
    cargo run -p cellarr-cli -- --data-dir "{{run_dir}}" --api-port "$CELLARR_API_PORT"

# --- full gate + cleanup --------------------------------------------------------------------

# The full CI gate (what a PR must pass). See docs/agents/definition-of-done.md.
ci: lint test test-pg web-test
    @echo "ci gate complete for run {{run_id}}"

# Tear down everything this run created: containers, the scratch dir.
clean-run:
    -docker rm -f "cellarr-pg-{{run_id}}" >/dev/null 2>&1
    -docker rm -f "cellarr-sonarr-gt-{{run_id}}" "cellarr-radarr-gt-{{run_id}}" >/dev/null 2>&1
    -docker compose -p "cellarr-oracle-{{run_id}}" -f tests/oracle/docker-compose.yml down -v >/dev/null 2>&1
    -rm -rf "{{run_dir}}"
    @echo "cleaned run {{run_id}}"
