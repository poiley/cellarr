import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { CellarrClient } from '@lib/api/client';
import SystemBackup from '@app/settings/_components/SystemBackup';

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

const BACKUPS = [
  {
    id: 1,
    backupId: 'bk-001',
    name: 'cellarr_backup_2026-06-20',
    type: 'manual',
    size: 1024 * 1024 * 5,
    time: '2026-06-20T10:00:00Z',
    path: '/backups/bk-001.zip',
  },
];

describe('SystemBackup (settings)', () => {
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

  it('lists existing backups with name, type, time and size', async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse(BACKUPS));
    const client = new CellarrClient({ fetchImpl });
    render(<SystemBackup client={client} />);

    await waitFor(() =>
      expect(screen.getByText('cellarr_backup_2026-06-20')).toBeTruthy()
    );
    expect(screen.getByText('manual')).toBeTruthy();
    expect(screen.getByText('5.0 MB')).toBeTruthy();
    // Download link points at the bundle bytes route.
    const dl = screen.getByLabelText('Download backup cellarr_backup_2026-06-20');
    expect(dl.closest('a')?.getAttribute('href')).toContain('/api/v3/system/backup/bk-001');
  });

  it("POSTs a manual backup when 'Backup now' is clicked", async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse([])) // load
      .mockResolvedValueOnce(jsonResponse({ ...BACKUPS[0], id: 2, backupId: 'bk-002' })) // create
      .mockResolvedValueOnce(jsonResponse(BACKUPS)); // reload
    const client = new CellarrClient({ fetchImpl });
    render(<SystemBackup client={client} />);

    await waitFor(() => expect(screen.getByText('Backup now')).toBeTruthy());
    fireEvent.click(screen.getByText('Backup now'));

    await waitFor(() => {
      const post = fetchImpl.mock.calls.find(
        ([url, opts]) =>
          String(url).endsWith('/api/v3/system/backup') && opts?.method === 'POST'
      );
      expect(post).toBeTruthy();
    });
  });

  it('confirms then DELETEs a backup', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(BACKUPS)) // load
      .mockResolvedValueOnce(jsonResponse({})) // delete
      .mockResolvedValueOnce(jsonResponse([])); // reload
    const client = new CellarrClient({ fetchImpl });
    render(<SystemBackup client={client} />);

    await waitFor(() =>
      expect(screen.getByLabelText('Delete backup cellarr_backup_2026-06-20')).toBeTruthy()
    );
    fireEvent.click(screen.getByLabelText('Delete backup cellarr_backup_2026-06-20'));
    // No DELETE before the confirm.
    expect(fetchImpl.mock.calls.find(([, o]) => o?.method === 'DELETE')).toBeFalsy();

    await waitFor(() => expect(screen.getByRole('alertdialog')).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: 'Delete backup' }));

    await waitFor(() => {
      const del = fetchImpl.mock.calls.find(
        ([url, opts]) =>
          String(url).endsWith('/api/v3/system/backup/bk-001') && opts?.method === 'DELETE'
      );
      expect(del).toBeTruthy();
    });
  });

  it('restore is gated behind a destructive confirm explaining the DB is replaced', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(BACKUPS)) // load
      .mockResolvedValueOnce(
        jsonResponse({ restored: 'bk-001', safetyBackupId: 'safety-9', restartRequired: true })
      ) // restore
      .mockResolvedValueOnce(jsonResponse(BACKUPS)); // reload
    const client = new CellarrClient({ fetchImpl });
    render(<SystemBackup client={client} />);

    await waitFor(() =>
      expect(screen.getByLabelText('Restore backup cellarr_backup_2026-06-20')).toBeTruthy()
    );
    fireEvent.click(screen.getByLabelText('Restore backup cellarr_backup_2026-06-20'));

    await waitFor(() => expect(screen.getByRole('alertdialog')).toBeTruthy());
    // The dialog spells out the destructive replacement + the safety backup.
    expect(screen.getByText(/replace the live database/i)).toBeTruthy();
    expect(screen.getByText(/pre-restore safety backup/i)).toBeTruthy();

    // No restore POST before confirming.
    expect(
      fetchImpl.mock.calls.find(([url]) => String(url).includes('/backup/restore/'))
    ).toBeFalsy();

    fireEvent.click(screen.getByRole('button', { name: 'Replace database & restore' }));
    await waitFor(() => {
      const post = fetchImpl.mock.calls.find(
        ([url, opts]) =>
          String(url).endsWith('/api/v3/system/backup/restore/bk-001') &&
          opts?.method === 'POST'
      );
      expect(post).toBeTruthy();
    });
  });
});
