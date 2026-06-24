'use client';

// Settings — Delay Profiles. SRCL-only: a list of delay profiles (each with Edit
// / Delete that opens the shared danger ConfirmDialog) and a form to author/edit
// one (preferred protocol Select, usenet/torrent delay Inputs in minutes, a
// bypass-if-highest Checkbox, an enabled Checkbox, an order Input), with Save.
//
// Reads + writes the Radarr/Sonarr-compatible /api/v3 shim
// (crates/cellarr-api/src/shim.rs), where delay profiles live with a stable
// numeric id (dp_numeric_id):
//   * GET    /api/v3/delayprofile      — the list, ordered by `order`;
//   * POST   /api/v3/delayprofile      — create;
//   * PUT    /api/v3/delayprofile/{id} — update (preserves id);
//   * DELETE /api/v3/delayprofile/{id} — remove.

import * as React from 'react';

import Card from '@components/Card';
import Input from '@components/Input';
import Select from '@components/Select';
import Checkbox from '@components/Checkbox';
import Button from '@components/Button';
import ButtonGroup from '@components/ButtonGroup';
import Badge from '@components/Badge';
import Divider from '@components/Divider';
import Text from '@components/Text';

import { ApiError, CellarrClient, api as defaultApi } from '@lib/api/client';
import type { DelayProfile, DelayProfileBody } from '@lib/api/types';

import { useToast } from '@app/_lib/ToastProvider';
import { useAsync, toApiError } from '@app/settings/_components/useAsync';
import { Loading, ErrorBanner, EmptyState } from '@app/settings/_components/StatusBanners';
import ConfirmDialog from '@app/settings/_components/ConfirmDialog';

type Preferred = 'usenet' | 'torrent' | 'either';

// The open editor's view-model. `id === undefined` marks a new profile. Delays
// are held as strings while editing the number inputs.
interface DpForm {
  id?: number;
  preferredProtocol: Preferred;
  usenetDelay: string;
  torrentDelay: string;
  bypassIfHighestQuality: boolean;
  enabled: boolean;
  order: string;
}

const PROTOCOL_OPTIONS: { value: Preferred; label: string }[] = [
  { value: 'either', label: 'Either (no preference)' },
  { value: 'usenet', label: 'Prefer Usenet' },
  { value: 'torrent', label: 'Prefer Torrent' },
];

const PROTOCOL_LABELS = PROTOCOL_OPTIONS.map((o) => o.label);

function labelForProtocol(p: Preferred): string {
  return PROTOCOL_OPTIONS.find((o) => o.value === p)?.label ?? PROTOCOL_OPTIONS[0].label;
}

function protocolForLabel(label: string): Preferred {
  return PROTOCOL_OPTIONS.find((o) => o.label === label)?.value ?? 'either';
}

function formFromProfile(dp: DelayProfile): DpForm {
  return {
    id: typeof dp.id === 'number' ? dp.id : undefined,
    preferredProtocol: (dp.preferredProtocol as Preferred) ?? 'either',
    usenetDelay: String(dp.usenetDelay ?? 0),
    torrentDelay: String(dp.torrentDelay ?? 0),
    bypassIfHighestQuality: dp.bypassIfHighestQuality === true,
    enabled: dp.enableUsenet === true || dp.enableTorrent === true,
    order: String(dp.order ?? 0),
  };
}

function blankForm(nextOrder: number): DpForm {
  return {
    preferredProtocol: 'either',
    usenetDelay: '0',
    torrentDelay: '0',
    bypassIfHighestQuality: false,
    enabled: true,
    order: String(nextOrder),
  };
}

const toMinutes = (s: string): number => {
  const n = Number.parseInt(s, 10);
  return Number.isNaN(n) || n < 0 ? 0 : n;
};

