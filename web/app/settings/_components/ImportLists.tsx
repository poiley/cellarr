'use client';

// Settings — Import Lists. Full CRUD against the Radarr/Sonarr-compatible
// /api/v3/importlist surface, plus a "Sync now" trigger and an import-list
// exclusions list. SRCL-only: Card, Table, Input, Select, Checkbox, Button,
// ButtonGroup, Badge, Divider, Text, plus the shared ConfirmDialog (destructive
// delete) and useToast (Test/Save/Sync/Delete feedback).
//
// The per-type credential fields are driven by /importlist/schema: choosing a
// source type renders exactly the fields that source advertises (TMDb api_key +
// list_id/feed/window, Trakt client_id + list, Plex token, IMDb json_url, …).
// The form maps those flat inputs back to the v3 `fields[]` body the shim reads,
// alongside the top-level shouldMonitor + cleanLibraryLevel flags and the
// qualityProfileId field. The safeguard's `lastSuccessfulSync` (and a failed
// sync's `fetchSucceeded:false`) are surfaced verbatim so an unavailable list
// never looks like an empty (clean-eligible) one.

import * as React from 'react';

import Card from '@components/Card';
import Table from '@components/Table';
import TableRow from '@components/TableRow';
import TableColumn from '@components/TableColumn';
import Input from '@components/Input';
import Select from '@components/Select';
import Checkbox from '@components/Checkbox';
import Button from '@components/Button';
import Badge from '@components/Badge';
import Divider from '@components/Divider';
import Text from '@components/Text';

import { ApiError, CellarrClient, api as defaultApi } from '@lib/api/client';
import type {
  ImportListBodyV3,
  ImportListConfigV3,
  ImportListExclusionV3,
  ImportListField,
  ImportListSchema,
  Library,
  QualityProfile,
} from '@lib/api/types';

import { useToast } from '@app/_lib/ToastProvider';
import { useAsync, toApiError } from '@app/settings/_components/useAsync';
import { Loading, ErrorBanner, EmptyState } from '@app/settings/_components/StatusBanners';
import ConfirmDialog from '@app/settings/_components/ConfirmDialog';
import ManagedBadge, { isManaged } from '@app/settings/_components/ManagedBadge';

// The clean-library options the shim accepts; "disabled" is the safe default and
// the only one that can never remove a library item.
const CLEAN_OPTIONS = ['disabled', 'logOnly', 'removeAndKeep'] as const;
const CLEAN_LABELS: Record<string, string> = {
  disabled: 'Disabled (never touch the library)',
  logOnly: 'Log only (unmonitor)',
  removeAndKeep: 'Remove from library (keep files)',
};

// A short, friendly name per implementation for the type dropdown.
const IMPL_LABELS: Record<string, string> = {
  TMDbListImport: 'TMDb List',
  TMDbCollectionImport: 'TMDb Collection',
  TraktList: 'Trakt',
  PlexImport: 'Plex',
  IMDbListImport: 'IMDb',
  CustomImport: 'Custom',
};

// The schema fields the form models as top-level controls (not free-form source
// settings), so they are not double-rendered as credential inputs.
const RESERVED_FIELDS = new Set(['shouldMonitor', 'cleanLibraryLevel', 'qualityProfileId']);

interface ImportListForm {
  /** Numeric id when editing an existing list; null for a new one. */
  id: number | null;
  name: string;
  implementation: string;
  enabled: boolean;
  shouldMonitor: boolean;
  cleanLibraryLevel: string;
  qualityProfileId: string;
  /** Source-specific credential/setting values keyed by field name. */
  settings: Record<string, string>;
}

function blankForm(implementation: string): ImportListForm {
  return {
    id: null,
    name: '',
    implementation,
    enabled: true,
    shouldMonitor: true,
    cleanLibraryLevel: 'disabled',
    qualityProfileId: '',
    settings: {},
  };
}

/** Read a stored config's source settings out of its `fields[]` projection. */
function settingsFromConfig(cfg: ImportListConfigV3): Record<string, string> {
  const out: Record<string, string> = {};
  for (const f of cfg.fields ?? []) {
    if (RESERVED_FIELDS.has(f.name)) continue;
    if (f.value == null) continue;
    out[f.name] = String(f.value);
  }
  return out;
}

