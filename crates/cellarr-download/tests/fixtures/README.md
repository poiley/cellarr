# Download-client record/replay fixtures

These fixtures drive the contract tests in `crates/cellarr-download/tests/`. They
let the adapters be exercised through their **full lifecycle** with **no live
download client** in CI, as required by `docs/06-integrations.md`.

## SYNTHETIC — not captured from live services

Every fixture in this tree is **hand-authored from the documented protocol
shapes**, not recorded from a running client:

- qBittorrent: WebUI API v2 (`/api/v2/auth/login`, `/torrents/add`,
  `/torrents/info`, `/torrents/delete`), `SID` cookie auth, `Referer`/`Origin`
  headers, and the two divergent 5.x login success responses.
- SABnzbd: `mode=`/`apikey=`/`output=json` HTTP API (`addurl`, `queue`,
  `history`).
- NZBGet: JSON-RPC 2.0 positional params over `/jsonrpc` (`append`,
  `listgroups`, `history`, `editqueue`) with HTTP Basic auth.

Ids, paths, hashes, and api keys are made up. They reproduce the *shape and
field names* documented for each API so the parsers and lifecycle logic are
pinned, without asserting anything about a specific real-world deployment.

## Fixture format

Each `*.json` file is one scenario: an ordered list of exchanges. The replay
transport (see `tests/common/mod.rs`) pops the next exchange per HTTP call,
asserts the adapter's request matches `expect`, and returns `response`.

```jsonc
{
  "scenario": "human description",
  "exchanges": [
    {
      "expect": {                 // all fields optional; only present ones are asserted
        "method": "POST",
        "url_contains": "/api/v2/auth/login",
        "header_equals": { "referer": "http://localhost:8080" },
        "body_contains": "username=admin"
      },
      "response": { "status": 200, "headers": { "set-cookie": "SID=abc; ..." }, "body": "Ok." }
    }
  ]
}
```
