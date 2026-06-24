'use client';

// Settings — Custom Formats. SRCL-only: a Table of formats (Table/TableRow/
// TableColumn) with quality Badges, an Input filter, a Select for condition
// type, and a Dialog to create/edit a format. Reads GET /api/v1/customformats
// (the native list). Writes go through the Radarr-compatible /api/v3 shim:
// POST /api/v1/customformats DOES NOT EXIST (the native surface only registers a
// GET — a POST 405s), whereas POST /api/v3/customformat is the working create
// route (crates/cellarr-api/src/shim.rs). Verified against the seeded daemon:
// the v3 create returns the persisted resource (200).

import * as React from 'react';

import Card from '@components/Card';
import Table from '@components/Table';
import TableRow from '@components/TableRow';
import TableColumn from '@components/TableColumn';
import Input from '@components/Input';
import Select from '@components/Select';
import Button from '@components/Button';
import ButtonGroup from '@components/ButtonGroup';
import Badge from '@components/Badge';
import Dialog from '@components/Dialog';
import Text from '@components/Text';
import Divider from '@components/Divider';

import { ApiError, CellarrClient, api as defaultApi } from '@lib/api/client';
import type { CustomFormat } from '@lib/api/types';

import { useAsync, toApiError } from '@app/settings/_components/useAsync';
import {
  Loading,
  ErrorBanner,
  SuccessBanner,
  EmptyState,
} from '@app/settings/_components/StatusBanners';

interface FormatRow {
  id: string;
  name: string;
  score: number;
  conditionType: string;
  conditionValue: string;
}

const CONDITION_TYPES = ['Release Title', 'Source', 'Resolution', 'Release Group', 'Language'];

function toRow(cf: CustomFormat): FormatRow {
  const rec = cf as Record<string, unknown>;
  const firstCond = Array.isArray(rec.conditions)
    ? (rec.conditions[0] as Record<string, unknown> | undefined)
    : undefined;
  return {
    id: String(rec.id ?? rec.name ?? ''),
    name: String(rec.name ?? rec.id ?? ''),
    score: typeof rec.score === 'number' ? rec.score : 0,
    conditionType: String(firstCond?.type ?? CONDITION_TYPES[0]),
    conditionValue: String(firstCond?.value ?? ''),
  };
}