function configToForm(cfg: ImportListConfigV3): ImportListForm {
  const qpField = (cfg.fields ?? []).find((f) => f.name === 'qualityProfileId');
  return {
    id: cfg.id,
    name: cfg.name ?? '',
    implementation: cfg.implementation ?? 'CustomImport',
    enabled: cfg.enabled !== false,
    shouldMonitor: cfg.shouldMonitor !== false,
    cleanLibraryLevel: cfg.cleanLibraryLevel ?? 'disabled',
    qualityProfileId: qpField?.value != null ? String(qpField.value) : '',
    settings: settingsFromConfig(cfg),
  };
}

/** Map the flat form to the v3 import-list write body. */
function formToBody(form: ImportListForm, schemaFields: ImportListField[]): ImportListBodyV3 {
  const fields: { name: string; value: unknown }[] = [];
  for (const f of schemaFields) {
    if (RESERVED_FIELDS.has(f.name)) continue;
    const raw = form.settings[f.name];
    if (raw == null || raw === '') continue;
    fields.push({ name: f.name, value: raw });
  }
  if (form.qualityProfileId) {
    fields.push({ name: 'qualityProfileId', value: form.qualityProfileId });
  }
  return {
    name: form.name.trim(),
    implementation: form.implementation,
    enabled: form.enabled,
    shouldMonitor: form.shouldMonitor,
    cleanLibraryLevel: form.cleanLibraryLevel,
    fields,
  };
}

