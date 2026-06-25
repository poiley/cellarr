'use client';

// Settings — Backups. Lists backup bundles (name / time / size) with Download,
// Delete (confirmed), and a 'Backup now' action, plus a destructive Restore that
// explains the live database will be replaced and a pre-restore safety backup is
// taken automatically. SRCL-only: Card, Table, Button, ButtonGroup, Badge,
// Divider, Text, the shared ConfirmDialog (#40) for both destructive actions, and
// the shared useToast (#39) for feedback. Data glue is the API client only.

import * as React from 'react';

import Card from '@components/Card';
import Table from '@components/Table';
import TableRow from '@components/TableRow';
import TableColumn from '@components/TableColumn';
import Button from '@components/Button';
import ButtonGroup from '@components/ButtonGroup';
import Badge from '@components/Badge';
import Divider from '@components/Divider';
import Text from '@components/Text';

import { CellarrClient, api as defaultApi } from '@lib/api/client';
import type { BackupRecord } from '@lib/api/types';

import { useToast } from '@app/_lib/ToastProvider';
import { useAsync, toApiError } from '@app/settings/_components/useAsync';
import { Loading, ErrorBanner, EmptyState } from '@app/settings/_components/StatusBanners';
import ConfirmDialog from '@app/settings/_components/ConfirmDialog';

// Bytes -> a compact human size for the size column.
function humanSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '—';
  const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v.toFixed(v >= 10 || i === 0 ? 0 : 1)} ${units[i]}`;
}

// ISO timestamp -> stable, terminal-friendly "YYYY-MM-DD HH:MM".
function formatTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const pad = (n: number) => String(n).padStart(2, '0');
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ` +
    `${pad(d.getHours())}:${pad(d.getMinutes())}`
  );
}

const SystemBackup: React.FC<{ client?: CellarrClient }> = ({ client = defaultApi }) => {
  const { success, error: toastError, info } = useToast();

  const load = React.useCallback(
    (signal: AbortSignal) => client.listBackups(signal),
    [client]
  );
  const { data, loading, error, reload } = useAsync<BackupRecord[]>(load);

  const [backingUp, setBackingUp] = React.useState(false);
  const [pendingDelete, setPendingDelete] = React.useState<BackupRecord | null>(null);
  const [deleting, setDeleting] = React.useState(false);
  const [pendingRestore, setPendingRestore] = React.useState<BackupRecord | null>(null);
  const [restoring, setRestoring] = React.useState(false);

  const backups = data ?? [];

  if (loading) return <Loading label="Loading backups" />;
  if (error) return <ErrorBanner error={error} />;

  // Prefer the real string id when present; the numeric projection also works.
  const idOf = (b: BackupRecord): number | string => b.backupId ?? b.id;

  const backupNow = async () => {
    setBackingUp(true);
    info('Taking a backup…');
    try {
      await client.createBackup();
      success('Backup created.');
      reload();
    } catch (err) {
      toastError(`Could not create backup — ${toApiError(err).message}`);
    } finally {
      setBackingUp(false);
    }
  };

  const confirmDelete = async () => {
    if (!pendingDelete) return;
    setDeleting(true);
    try {
      await client.deleteBackup(idOf(pendingDelete));
      success('Backup deleted.');
      setPendingDelete(null);
      reload();
    } catch (err) {
      toastError(`Could not delete backup — ${toApiError(err).message}`);
    } finally {
      setDeleting(false);
    }
  };

  const confirmRestore = async () => {
    if (!pendingRestore) return;
    setRestoring(true);
    try {
      const result = await client.restoreBackup(idOf(pendingRestore));
      const note = result?.restartRequired
        ? ' Restart the daemon to finish applying it.'
        : '';
      const safety =
        result?.safetyBackupId != null
          ? ` A safety backup (${result.safetyBackupId}) was taken first.`
          : '';
      success(`Backup restored.${safety}${note}`);
      setPendingRestore(null);
      reload();
    } catch (err) {
      toastError(`Could not restore backup — ${toApiError(err).message}`);
    } finally {
      setRestoring(false);
    }
  };

  return (
    <Card title="Backups">
      <Text style={{ opacity: 0.6 }}>
        Backups capture the cellarr database and configuration. Take one before risky changes;
        restoring replaces the live database with the bundle&apos;s contents.
      </Text>

      <Divider type="GRADIENT" />

      {backups.length ? (
        <Table>
          <TableRow>
            <TableColumn>Name</TableColumn>
            <TableColumn>Type</TableColumn>
            <TableColumn>Time</TableColumn>
            <TableColumn>Size</TableColumn>
            <TableColumn> </TableColumn>
          </TableRow>
          {backups.map((b) => (
            <TableRow key={String(idOf(b))}>
              <TableColumn>{b.name}</TableColumn>
              <TableColumn>
                <Badge>{b.type}</Badge>
              </TableColumn>
              <TableColumn>{formatTime(b.time)}</TableColumn>
              <TableColumn>{humanSize(b.size)}</TableColumn>
              <TableColumn>
                <div style={{ display: 'flex', gap: '1ch', justifyContent: 'flex-end' }}>
                  <a
                    href={client.backupDownloadUrl(idOf(b))}
                    download
                    aria-label={`Download backup ${b.name}`}
                    style={{ textDecoration: 'none' }}
                  >
                    <Button theme="SECONDARY">Download</Button>
                  </a>
                  <Button
                    theme="SECONDARY"
                    aria-label={`Restore backup ${b.name}`}
                    onClick={() => setPendingRestore(b)}
                  >
                    Restore
                  </Button>
                  <Button
                    theme="DANGER"
                    aria-label={`Delete backup ${b.name}`}
                    onClick={() => setPendingDelete(b)}
                  >
                    Delete
                  </Button>
                </div>
              </TableColumn>
            </TableRow>
          ))}
        </Table>
      ) : (
        <EmptyState>No backups yet. Take one now to capture the current state.</EmptyState>
      )}

      <Divider type="GRADIENT" />

      <ButtonGroup
        items={[
          {
            body: backingUp ? 'Backing up…' : 'Backup now',
            onClick: backingUp ? undefined : backupNow,
          },
        ]}
      />

      {pendingDelete ? (
        <ConfirmDialog
          title="Delete backup"
          confirmLabel="Delete backup"
          pendingLabel="Deleting…"
          pending={deleting}
          onConfirm={confirmDelete}
          onCancel={() => (deleting ? undefined : setPendingDelete(null))}
        >
          <Text>
            Delete the backup <code>{pendingDelete.name}</code>? The bundle file is removed from
            disk. Your live library is not affected.
          </Text>
        </ConfirmDialog>
      ) : null}

      {pendingRestore ? (
        <ConfirmDialog
          title="Restore backup"
          confirmLabel="Replace database & restore"
          pendingLabel="Restoring…"
          pending={restoring}
          onConfirm={confirmRestore}
          onCancel={() => (restoring ? undefined : setPendingRestore(null))}
        >
          <Text>
            Restoring <code>{pendingRestore.name}</code> will <strong>replace the live database</strong>{' '}
            with the contents of this backup. Any changes made since it was taken will be lost.
          </Text>
          <Text style={{ marginTop: '1ch', opacity: 0.8 }}>
            A pre-restore safety backup of the current database is taken automatically first, so you
            can roll this back. A daemon restart may be required to finish applying the restore.
          </Text>
        </ConfirmDialog>
      ) : null}
    </Card>
  );
};

export default SystemBackup;
