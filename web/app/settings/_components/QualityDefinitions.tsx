'use client';

// Settings — Quality Definitions editor. Each quality (SDTV, WEBDL-1080p,
// Bluray-2160p, …) carries a size envelope — a minimum and maximum
// bytes-per-minute — that the decision engine uses to reject releases that are
// implausibly small (broken/sample) or implausibly large for their runtime. The
// daemon exposes these via GET /api/v3/qualitydefinition and now persists edits
// via PUT /api/v3/qualitydefinition/{id}.
//
// This section renders one editable row per definition: a display Title, a
// "Min size" and a "Max size" numeric Input, both in MB/min (the daemon stores
// bytes-per-minute; we convert at the edge). Save bulk-PUTs every changed row.
//
// SRCL-only: composed from vendored primitives (Card, Input, Button, Text,
// Divider, ListItem). Every input carries a real aria-label, units are shown in
// the label and as an adjacent glyph, and action results surface via the shared
// toast.

import * as React from 'react';

import Card from '@components/Card';
import Input from '@components/Input';
import Button from '@components/Button';
import Text from '@components/Text';
import Divider from '@components/Divider';
import ListItem from '@components/ListItem';

import { CellarrClient, api as defaultApi } from '@lib/api/client';
import type { QualityDefinition, QualityDefinitionBody } from '@lib/api/types';

import { buildQualityNameMap, resolveQualityName } from '@app/_lib/quality';

import { useToast } from '@app/_lib/ToastProvider';
import { useAsync, toApiError } from '@app/settings/_components/useAsync';
import { Loading, ErrorBanner, EmptyState } from '@app/settings/_components/StatusBanners';

// The daemon stores size bounds as bytes-per-minute; the editor speaks MB/min.
const BYTES_PER_MB = 1024 * 1024;

/** bytes-per-minute -> MB/min, rounded to 2dp for a clean editable field. */
function toMbPerMin(bytes: number | null | undefined): number {
  if (typeof bytes !== 'number' || !Number.isFinite(bytes) || bytes <= 0) return 0;
  return Math.round((bytes / BYTES_PER_MB) * 100) / 100;
}

/** MB/min -> bytes-per-minute (the wire unit). */
function toBytesPerMin(mb: number): number {
  return Math.round(mb * BYTES_PER_MB);
}

// The editable view-model for one definition row. `id` addresses the PUT route.
interface DefRow {
  id: number;
  name: string;
  title: string;
  minMb: number;
  /** null = no maximum (unbounded). */
  maxMb: number | null;
}

function toRow(d: QualityDefinition, names: ReturnType<typeof buildQualityNameMap>): DefRow {
  const id = String(d.quality?.id ?? d.id);
  const name = resolveQualityName(id, d.quality?.name ?? d.title, names);
  return {
    id: d.id,
    name,
    title: typeof d.title === 'string' ? d.title : name,
    minMb: toMbPerMin(d.minSize),
    maxMb: d.maxSize === null || d.maxSize === undefined ? null : toMbPerMin(d.maxSize),
  };
}

// A parseable, clamped non-negative number from a raw input value ('' -> 0).
function parseSize(raw: string): number {
  if (raw.trim() === '') return 0;
  const n = Number.parseFloat(raw);
  if (!Number.isFinite(n) || n < 0) return 0;
  return n;
}

// Has this row been edited away from the definition it was seeded from?
function isDirty(row: DefRow, original: DefRow): boolean {
  return (
    row.title !== original.title ||
    row.minMb !== original.minMb ||
    (row.maxMb ?? null) !== (original.maxMb ?? null)
  );
}

function toBody(row: DefRow): QualityDefinitionBody {
  return {
    id: row.id,
    title: row.title,
    minSize: toBytesPerMin(row.minMb),
    maxSize: row.maxMb === null ? null : toBytesPerMin(row.maxMb),
  };
}

