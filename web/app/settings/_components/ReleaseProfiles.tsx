'use client';

// Settings — Release Profiles. SRCL-only: a list of release profiles (each with
// Edit / Delete that opens the shared danger ConfirmDialog) and a form to
// author/edit one — a name, an enabled Checkbox, a tag-chip scope field, and
// three repeatable term lists: Required terms (a release MUST contain), Ignored
// terms (reject on match), and Preferred terms (each a term + a numeric score
// nudge). Save toasts on success.
//
// Reads + writes the Sonarr-compatible /api/v3 shim (crates/cellarr-api/src/shim.rs),
// where release profiles live with a stable JS-safe numeric id:
//   * GET    /api/v3/releaseprofile      — the list;
//   * POST   /api/v3/releaseprofile      — create;
//   * PUT    /api/v3/releaseprofile/{id} — update (preserves id);
//   * DELETE /api/v3/releaseprofile/{id} — remove.
// `preferred[]` rides as Sonarr's `{ key: term, value: score }`; `required` and
// `ignored` are plain term arrays; tags scope the profile (empty = everywhere).

import * as React from 'react';

import Card from '@components/Card';
import Input from '@components/Input';
import Checkbox from '@components/Checkbox';
import Button from '@components/Button';
import ButtonGroup from '@components/ButtonGroup';
import Badge from '@components/Badge';
import Divider from '@components/Divider';
import Text from '@components/Text';

import { ApiError, CellarrClient, api as defaultApi } from '@lib/api/client';
import type {
  ReleaseProfile,
  ReleaseProfileBody,
  ReleaseProfilePreferred,
  Tag,
} from '@lib/api/types';

import { useToast } from '@app/_lib/ToastProvider';
import { useAsync, toApiError } from '@app/settings/_components/useAsync';
import { Loading, ErrorBanner, EmptyState } from '@app/settings/_components/StatusBanners';
import ConfirmDialog from '@app/settings/_components/ConfirmDialog';
import ManagedBadge, { isManaged } from '@app/settings/_components/ManagedBadge';
import TagInput from '@app/settings/_components/TagInput';

// A single editable preferred row: a match term and the score it adds. Score is
// held as a string while the number input is edited.
interface PreferredRow {
  term: string;
  score: string;
}

// The open editor's view-model. `id === undefined` marks a not-yet-persisted
// (new) profile.
interface RpForm {
  id?: number;
  name: string;
  enabled: boolean;
  required: string[];
  ignored: string[];
  preferred: PreferredRow[];
  tags: number[];
}

function blankForm(): RpForm {
  return {
    name: '',
    enabled: true,
    required: [''],
    ignored: [''],
    preferred: [{ term: '', score: '0' }],
    tags: [],
  };
}

// Always keep at least one editable row so the builder shows an input even when
// a profile arrives with an empty list.
function withFloor<T>(rows: T[], blank: T): T[] {
  return rows.length ? rows : [blank];
}

function formFromProfile(rp: ReleaseProfile): RpForm {
  const preferred: PreferredRow[] = Array.isArray(rp.preferred)
    ? rp.preferred.map((p) => ({ term: String(p.key ?? ''), score: String(p.value ?? 0) }))
    : [];
  const tags = Array.isArray(rp.tags)
    ? rp.tags.filter((t): t is number => typeof t === 'number')
    : [];
  return {
    id: typeof rp.id === 'number' ? rp.id : undefined,
    name: String(rp.name ?? ''),
    enabled: rp.enabled !== false,
    required: withFloor(Array.isArray(rp.required) ? rp.required.map(String) : [], ''),
    ignored: withFloor(Array.isArray(rp.ignored) ? rp.ignored.map(String) : [], ''),
    preferred: withFloor(preferred, { term: '', score: '0' }),
    tags,
  };
}

// A score string → a finite integer (default 0). Negative is allowed.
function toScore(s: string): number {
  const n = Number.parseInt(s, 10);
  return Number.isFinite(n) ? n : 0;
}

