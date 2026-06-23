# cellarr task runner. `just` to list recipes.
#
# ISOLATION MODEL (see docs/16-local-dev-and-testing.md):
#   Every workflow/worktree gets a unique RUN_ID (its folder name by default). All ephemeral
#   resources are namespaced by RUN_ID and all long-lived servers use a per-run PORT BLOCK, so
#   many workflows can test their own versions on this machine at once without colliding.

set shell := ["bash", "-uc"]

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

# Install toolchains, wire the no-push git hook, install web deps.
setup:
    mise install
    git config core.hooksPath .githooks
    @echo "pre-push guard wired (pushing is blocked by policy — see CLAUDE.md)"
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
    @if [ ! -f Cargo.toml ]; then echo "no crates yet (pre Phase 0) — nothing to test"; exit 0; fi
    cargo test --workspace {{ARGS}}

# Lint + format gates.
lint:
    @if [ ! -f Cargo.toml ]; then echo "no crates yet (pre Phase 0)"; exit 0; fi
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings

# Repository tests against an EPHEMERAL, per-run Postgres (unique container + ephemeral host port).
# Multiple runs never collide: container is named cellarr-pg-<RUN_ID>.
test-pg *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
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

# Run the parity oracle. Per-run compose project + ephemeral ports → many runs coexist.
oracle *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    compose="tests/oracle/docker-compose.yml"
    if [ ! -f "$compose" ]; then echo "oracle compose not present yet (Phase 0 deliverable)"; exit 0; fi
    proj="cellarr-oracle-{{run_id}}"
    trap 'docker compose -p "$proj" -f "$compose" down -v >/dev/null 2>&1 || true' EXIT
    docker compose -p "$proj" -f "$compose" up -d
    sonarr="$(docker compose -p "$proj" -f "$compose" port sonarr 8989 | sed 's/.*://')"
    radarr="$(docker compose -p "$proj" -f "$compose" port radarr 7878 | sed 's/.*://')"
    export CELLARR_ORACLE_SONARR="http://127.0.0.1:${sonarr}"
    export CELLARR_ORACLE_RADARR="http://127.0.0.1:${radarr}"
    echo "oracle {{run_id}}: sonarr=${sonarr} radarr=${radarr}"
    cargo test --workspace --features oracle -- --ignored oracle {{ARGS}}

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
    -docker compose -p "cellarr-oracle-{{run_id}}" -f tests/oracle/docker-compose.yml down -v >/dev/null 2>&1
    -rm -rf "{{run_dir}}"
    @echo "cleaned run {{run_id}}"
