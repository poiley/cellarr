'use client';

// Settings — Quality Profiles (full CRUD). Composed only from SRCL primitives:
//   Select (pick a profile), Input (rename), Checkbox (upgrades allowed +
//   per-quality allow), a controlled RadioButton group (cutoff quality), a clean
//   number Input (minimum custom-format score), a ListItem list whose order is
//   edited with SRCL Buttons (move up/down), a ButtonGroup with New / Save and a
//   danger-tinted Delete that opens a SRCL ConfirmDialog. Action results surface
//   via the shared toast.
// Reads GET /api/v3/qualityprofile (where the seeded profiles actually live —
// the native /api/v1/qualityprofiles route returns []); creates via POST,
// edits via PUT (updateQualityProfile), and removes via DELETE
// (deleteQualityProfile). New-profile defaults are seeded from the quality
// ladder reported by GET /api/v3/qualitydefinition.

import * as React from 'react';

import Card from '@components/Card';
import Select from '@components/Select';
import Input from '@components/Input';
import Checkbox from '@components/Checkbox';
import RadioButton from '@components/RadioButton';
import Button from '@components/Button';
import ButtonGroup from '@components/ButtonGroup';
import Badge from '@components/Badge';
import Divider from '@components/Divider';
import Text from '@components/Text';
import ListItem from '@components/ListItem';

import { ApiError, CellarrClient, api as defaultApi } from '@lib/api/client';
import type { QualityProfile, QualityDefinition } from '@lib/api/types';

import {
  buildQualityNameMap,
  resolveQualityName,
  type QualityNameMap,
} from '@app/_lib/quality';

import { useToast } from '@app/_lib/ToastProvider';
import { useAsync, toApiError } from '@app/settings/_components/useAsync';
import { Loading, ErrorBanner, EmptyState } from '@app/settings/_components/StatusBanners';
import ConfirmDialog from '@app/settings/_components/ConfirmDialog';

interface QualityItem {
  id: string;
  name: string;
  allowed: boolean;
}

// View-model derived from the open QualityProfile shape. `id === ''` marks a
// not-yet-persisted (new) profile.
interface ProfileForm {
  id: string;
  name: string;
  upgradesAllowed: boolean;
  cutoff: string;
  minScore: number;
  qualities: QualityItem[];
}

// Flatten the v3 `items[]` ladder (each row carries a `quality` or is a group)
// into the simple {id,name,allowed} list the form edits. Names are resolved
// against the quality-definition map so the unhelpful "rank-N" placeholders the
// profile carries become human names ("WEBDL-1080p", "Bluray-1080p", …).
function asQualities(items: QualityProfile['items'], names: QualityNameMap): QualityItem[] {
  if (!Array.isArray(items)) return [];
  return items.map((item, i) => {
    const q = item.quality;
    const id = q ? String(q.id) : String(item.id ?? item.name ?? i);
    const fallback = q?.name ?? item.name;
    return { id, name: resolveQualityName(id, fallback, names), allowed: item.allowed !== false };
  });
}

function toForm(p: QualityProfile, names: QualityNameMap): ProfileForm {
  return {
    id: p.id,
    name: typeof p.name === 'string' ? p.name : p.id,
    upgradesAllowed: p.upgradeAllowed !== false,
    cutoff: p.cutoff !== undefined && p.cutoff !== null ? String(p.cutoff) : '',
    minScore: typeof p.minFormatScore === 'number' ? p.minFormatScore : 0,
    qualities: asQualities(p.items, names),
  };
}

// Build a blank form for a brand-new profile. When the quality definitions are
// available, seed the ladder from them (all allowed) so the user can immediately
// pick a cutoff; otherwise start empty.
function blankForm(defs: QualityDefinition[] | undefined, names: QualityNameMap): ProfileForm {
  const qualities: QualityItem[] = Array.isArray(defs)
    ? defs
        .filter((d) => d && d.quality)
        .map((d) => ({
          id: String(d.quality.id),
          name: resolveQualityName(String(d.quality.id), d.quality.name, names),
          allowed: true,
        }))
    : [];
  return {
    id: '',
    name: '',
    upgradesAllowed: true,
    cutoff: qualities.length ? qualities[qualities.length - 1].id : '',
    minScore: 0,
    qualities,
  };
}