const QualityDefinitions: React.FC<{ client?: CellarrClient }> = ({ client = defaultApi }) => {
  const { success, error: toastError } = useToast();

  const load = React.useCallback(
    (signal: AbortSignal) => client.getQualityDefinitions(signal),
    [client]
  );
  const { data, loading, error, reload } = useAsync<QualityDefinition[]>(load);

  const nameMap = React.useMemo(() => buildQualityNameMap(data), [data]);

  // The original (last-loaded) rows, and the working copy the user edits.
  const [rows, setRows] = React.useState<DefRow[]>([]);
  const [original, setOriginal] = React.useState<DefRow[]>([]);
  const [saving, setSaving] = React.useState(false);

  React.useEffect(() => {
    if (!data) return;
    const next = data.map((d) => toRow(d, nameMap));
    setRows(next);
    setOriginal(next);
  }, [data, nameMap]);

  if (loading) return <Loading label="Loading quality definitions" />;
  if (error) return <ErrorBanner error={error} />;

  const patchRow = (id: number, patch: Partial<DefRow>) => {
    setRows((rs) => rs.map((r) => (r.id === id ? { ...r, ...patch } : r)));
  };

  const dirtyRows = rows.filter((r, i) => original[i] && isDirty(r, original[i]));

  const save = async () => {
    if (!dirtyRows.length || saving) return;
    setSaving(true);
    try {
      await client.updateQualityDefinitions(dirtyRows.map(toBody));
      success(
        dirtyRows.length === 1
          ? 'Quality definition saved.'
          : `${dirtyRows.length} quality definitions saved.`
      );
      reload();
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not save quality definitions — ${e.message}`);
    } finally {
      setSaving(false);
    }
  };

  if (!rows.length) {
    return (
      <Card title="Quality Definitions">
        <EmptyState>No quality definitions yet.</EmptyState>
      </Card>
    );
  }

  return (
    <Card title="Quality Definitions">
      <Text style={{ opacity: 0.6 }}>
        Per-quality size envelope — releases below the minimum or above the maximum MB/min for
        their runtime are rejected. Leave Max empty for no upper bound; Min 0 means no minimum.
      </Text>

      <Divider type="GRADIENT" />

      <ul style={{ listStyle: 'none', padding: 0, margin: 0 }}>
        {rows.map((row) => (
          <ListItem key={row.id}>
            <div
              style={{
                display: 'flex',
                flexWrap: 'wrap',
                gap: '1ch',
                alignItems: 'flex-end',
              }}
            >
              <div style={{ flex: '2 1 16ch', minWidth: '16ch' }}>
                <Text style={{ opacity: 0.6 }}>Title</Text>
                <Input
                  name={`quality-def-title-${row.id}`}
                  aria-label={`Title for ${row.name}`}
                  value={row.title}
                  onChange={(e) => patchRow(row.id, { title: e.target.value })}
                />
              </div>

              <div style={{ flex: '1 1 12ch', minWidth: '11ch' }}>
                <Text style={{ opacity: 0.6 }}>Min (MB/min)</Text>
                <Input
                  name={`quality-def-min-${row.id}`}
                  aria-label={`Minimum size for ${row.name} in megabytes per minute`}
                  type="number"
                  min={0}
                  step={1}
                  value={String(row.minMb)}
                  onChange={(e) => patchRow(row.id, { minMb: parseSize(e.target.value) })}
                />
              </div>

              <div style={{ flex: '1 1 12ch', minWidth: '11ch' }}>
                <Text style={{ opacity: 0.6 }}>Max (MB/min)</Text>
                <Input
                  name={`quality-def-max-${row.id}`}
                  aria-label={`Maximum size for ${row.name} in megabytes per minute (empty for no maximum)`}
                  type="number"
                  min={0}
                  step={1}
                  placeholder="∞"
                  value={row.maxMb === null ? '' : String(row.maxMb)}
                  onChange={(e) => {
                    const raw = e.target.value;
                    patchRow(row.id, { maxMb: raw.trim() === '' ? null : parseSize(raw) });
                  }}
                />
              </div>
            </div>
          </ListItem>
        ))}
      </ul>

      <Divider type="GRADIENT" />

      <div style={{ marginTop: '1ch', display: 'flex', alignItems: 'center', gap: '1ch' }}>
        <Button
          theme="PRIMARY"
          isDisabled={saving || !dirtyRows.length}
          onClick={saving || !dirtyRows.length ? undefined : save}
        >
          {saving ? 'Saving…' : 'Save definitions'}
        </Button>
        {dirtyRows.length ? (
          <Text style={{ opacity: 0.6 }}>
            {dirtyRows.length} unsaved {dirtyRows.length === 1 ? 'change' : 'changes'}
          </Text>
        ) : null}
      </div>
    </Card>
  );
};

export default QualityDefinitions;
