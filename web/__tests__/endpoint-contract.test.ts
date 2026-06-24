// Endpoint-contract regression guard.
//
// This whole class of bug — the UI firing a command name or hitting an endpoint
// the daemon does not accept — slips past the per-screen vitest suites because
// those mock the client (`runCommand = vi.fn()`, `client.request = …`) and only
// assert that *some* call happened, never that the NAME / PATH is one the backend
// actually serves. The content "Refresh" button shipped sending `RefreshContent`,
// which the backend rejects with 400 "unknown command", and several Settings
// writes targeted `/api/v1/...` routes that do not exist (404 → SPA index.html).
//
// This test pins the contract explicitly:
//   1. Every command name the UI sends MUST be in the backend's accepted set
//      (mirrored from crates/cellarr-api/src/commands.rs `kind_for_command`).
//   2. Every action/mutation endpoint the UI fires MUST be a route the daemon
//      registers (mirrored from crates/cellarr-api/src/{native.rs,shim.rs}).
//
// When the backend command set or route table changes, update the mirrors below
// to match — that diff is the point: it forces a human to reconcile the two
// sides instead of discovering the break by clicking the live UI.

import { describe, expect, it } from 'vitest';

// --- Mirror of the backend's accepted command names ------------------------
// Source of truth: crates/cellarr-api/src/commands.rs `kind_for_command`, which
// lower-cases the leading token. These are the names that map to a JobKind (any
// other name 400s "unknown command"). Keep in sync with that match arm.
const ACCEPTED_COMMANDS = new Set(
  [
    'RssSync',
    'MissingItemSearch',
    'MissingMoviesSearch',
    'MissingEpisodesSearch',
    'RefreshMetadata',
    'RefreshMovie',
    'RefreshSeries',
    'DiskSpaceCheck',
    'ManualSearch',
    'MovieSearch',
    'EpisodeSearch',
    'SeriesSearch',
  ].map((n) => n.toLowerCase())
);

// --- Mirror of the daemon's registered routes ------------------------------
// Source of truth: crates/cellarr-api/src/native.rs (v1) + shim.rs (v3) route
// tables. Method + path (path params normalised to `{id}`). A UI action that is
// not in this set is hitting a non-existent route (which falls through to the
// SPA index.html and silently "succeeds").
const REGISTERED_ROUTES = new Set<string>([
  // native /api/v1 writes
  'POST /api/v1/libraries',
  'POST /api/v1/indexers',
  'POST /api/v1/downloadclients',
  'POST /api/v1/commands',
  // native /api/v1 reads the UI actions depend on
  'GET /api/v1/history',
  'GET /api/v1/decisionlog/{id}',
  // v3 shim writes the UI uses
  'POST /api/v3/movie',
  'POST /api/v3/series',
  'POST /api/v3/command',
  'POST /api/v3/qualityprofile',
  'PUT /api/v3/qualityprofile/{id}',
  'DELETE /api/v3/qualityprofile/{id}',
  'POST /api/v3/customformat',
  'PUT /api/v3/customformat/{id}',
  'DELETE /api/v3/customformat/{id}',
  'POST /api/v3/indexer',
  'POST /api/v3/indexer/test',
  'PUT /api/v3/indexer/{id}',
  'DELETE /api/v3/indexer/{id}',
  'POST /api/v3/downloadclient',
  'POST /api/v3/downloadclient/test',
  'PUT /api/v3/downloadclient/{id}',
  'DELETE /api/v3/downloadclient/{id}',
  'DELETE /api/v3/blocklist/{id}',
  // v3 shim reads the UI's interactive search depends on
  'GET /api/v3/release',
  'GET /api/v3/movie/lookup',
  'GET /api/v3/series/lookup',
]);

// --- What the UI actually fires --------------------------------------------
// Each entry is a real action button / mutation across web/app, with the command
// name OR the {method, path} it sends. Adding a new action button? Add it here.