const MAX_SCORE = 1000;
const NEW_OPTION = '+ New profile';

const QualityProfiles: React.FC<{ client?: CellarrClient }> = ({ client = defaultApi }) => {
  const { success, error: toastError } = useToast();

  const load = React.useCallback(
    (signal: AbortSignal) => client.getQualityProfiles(signal),
    [client]
  );
  const { data, loading, error, reload } = useAsync<QualityProfile[]>(load);

  // Quality definitions seed the ladder for new profiles. Best-effort: a failure
  // here just means an empty starting ladder, so it never blocks the screen.
  const loadDefs = React.useCallback(
    (signal: AbortSignal) => client.getQualityDefinitions(signal),
    [client]
  );
  const { data: defsData } = useAsync<QualityDefinition[]>(loadDefs);

  // id -> human name lookup from the quality definitions. The profile payload's
  // own `quality.name` is an unreliable "rank-N" placeholder, so the qualities
  // list and cutoff selector resolve names through this map.
  const nameMap = React.useMemo(() => buildQualityNameMap(defsData), [defsData]);

  const [selectedId, setSelectedId] = React.useState<string>('');
  const [creating, setCreating] = React.useState(false);
  const [form, setForm] = React.useState<ProfileForm | undefined>(undefined);
  const [saving, setSaving] = React.useState(false);
  const [saveError, setSaveError] = React.useState<ApiError | undefined>(undefined);
  const [deleting, setDeleting] = React.useState(false);
  const [confirmDelete, setConfirmDelete] = React.useState(false);

  const profiles = data ?? [];

  // When data arrives, default the selection to the first profile (unless the
  // user is mid-create).
  React.useEffect(() => {
    if (!creating && profiles.length && !selectedId) {
      setSelectedId(profiles[0].id);
    }
  }, [profiles, selectedId, creating]);

  // Load the selected profile (or a blank one when creating) into the form.
  React.useEffect(() => {
    if (creating) {
      setForm(blankForm(defsData, nameMap));
      setSaveError(undefined);
      setConfirmDelete(false);
      return;
    }
    const found = profiles.find((p) => p.id === selectedId);
    setForm(found ? toForm(found, nameMap) : undefined);
    setSaveError(undefined);
    setConfirmDelete(false);
  }, [selectedId, data, creating, defsData, nameMap]);

  if (loading) return <Loading label="Loading quality profiles" />;
  if (error) return <ErrorBanner error={error} />;

  const update = (patch: Partial<ProfileForm>) => {
    setForm((f) => (f ? { ...f, ...patch } : f));
  };

  const startNew = () => {
    setCreating(true);
    setSelectedId('');
    setSaveError(undefined);
    setConfirmDelete(false);
  };

  const cancelNew = () => {
    setCreating(false);
    setSelectedId(profiles.length ? profiles[0].id : '');
    setSaveError(undefined);
  };

  const move = (index: number, dir: -1 | 1) => {
    setForm((f) => {
      if (!f) return f;
      const next = [...f.qualities];
      const target = index + dir;
      if (target < 0 || target >= next.length) return f;
      [next[index], next[target]] = [next[target], next[index]];
      return { ...f, qualities: next };
    });
  };

  const toggleAllowed = (id: string, allowed: boolean) => {
    setForm((f) =>
      f
        ? {
            ...f,
            qualities: f.qualities.map((q) => (q.id === id ? { ...q, allowed } : q)),
          }
        : f
    );
  };

  const save = async () => {
    if (!form) return;
    setSaving(true);
    setSaveError(undefined);
    try {
      const original = form.id ? profiles.find((p) => p.id === form.id) : undefined;
      const cutoffNum = Number.parseInt(form.cutoff, 10);
      const body: Partial<QualityProfile> = {
        ...original,
        name: form.name,
        upgradeAllowed: form.upgradesAllowed,
        cutoff: Number.isNaN(cutoffNum) ? (original?.cutoff ?? 0) : cutoffNum,
        minFormatScore: form.minScore,
        items: form.qualities.map((q) => ({
          quality: { id: Number.parseInt(q.id, 10) || 0, name: q.name },
          allowed: q.allowed,
          items: [],
        })),
      };
      const wasCreating = !form.id;
      if (form.id) {
        body.id = form.id;
        await client.updateQualityProfile(form.id, body);
      } else {
        delete body.id;
        const created = await client.createQualityProfile(body);
        setCreating(false);
        if (created && typeof created.id === 'string') setSelectedId(created.id);
      }
      success(wasCreating ? 'Profile created.' : 'Profile saved.');
      reload();
    } catch (err) {
      const e = toApiError(err);
      setSaveError(e);
      toastError(`Could not save profile — ${e.message}`);
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    if (!form || !form.id) return;
    setDeleting(true);
    setSaveError(undefined);
    try {
      await client.deleteQualityProfile(form.id);
      success('Profile deleted.');
      setConfirmDelete(false);
      setSelectedId('');
      reload();
    } catch (err) {
      const e = toApiError(err);
      setSaveError(e);
      toastError(`Could not delete profile — ${e.message}`);
    } finally {
      setDeleting(false);
    }
  };

  const profileName = (p: QualityProfile) => (typeof p.name === 'string' ? p.name : p.id);

  // Empty + not creating: offer a way to create the first profile.
  if (!profiles.length && !creating) {
    return (
      <Card title="Quality Profiles">
        <EmptyState>No quality profiles yet. Create one to get started.</EmptyState>
        <div style={{ marginTop: '1ch' }}>
          <ButtonGroup items={[{ body: 'New profile', onClick: startNew }]} />
        </div>
      </Card>
    );
  }

  return (
    <Card title="Quality Profiles">
      {!creating ? (
        <div style={{ marginBottom: '1ch' }}>
          <Text style={{ opacity: 0.6 }}>Profile</Text>
          <div style={{ display: 'flex', gap: '1ch', alignItems: 'flex-end' }}>
            <div style={{ flex: 1 }}>
              <Select
                name="quality-profile"
                aria-label="Quality profile"
                options={[...profiles.map(profileName), NEW_OPTION]}
                defaultValue={form ? form.name : ''}
                onChange={(value) => {
                  if (value === NEW_OPTION) {
                    startNew();
                    return;
                  }
                  const match = profiles.find((p) => profileName(p) === value);
                  if (match) setSelectedId(match.id);
                }}
              />
            </div>
            <Button theme="SECONDARY" onClick={startNew}>
              New profile
            </Button>
          </div>
        </div>
      ) : (
        <div style={{ marginBottom: '1ch' }}>
          <Badge>new profile</Badge>
        </div>
      )}

      {form ? (
        <>
          <Divider type="GRADIENT" />

          <div style={{ margin: '1ch 0' }}>
            <Text style={{ opacity: 0.6 }}>Name</Text>
            <Input
              name="profile-name"
              aria-label="Profile name"
              placeholder="My profile"
              value={form.name}
              onChange={(e) => update({ name: e.target.value })}
            />
          </div>

          <div style={{ margin: '1ch 0' }}>
            <Checkbox
              key={`${form.id || 'new'}-upgrades`}
              name="upgrades-allowed"
              aria-label="Allow upgrades to a higher quality"
              defaultChecked={form.upgradesAllowed}
              onChange={(e) => update({ upgradesAllowed: e.target.checked })}
            >
              Allow upgrades to a higher quality
            </Checkbox>
          </div>

          <Divider type="GRADIENT" />

          <div style={{ margin: '1ch 0' }}>
            <Text style={{ opacity: 0.6 }}>
              Qualities — tick to allow, use ▲ / ▼ to set priority (highest first)
            </Text>
            <ul style={{ listStyle: 'none', padding: 0, margin: 0 }}>
              {form.qualities.length ? (
                form.qualities.map((q, i) => (
                  <ListItem key={q.id}>
                    <span
                      style={{
                        display: 'flex',
                        alignItems: 'center',
                        justifyContent: 'space-between',
                        gap: '1ch',
                      }}
                    >
                      {/* The checkbox is the allow toggle and is explicitly
                          labelled "Allow" so it never reads as a "remove" box. */}
                      <Checkbox
                        key={`${form.id || 'new'}-quality-${q.id}`}
                        name={`quality-${q.id}`}
                        aria-label={`Allow ${q.name}`}
                        defaultChecked={q.allowed}
                        onChange={(e) => toggleAllowed(q.id, e.target.checked)}
                      >
                        Allow <strong>{q.name}</strong>
                      </Checkbox>
                      <span style={{ display: 'inline-flex', gap: '0.5ch' }}>
                        <Button
                          theme="SECONDARY"
                          aria-label={`Move ${q.name} up`}
                          isDisabled={i === 0}
                          onClick={() => move(i, -1)}
                        >
                          ▲
                        </Button>
                        <Button
                          theme="SECONDARY"
                          aria-label={`Move ${q.name} down`}
                          isDisabled={i === form.qualities.length - 1}
                          onClick={() => move(i, 1)}
                        >
                          ▼
                        </Button>
                      </span>
                    </span>
                  </ListItem>
                ))
              ) : (
                <li>
                  <EmptyState>This profile has no qualities configured.</EmptyState>
                </li>
              )}
            </ul>
          </div>

          <Divider type="GRADIENT" />

          <div style={{ margin: '1ch 0' }}>
            <Text style={{ opacity: 0.6 }}>Cutoff quality</Text>
            <div role="radiogroup" aria-label="Cutoff quality">
              {form.qualities
                .filter((q) => q.allowed)
                .map((q) => (
                  <RadioButton
                    key={q.id}
                    name="cutoff-quality"
                    value={q.id}
                    aria-label={q.name}
                    selected={form.cutoff === q.id}
                    onSelect={(value) => update({ cutoff: value })}
                  >
                    {q.name}
                  </RadioButton>
                ))}
            </div>
          </div>

          <Divider type="GRADIENT" />

          <div style={{ margin: '1ch 0' }}>
            <Text style={{ opacity: 0.6 }}>Minimum custom-format score</Text>
            {/* A single clean SRCL number input (0..MAX_SCORE). The previous
                slider rendered a zero-padded "0000" display stacked over this
                field, which read as a bug. */}
            <Input
              name="min-format-score"
              aria-label="Minimum custom-format score"
              type="number"
              min={0}
              max={MAX_SCORE}
              step={5}
              value={String(form.minScore)}
              onChange={(e) => {
                const raw = e.target.value;
                if (raw === '') {
                  update({ minScore: 0 });
                  return;
                }
                const n = Number.parseInt(raw, 10);
                update({ minScore: Number.isNaN(n) ? 0 : Math.min(MAX_SCORE, Math.max(0, n)) });
              }}
            />
          </div>

          <Divider type="GRADIENT" />

          {saveError ? <ErrorBanner error={saveError} /> : null}

          <div
            style={{
              marginTop: '1ch',
              display: 'flex',
              justifyContent: 'space-between',
              alignItems: 'center',
              gap: '1ch',
              flexWrap: 'wrap',
            }}
          >
            {/* Primary Save is the inverse full-width affordance shared across
                every settings tab; Cancel sits beside it as a SECONDARY. */}
            <div style={{ display: 'flex', gap: '1ch', alignItems: 'center', flexWrap: 'wrap' }}>
              <Button theme="PRIMARY" isDisabled={saving} onClick={saving ? undefined : save}>
                {saving ? 'Saving…' : creating ? 'Create profile' : 'Save profile'}
              </Button>
              {creating ? (
                <Button theme="SECONDARY" onClick={cancelNew}>
                  Cancel
                </Button>
              ) : null}
            </div>
            {/* Delete is the shared DANGER affordance (red outline, solid red on
                hover) — distinct from a benign action yet subordinate to Save. It
                opens a confirm dialog rather than mutating inline. */}
            {!creating && form.id ? (
              <Button theme="DANGER" aria-label="Delete profile" onClick={() => setConfirmDelete(true)}>
                ✗ Delete profile
              </Button>
            ) : null}
          </div>

          {confirmDelete && form.id ? (
            <ConfirmDialog
              title="Delete profile"
              confirmLabel="Delete profile"
              pending={deleting}
              onConfirm={remove}
              onCancel={() => (deleting ? undefined : setConfirmDelete(false))}
            >
              <Text>
                Delete <strong>{form.name || 'this profile'}</strong>? Any content using it will need
                a new quality profile assigned.
              </Text>
            </ConfirmDialog>
          ) : null}
        </>
      ) : null}
    </Card>
  );
};

export default QualityProfiles;
