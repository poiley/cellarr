'use client';

// Settings — Root Folders (#36). Full CRUD against the Radarr-compatible
// /api/v3/rootfolder surface: GET lists, POST creates, DELETE removes. SRCL-only:
// Card, Table/TableRow/TableColumn, Input, Button, ButtonGroup, Badge, Divider,
// Text, plus the shared ConfirmDialog for the destructive delete (#40) and the
// shared useToast for save/delete feedback (#39).

import * as React from 'react';

import Card from '@components/Card';
import Table from '@components/Table';
import TableRow from '@components/TableRow';
import TableColumn from '@components/TableColumn';
import Input from '@components/Input';
import Button from '@components/Button';
import ButtonGroup from '@components/ButtonGroup';
import Badge from '@components/Badge';
import Divider from '@components/Divider';
import Text from '@components/Text';

import { CellarrClient, api as defaultApi } from '@lib/api/client';
import type { RootFolder } from '@lib/api/types';

import { useToast } from '@app/_lib/ToastProvider';
import { useAsync, toApiError } from '@app/settings/_components/useAsync';
import { Loading, ErrorBanner, EmptyState } from '@app/settings/_components/StatusBanners';
import ConfirmDialog from '@app/settings/_components/ConfirmDialog';
import { createRootFolder, deleteRootFolder } from '@app/settings/_lib/settings';

// Bytes -> a compact human size for the free-space column.
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

const RootFolders: React.FC<{ client?: CellarrClient }> = ({ client = defaultApi }) => {
  const { success, error: toastError } = useToast();

  const load = React.useCallback(
    (signal: AbortSignal) => client.listRootFolders(signal),
    [client]
  );
  const { data, loading, error, reload } = useAsync<RootFolder[]>(load);

  const [path, setPath] = React.useState('');
  const [name, setName] = React.useState('');
  const [adding, setAdding] = React.useState(false);
  const [pendingDelete, setPendingDelete] = React.useState<RootFolder | null>(null);
  const [deleting, setDeleting] = React.useState(false);

  const folders = data ?? [];

  if (loading) return <Loading label="Loading root folders" />;
  if (error) return <ErrorBanner error={error} />;

  const add = async () => {
    const trimmed = path.trim();
    if (!trimmed) {
      toastError('Enter a folder path first.');
      return;
    }
    setAdding(true);
    try {
      await createRootFolder(client, { path: trimmed, name: name.trim() || undefined });
      setPath('');
      setName('');
      success('Root folder added.');
      reload();
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not add root folder — ${e.message}`);
    } finally {
      setAdding(false);
    }
  };

  const confirmDelete = async () => {
    if (!pendingDelete) return;
    setDeleting(true);
    try {
      await deleteRootFolder(client, pendingDelete.id);
      success('Root folder removed.');
      setPendingDelete(null);
      reload();
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not remove root folder — ${e.message}`);
    } finally {
      setDeleting(false);
    }
  };

  return (
    <Card title="Root Folders">
      <Text style={{ opacity: 0.6 }}>
        Directories where your media is stored. New items are placed under the root folder you pick
        when adding them.
      </Text>

      <Divider type="GRADIENT" />

      {folders.length ? (
        <Table>
          <TableRow>
            <TableColumn>Path</TableColumn>
            <TableColumn>Status</TableColumn>
            <TableColumn>Free space</TableColumn>
            <TableColumn> </TableColumn>
          </TableRow>
          {folders.map((f) => (
            <TableRow key={f.id}>
              <TableColumn>
                <code>{f.path}</code>
              </TableColumn>
              <TableColumn>
                <Badge>{f.accessible ? '● accessible' : '✗ unavailable'}</Badge>
              </TableColumn>
              <TableColumn>{humanSize(f.freeSpace)}</TableColumn>
              <TableColumn>
                <Button
                  theme="SECONDARY"
                  aria-label={`Remove root folder ${f.path}`}
                  onClick={() => setPendingDelete(f)}
                >
                  Remove
                </Button>
              </TableColumn>
            </TableRow>
          ))}
        </Table>
      ) : (
        <EmptyState>No root folders yet. Add one to tell cellarr where your media lives.</EmptyState>
      )}

      <Divider type="GRADIENT" />

      <Text style={{ opacity: 0.6, margin: '1ch 0 0.5ch' }}>Add a root folder</Text>
      <div style={{ display: 'flex', gap: '1ch', alignItems: 'flex-end' }}>
        <div style={{ flex: 2 }}>
          <Text style={{ opacity: 0.6 }}>Path</Text>
          <Input
            name="rootfolder-path"
            aria-label="Root folder path"
            placeholder="/media/movies"
            value={path}
            onChange={(e) => setPath(e.target.value)}
          />
        </div>
        <div style={{ flex: 1 }}>
          <Text style={{ opacity: 0.6 }}>Name (optional)</Text>
          <Input
            name="rootfolder-name"
            aria-label="Root folder name"
            placeholder="Movies"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </div>
      </div>

      <div style={{ marginTop: '1ch' }}>
        <ButtonGroup
          items={[{ body: adding ? 'Adding…' : 'Add root folder', onClick: adding ? undefined : add }]}
        />
      </div>

      {pendingDelete ? (
        <ConfirmDialog
          title="Remove root folder"
          confirmLabel="Remove root folder"
          pendingLabel="Removing…"
          pending={deleting}
          onConfirm={confirmDelete}
          onCancel={() => (deleting ? undefined : setPendingDelete(null))}
        >
          <Text>
            Remove <code>{pendingDelete.path}</code> from cellarr? Existing files on disk are left
            untouched, but cellarr will stop tracking this location.
          </Text>
        </ConfirmDialog>
      ) : null}
    </Card>
  );
};

export default RootFolders;
