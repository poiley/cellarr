'use client';

// Settings — Remote Path Mappings (#36). Full CRUD against the Radarr-compatible
// /api/v3/remotepathmapping surface (already modelled on the client): GET lists,
// POST creates, PUT edits, DELETE removes. A mapping rewrites a download client's
// remote path (host + remotePath) to the local path cellarr can read.
//
// SRCL-only: Card, Table/TableRow/TableColumn, Input, Button, ButtonGroup,
// Divider, Text, plus the shared ConfirmDialog for the destructive delete (#40)
// and the shared useToast for save/delete feedback (#39).

import * as React from 'react';

import Card from '@components/Card';
import Table from '@components/Table';
import TableRow from '@components/TableRow';
import TableColumn from '@components/TableColumn';
import Input from '@components/Input';
import Button from '@components/Button';
import ButtonGroup from '@components/ButtonGroup';
import Divider from '@components/Divider';
import Text from '@components/Text';

import { CellarrClient, api as defaultApi } from '@lib/api/client';
import type { RemotePathMapping } from '@lib/api/types';

import { useToast } from '@app/_lib/ToastProvider';
import { useAsync, toApiError } from '@app/settings/_components/useAsync';
import { Loading, ErrorBanner, EmptyState } from '@app/settings/_components/StatusBanners';
import ConfirmDialog from '@app/settings/_components/ConfirmDialog';

interface MappingForm {
  id: number | null;
  host: string;
  remotePath: string;
  localPath: string;
}

const BLANK: MappingForm = { id: null, host: '', remotePath: '', localPath: '' };

const RemotePathMappings: React.FC<{ client?: CellarrClient }> = ({ client = defaultApi }) => {
  const { success, error: toastError } = useToast();

  const load = React.useCallback(
    (signal: AbortSignal) => client.listRemotePathMappings(signal),
    [client]
  );
  const { data, loading, error, reload } = useAsync<RemotePathMapping[]>(load);

  const [form, setForm] = React.useState<MappingForm>(BLANK);
  const [saving, setSaving] = React.useState(false);
  const [pendingDelete, setPendingDelete] = React.useState<RemotePathMapping | null>(null);
  const [deleting, setDeleting] = React.useState(false);

  const mappings = data ?? [];

  if (loading) return <Loading label="Loading remote path mappings" />;
  if (error) return <ErrorBanner error={error} />;

  const edit = (m: RemotePathMapping) =>
    setForm({ id: m.id, host: m.host, remotePath: m.remotePath, localPath: m.localPath });

  const reset = () => setForm(BLANK);

  const save = async () => {
    if (!form.host.trim() || !form.remotePath.trim() || !form.localPath.trim()) {
      toastError('Host, remote path and local path are all required.');
      return;
    }
    setSaving(true);
    try {
      const body = {
        host: form.host.trim(),
        remotePath: form.remotePath.trim(),
        localPath: form.localPath.trim(),
      };
      if (form.id !== null) {
        await client.updateRemotePathMapping(form.id, body);
        success('Mapping updated.');
      } else {
        await client.createRemotePathMapping(body);
        success('Mapping added.');
      }
      reset();
      reload();
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not save mapping — ${e.message}`);
    } finally {
      setSaving(false);
    }
  };

  const confirmDelete = async () => {
    if (!pendingDelete) return;
    setDeleting(true);
    try {
      await client.deleteRemotePathMapping(pendingDelete.id);
      success('Mapping removed.');
      if (form.id === pendingDelete.id) reset();
      setPendingDelete(null);
      reload();
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not remove mapping — ${e.message}`);
    } finally {
      setDeleting(false);
    }
  };

  return (
    <Card title="Remote Path Mappings">
      <Text style={{ opacity: 0.6 }}>
        Rewrite a download client&apos;s remote path to the local path cellarr can read. Use these
        when a download client reports files under a different path than where cellarr sees them.
      </Text>

      <Divider type="GRADIENT" />

      {mappings.length ? (
        <Table>
          <TableRow>
            <TableColumn>Host</TableColumn>
            <TableColumn>Remote path</TableColumn>
            <TableColumn>Local path</TableColumn>
            <TableColumn> </TableColumn>
          </TableRow>
          {mappings.map((m) => (
            <TableRow key={m.id}>
              <TableColumn>{m.host}</TableColumn>
              <TableColumn>
                <code>{m.remotePath}</code>
              </TableColumn>
              <TableColumn>
                <code>{m.localPath}</code>
              </TableColumn>
              <TableColumn>
                <span style={{ display: 'inline-flex', gap: '0.5ch' }}>
                  <Button theme="SECONDARY" aria-label={`Edit mapping for ${m.host}`} onClick={() => edit(m)}>
                    Edit
                  </Button>
                  <Button
                    theme="DANGER"
                    aria-label={`Remove mapping for ${m.host}`}
                    onClick={() => setPendingDelete(m)}
                  >
                    Remove
                  </Button>
                </span>
              </TableColumn>
            </TableRow>
          ))}
        </Table>
      ) : (
        <EmptyState>No remote path mappings configured yet.</EmptyState>
      )}

      <Divider type="GRADIENT" />

      <Text style={{ opacity: 0.6, margin: '1ch 0 0.5ch' }}>
        {form.id !== null ? `Editing mapping for ${form.host || '(host)'}` : 'Add a mapping'}
      </Text>

      <div style={{ margin: '0.5ch 0' }}>
        <Text style={{ opacity: 0.6 }}>Host</Text>
        <Input
          name="rpm-host"
          aria-label="Mapping host"
          placeholder="download-client-host"
          value={form.host}
          onChange={(e) => setForm({ ...form, host: e.target.value })}
        />
      </div>
      <div style={{ margin: '0.5ch 0' }}>
        <Text style={{ opacity: 0.6 }}>Remote path</Text>
        <Input
          name="rpm-remote"
          aria-label="Mapping remote path"
          placeholder="/downloads/"
          value={form.remotePath}
          onChange={(e) => setForm({ ...form, remotePath: e.target.value })}
        />
      </div>
      <div style={{ margin: '0.5ch 0' }}>
        <Text style={{ opacity: 0.6 }}>Local path</Text>
        <Input
          name="rpm-local"
          aria-label="Mapping local path"
          placeholder="/mnt/downloads/"
          value={form.localPath}
          onChange={(e) => setForm({ ...form, localPath: e.target.value })}
        />
      </div>

      <div style={{ marginTop: '1ch' }}>
        <ButtonGroup
          items={[
            {
              body: saving ? 'Saving…' : form.id !== null ? 'Save mapping' : 'Add mapping',
              onClick: saving ? undefined : save,
            },
            ...(form.id !== null ? [{ body: 'Cancel', onClick: reset }] : []),
          ]}
        />
      </div>

      {pendingDelete ? (
        <ConfirmDialog
          title="Remove mapping"
          confirmLabel="Remove mapping"
          pendingLabel="Removing…"
          pending={deleting}
          onConfirm={confirmDelete}
          onCancel={() => (deleting ? undefined : setPendingDelete(null))}
        >
          <Text>
            Remove the mapping for <code>{pendingDelete.host}</code> (
            <code>{pendingDelete.remotePath}</code> → <code>{pendingDelete.localPath}</code>)?
          </Text>
        </ConfirmDialog>
      ) : null}
    </Card>
  );
};

export default RemotePathMappings;