const ReleaseProfiles: React.FC<{ client?: CellarrClient }> = ({ client = defaultApi }) => {
  const { success, error: toastError } = useToast();

  const load = React.useCallback(
    (signal: AbortSignal) => client.getReleaseProfiles(signal),
    [client]
  );
  const loadTags = React.useCallback(
    (signal: AbortSignal) => client.listTags(signal),
    [client]
  );
  const { data, loading, error, reload } = useAsync<ReleaseProfile[]>(load);
  const { data: tagList, reload: reloadTags } = useAsync<Tag[]>(loadTags);

  const tags = tagList ?? [];

  // Mint a new tag inline (the TagInput "+ new" path) + refresh the catalogue.
  const createTag = React.useCallback(
    async (label: string): Promise<Tag> => {
      const tag = await client.createTag({ label });
      reloadTags();
      return tag;
    },
    [client, reloadTags]
  );

  const [form, setForm] = React.useState<RpForm | undefined>(undefined);
  const [saving, setSaving] = React.useState(false);
  const [saveError, setSaveError] = React.useState<ApiError | undefined>(undefined);
  const [pendingDelete, setPendingDelete] = React.useState<ReleaseProfile | null>(null);
  const [deleting, setDeleting] = React.useState(false);

  const profiles = data ?? [];

  if (loading) return <Loading label="Loading release profiles" />;
  if (error) return <ErrorBanner error={error} />;

  const openNew = () => {
    setForm(blankForm());
    setSaveError(undefined);
  };

  const openEdit = (rp: ReleaseProfile) => {
    setForm(formFromProfile(rp));
    setSaveError(undefined);
  };

  const closeForm = () => {
    setForm(undefined);
    setSaveError(undefined);
  };

  const update = (patch: Partial<RpForm>) => setForm((f) => (f ? { ...f, ...patch } : f));

  // --- Required / Ignored term list editing (string[] lists) ----------------

  const updateTerm = (kind: 'required' | 'ignored', index: number, value: string) => {
    setForm((f) =>
      f ? { ...f, [kind]: f[kind].map((t, i) => (i === index ? value : t)) } : f
    );
  };

  const addTerm = (kind: 'required' | 'ignored') => {
    setForm((f) => (f ? { ...f, [kind]: [...f[kind], ''] } : f));
  };

  const removeTerm = (kind: 'required' | 'ignored', index: number) => {
    setForm((f) => (f ? { ...f, [kind]: f[kind].filter((_, i) => i !== index) } : f));
  };

  // --- Preferred term + score rows ------------------------------------------

  const updatePreferred = (index: number, patch: Partial<PreferredRow>) => {
    setForm((f) =>
      f
        ? { ...f, preferred: f.preferred.map((r, i) => (i === index ? { ...r, ...patch } : r)) }
        : f
    );
  };

  const addPreferred = () => {
    setForm((f) => (f ? { ...f, preferred: [...f.preferred, { term: '', score: '0' }] } : f));
  };

  const removePreferred = (index: number) => {
    setForm((f) => (f ? { ...f, preferred: f.preferred.filter((_, i) => i !== index) } : f));
  };

  const save = async () => {
    if (!form) return;
    if (!form.name.trim()) {
      toastError('Give the release profile a name first.');
      return;
    }
    setSaving(true);
    setSaveError(undefined);
    try {
      const required = form.required.map((t) => t.trim()).filter((t) => t.length > 0);
      const ignored = form.ignored.map((t) => t.trim()).filter((t) => t.length > 0);
      const preferred: ReleaseProfilePreferred[] = form.preferred
        .map((r) => ({ key: r.term.trim(), value: toScore(r.score) }))
        .filter((p) => p.key.length > 0);
      const body: ReleaseProfileBody = {
        name: form.name.trim(),
        enabled: form.enabled,
        required,
        ignored,
        preferred,
        tags: form.tags,
      };
      const editingId = form.id;
      if (typeof editingId === 'number') {
        await client.updateReleaseProfile(editingId, body);
      } else {
        await client.createReleaseProfile(body);
      }
      success(typeof editingId === 'number' ? 'Release profile saved.' : 'Release profile created.');
      closeForm();
      reload();
    } catch (err) {
      const e = toApiError(err);
      setSaveError(e);
      toastError(`Could not save release profile — ${e.message}`);
    } finally {
      setSaving(false);
    }
  };

  const confirmDelete = async () => {
    if (!pendingDelete) return;
    setDeleting(true);
    try {
      await client.deleteReleaseProfile(pendingDelete.id);
      success('Release profile removed.');
      if (form && form.id === pendingDelete.id) closeForm();
      setPendingDelete(null);
      reload();
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not remove release profile — ${e.message}`);
    } finally {
      setDeleting(false);
    }
  };

  // Render a Required / Ignored repeatable string-list editor.
  const renderTermList = (kind: 'required' | 'ignored', heading: string, hint: string) => {
    if (!form) return null;
    const rows = form[kind];
    return (
      <div style={{ margin: '1ch 0' }}>
        <Text style={{ opacity: 0.6 }}>{heading}</Text>
        <Text style={{ opacity: 0.4, fontStyle: 'italic' }}>{hint}</Text>
        {rows.map((term, index) => (
          <div
            key={index}
            style={{ display: 'flex', gap: '0.5ch', alignItems: 'stretch', margin: '0.5ch 0' }}
          >
            <div style={{ flex: 1 }}>
              <Input
                name={`${kind}-${index}`}
                aria-label={`${heading} ${index + 1}`}
                placeholder="term or /regex/"
                value={term}
                onChange={(e) => updateTerm(kind, index, e.target.value)}
              />
            </div>
            <Button
              theme="SECONDARY"
              aria-label={`Remove ${heading.toLowerCase()} ${index + 1}`}
              isDisabled={rows.length <= 1}
              onClick={() => removeTerm(kind, index)}
            >
              ✗
            </Button>
          </div>
        ))}
        <Button
          theme="SECONDARY"
          aria-label={`Add ${heading.toLowerCase()}`}
          onClick={() => addTerm(kind)}
        >
          + Add term
        </Button>
      </div>
    );
  };

  return (
    <Card title="Release Profiles">
      <Text style={{ opacity: 0.6, marginBottom: '1ch' }}>
        Steer grabs with release-title terms: require terms a release must carry, ignore terms that
        reject it, and prefer terms that nudge its score up or down.
      </Text>

      {profiles.length ? (
        <ul style={{ listStyle: 'none', padding: 0, margin: '0 0 1ch 0' }}>
          {profiles.map((rp) => {
            const managed = isManaged(rp);
            const rpName = rp.name || `release profile ${rp.id}`;
            return (
              <li
                key={rp.id}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  gap: '1ch',
                  padding: '0.5ch 0',
                }}
              >
                <span>
                  <strong>{rp.name || `Profile #${rp.id}`}</strong>{' '}
                  <Badge>{rp.enabled === false ? 'disabled' : 'enabled'}</Badge>{' '}
                  <Badge>
                    {(rp.required?.length ?? 0)}R · {(rp.ignored?.length ?? 0)}I ·{' '}
                    {(rp.preferred?.length ?? 0)}P
                  </Badge>{' '}
                  {managed ? <ManagedBadge entityLabel={`Release profile ${rpName}`} /> : null}
                </span>
                <span style={{ display: 'inline-flex', gap: '0.5ch' }}>
                  <Button
                    theme="SECONDARY"
                    aria-label={`Edit ${rpName}`}
                    isDisabled={managed}
                    onClick={managed ? undefined : () => openEdit(rp)}
                  >
                    Edit
                  </Button>
                  <Button
                    theme="DANGER"
                    aria-label={`Delete ${rpName}`}
                    isDisabled={managed}
                    onClick={managed ? undefined : () => setPendingDelete(rp)}
                  >
                    Delete
                  </Button>
                </span>
              </li>
            );
          })}
        </ul>
      ) : (
        <EmptyState>No release profiles yet. Add one to steer grabs by release term.</EmptyState>
      )}

      <div style={{ marginBottom: '1ch' }}>
        <ButtonGroup items={[{ body: 'Add release profile', onClick: openNew }]} />
      </div>

      {form ? (
        <>
          <Divider type="GRADIENT" />
          <Text style={{ opacity: 0.6, margin: '1ch 0 0.5ch' }}>
            {typeof form.id === 'number' ? `Editing ${form.name || 'profile'}` : 'New release profile'}
          </Text>

          <div style={{ margin: '0.5ch 0' }}>
            <Text style={{ opacity: 0.6 }}>Name</Text>
            <Input
              name="rp-name"
              aria-label="Release profile name"
              value={form.name}
              onChange={(e) => update({ name: e.target.value })}
            />
          </div>

          <div style={{ margin: '0.5ch 0' }}>
            <Checkbox
              key={`${form.id ?? 'new'}-enabled`}
              name="rp-enabled"
              aria-label="Release profile enabled"
              defaultChecked={form.enabled}
              onChange={(e) => update({ enabled: e.target.checked })}
            >
              Enabled
            </Checkbox>
          </div>

          <div style={{ margin: '0.5ch 0' }}>
            <Text style={{ opacity: 0.6 }}>Tags</Text>
            <TagInput
              label="Tags for this release profile"
              available={tags}
              value={form.tags}
              onChange={(next) => update({ tags: next })}
              onCreate={createTag}
              disabled={saving}
            />
          </div>

          {renderTermList(
            'required',
            'Required terms',
            'A release must contain every required term, or it is rejected.'
          )}

          {renderTermList(
            'ignored',
            'Ignored terms',
            'A release containing any ignored term is rejected.'
          )}

          <div style={{ margin: '1ch 0' }}>
            <Text style={{ opacity: 0.6 }}>Preferred terms</Text>
            <Text style={{ opacity: 0.4, fontStyle: 'italic' }}>
              Each matching term adds its score (negative scores penalize).
            </Text>
            {form.preferred.map((row, index) => (
              <div
                key={index}
                style={{ display: 'flex', gap: '0.5ch', alignItems: 'flex-end', margin: '0.5ch 0' }}
              >
                <div style={{ flex: 2 }}>
                  <Text style={{ opacity: 0.6 }}>Term</Text>
                  <Input
                    name={`preferred-${index}-term`}
                    aria-label={`Preferred term ${index + 1}`}
                    placeholder="term or /regex/"
                    value={row.term}
                    onChange={(e) => updatePreferred(index, { term: e.target.value })}
                  />
                </div>
                <div style={{ flex: 1 }}>
                  <Text style={{ opacity: 0.6 }}>Score</Text>
                  <Input
                    name={`preferred-${index}-score`}
                    aria-label={`Preferred term ${index + 1} score`}
                    type="number"
                    value={row.score}
                    onChange={(e) => updatePreferred(index, { score: e.target.value })}
                  />
                </div>
                <Button
                  theme="SECONDARY"
                  aria-label={`Remove preferred term ${index + 1}`}
                  isDisabled={form.preferred.length <= 1}
                  onClick={() => removePreferred(index)}
                >
                  ✗
                </Button>
              </div>
            ))}
            <Button theme="SECONDARY" aria-label="Add preferred term" onClick={addPreferred}>
              + Add preferred term
            </Button>
          </div>

          {saveError ? <ErrorBanner error={saveError} /> : null}

          <div style={{ marginTop: '1ch' }}>
            <ButtonGroup
              items={[
                {
                  body: saving
                    ? 'Saving…'
                    : typeof form.id === 'number'
                      ? 'Save profile'
                      : 'Create profile',
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
          title="Delete release profile"
          confirmLabel="Delete release profile"
          pendingLabel="Deleting…"
          pending={deleting}
          onConfirm={confirmDelete}
          onCancel={() => (deleting ? undefined : setPendingDelete(null))}
        >
          <Text>
            Delete <strong>{pendingDelete.name || `release profile #${pendingDelete.id}`}</strong>?
            Grabs will no longer be steered by its required / ignored / preferred terms.
          </Text>
        </ConfirmDialog>
      ) : null}
    </Card>
  );
};

export default ReleaseProfiles;