const DelayProfiles: React.FC<{ client?: CellarrClient }> = ({ client = defaultApi }) => {
  const { success, error: toastError } = useToast();

  const load = React.useCallback(
    (signal: AbortSignal) => client.getDelayProfiles(signal),
    [client]
  );
  const { data, loading, error, reload } = useAsync<DelayProfile[]>(load);

  const [form, setForm] = React.useState<DpForm | undefined>(undefined);
  const [saving, setSaving] = React.useState(false);
  const [saveError, setSaveError] = React.useState<ApiError | undefined>(undefined);
  const [pendingDelete, setPendingDelete] = React.useState<DelayProfile | null>(null);
  const [deleting, setDeleting] = React.useState(false);

  const profiles = data ?? [];
  const nextOrder = profiles.reduce((m, p) => Math.max(m, p.order ?? 0), 0) + 1;

  if (loading) return <Loading label="Loading delay profiles" />;
  if (error) return <ErrorBanner error={error} />;

  const openNew = () => {
    setForm(blankForm(nextOrder));
    setSaveError(undefined);
  };

  const openEdit = (dp: DelayProfile) => {
    setForm(formFromProfile(dp));
    setSaveError(undefined);
  };

  const closeForm = () => {
    setForm(undefined);
    setSaveError(undefined);
  };

  const update = (patch: Partial<DpForm>) => setForm((f) => (f ? { ...f, ...patch } : f));

  const save = async () => {
    if (!form) return;
    setSaving(true);
    setSaveError(undefined);
    try {
      const body: DelayProfileBody = {
        enabled: form.enabled,
        preferredProtocol: form.preferredProtocol,
        usenetDelay: toMinutes(form.usenetDelay),
        torrentDelay: toMinutes(form.torrentDelay),
        bypassIfHighestQuality: form.bypassIfHighestQuality,
        tags: [],
        order: toMinutes(form.order),
      };
      const editingId = form.id;
      if (typeof editingId === 'number') {
        await client.updateDelayProfile(editingId, body);
      } else {
        await client.createDelayProfile(body);
      }
      success(typeof editingId === 'number' ? 'Delay profile saved.' : 'Delay profile created.');
      closeForm();
      reload();
    } catch (err) {
      const e = toApiError(err);
      setSaveError(e);
      toastError(`Could not save delay profile — ${e.message}`);
    } finally {
      setSaving(false);
    }
  };

  const confirmDelete = async () => {
    if (!pendingDelete) return;
    setDeleting(true);
    try {
      await client.deleteDelayProfile(pendingDelete.id);
      success('Delay profile removed.');
      if (form && form.id === pendingDelete.id) closeForm();
      setPendingDelete(null);
      reload();
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not remove delay profile — ${e.message}`);
    } finally {
      setDeleting(false);
    }
  };

  const describe = (dp: DelayProfile): string => {
    const parts: string[] = [];
    parts.push(labelForProtocol((dp.preferredProtocol as Preferred) ?? 'either'));
    parts.push(`usenet ${dp.usenetDelay ?? 0}m`);
    parts.push(`torrent ${dp.torrentDelay ?? 0}m`);
    if (dp.bypassIfHighestQuality) parts.push('bypass on highest');
    return parts.join(' · ');
  };

  return (
    <Card title="Delay Profiles">
      <Text style={{ opacity: 0.6, marginBottom: '1ch' }}>
        Hold grabs for a set time so a preferred release can appear before a lesser one is taken.
      </Text>

      {profiles.length ? (
        <ul style={{ listStyle: 'none', padding: 0, margin: '0 0 1ch 0' }}>
          {profiles.map((dp) => (
            <li
              key={dp.id}
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'space-between',
                gap: '1ch',
                padding: '0.5ch 0',
              }}
            >
              <span>
                <Badge>#{dp.order ?? 0}</Badge> {describe(dp)}
              </span>
              <span style={{ display: 'inline-flex', gap: '0.5ch' }}>
                <Button theme="SECONDARY" aria-label={`Edit delay profile ${dp.order ?? 0}`} onClick={() => openEdit(dp)}>
                  Edit
                </Button>
                <Button
                  theme="SECONDARY"
                  aria-label={`Delete delay profile ${dp.order ?? 0}`}
                  onClick={() => setPendingDelete(dp)}
                >
                  Delete
                </Button>
              </span>
            </li>
          ))}
        </ul>
      ) : (
        <EmptyState>No delay profiles yet. Add one to stagger grabs.</EmptyState>
      )}

      <div style={{ marginBottom: '1ch' }}>
        <ButtonGroup items={[{ body: 'Add delay profile', onClick: openNew }]} />
      </div>

      {form ? (
        <>
          <Divider type="GRADIENT" />
          <Text style={{ opacity: 0.6, margin: '1ch 0 0.5ch' }}>
            {typeof form.id === 'number' ? 'Editing delay profile' : 'New delay profile'}
          </Text>

          <div style={{ margin: '0.5ch 0' }}>
            <Text style={{ opacity: 0.6 }}>Preferred protocol</Text>
            <Select
              name="dp-protocol"
              options={PROTOCOL_LABELS}
              defaultValue={labelForProtocol(form.preferredProtocol)}
              onChange={(value) => update({ preferredProtocol: protocolForLabel(value) })}
            />
          </div>

          <div style={{ display: 'flex', gap: '1ch' }}>
            <div style={{ flex: 1, margin: '0.5ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Usenet delay (minutes)</Text>
              <Input
                name="dp-usenet-delay"
                aria-label="Usenet delay minutes"
                type="number"
                min={0}
                value={form.usenetDelay}
                onChange={(e) => update({ usenetDelay: e.target.value })}
              />
            </div>
            <div style={{ flex: 1, margin: '0.5ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Torrent delay (minutes)</Text>
              <Input
                name="dp-torrent-delay"
                aria-label="Torrent delay minutes"
                type="number"
                min={0}
                value={form.torrentDelay}
                onChange={(e) => update({ torrentDelay: e.target.value })}
              />
            </div>
            <div style={{ flex: 1, margin: '0.5ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Order</Text>
              <Input
                name="dp-order"
                aria-label="Order"
                type="number"
                min={0}
                value={form.order}
                onChange={(e) => update({ order: e.target.value })}
              />
            </div>
          </div>

          <div style={{ display: 'flex', gap: '2ch', margin: '0.5ch 0' }}>
            <Checkbox
              key={`${form.id ?? 'new'}-bypass`}
              name="dp-bypass"
              defaultChecked={form.bypassIfHighestQuality}
              onChange={(e) => update({ bypassIfHighestQuality: e.target.checked })}
            >
              Bypass if highest quality
            </Checkbox>
            <Checkbox
              key={`${form.id ?? 'new'}-enabled`}
              name="dp-enabled"
              defaultChecked={form.enabled}
              onChange={(e) => update({ enabled: e.target.checked })}
            >
              Enabled
            </Checkbox>
          </div>

          {saveError ? <ErrorBanner error={saveError} /> : null}

          <div style={{ marginTop: '1ch' }}>
            <ButtonGroup
              items={[
                {
                  body: saving ? 'Saving…' : typeof form.id === 'number' ? 'Save profile' : 'Create profile',
                  onClick: saving ? undefined : save,
                },
                { body: 'Cancel', onClick: saving ? undefined : closeForm },
              ]}
            />
          </div>
        </>
      ) : null}

      {pendingDelete ? (
        <ConfirmDialog
          title="Delete delay profile"
          confirmLabel="Delete delay profile"
          pendingLabel="Deleting…"
          pending={deleting}
          onConfirm={confirmDelete}
          onCancel={() => (deleting ? undefined : setPendingDelete(null))}
        >
          <Text>
            Delete delay profile <strong>#{pendingDelete.order ?? 0}</strong>? Grabs it governed will
            no longer be staggered.
          </Text>
        </ConfirmDialog>
      ) : null}
    </Card>
  );
};

export default DelayProfiles;
