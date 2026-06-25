'use client';

// Settings — Custom Formats (full editor). SRCL-only: a list of formats (each
// with Edit / Delete that opens the shared danger ConfirmDialog), and a form
// that authors/edits a format from a repeatable SPECIFICATION row builder plus a
// LIVE TEST box.
//
// The whole screen reads + writes the Radarr/Sonarr-compatible /api/v3 shim
// (crates/cellarr-api/src/shim.rs), where custom formats carry their match
// `specifications[]` and a stable numeric id (cf_numeric_id):
//   * GET  /api/v3/customformat         — the list (specifications[] per format);
//   * GET  /api/v3/customformat/schema  — the spec templates (per-implementation
//       fields: ReleaseTitle/Group/Language → a free-text value, Source/Resolution/
//       QualityModifier/ReleaseType → a select, Size → min/max numbers);
//   * POST /api/v3/customformat         — create; PUT /{id} — update (preserves
//       id + score); DELETE /{id} — remove;
//   * POST /api/v3/customformat/test    — live preview: which stored formats match
//       a typed release title.
// The native /api/v1/customformats surface only registers a GET (a POST 405s) and
// has no schema/test routes, so this screen never touches it.

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
import type {
  CustomFormatV3,
  CustomFormatSchema,
  CustomFormatSchemaField,
  CustomFormatSpecification,
  CustomFormatTestResult,
} from '@lib/api/types';

import { useToast } from '@app/_lib/ToastProvider';
import { useAsync, toApiError } from '@app/settings/_components/useAsync';
import { Loading, ErrorBanner, EmptyState } from '@app/settings/_components/StatusBanners';
import ConfirmDialog from '@app/settings/_components/ConfirmDialog';

// A single editable specification row. `value` covers the common single-`value`
// field (regex / token / select); `sizeMin`/`sizeMax` cover the two-field Size
// spec. Which inputs render is decided by the schema entry for `implementation`.
interface SpecRow {
  implementation: string;
  negate: boolean;
  required: boolean;
  value: string;
  sizeMin: string;
  sizeMax: string;
}

// The open editor's view-model. `id === undefined` marks a not-yet-persisted
// (new) format.
interface CfForm {
  id?: number;
  name: string;
  specs: SpecRow[];
}

// A fallback schema used when GET /customformat/schema is unavailable (offline /
// older daemon), so the editor still offers the modelled implementations. Mirrors
// the daemon's catalogue order.
const FALLBACK_SCHEMA: CustomFormatSchema[] = [
  { implementation: 'ReleaseTitleSpecification', implementationName: 'Release Title', negate: false, required: false, fields: [{ name: 'value', type: 'textbox' }] },
  { implementation: 'ReleaseGroupSpecification', implementationName: 'Release Group', negate: false, required: false, fields: [{ name: 'value', type: 'textbox' }] },
  { implementation: 'SourceSpecification', implementationName: 'Source', negate: false, required: false, fields: [{ name: 'value', type: 'select', selectOptions: ['webdl', 'webrip', 'bluray', 'remux', 'hdtv', 'dvd'].map((v) => ({ value: v, name: v })) }] },
  { implementation: 'ResolutionSpecification', implementationName: 'Resolution', negate: false, required: false, fields: [{ name: 'value', type: 'select', selectOptions: ['480p', '576p', '720p', '1080p', '2160p'].map((v) => ({ value: v, name: v })) }] },
  { implementation: 'LanguageSpecification', implementationName: 'Language', negate: false, required: false, fields: [{ name: 'value', type: 'textbox' }] },
];

function blankRow(schema: CustomFormatSchema[]): SpecRow {
  return {
    implementation: schema[0]?.implementation ?? 'ReleaseTitleSpecification',
    negate: false,
    required: false,
    value: '',
    sizeMin: '',
    sizeMax: '',
  };
}

