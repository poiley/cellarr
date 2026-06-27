import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';

import { CellarrClient } from '@lib/api/client';
import RootFolders from '@app/settings/_components/RootFolders';
import CustomFormats from '@app/settings/_components/CustomFormats';
import QualityProfiles from '@app/settings/_components/QualityProfiles';
import IntegrationSection from '@app/settings/_components/IntegrationSection';
import ReleaseProfiles from '@app/settings/_components/ReleaseProfiles';
import DelayProfiles from '@app/settings/_components/DelayProfiles';
import Notifications from '@app/settings/_components/Notifications';
import ImportLists from '@app/settings/_components/ImportLists';
import { MANAGED_BADGE_LABEL } from '@app/settings/_components/ManagedBadge';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

// Most settings screens read a single list whose elements carry the additive
// read-only `managed` flag directly. A helper that answers a given list for the
// screen's primary GET and `[]` for any side catalogue (tags / quality defs /
// schema) keeps these reads order-independent.
function listOnly(matchPath: string, list: unknown[]) {
  return vi.fn().mockImplementation((url: string, opts?: RequestInit) => {
    const u = String(url);
    const isGet = !opts || opts.method === undefined || opts.method === 'GET';
    if (u.endsWith(matchPath) && isGet) return Promise.resolve(jsonResponse(list));
    // Every other GET (tags, schema, quality definitions, …) → empty.
    return Promise.resolve(jsonResponse([]));
  });
}