const CustomFormats: React.FC<{ client?: CellarrClient }> = ({ client = defaultApi }) => {
  const load = React.useCallback(
    (signal: AbortSignal) => client.listCustomFormats(signal),
    [client]
  );
  const { data, loading, error, reload } = useAsync<CustomFormat[]>(load);

  const [filter, setFilter] = React.useState('');
  const [editing, setEditing] = React.useState<FormatRow | null>(null);
  const [saving, setSaving] = React.useState(false);
  const [saved, setSaved] = React.useState(false);
  const [saveError, setSaveError] = React.useState<ApiError | undefined>(undefined);

  const rows = React.useMemo(() => (data ?? []).map(toRow), [data]);
  const filtered = React.useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return rows;
    return rows.filter((r) => r.name.toLowerCase().includes(q));
  }, [rows, filter]);

  if (loading) return <Loading label="Loading custom formats" />;
  if (error) return <ErrorBanner error={error} />;

  const openNew = () =>
    setEditing({
      id: '',
      name: '',
      score: 0,
      conditionType: CONDITION_TYPES[0],
      conditionValue: '',
    });

  const save = async () => {
    if (!editing) return;
    setSaving(true);
    setSaveError(undefined);
    try {
      // The v3 create route is the working write path. We do not PUT-update here:
      // the native list this screen reads carries uuid ids, while the v3 resource
      // is keyed by a numeric projection (no stable uuid->numeric map on the
      // client), and there is no native customformat update route — so an edit is
      // persisted as a create. (PUT /api/v3/customformat/{numericId} exists for a
      // known numeric id; this screen does not hold one.)
      await client.createCustomFormat({
        name: editing.name,
        score: editing.score,
        conditions: [{ type: editing.conditionType, value: editing.conditionValue }],
      });
      setEditing(null);
      setSaved(true);
      reload();
    } catch (err) {
      setSaveError(toApiError(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Card title="Custom Formats">
      <div
        style={{
          display: 'flex',
          gap: '1ch',
          alignItems: 'flex-end',
          justifyContent: 'space-between',
          marginBottom: '1ch',
        }}
      >
        <div style={{ flex: 1 }}>
          <Text style={{ opacity: 0.6 }}>Filter</Text>
          <Input
            name="cf-filter"
            aria-label="Filter custom formats"
            placeholder="Search by name"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
        </div>
        <ButtonGroup items={[{ body: 'Add custom format', onClick: openNew }]} />
      </div>

      {saved ? <SuccessBanner>Custom format saved.</SuccessBanner> : null}

      <Divider type="GRADIENT" />

      {filtered.length ? (
        <Table>
          <TableRow>
            <TableColumn>Name</TableColumn>
            <TableColumn>Condition</TableColumn>
            <TableColumn>Score</TableColumn>
            <TableColumn> </TableColumn>
          </TableRow>
          {filtered.map((r) => (
            <TableRow key={r.id || r.name}>
              <TableColumn>{r.name}</TableColumn>
              <TableColumn>
                <Badge>{r.conditionType}</Badge>{' '}
                {r.conditionValue ? <code>{r.conditionValue}</code> : <span>—</span>}
              </TableColumn>
              <TableColumn>
                <Badge>{r.score >= 0 ? `+${r.score}` : r.score}</Badge>
              </TableColumn>
              <TableColumn>
                <Button theme="SECONDARY" onClick={() => setEditing({ ...r })}>
                  Edit
                </Button>
              </TableColumn>
            </TableRow>
          ))}
        </Table>
      ) : rows.length ? (
        <EmptyState>No custom formats match “{filter}”.</EmptyState>
      ) : (
        <EmptyState>No custom formats yet. Add one to score releases.</EmptyState>
      )}

      {editing ? (
        <div
          style={{
            position: 'fixed',
            inset: 0,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            background: 'var(--theme-overlay)',
            zIndex: 50,
            padding: '2ch',
          }}
        >
          <Dialog
            title={editing.id ? `Edit ${editing.name || 'format'}` : 'New custom format'}
            onConfirm={saving ? undefined : save}
            onCancel={() => setEditing(null)}
            style={{ maxWidth: '60ch', width: '100%' }}
          >
            <div style={{ margin: '1ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Name</Text>
              <Input
                name="cf-name"
                aria-label="Custom format name"
                value={editing.name}
                onChange={(e) => setEditing({ ...editing, name: e.target.value })}
              />
            </div>
            <div style={{ margin: '1ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Condition type</Text>
              <Select
                name="cf-condition-type"
                options={CONDITION_TYPES}
                defaultValue={editing.conditionType}
                onChange={(value) => setEditing({ ...editing, conditionType: value })}
              />
            </div>
            <div style={{ margin: '1ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Condition value (regex / term)</Text>
              <Input
                name="cf-condition-value"
                aria-label="Condition value"
                placeholder="e.g. \\b(2160p|4k)\\b"
                value={editing.conditionValue}
                onChange={(e) => setEditing({ ...editing, conditionValue: e.target.value })}
              />
            </div>
            <div style={{ margin: '1ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Score</Text>
              <Input
                name="cf-score"
                aria-label="Score"
                type="number"
                value={String(editing.score)}
                onChange={(e) => {
                  const n = Number.parseInt(e.target.value, 10);
                  setEditing({ ...editing, score: Number.isNaN(n) ? 0 : n });
                }}
              />
            </div>
            {saveError ? <ErrorBanner error={saveError} /> : null}
            {saving ? <Text role="status">Saving…</Text> : null}
          </Dialog>
        </div>
      ) : null}
    </Card>
  );
};

export default CustomFormats;