// Pull the editable scalar out of a stored spec's `fields[]` for editing.
function rowFromSpec(spec: CustomFormatSpecification): SpecRow {
  const field = (name: string) => spec.fields.find((f) => f.name === name)?.value;
  const value = field('value');
  // Size carries either a {min,max} object on `value`, or explicit min/max fields.
  let sizeMin = '';
  let sizeMax = '';
  if (value && typeof value === 'object') {
    const obj = value as Record<string, unknown>;
    if (obj.min != null) sizeMin = String(obj.min);
    if (obj.max != null) sizeMax = String(obj.max);
  } else {
    const min = field('min');
    const max = field('max');
    if (min != null) sizeMin = String(min);
    if (max != null) sizeMax = String(max);
  }
  return {
    implementation: spec.implementation,
    negate: spec.negate === true,
    required: spec.required === true,
    value: value != null && typeof value !== 'object' ? String(value) : '',
    sizeMin,
    sizeMax,
  };
}

function formFromCf(cf: CustomFormatV3, schema: CustomFormatSchema[]): CfForm {
  const specs = Array.isArray(cf.specifications) && cf.specifications.length
    ? cf.specifications.map(rowFromSpec)
    : [blankRow(schema)];
  return { id: typeof cf.id === 'number' ? cf.id : undefined, name: cf.name, specs };
}

// The select field of a schema entry (if it has one).
function selectField(entry: CustomFormatSchema | undefined): CustomFormatSchemaField | undefined {
  return entry?.fields.find((f) => f.type === 'select');
}

// Is this a Size spec (two numeric min/max fields, no single `value`)?
function isSizeImpl(entry: CustomFormatSchema | undefined): boolean {
  return entry?.implementation === 'SizeSpecification';
}

// Map a row back to the v3 spec write shape. Size emits a {min,max} value object
// (bytes); every other implementation emits its single `value` field.
function rowToSpecBody(row: SpecRow, entry: CustomFormatSchema | undefined): CustomFormatSpecification {
  let fields: CustomFormatSpecification['fields'];
  if (isSizeImpl(entry)) {
    const min = row.sizeMin.trim() === '' ? undefined : Number(row.sizeMin);
    const max = row.sizeMax.trim() === '' ? undefined : Number(row.sizeMax);
    fields = [{ name: 'value', value: { min: min ?? null, max: max ?? null } }];
  } else {
    fields = [{ name: 'value', value: row.value }];
  }
  return {
    name: '',
    implementation: row.implementation,
    negate: row.negate,
    required: row.required,
    fields,
  };
}

