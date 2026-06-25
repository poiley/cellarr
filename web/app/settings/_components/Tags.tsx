'use client';

// Settings — Tags. A manager for the label tags that scope content, indexers,
// download clients, and notifications. Lists every tag (with its id), creates a
// new one from a label (Enter or the Add button), and removes one behind a
// destructive confirm. Persistent + DB-backed via /api/v3/tag, so ids stay
// stable across restart. SRCL-only: Card, Input, Button, Badge, Divider, Text.

import * as React from 'react';

import Card from '@components/Card';
import Input from '@components/Input';
import Button from '@components/Button';
import Badge from '@components/Badge';
import Divider from '@components/Divider';
import Text from '@components/Text';

import { CellarrClient, api as defaultApi } from '@lib/api/client';
import type { Tag } from '@lib/api/types';

import { useToast } from '@app/_lib/ToastProvider';
import { useAsync, toApiError } from '@app/settings/_components/useAsync';
import { Loading, ErrorBanner, EmptyState } from '@app/settings/_components/StatusBanners';
import ConfirmDialog from '@app/settings/_components/ConfirmDialog';

export interface TagsProps {
  client?: CellarrClient;
}

const Tags: React.FC<TagsProps> = ({ client = defaultApi }) => {
  const loadTags = React.useCallback((signal: AbortSignal) => client.listTags(signal), [client]);
  const { data, loading, error, reload } = useAsync<Tag[]>(loadTags);
  const { success, error: toastError } = useToast();

  const [label, setLabel] = React.useState('');
  const [creating, setCreating] = React.useState(false);
  const [pendingDelete, setPendingDelete] = React.useState<Tag | null>(null);
  const [deleting, setDeleting] = React.useState(false);

  const tags = React.useMemo(
    () => (data ?? []).slice().sort((a, b) => a.label.localeCompare(b.label)),
    [data]
  );

  const create = async () => {
    const text = label.trim();
    if (!text) {
      toastError('Type a label first.');
      return;
    }
    setCreating(true);
    try {
      const tag = await client.createTag({ label: text });
      success(`Tag #${tag.label} saved.`);
      setLabel('');
      reload();
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not add tag — ${e.message}`);
    } finally {
      setCreating(false);
    }
  };

  const confirmDelete = async () => {
    if (!pendingDelete) return;
    setDeleting(true);
    try {
      await client.deleteTag(pendingDelete.id);
      success(`Tag #${pendingDelete.label} removed.`);
      setPendingDelete(null);
      reload();
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not remove — ${e.message}`);
    } finally {
      setDeleting(false);
    }
  };

  return (
    <Card title="Tags">
      <Text style={{ opacity: 0.6, margin: '0 0 1ch' }}>
        Tags scope delay profiles, indexers, download clients, and notifications. A config with
        no tags applies everywhere; add a tag to a config and to content to route them together.
      </Text>

      {loading ? (
        <Loading label="Loading tags" />
      ) : error ? (
        <ErrorBanner error={error} />
      ) : (
        <>
          {tags.length ? (
            <ul style={{ listStyle: 'none', padding: 0, margin: '0 0 1ch 0' }}>
              {tags.map((tag) => (
                <li
                  key={tag.id}
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'space-between',
                    gap: '1ch',
                    padding: '0.5ch 0',
                  }}
                >
                  <span>
                    <Badge>#{tag.label}</Badge>{' '}
                    <span style={{ opacity: 0.4 }}>id {tag.id}</span>
                  </span>
                  <Button
                    theme="DANGER"
                    aria-label={`Remove tag ${tag.label}`}
                    onClick={() => setPendingDelete(tag)}
                  >
                    Remove
                  </Button>
                </li>
              ))}
            </ul>
          ) : (
            <EmptyState>No tags yet. Add one below.</EmptyState>
          )}

          <Divider type="GRADIENT" />

          <Text style={{ opacity: 0.6, margin: '1ch 0 0.5ch' }}>New tag</Text>
          <div style={{ display: 'flex', gap: '0.5ch', alignItems: 'stretch' }}>
            <div style={{ flex: 1 }}>
              <Input
                name="tag-label"
                aria-label="Tag label"
                placeholder="e.g. hd, kids, archive"
                value={label}
                disabled={creating}
                onChange={(e) => setLabel(e.target.value)}
                onKeyDown={(e: React.KeyboardEvent) => {
                  if (e.key === 'Enter') {
                    e.preventDefault();
                    void create();
                  }
                }}
              />
            </div>
            <Button
              theme="PRIMARY"
              aria-label="Add tag"
              isDisabled={creating || !label.trim()}
              onClick={creating || !label.trim() ? undefined : create}
            >
              {creating ? 'Adding…' : '+ add'}
            </Button>
          </div>

          {pendingDelete ? (
            <ConfirmDialog
              title="Remove tag"
              confirmLabel="Remove tag"
              pendingLabel="Removing…"
              pending={deleting}
              onConfirm={confirmDelete}
              onCancel={() => (deleting ? undefined : setPendingDelete(null))}
            >
              <Text>
                Remove <strong>#{pendingDelete.label}</strong>? Any content or config still
                referencing it will fall back to applying everywhere.
              </Text>
            </ConfirmDialog>
          ) : null}
        </>
      )}
    </Card>
  );
};

export default Tags;