const ImportLists: React.FC<{ client?: CellarrClient }> = ({ client = defaultApi }) => {
  const { success, error: toastError, info } = useToast();

  const loadLists = React.useCallback(
    (signal: AbortSignal) => client.listImportLists(signal),
    [client]
  );
  const { data: listsData, loading, error, reload } = useAsync<ImportListConfigV3[]>(loadLists);

  const loadSchema = React.useCallback(
    (signal: AbortSignal) => client.getImportListSchema(signal),
    [client]
  );
  const { data: schemaData } = useAsync<ImportListSchema[]>(loadSchema);

  const loadProfiles = React.useCallback(
    (signal: AbortSignal) => client.getQualityProfiles(signal),
    [client]
  );
  const { data: profilesData } = useAsync<QualityProfile[]>(loadProfiles);

  const loadLibraries = React.useCallback(
    (signal: AbortSignal) => client.listLibraries(signal),
    [client]
  );
  const { data: librariesData } = useAsync<Library[]>(loadLibraries);

  const loadExclusions = React.useCallback(
    (signal: AbortSignal) => client.listImportListExclusions(signal),
    [client]
  );
  const {
    data: exclusionsData,
    loading: exclusionsLoading,
    reload: reloadExclusions,
  } = useAsync<ImportListExclusionV3[]>(loadExclusions);

  const schema = schemaData ?? [];
  const lists = listsData ?? [];
  const profiles = profilesData ?? [];
  const libraries = librariesData ?? [];
  const exclusions = exclusionsData ?? [];

  const implementations = schema.map((s) => s.implementation);
  const defaultImpl = implementations[0] ?? 'CustomImport';

  const [form, setForm] = React.useState<ImportListForm>(() => blankForm(defaultImpl));
  // The form is initialized once with a fallback impl; align it to the real
  // schema's first implementation once that loads (only while still untouched).
  const alignedRef = React.useRef(false);
  React.useEffect(() => {
    if (alignedRef.current || form.id !== null) return;
    if (implementations.length && !implementations.includes(form.implementation)) {
      setForm((f) => ({ ...f, implementation: implementations[0] }));
      alignedRef.current = true;
    }
  }, [implementations, form.id, form.implementation]);

  // Default a NEW list's quality profile to the first available one, so the
  // dropdown never renders blank while Target Library already shows a value (and
  // a list can't silently save with no profile). Editing keeps the stored value.
  React.useEffect(() => {
    if (form.id !== null) return;
    if (!form.qualityProfileId && profiles.length) {
      setForm((f) => (f.id === null && !f.qualityProfileId ? { ...f, qualityProfileId: profiles[0].id } : f));
    }
  }, [profiles, form.id, form.qualityProfileId]);

  const [testing, setTesting] = React.useState(false);
  const [saving, setSaving] = React.useState(false);
  const [syncing, setSyncing] = React.useState<number | null>(null);
  const [saveError, setSaveError] = React.useState<ApiError | undefined>(undefined);
  const [pendingDelete, setPendingDelete] = React.useState<ImportListConfigV3 | null>(null);
  const [deleting, setDeleting] = React.useState(false);

  // The new-exclusion mini-form (id type + value + title).
  const [exForm, setExForm] = React.useState({ idType: 'tmdbId', idValue: '', title: '' });
  const [exAdding, setExAdding] = React.useState(false);

  const schemaFor = (impl: string): ImportListField[] =>
    schema.find((s) => s.implementation === impl)?.fields ?? [];

  const credentialFields = schemaFor(form.implementation).filter(
    (f) => !RESERVED_FIELDS.has(f.name)
  );

  const edit = (cfg: ImportListConfigV3) => {
    setForm(configToForm(cfg));
    setSaveError(undefined);
  };

  const reset = () => {
    setForm(blankForm(implementations[0] ?? defaultImpl));
    setSaveError(undefined);
  };

  const setSetting = (name: string, value: string) =>
    setForm((f) => ({ ...f, settings: { ...f.settings, [name]: value } }));

  const test = async () => {
    if (!form.name.trim()) {
      toastError('Give the list a name before testing.');
      return;
    }
    setTesting(true);
    info('Validating import list…', { durationMs: 2000 });
    try {
      await client.testImportList(formToBody(form, schemaFor(form.implementation)));
      success('Import list is valid.');
    } catch (err) {
      const e = toApiError(err);
      toastError(`Test failed — ${e.message}`);
    } finally {
      setTesting(false);
    }
  };

  const save = async () => {
    if (!form.name.trim()) {
      toastError('Give the list a name before saving.');
      return;
    }
    setSaving(true);
    setSaveError(undefined);
    try {
      const body = formToBody(form, schemaFor(form.implementation));
      if (form.id !== null) {
        await client.updateImportList(form.id, body);
        success('Import list updated.');
      } else {
        await client.createImportList(body);
        success('Import list saved.');
      }
      reset();
      reload();
    } catch (err) {
      const e = toApiError(err);
      setSaveError(e);
      toastError(`Could not save — ${e.message}`);
    } finally {
      setSaving(false);
    }
  };

  const syncNow = async (cfg: ImportListConfigV3) => {
    setSyncing(cfg.id);
    info(`Syncing ${cfg.name || 'import list'}…`);
    try {
      const result = await client.syncImportList(cfg.id);
      if (!result.triggered) {
        toastError(result.message || 'Sync is not available.');
      } else {
        const report = result.lists?.[0];
        if (report && !report.fetchSucceeded) {
          // The safeguard path: a failed fetch cleaned nothing. Say so plainly.
          toastError(
            `${cfg.name || 'List'} unavailable — ${report.failureReason || 'source fetch failed'} (library untouched)`
          );
        } else {
          const added = report?.added ?? 0;
          const cleaned = report?.cleaned ?? 0;
          success(`Synced ${cfg.name || 'list'}: ${added} added, ${cleaned} cleaned.`);
        }
        reload();
      }
    } catch (err) {
      const e = toApiError(err);
      toastError(`Sync failed — ${e.message}`);
    } finally {
      setSyncing(null);
    }
  };

  const confirmDelete = async () => {
    if (!pendingDelete) return;
    setDeleting(true);
    try {
      await client.deleteImportList(pendingDelete.id);
      success('Import list removed.');
      if (form.id === pendingDelete.id) reset();
      setPendingDelete(null);
      reload();
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not remove — ${e.message}`);
    } finally {
      setDeleting(false);
    }
  };

  const addExclusion = async () => {
    if (!exForm.idValue.trim()) {
      toastError('An external id is required to add an exclusion.');
      return;
    }
    setExAdding(true);
    try {
      await client.createImportListExclusion({
        [exForm.idType]: exForm.idValue.trim(),
        title: exForm.title.trim() || undefined,
      });
      success('Exclusion added.');
      setExForm({ idType: exForm.idType, idValue: '', title: '' });
      reloadExclusions();
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not add exclusion — ${e.message}`);
    } finally {
      setExAdding(false);
    }
  };

  const removeExclusion = async (ex: ImportListExclusionV3) => {
    try {
      await client.deleteImportListExclusion(ex.id);
      success('Exclusion removed.');
      reloadExclusions();
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not remove exclusion — ${e.message}`);
    }
  };

  function exclusionExternalId(ex: ImportListExclusionV3): string {
    if (ex.tmdbId != null) return `tmdb:${ex.tmdbId}`;
    if (ex.tvdbId != null) return `tvdb:${ex.tvdbId}`;
    if (ex.imdbId != null) return `imdb:${ex.imdbId}`;
    return '—';
  }

  if (loading) return <Loading label="Loading import lists" />;
  if (error) return <ErrorBanner error={error} />;

  return (
    <Card title="Import Lists">
      <Text style={{ opacity: 0.6 }}>
        Sync titles from an external source (TMDb, Trakt, Plex, IMDb, a TMDb collection) into your
        library. A failed source fetch never cleans anything — the safeguard only cleans after a
        confirmed-good fetch.
      </Text>

      <Divider type="GRADIENT" />

      {lists.length ? (
        <Table>
          <TableRow>
            <TableColumn>Name</TableColumn>
            <TableColumn>Type</TableColumn>
            <TableColumn>Monitor</TableColumn>
            <TableColumn>Last sync</TableColumn>
            <TableColumn> </TableColumn>
          </TableRow>
          {lists.map((cfg) => {
            const managed = isManaged(cfg);
            return (
              <TableRow key={cfg.id}>
                <TableColumn>
                  <Badge>{cfg.enabled ? 'enabled' : 'disabled'}</Badge> {cfg.name || '(unnamed)'}{' '}
                  {managed ? (
                    <ManagedBadge entityLabel={`Import list ${cfg.name || '(unnamed)'}`} />
                  ) : null}
                </TableColumn>
                <TableColumn>{IMPL_LABELS[cfg.implementation] ?? cfg.implementation}</TableColumn>
                <TableColumn>{cfg.shouldMonitor ? 'monitored' : 'not monitored'}</TableColumn>
                <TableColumn style={{ whiteSpace: 'nowrap' }}>
                  {cfg.lastSuccessfulSync
                    ? new Date(cfg.lastSuccessfulSync * 1000).toLocaleString()
                    : 'never'}
                </TableColumn>
                <TableColumn>
                  <span style={{ display: 'inline-flex', gap: '0.5ch' }}>
                    <Button
                      theme="SECONDARY"
                      aria-label={`Sync ${cfg.name || 'list'} now`}
                      isDisabled={syncing === cfg.id}
                      onClick={() => syncNow(cfg)}
                    >
                      {syncing === cfg.id ? 'Syncing…' : 'Sync now'}
                    </Button>
                    <Button
                      theme="SECONDARY"
                      aria-label={`Edit ${cfg.name || 'import list'}`}
                      isDisabled={managed}
                      onClick={managed ? undefined : () => edit(cfg)}
                    >
                      Edit
                    </Button>
                    <Button
                      theme="DANGER"
                      aria-label={`Remove ${cfg.name || 'import list'}`}
                      isDisabled={managed}
                      onClick={managed ? undefined : () => setPendingDelete(cfg)}
                    >
                      Remove
                    </Button>
                  </span>
                </TableColumn>
              </TableRow>
            );
          })}
        </Table>
      ) : (
        <EmptyState>No import lists configured yet.</EmptyState>
      )}

      <Divider type="GRADIENT" />

      <Text style={{ opacity: 0.6, margin: '1ch 0 0.5ch' }}>
        {form.id !== null ? `Editing ${form.name || '(unnamed)'}` : 'New import list'}
      </Text>

      <div style={{ margin: '0.5ch 0' }}>
        <Text style={{ opacity: 0.6 }}>
          Name<span aria-hidden="true"> *</span>
        </Text>
        <Input
          name="importlist-name"
          aria-label="Import list name"
          aria-required
          value={form.name}
          onChange={(e) => setForm({ ...form, name: e.target.value })}
        />
      </div>

      <div style={{ margin: '0.5ch 0' }}>
        <Text style={{ opacity: 0.6 }}>Type</Text>
        <Select
          name="importlist-type"
          aria-label="Import list type"
          options={implementations.length ? implementations : [defaultImpl]}
          defaultValue={form.implementation}
          onChange={(value) => setForm({ ...form, implementation: value, settings: {} })}
        />
      </div>

      {/* Per-type credential / setting fields, straight from the schema. */}
      {credentialFields.map((f) => {
        const isSelect = f.type === 'select' && (f.selectOptions?.length ?? 0) > 0;
        const isCheckbox = f.type === 'checkbox';
        const label = f.label || f.name;
        if (isCheckbox) {
          return (
            <div key={f.name} style={{ margin: '0.5ch 0' }}>
              <Checkbox
                name={`importlist-${f.name}`}
                aria-label={label}
                defaultChecked={form.settings[f.name] === 'true'}
                onChange={(e) => setSetting(f.name, e.target.checked ? 'true' : 'false')}
              >
                {label}
              </Checkbox>
            </div>
          );
        }
        return (
          <div key={f.name} style={{ margin: '0.5ch 0' }}>
            <Text style={{ opacity: 0.6 }}>{label}</Text>
            {isSelect ? (
              <Select
                name={`importlist-${f.name}`}
                aria-label={label}
                options={(f.selectOptions ?? []).map((o) => o.value)}
                defaultValue={form.settings[f.name] ?? String(f.value ?? '')}
                onChange={(value) => setSetting(f.name, value)}
              />
            ) : (
              <Input
                name={`importlist-${f.name}`}
                aria-label={label}
                type={f.privacy === 'apiKey' ? 'password' : 'text'}
                value={form.settings[f.name] ?? ''}
                onChange={(e) => setSetting(f.name, e.target.value)}
              />
            )}
            {f.helpText ? (
              <Text style={{ opacity: 0.4, fontSize: '0.85em' }}>{f.helpText}</Text>
            ) : null}
          </div>
        );
      })}

      {/* Target library (informational; the face/media-type pins the surface). */}
      {libraries.length ? (
        <div style={{ margin: '0.5ch 0' }}>
          <Text style={{ opacity: 0.6 }}>Target library</Text>
          <Select
            name="importlist-library"
            aria-label="Target library"
            options={libraries.map((l) => l.name)}
            defaultValue={libraries[0]?.name ?? ''}
          />
        </div>
      ) : null}

      <div style={{ margin: '0.5ch 0' }}>
        <Text style={{ opacity: 0.6 }}>Quality profile</Text>
        <Select
          name="importlist-quality"
          aria-label="Quality profile"
          key={`qp-${form.id ?? 'new'}-${form.qualityProfileId}`}
          options={profiles.map((p) => p.name)}
          placeholder="Select a quality profile…"
          defaultValue={profiles.find((p) => p.id === form.qualityProfileId)?.name ?? ''}
          onChange={(value) => {
            const match = profiles.find((p) => p.name === value);
            setForm({ ...form, qualityProfileId: match ? match.id : '' });
          }}
        />
      </div>

      <div style={{ margin: '0.5ch 0' }}>
        <Text style={{ opacity: 0.6 }}>Clean library</Text>
        <Select
          name="importlist-clean"
          aria-label="Clean library level"
          options={CLEAN_OPTIONS.map((o) => o)}
          defaultValue={form.cleanLibraryLevel}
          onChange={(value) => setForm({ ...form, cleanLibraryLevel: value })}
        />
        <Text style={{ opacity: 0.4, fontSize: '0.85em' }}>
          {CLEAN_LABELS[form.cleanLibraryLevel] ?? form.cleanLibraryLevel}
        </Text>
      </div>

      <div style={{ display: 'flex', gap: '2ch', margin: '0.5ch 0' }}>
        <Checkbox
          name="importlist-monitor"
          aria-label="Monitor added items"
          defaultChecked={form.shouldMonitor}
          onChange={(e) => setForm({ ...form, shouldMonitor: e.target.checked })}
        >
          Monitor added items
        </Checkbox>
        <Checkbox
          name="importlist-enabled"
          aria-label="Enabled"
          defaultChecked={form.enabled}
          onChange={(e) => setForm({ ...form, enabled: e.target.checked })}
        >
          Enabled
        </Checkbox>
      </div>

      {saveError ? <ErrorBanner error={saveError} /> : null}

      <div style={{ marginTop: '1ch', display: 'flex', gap: '1ch', alignItems: 'center', flexWrap: 'wrap' }}>
        <Button
          theme="PRIMARY"
          isDisabled={saving}
          onClick={saving ? undefined : save}
        >
          {saving ? 'Saving…' : form.id !== null ? 'Save list' : 'Save'}
        </Button>
        <Button theme="SECONDARY" isDisabled={testing} onClick={testing ? undefined : test}>
          {testing ? 'Testing…' : 'Test'}
        </Button>
        {form.id !== null ? (
          <>
            <Button
              theme="SECONDARY"
              onClick={() => {
                const cfg = lists.find((l) => l.id === form.id);
                if (cfg) void syncNow(cfg);
              }}
            >
              Sync now
            </Button>
            <Button theme="SECONDARY" onClick={reset}>
              New
            </Button>
          </>
        ) : null}
      </div>

      {pendingDelete ? (
        <ConfirmDialog
          title="Remove import list"
          confirmLabel="Remove import list"
          pendingLabel="Removing…"
          pending={deleting}
          onConfirm={confirmDelete}
          onCancel={() => (deleting ? undefined : setPendingDelete(null))}
        >
          <Text>
            Remove <strong>{pendingDelete.name || 'this import list'}</strong>? cellarr will stop
            syncing titles from it. Existing library items are not removed.
          </Text>
        </ConfirmDialog>
      ) : null}

      {/* --- Exclusions ---------------------------------------------------- */}
      <Divider type="GRADIENT" />

      <Text style={{ opacity: 0.6, margin: '1ch 0 0.5ch' }}>Import-list exclusions</Text>
      <Text style={{ opacity: 0.5, marginBottom: '0.5ch' }}>
        Titles an import list must never re-add. Keyed by external id.
      </Text>

      {exclusionsLoading ? (
        <Loading label="Loading exclusions" />
      ) : exclusions.length ? (
        <Table>
          <TableRow>
            <TableColumn>Title</TableColumn>
            <TableColumn>External id</TableColumn>
            <TableColumn> </TableColumn>
          </TableRow>
          {exclusions.map((ex) => (
            <TableRow key={ex.id}>
              <TableColumn>{ex.title || '(untitled)'}</TableColumn>
              <TableColumn>
                <code>{exclusionExternalId(ex)}</code>
              </TableColumn>
              <TableColumn>
                <Button
                  theme="DANGER"
                  aria-label={`Remove exclusion ${ex.title || exclusionExternalId(ex)}`}
                  onClick={() => removeExclusion(ex)}
                >
                  Remove
                </Button>
              </TableColumn>
            </TableRow>
          ))}
        </Table>
      ) : (
        <EmptyState>No exclusions.</EmptyState>
      )}

      <div style={{ display: 'flex', gap: '1ch', alignItems: 'flex-end', marginTop: '1ch' }}>
        <div style={{ flex: '0 0 14ch' }}>
          <Text style={{ opacity: 0.6 }}>Id type</Text>
          <Select
            name="exclusion-idtype"
            aria-label="Exclusion id type"
            options={['tmdbId', 'tvdbId', 'imdbId']}
            defaultValue={exForm.idType}
            onChange={(value) => setExForm({ ...exForm, idType: value })}
          />
        </div>
        <div style={{ flex: 1 }}>
          <Text style={{ opacity: 0.6 }}>External id</Text>
          <Input
            name="exclusion-idvalue"
            aria-label="Exclusion external id"
            placeholder="603"
            value={exForm.idValue}
            onChange={(e) => setExForm({ ...exForm, idValue: e.target.value })}
          />
        </div>
        <div style={{ flex: 1 }}>
          <Text style={{ opacity: 0.6 }}>Title (optional)</Text>
          <Input
            name="exclusion-title"
            aria-label="Exclusion title"
            placeholder="The Matrix"
            value={exForm.title}
            onChange={(e) => setExForm({ ...exForm, title: e.target.value })}
          />
        </div>
        <Button isDisabled={exAdding} onClick={exAdding ? undefined : addExclusion}>
          {exAdding ? 'Adding…' : 'Add exclusion'}
        </Button>
      </div>
    </Card>
  );
};

export default ImportLists;