const CustomFormats: React.FC<{ client?: CellarrClient }> = ({ client = defaultApi }) => {
  const { success, error: toastError } = useToast();

  const load = React.useCallback(
    (signal: AbortSignal) => client.listCustomFormatsV3(signal),
    [client]
  );
  const { data, loading, error, reload } = useAsync<CustomFormatV3[]>(load);

  // The schema seeds the spec-row builder's implementation choices + fields. A
  // failure here just falls back to the modelled set, so it never blocks the
  // screen.
  const loadSchema = React.useCallback(
    (signal: AbortSignal) => client.getCustomFormatSchema(signal),
    [client]
  );
  const { data: schemaData } = useAsync<CustomFormatSchema[]>(loadSchema);
  const schema = React.useMemo(
    () => (schemaData && schemaData.length ? schemaData : FALLBACK_SCHEMA),
    [schemaData]
  );
  const schemaFor = React.useCallback(
    (impl: string) => schema.find((s) => s.implementation === impl),
    [schema]
  );

  const [filter, setFilter] = React.useState('');
  const [form, setForm] = React.useState<CfForm | undefined>(undefined);
  const [saving, setSaving] = React.useState(false);
  const [saveError, setSaveError] = React.useState<ApiError | undefined>(undefined);
  const [pendingDelete, setPendingDelete] = React.useState<CustomFormatV3 | null>(null);
  const [deleting, setDeleting] = React.useState(false);

  // Live test box.
  const [testTitle, setTestTitle] = React.useState('');
  const [testing, setTesting] = React.useState(false);
  const [testResults, setTestResults] = React.useState<CustomFormatTestResult[] | undefined>(undefined);

  const formats = data ?? [];
  const filtered = React.useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return formats;
    return formats.filter((cf) => String(cf.name).toLowerCase().includes(q));
  }, [formats, filter]);

  if (loading) return <Loading label="Loading custom formats" />;
  if (error) return <ErrorBanner error={error} />;

  const openNew = () => {
    setForm({ name: '', specs: [blankRow(schema)] });
    setSaveError(undefined);
  };

  const openEdit = (cf: CustomFormatV3) => {
    setForm(formFromCf(cf, schema));
    setSaveError(undefined);
  };

  const closeForm = () => {
    setForm(undefined);
    setSaveError(undefined);
  };

  const updateRow = (index: number, patch: Partial<SpecRow>) => {
    setForm((f) =>
      f ? { ...f, specs: f.specs.map((r, i) => (i === index ? { ...r, ...patch } : r)) } : f
    );
  };

  const addRow = () => {
    setForm((f) => (f ? { ...f, specs: [...f.specs, blankRow(schema)] } : f));
  };

  const removeRow = (index: number) => {
    setForm((f) =>
      f ? { ...f, specs: f.specs.filter((_, i) => i !== index) } : f
    );
  };

  const save = async () => {
    if (!form) return;
    if (!form.name.trim()) {
      toastError('Give the custom format a name first.');
      return;
    }
    setSaving(true);
    setSaveError(undefined);
    try {
      const body: Partial<CustomFormatV3> = {
        name: form.name,
        specifications: form.specs.map((r) => rowToSpecBody(r, schemaFor(r.implementation))),
      };
      const editingId = form.id;
      if (typeof editingId === 'number') {
        await client.updateCustomFormat(editingId, body);
      } else {
        await client.createCustomFormat(body);
      }
      success(typeof editingId === 'number' ? 'Custom format saved.' : 'Custom format created.');
      closeForm();
      reload();
    } catch (err) {
      const e = toApiError(err);
      setSaveError(e);
      toastError(`Could not save custom format — ${e.message}`);
    } finally {
      setSaving(false);
    }
  };

  const confirmDelete = async () => {
    if (!pendingDelete) return;
    setDeleting(true);
    try {
      await client.deleteCustomFormat(pendingDelete.id);
      success('Custom format removed.');
      if (form && form.id === pendingDelete.id) closeForm();
      setPendingDelete(null);
      reload();
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not remove custom format — ${e.message}`);
    } finally {
      setDeleting(false);
    }
  };

  const runTest = async () => {
    setTesting(true);
    try {
      const results = await client.testCustomFormat({ title: testTitle });
      setTestResults(results);
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not run test — ${e.message}`);
    } finally {
      setTesting(false);
    }
  };

  // Render the value input(s) for a row, based on its schema entry: a select for
  // closed-choice specs, two number inputs for Size, else a free-text regex/token.
  const renderRowValue = (row: SpecRow, index: number) => {
    const entry = schemaFor(row.implementation);
    if (isSizeImpl(entry)) {
      return (
        <div style={{ display: 'flex', gap: '1ch' }}>
          <div style={{ flex: 1 }}>
            <Text style={{ opacity: 0.6 }}>Min size (bytes)</Text>
            <Input
              name={`spec-${index}-min`}
              aria-label={`Specification ${index + 1} minimum size`}
              type="number"
              value={row.sizeMin}
              onChange={(e) => updateRow(index, { sizeMin: e.target.value })}
            />
          </div>
          <div style={{ flex: 1 }}>
            <Text style={{ opacity: 0.6 }}>Max size (bytes)</Text>
            <Input
              name={`spec-${index}-max`}
              aria-label={`Specification ${index + 1} maximum size`}
              type="number"
              value={row.sizeMax}
              onChange={(e) => updateRow(index, { sizeMax: e.target.value })}
            />
          </div>
        </div>
      );
    }
    const sel = selectField(entry);
    if (sel && sel.selectOptions && sel.selectOptions.length) {
      const options = sel.selectOptions.map((o) => o.value);
      return (
        <div>
          <Text style={{ opacity: 0.6 }}>{sel.label || 'Value'}</Text>
          <Select
            name={`spec-${index}-value`}
            aria-label={`Specification ${index + 1} ${sel.label || 'value'}`}
            options={options}
            defaultValue={row.value || options[0]}
            onChange={(value) => updateRow(index, { value })}
          />
        </div>
      );
    }
    return (
      <div>
        <Text style={{ opacity: 0.6 }}>Value (regex / term)</Text>
        <Input
          name={`spec-${index}-value`}
          aria-label={`Specification ${index + 1} value`}
          placeholder="e.g. \\b(2160p|4k)\\b"
          value={row.value}
          onChange={(e) => updateRow(index, { value: e.target.value })}
        />
      </div>
    );
  };

  const implOptions = schema.map((s) => s.implementationName);
  const implByName = (name: string) =>
    schema.find((s) => s.implementationName === name)?.implementation ??
    schema[0]?.implementation ??
    'ReleaseTitleSpecification';
  const nameByImpl = (impl: string) =>
    schema.find((s) => s.implementation === impl)?.implementationName ?? impl;

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

      <Divider type="GRADIENT" />

      {filtered.length ? (
        <ul style={{ listStyle: 'none', padding: 0, margin: '0 0 1ch 0' }}>
          {filtered.map((cf) => (
            <li
              key={cf.id}
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'space-between',
                gap: '1ch',
                padding: '0.5ch 0',
              }}
            >
              <span>
                <strong>{cf.name}</strong>{' '}
                <Badge>
                  {cf.specifications?.length ?? 0} spec
                  {(cf.specifications?.length ?? 0) === 1 ? '' : 's'}
                </Badge>
              </span>
              <span style={{ display: 'inline-flex', gap: '0.5ch' }}>
                <Button theme="SECONDARY" aria-label={`Edit ${cf.name}`} onClick={() => openEdit(cf)}>
                  Edit
                </Button>
                <Button
                  theme="DANGER"
                  aria-label={`Delete ${cf.name}`}
                  onClick={() => setPendingDelete(cf)}
                >
                  Delete
                </Button>
              </span>
            </li>
          ))}
        </ul>
      ) : formats.length ? (
        <EmptyState>No custom formats match “{filter}”.</EmptyState>
      ) : (
        <EmptyState>No custom formats yet. Add one to score releases.</EmptyState>
      )}

      <Divider type="GRADIENT" />

      {/* Live test box — type a release title and see which stored formats match. */}
      <div style={{ margin: '1ch 0' }}>
        <Text style={{ opacity: 0.6 }}>Test a release title</Text>
        <div style={{ display: 'flex', gap: '1ch', alignItems: 'flex-end' }}>
          <div style={{ flex: 1 }}>
            <Input
              name="cf-test-title"
              aria-label="Test release title"
              placeholder="The.Movie.2024.2160p.WEB-DL.x265-GROUP"
              value={testTitle}
              onChange={(e) => setTestTitle(e.target.value)}
            />
          </div>
          <Button
            theme="SECONDARY"
            aria-label="Run test"
            onClick={testing ? undefined : runTest}
          >
            {testing ? 'Testing…' : 'Test'}
          </Button>
        </div>
        {testResults ? (
          testResults.length ? (
            <ul role="list" style={{ listStyle: 'none', padding: 0, margin: '1ch 0 0 0' }}>
              {testResults.map((r) => (
                <li
                  key={r.id}
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: '1ch',
                    padding: '0.25ch 0',
                  }}
                >
                  <Badge>{r.matched ? '✓ match' : '✗ no'}</Badge>
                  <span style={{ opacity: r.matched ? 1 : 0.5 }}>{r.name}</span>
                  <span style={{ opacity: 0.5 }}>
                    {r.score >= 0 ? `+${r.score}` : r.score}
                  </span>
                </li>
              ))}
            </ul>
          ) : (
            <EmptyState>No custom formats to test against.</EmptyState>
          )
        ) : null}
      </div>

      {/* The author / edit form. */}
      {form ? (
        <>
          <Divider type="GRADIENT" />
          <Text style={{ opacity: 0.6, margin: '1ch 0 0.5ch' }}>
            {typeof form.id === 'number' ? `Editing ${form.name || 'format'}` : 'New custom format'}
          </Text>

          <div style={{ margin: '0.5ch 0' }}>
            <Text style={{ opacity: 0.6 }}>Name</Text>
            <Input
              name="cf-name"
              aria-label="Custom format name"
              value={form.name}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
            />
          </div>

          <Text style={{ opacity: 0.6, margin: '1ch 0 0.5ch' }}>Specifications</Text>
          {form.specs.map((row, index) => {
            const entry = schemaFor(row.implementation);
            return (
              <div
                key={index}
                style={{
                  border: '1px solid var(--theme-border, currentColor)',
                  padding: '1ch',
                  margin: '0.5ch 0',
                }}
              >
                <div style={{ display: 'flex', gap: '1ch', alignItems: 'flex-end' }}>
                  <div style={{ flex: 1 }}>
                    <Text style={{ opacity: 0.6 }}>Condition</Text>
                    <Select
                      name={`spec-${index}-impl`}
                      aria-label={`Specification ${index + 1} condition`}
                      options={implOptions}
                      defaultValue={nameByImpl(row.implementation)}
                      onChange={(value) =>
                        updateRow(index, { implementation: implByName(value), value: '' })
                      }
                    />
                  </div>
                  <Button
                    theme="SECONDARY"
                    aria-label={`Remove specification ${index + 1}`}
                    isDisabled={form.specs.length <= 1}
                    onClick={() => removeRow(index)}
                  >
                    ✗
                  </Button>
                </div>

                <div style={{ margin: '0.5ch 0' }}>{renderRowValue(row, index)}</div>

                <div style={{ display: 'flex', gap: '2ch', margin: '0.5ch 0' }}>
                  <Checkbox
                    key={`${index}-${row.implementation}-negate`}
                    name={`spec-${index}-negate`}
                    aria-label={`Negate specification ${index + 1}`}
                    defaultChecked={row.negate}
                    onChange={(e) => updateRow(index, { negate: e.target.checked })}
                  >
                    Negate
                  </Checkbox>
                  <Checkbox
                    key={`${index}-${row.implementation}-required`}
                    name={`spec-${index}-required`}
                    aria-label={`Require specification ${index + 1}`}
                    defaultChecked={row.required}
                    onChange={(e) => updateRow(index, { required: e.target.checked })}
                  >
                    Required
                  </Checkbox>
                  {entry ? (
                    <span style={{ opacity: 0.4 }}>{entry.implementationName}</span>
                  ) : null}
                </div>
              </div>
            );
          })}

          <div style={{ margin: '0.5ch 0' }}>
            <Button theme="SECONDARY" aria-label="Add specification" onClick={addRow}>
              + Add specification
            </Button>
          </div>

          {saveError ? <ErrorBanner error={saveError} /> : null}

          <div style={{ marginTop: '1ch', display: 'flex', gap: '1ch', alignItems: 'center', flexWrap: 'wrap' }}>
            <Button theme="PRIMARY" isDisabled={saving} onClick={saving ? undefined : save}>
              {saving ? 'Saving…' : typeof form.id === 'number' ? 'Save format' : 'Create format'}
            </Button>
            <Button theme="SECONDARY" isDisabled={saving} onClick={saving ? undefined : closeForm}>
              Cancel
            </Button>
          </div>
        </>
      ) : null}

      {pendingDelete ? (
        <ConfirmDialog
          title="Delete custom format"
          confirmLabel="Delete custom format"
          pendingLabel="Deleting…"
          pending={deleting}
          onConfirm={confirmDelete}
          onCancel={() => (deleting ? undefined : setPendingDelete(null))}
        >
          <Text>
            Delete <strong>{pendingDelete.name || 'this format'}</strong>? Any quality profile
            scoring against it will lose those points.
          </Text>
        </ConfirmDialog>
      ) : null}
    </Card>
  );
};

export default CustomFormats;
