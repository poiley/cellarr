'use client';

// Settings — Quality Profiles (full CRUD). Composed only from SRCL primitives:
//   Select (pick a profile), Input (rename), Checkbox (upgrades allowed +
//   per-quality allow), a controlled RadioButton group (cutoff quality),
//   NumberRangeSlider (minimum custom-format score), a ListItem list whose
//   order is edited with SRCL Buttons (move up/down), and a ButtonGroup with
//   New / Save / Delete actions.
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
import NumberRangeSlider from '@components/NumberRangeSlider';
import Button from '@components/Button';
import ButtonGroup from '@components/ButtonGroup';
import Badge from '@components/Badge';
import Divider from '@components/Divider';
import Text from '@components/Text';
import ListItem from '@components/ListItem';

import { ApiError, CellarrClient, api as defaultApi } from '@lib/api/client';
import type { QualityProfile, QualityDefinition } from '@lib/api/types';

import { useAsync, toApiError } from '@app/settings/_components/useAsync';
import {
  Loading,
  ErrorBanner,
  SuccessBanner,
  EmptyState,
} from '@app/settings/_components/StatusBanners';

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
// into the simple {id,name,allowed} list the form edits.
function asQualities(items: QualityProfile['items']): QualityItem[] {
  if (!Array.isArray(items)) return [];
  return items.map((item, i) => {
    const q = item.quality;
    const id = q ? String(q.id) : String(item.id ?? item.name ?? i);
    const name = q?.name ?? item.name ?? `Quality ${i + 1}`;
    return { id, name, allowed: item.allowed !== false };
  });
}

function toForm(p: QualityProfile): ProfileForm {
  return {
    id: p.id,
    name: typeof p.name === 'string' ? p.name : p.id,
    upgradesAllowed: p.upgradeAllowed !== false,
    cutoff: p.cutoff !== undefined && p.cutoff !== null ? String(p.cutoff) : '',
    minScore: typeof p.minFormatScore === 'number' ? p.minFormatScore : 0,
    qualities: asQualities(p.items),
  };
}

// Build a blank form for a brand-new profile. When the quality definitions are
// available, seed the ladder from them (all allowed) so the user can immediately
// pick a cutoff; otherwise start empty.
function blankForm(defs: QualityDefinition[] | undefined): ProfileForm {
  const qualities: QualityItem[] = Array.isArray(defs)
    ? defs
        .filter((d) => d && d.quality)
        .map((d) => ({ id: String(d.quality.id), name: d.quality.name, allowed: true }))
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

  const [selectedId, setSelectedId] = React.useState<string>('');
  const [creating, setCreating] = React.useState(false);
  const [form, setForm] = React.useState<ProfileForm | undefined>(undefined);
  const [saving, setSaving] = React.useState(false);
  const [saved, setSaved] = React.useState(false);
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
      setForm(blankForm(defsData));
      setSaved(false);
      setSaveError(undefined);
      setConfirmDelete(false);
      return;
    }
    const found = profiles.find((p) => p.id === selectedId);
    setForm(found ? toForm(found) : undefined);
    setSaved(false);
    setSaveError(undefined);
    setConfirmDelete(false);
  }, [selectedId, data, creating, defsData]);

  if (loading) return <Loading label="Loading quality profiles" />;
  if (error) return <ErrorBanner error={error} />;

  const update = (patch: Partial<ProfileForm>) => {
    setForm((f) => (f ? { ...f, ...patch } : f));
    setSaved(false);
  };

  const startNew = () => {
    setCreating(true);
    setSelectedId('');
    setSaved(false);
    setSaveError(undefined);
    setConfirmDelete(false);
  };

  const cancelNew = () => {
    setCreating(false);
    setSelectedId(profiles.length ? profiles[0].id : '');
    setSaved(false);
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
    setSaved(false);
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
    setSaved(false);
  };

  const save = async () => {
    if (!form) return;
    setSaving(true);
    setSaved(false);
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
      if (form.id) {
        body.id = form.id;
        await client.updateQualityProfile(form.id, body);
      } else {
        delete body.id;
        const created = await client.createQualityProfile(body);
        setCreating(false);
        if (created && typeof created.id === 'string') setSelectedId(created.id);
      }
      setSaved(true);
      reload();
    } catch (err) {
      setSaveError(toApiError(err));
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    if (!form || !form.id) return;
    if (!confirmDelete) {
      setConfirmDelete(true);
      return;
    }
    setDeleting(true);
    setSaveError(undefined);
    try {
      await client.deleteQualityProfile(form.id);
      setConfirmDelete(false);
      setSelectedId('');
      reload();
    } catch (err) {
      setSaveError(toApiError(err));
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
              defaultChecked={form.upgradesAllowed}
              onChange={(e) => update({ upgradesAllowed: e.target.checked })}
            >
              Allow upgrades to a higher quality
            </Checkbox>
          </div>

          <Divider type="GRADIENT" />

          <div style={{ margin: '1ch 0' }}>
            <Text style={{ opacity: 0.6 }}>Qualities (drag-order via move controls)</Text>
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
                      <Checkbox
                        key={`${form.id || 'new'}-quality-${q.id}`}
                        name={`quality-${q.id}`}
                        defaultChecked={q.allowed}
                        onChange={(e) => toggleAllowed(q.id, e.target.checked)}
                      >
                        {q.name}
                      </Checkbox>
                      <span style={{ display: 'inline-flex', gap: '0.5ch' }}>
                        <Button
                          theme="SECONDARY"
                          aria-label={`Move ${q.name} up`}
                          isDisabled={i === 0}
                          onClick={() => move(i, -1)}
                        >
                          ↑
                        </Button>
                        <Button
                          theme="SECONDARY"
                          aria-label={`Move ${q.name} down`}
                          isDisabled={i === form.qualities.length - 1}
                          onClick={() => move(i, 1)}
                        >
                          ↓
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
            <Text style={{ opacity: 0.6 }}>
              Minimum custom-format score <Badge>{form.minScore}</Badge>
            </Text>
            <NumberRangeSlider
              key={`${form.id || 'new'}-${form.minScore}`}
              defaultValue={form.minScore}
              min={0}
              max={MAX_SCORE}
              step={5}
            />
            <Input
              name="min-format-score"
              aria-label="Minimum custom-format score"
              type="number"
              value={String(form.minScore)}
              onChange={(e) => {
                const n = Number.parseInt(e.target.value, 10);
                update({ minScore: Number.isNaN(n) ? 0 : Math.min(MAX_SCORE, Math.max(0, n)) });
              }}
            />
          </div>

          <Divider type="GRADIENT" />

          {saveError ? <ErrorBanner error={saveError} /> : null}
          {saved ? <SuccessBanner>Profile saved.</SuccessBanner> : null}
          {confirmDelete ? (
            <SuccessBanner>
              Delete this profile? Click Delete again to confirm.
            </SuccessBanner>
          ) : null}

          <div style={{ marginTop: '1ch' }}>
            <ButtonGroup
              items={[
                {
                  body: saving ? 'Saving…' : creating ? 'Create profile' : 'Save profile',
                  onClick: saving ? undefined : save,
                },
                ...(creating
                  ? [{ body: 'Cancel', onClick: cancelNew }]
                  : [
                      {
                        body: deleting
                          ? 'Deleting…'
                          : confirmDelete
                            ? 'Confirm delete'
                            : 'Delete profile',
                        onClick: deleting ? undefined : remove,
                      },
                    ]),
              ]}
            />
          </div>
        </>
      ) : null}
    </Card>
  );
};

export default QualityProfiles;
