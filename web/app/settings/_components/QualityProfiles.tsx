'use client';

// Settings — Quality Profiles. Composed only from SRCL primitives:
//   Select (pick a profile), Input (rename), Checkbox (upgrades allowed),
//   a controlled RadioButton group (cutoff quality), NumberRangeSlider
//   (minimum custom-format score), and a ListItem list whose order is edited
//   with SRCL Buttons (move up/down) — "reorder via ListItems".
// Reads GET /api/v1/qualityprofiles; writes POST /api/v1/qualityprofiles/:id.

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
import type { QualityProfile } from '@lib/api/types';

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

// View-model derived from the open QualityProfile shape.
interface ProfileForm {
  id: string;
  name: string;
  upgradesAllowed: boolean;
  cutoff: string;
  minScore: number;
  qualities: QualityItem[];
}

function asQualities(raw: unknown): QualityItem[] {
  if (!Array.isArray(raw)) return [];
  return raw.map((q, i) => {
    const obj = (q ?? {}) as Record<string, unknown>;
    return {
      id: String(obj.id ?? obj.name ?? i),
      name: String(obj.name ?? obj.id ?? `Quality ${i + 1}`),
      allowed: obj.allowed !== false,
    };
  });
}

function toForm(p: QualityProfile): ProfileForm {
  const rec = p as Record<string, unknown>;
  return {
    id: p.id,
    name: typeof p.name === 'string' ? p.name : p.id,
    upgradesAllowed: rec.upgrades_allowed !== false,
    cutoff: typeof rec.cutoff === 'string' ? rec.cutoff : '',
    minScore: typeof rec.min_format_score === 'number' ? rec.min_format_score : 0,
    qualities: asQualities(rec.qualities ?? rec.items),
  };
}

const MAX_SCORE = 1000;

const QualityProfiles: React.FC<{ client?: CellarrClient }> = ({ client = defaultApi }) => {
  const load = React.useCallback(
    (signal: AbortSignal) => client.getQualityProfiles(undefined, signal),
    [client]
  );
  const { data, loading, error, reload } = useAsync<QualityProfile[]>(load);

  const [selectedId, setSelectedId] = React.useState<string>('');
  const [form, setForm] = React.useState<ProfileForm | undefined>(undefined);
  const [saving, setSaving] = React.useState(false);
  const [saved, setSaved] = React.useState(false);
  const [saveError, setSaveError] = React.useState<ApiError | undefined>(undefined);

  const profiles = data ?? [];

  // When data arrives, default the selection to the first profile.
  React.useEffect(() => {
    if (profiles.length && !selectedId) {
      setSelectedId(profiles[0].id);
    }
  }, [profiles, selectedId]);

  // Load the selected profile into an editable form.
  React.useEffect(() => {
    const found = profiles.find((p) => p.id === selectedId);
    setForm(found ? toForm(found) : undefined);
    setSaved(false);
    setSaveError(undefined);
  }, [selectedId, data]);

  if (loading) return <Loading label="Loading quality profiles" />;
  if (error) return <ErrorBanner error={error} />;
  if (!profiles.length) {
    return <EmptyState>No quality profiles yet. Create one to get started.</EmptyState>;
  }

  const update = (patch: Partial<ProfileForm>) => {
    setForm((f) => (f ? { ...f, ...patch } : f));
    setSaved(false);
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
      await client.request<QualityProfile>(`/qualityprofiles/${form.id}`, {
        method: 'POST',
        body: {
          name: form.name,
          upgrades_allowed: form.upgradesAllowed,
          cutoff: form.cutoff || undefined,
          min_format_score: form.minScore,
          qualities: form.qualities.map((q) => ({ id: q.id, name: q.name, allowed: q.allowed })),
        },
      });
      setSaved(true);
      reload();
    } catch (err) {
      setSaveError(toApiError(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Card title="Quality Profiles">
      <div style={{ marginBottom: '1ch' }}>
        <Text style={{ opacity: 0.6 }}>Profile</Text>
        <Select
          name="quality-profile"
          options={profiles.map((p) => (typeof p.name === 'string' ? p.name : p.id))}
          defaultValue={form ? form.name : ''}
          onChange={(value) => {
            const match = profiles.find(
              (p) => (typeof p.name === 'string' ? p.name : p.id) === value
            );
            if (match) setSelectedId(match.id);
          }}
        />
      </div>

      {form ? (
        <>
          <Divider type="GRADIENT" />

          <div style={{ margin: '1ch 0' }}>
            <Text style={{ opacity: 0.6 }}>Name</Text>
            <Input
              name="profile-name"
              aria-label="Profile name"
              value={form.name}
              onChange={(e) => update({ name: e.target.value })}
            />
          </div>

          <div style={{ margin: '1ch 0' }}>
            <Checkbox
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
              key={`${form.id}-${form.minScore}`}
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

          <div style={{ marginTop: '1ch' }}>
            <ButtonGroup
              items={[
                {
                  body: saving ? 'Saving…' : 'Save profile',
                  onClick: saving ? undefined : save,
                },
              ]}
            />
          </div>
        </>
      ) : null}
    </Card>
  );
};

export default QualityProfiles;