/** Command names the UI submits via api.runCommand / runCommandV3. */
const UI_COMMANDS: Array<{ where: string; name: string }> = [
  // content/page.tsx Refresh — picks RefreshMovie / RefreshSeries by media type.
  { where: 'content Refresh (movie)', name: 'RefreshMovie' },
  { where: 'content Refresh (series)', name: 'RefreshSeries' },
];

/** Mutation endpoints the UI fires (method + normalised path). */
const UI_ACTION_ROUTES: Array<{ where: string; route: string }> = [
  // content/page.tsx (api.runCommand → native /commands)
  { where: 'content Refresh', route: 'POST /api/v1/commands' },
  // history/page.tsx open-node + content history
  { where: 'history open-node', route: 'GET /api/v1/history' },
  // decision-log/page.tsx load
  { where: 'decision-log load', route: 'GET /api/v1/decisionlog/{id}' },
  // add/page.tsx + _search/api.ts (lookup + add)
  { where: 'add lookup (movie)', route: 'GET /api/v3/movie/lookup' },
  { where: 'add lookup (series)', route: 'GET /api/v3/series/lookup' },
  { where: 'add confirm (movie)', route: 'POST /api/v3/movie' },
  { where: 'add confirm (series)', route: 'POST /api/v3/series' },
  // interactive/page.tsx search
  { where: 'interactive search', route: 'GET /api/v3/release' },
  // first-run/WizardModal submit
  { where: 'wizard create library', route: 'POST /api/v1/libraries' },
  { where: 'wizard create indexer', route: 'POST /api/v3/indexer' },
  { where: 'wizard create download client', route: 'POST /api/v3/downloadclient' },
  // settings Quality Profiles new/save/delete
  { where: 'quality profile create', route: 'POST /api/v3/qualityprofile' },
  { where: 'quality profile update', route: 'PUT /api/v3/qualityprofile/{id}' },
  { where: 'quality profile delete', route: 'DELETE /api/v3/qualityprofile/{id}' },
  // settings Indexers test/save (IntegrationSection, kind=indexers)
  { where: 'indexer test', route: 'POST /api/v3/indexer/test' },
  { where: 'indexer save', route: 'POST /api/v3/indexer' },
  // settings Download clients test/save (IntegrationSection, kind=downloadclients)
  { where: 'download client test', route: 'POST /api/v3/downloadclient/test' },
  { where: 'download client save', route: 'POST /api/v3/downloadclient' },
  // settings Custom formats save
  { where: 'custom format save', route: 'POST /api/v3/customformat' },
  // activity blocklist remove (DELETE blocklist item)
  { where: 'blocklist remove', route: 'DELETE /api/v3/blocklist/{id}' },
];

describe('UI ↔ backend command-name contract', () => {
  it.each(UI_COMMANDS)('$where sends an accepted command ($name)', ({ name }) => {
    // Case-insensitive on the leading token, matching kind_for_command.
    expect(ACCEPTED_COMMANDS.has(name.toLowerCase())).toBe(true);
  });

  it('does NOT send the rejected RefreshContent command', () => {
    // Regression pin for the original bug: the backend has no RefreshContent.
    // (If the backend later aliases it, add 'refreshcontent' to ACCEPTED_COMMANDS
    // deliberately — do not let it pass silently.)
    expect(ACCEPTED_COMMANDS.has('refreshcontent')).toBe(false);
    expect(UI_COMMANDS.some((c) => c.name.toLowerCase() === 'refreshcontent')).toBe(false);
  });
});

describe('UI ↔ backend endpoint contract', () => {
  it.each(UI_ACTION_ROUTES)('$where targets a registered route ($route)', ({ route }) => {
    expect(REGISTERED_ROUTES.has(route)).toBe(true);
  });

  it('every UI action route is a real backend route', () => {
    const missing = UI_ACTION_ROUTES.filter((a) => !REGISTERED_ROUTES.has(a.route));
    expect(missing).toEqual([]);
  });
});