describe('managed-by-config badge', () => {
  beforeEach(() => {
    window.matchMedia = vi.fn().mockReturnValue({
      matches: false,
      addEventListener: () => {},
      removeEventListener: () => {},
    }) as never;
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  // --- Root folders (Table-based list) ------------------------------------

  it('badges a managed root folder and disables its Remove control', async () => {
    const client = new CellarrClient({
      fetchImpl: listOnly('/api/v3/rootfolder', [
        { id: 1, path: '/media/movies', accessible: true, freeSpace: 0, unmappedFolders: [], managed: true },
      ]),
    });
    render(<RootFolders client={client} />);
    await waitFor(() => expect(screen.getAllByText('/media/movies').length).toBeGreaterThan(0));

    // The badge is shown with its visible + accessible label.
    expect(screen.getByText(MANAGED_BADGE_LABEL)).toBeTruthy();
    const badge = screen.getByText(MANAGED_BADGE_LABEL);
    expect(badge.getAttribute('aria-label')).toContain(MANAGED_BADGE_LABEL);
    expect(badge.getAttribute('role')).toBe('status');

    // The interactive Remove button is gone — the SRCL disabled Button renders a
    // non-interactive element, so it is removed from the a11y button tree.
    expect(
      screen.queryByRole('button', { name: 'Remove root folder /media/movies' })
    ).toBeNull();
  });

  it('keeps an unmanaged root folder editable and unbadged', async () => {
    const client = new CellarrClient({
      fetchImpl: listOnly('/api/v3/rootfolder', [
        { id: 2, path: '/media/tv', accessible: true, freeSpace: 0, unmappedFolders: [] },
      ]),
    });
    render(<RootFolders client={client} />);
    await waitFor(() => expect(screen.getAllByText('/media/tv').length).toBeGreaterThan(0));

    expect(screen.queryByText(MANAGED_BADGE_LABEL)).toBeNull();
    // The Remove control is a live, enabled button.
    expect(screen.getByRole('button', { name: 'Remove root folder /media/tv' })).toBeTruthy();
  });

  // --- Custom formats (per-row list with Edit + Delete) -------------------

  it('badges a managed custom format and disables both Edit and Delete', async () => {
    const client = new CellarrClient({
      fetchImpl: listOnly('/api/v3/customformat', [
        { id: 1, name: 'x265', specifications: [], managed: true },
      ]),
    });
    render(<CustomFormats client={client} />);
    await waitFor(() => expect(screen.getByText('x265')).toBeTruthy());

    expect(screen.getByText(MANAGED_BADGE_LABEL)).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Edit x265' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Delete x265' })).toBeNull();
  });

  it('keeps an unmanaged custom format fully editable', async () => {
    const client = new CellarrClient({
      fetchImpl: listOnly('/api/v3/customformat', [
        { id: 2, name: 'HDR', specifications: [] },
      ]),
    });
    render(<CustomFormats client={client} />);
    await waitFor(() => expect(screen.getByText('HDR')).toBeTruthy());

    expect(screen.queryByText(MANAGED_BADGE_LABEL)).toBeNull();
    expect(screen.getByRole('button', { name: 'Edit HDR' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Delete HDR' })).toBeTruthy();
  });

  // --- Quality profiles (single-profile editor form) ----------------------

  it('badges a managed quality profile and disables Save + Delete + Name', async () => {
    const client = new CellarrClient({
      fetchImpl: listOnly('/api/v3/qualityprofile', [
        {
          id: 'p1',
          name: 'HD-1080p',
          upgradeAllowed: true,
          cutoff: 1,
          cutoffFormatScore: 0,
          minFormatScore: 0,
          minUpgradeFormatScore: 0,
          items: [],
          formatItems: [],
          managed: true,
        },
      ]),
    });
    render(<QualityProfiles client={client} />);
    await waitFor(() => expect(screen.getByText(MANAGED_BADGE_LABEL)).toBeTruthy());

    // The editable name input is disabled (aria/DOM disabled).
    const nameInput = screen.getByLabelText('Profile name') as HTMLInputElement;
    expect(nameInput.disabled).toBe(true);

    // Save + Delete are no longer interactive buttons (SRCL disabled → div).
    expect(screen.queryByRole('button', { name: 'Save profile' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Delete profile' })).toBeNull();
  });

  it('keeps an unmanaged quality profile editable', async () => {
    const client = new CellarrClient({
      fetchImpl: listOnly('/api/v3/qualityprofile', [
        {
          id: 'p2',
          name: 'WEB-1080p',
          upgradeAllowed: true,
          cutoff: 1,
          cutoffFormatScore: 0,
          minFormatScore: 0,
          minUpgradeFormatScore: 0,
          items: [],
          formatItems: [],
        },
      ]),
    });
    render(<QualityProfiles client={client} />);
    await waitFor(() => expect(screen.getByLabelText('Profile name')).toBeTruthy());

    expect(screen.queryByText(MANAGED_BADGE_LABEL)).toBeNull();
    const nameInput = screen.getByLabelText('Profile name') as HTMLInputElement;
    expect(nameInput.disabled).toBe(false);
    expect(screen.getByRole('button', { name: 'Save profile' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Delete profile' })).toBeTruthy();
  });

  // --- Indexers (native list, managed derived from the v3 list by name) ---

  it('badges a managed indexer (cross-referenced from the v3 list) and locks its controls', async () => {
    // The native /api/v1 list (no `managed`) is the form source; the v3 list
    // carries `managed`, matched back by name.
    const fetchImpl = vi.fn().mockImplementation((url: string, opts?: RequestInit) => {
      const u = String(url);
      const isGet = !opts || opts.method === undefined || opts.method === 'GET';
      if (u.endsWith('/api/v1/indexers') && isGet) {
        return Promise.resolve(
          jsonResponse([{ id: 'ix1', name: 'Prowlarr', implementation: 'Prowlarr', enabled: true }])
        );
      }
      if (u.endsWith('/api/v3/indexer') && isGet) {
        return Promise.resolve(jsonResponse([{ id: 1, name: 'Prowlarr', managed: true }]));
      }
      return Promise.resolve(jsonResponse([])); // tags + anything else
    });
    const client = new CellarrClient({ fetchImpl });
    render(
      <IntegrationSection kind="indexers" title="Indexers" implementations={['Prowlarr']} client={client} />
    );
    await waitFor(() => expect(screen.getByText(MANAGED_BADGE_LABEL)).toBeTruthy());

    expect(screen.queryByRole('button', { name: 'Edit Prowlarr' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Remove Prowlarr' })).toBeNull();
  });

  it('keeps an unmanaged indexer editable when the v3 list reports it unmanaged', async () => {
    const fetchImpl = vi.fn().mockImplementation((url: string, opts?: RequestInit) => {
      const u = String(url);
      const isGet = !opts || opts.method === undefined || opts.method === 'GET';
      if (u.endsWith('/api/v1/indexers') && isGet) {
        return Promise.resolve(
          jsonResponse([{ id: 'ix2', name: 'NZBgeek', implementation: 'Newznab', enabled: true }])
        );
      }
      if (u.endsWith('/api/v3/indexer') && isGet) {
        return Promise.resolve(jsonResponse([{ id: 2, name: 'NZBgeek', managed: false }]));
      }
      return Promise.resolve(jsonResponse([]));
    });
    const client = new CellarrClient({ fetchImpl });
    render(
      <IntegrationSection kind="indexers" title="Indexers" implementations={['Newznab']} client={client} />
    );
    await waitFor(() => expect(screen.getAllByText('NZBgeek').length).toBeGreaterThan(0));

    expect(screen.queryByText(MANAGED_BADGE_LABEL)).toBeNull();
    expect(screen.getByRole('button', { name: 'Edit NZBgeek' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Remove NZBgeek' })).toBeTruthy();
  });

  // --- Release profiles ----------------------------------------------------

  it('badges a managed release profile and disables Edit + Delete', async () => {
    const client = new CellarrClient({
      fetchImpl: listOnly('/api/v3/releaseprofile', [
        { id: 3, name: 'No CAM', enabled: true, required: [], ignored: [], preferred: [], includePreferredWhenRenaming: false, indexerId: 0, tags: [], managed: true },
      ]),
    });
    render(<ReleaseProfiles client={client} />);
    await waitFor(() => expect(screen.getByText(MANAGED_BADGE_LABEL)).toBeTruthy());

    expect(screen.queryByRole('button', { name: 'Edit No CAM' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Delete No CAM' })).toBeNull();
  });

  it('keeps an unmanaged release profile editable', async () => {
    const client = new CellarrClient({
      fetchImpl: listOnly('/api/v3/releaseprofile', [
        { id: 4, name: 'Repacks', enabled: true, required: [], ignored: [], preferred: [], includePreferredWhenRenaming: false, indexerId: 0, tags: [] },
      ]),
    });
    render(<ReleaseProfiles client={client} />);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Edit Repacks' })).toBeTruthy());

    expect(screen.queryByText(MANAGED_BADGE_LABEL)).toBeNull();
    expect(screen.getByRole('button', { name: 'Delete Repacks' })).toBeTruthy();
  });

  // --- Delay profiles ------------------------------------------------------

  it('badges a managed delay profile and disables Edit + Delete', async () => {
    const client = new CellarrClient({
      fetchImpl: listOnly('/api/v3/delayprofile', [
        { id: 5, enableUsenet: true, enableTorrent: true, preferredProtocol: 'either', usenetDelay: 0, torrentDelay: 0, bypassIfHighestQuality: false, tags: [], order: 1, managed: true },
      ]),
    });
    render(<DelayProfiles client={client} />);
    await waitFor(() => expect(screen.getByText(MANAGED_BADGE_LABEL)).toBeTruthy());

    expect(screen.queryByRole('button', { name: 'Edit delay profile 1' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Delete delay profile 1' })).toBeNull();
  });

  it('keeps an unmanaged delay profile editable', async () => {
    const client = new CellarrClient({
      fetchImpl: listOnly('/api/v3/delayprofile', [
        { id: 6, enableUsenet: true, enableTorrent: true, preferredProtocol: 'either', usenetDelay: 0, torrentDelay: 0, bypassIfHighestQuality: false, tags: [], order: 2 },
      ]),
    });
    render(<DelayProfiles client={client} />);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Edit delay profile 2' })).toBeTruthy());

    expect(screen.queryByText(MANAGED_BADGE_LABEL)).toBeNull();
    expect(screen.getByRole('button', { name: 'Delete delay profile 2' })).toBeTruthy();
  });

  // --- Notifications -------------------------------------------------------

  it('badges a managed notification and disables Edit + Remove', async () => {
    const client = new CellarrClient({
      fetchImpl: listOnly('/api/v3/notification', [
        { id: 7, name: 'Discord', implementation: 'Discord', implementationName: 'Discord', configContract: 'DiscordSettings', onGrab: true, onDownload: true, onUpgrade: true, onRename: false, onHealthIssue: true, onHealthRestored: true, fields: [], tags: [], enabled: true, managed: true },
      ]),
    });
    render(<Notifications client={client} />);
    await waitFor(() => expect(screen.getByText(MANAGED_BADGE_LABEL)).toBeTruthy());

    expect(screen.queryByRole('button', { name: 'Edit Discord' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Remove Discord' })).toBeNull();
  });

  it('keeps an unmanaged notification editable', async () => {
    const client = new CellarrClient({
      fetchImpl: listOnly('/api/v3/notification', [
        { id: 8, name: 'Telegram', implementation: 'Telegram', implementationName: 'Telegram', configContract: 'TelegramSettings', onGrab: true, onDownload: true, onUpgrade: true, onRename: false, onHealthIssue: true, onHealthRestored: true, fields: [], tags: [], enabled: true },
      ]),
    });
    render(<Notifications client={client} />);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Edit Telegram' })).toBeTruthy());

    expect(screen.queryByText(MANAGED_BADGE_LABEL)).toBeNull();
    expect(screen.getByRole('button', { name: 'Remove Telegram' })).toBeTruthy();
  });

  // --- Import lists --------------------------------------------------------

  it('badges a managed import list and disables Edit + Remove (Sync stays live)', async () => {
    const client = new CellarrClient({
      fetchImpl: listOnly('/api/v3/importlist', [
        { id: 9, name: 'My TMDb', implementation: 'TMDbListImport', implementationName: 'TMDb', configContract: 'TMDbSettings', enabled: true, enableAuto: true, monitor: 'all', shouldMonitor: true, cleanLibraryLevel: 'disabled', lastSuccessfulSync: null, fields: [], tags: [], managed: true },
      ]),
    });
    render(<ImportLists client={client} />);
    await waitFor(() => expect(screen.getByText(MANAGED_BADGE_LABEL)).toBeTruthy());

    expect(screen.queryByRole('button', { name: 'Edit My TMDb' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Remove My TMDb' })).toBeNull();
    // Sync is an operation, not an edit — it stays available for managed lists.
    expect(screen.getByRole('button', { name: 'Sync My TMDb now' })).toBeTruthy();
  });

  it('keeps an unmanaged import list editable', async () => {
    const client = new CellarrClient({
      fetchImpl: listOnly('/api/v3/importlist', [
        { id: 10, name: 'My Trakt', implementation: 'TraktList', implementationName: 'Trakt', configContract: 'TraktSettings', enabled: true, enableAuto: true, monitor: 'all', shouldMonitor: true, cleanLibraryLevel: 'disabled', lastSuccessfulSync: null, fields: [], tags: [] },
      ]),
    });
    render(<ImportLists client={client} />);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Edit My Trakt' })).toBeTruthy());

    expect(screen.queryByText(MANAGED_BADGE_LABEL)).toBeNull();
    expect(screen.getByRole('button', { name: 'Remove My Trakt' })).toBeTruthy();
  });
});
