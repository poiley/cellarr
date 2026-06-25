'use client';

// Activity — per-download-row queue management actions, composed exclusively from
// vendored SRCL primitives (Button, Card, Input, Checkbox, Text, Badge). Each
// in-flight download row gets three actions wired to the v3 queue surface:
//
//   * Remove — a confirm dialog with two options: "remove from client" (also
//     deletes the download + its data) and "blocklist" (so a re-search never
//     re-grabs it). DELETE /api/v3/queue/{id}?removeFromClient=&blocklist=.
//   * Manual import — for a completed-but-unmatched download: a small picker to
//     choose the library item it should satisfy plus the on-disk path, committed
//     via POST /api/v3/queue/grab (the same crash-safe manual-import commit).
//   * Change category — retag the queued download (PUT /api/v3/queue/{id}).
//
// All three give toast feedback and call onChanged() so the queue re-snapshots.

import * as React from 'react';

import Badge from '@components/Badge';
import Button from '@components/Button';
import Card from '@components/Card';
import Checkbox from '@components/Checkbox';
import Input from '@components/Input';
import Text from '@components/Text';

import { CellarrClient, api as defaultApi } from '@lib/api/client';
import type { Movie, QueueRecord, Series } from '@lib/api/types';

import { useToast } from '@app/_lib/ToastProvider';

// A bordered SRCL Card floated over a scrim — the shared modal shell the queue
// dialogs reuse (mirrors the settings ConfirmDialog pattern). Kept local so the
// activity screen does not reach into the settings tree.
const Modal: React.FC<{
  title: string;
  glyph?: string;
  onClose: () => void;
  children: React.ReactNode;
}> = ({ title, glyph = '', onClose, children }) => {
  React.useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);
  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        background: 'var(--theme-overlay)',
        zIndex: 60,
        padding: '2ch',
      }}
    >
      <div role="dialog" aria-modal="true" aria-label={title} style={{ maxWidth: '64ch', width: '100%' }}>
        <Card title={`${glyph}${title}`} mode="left">
          {children}
        </Card>
      </div>
    </div>
  );
};


export interface QueueActionsProps {
  record: QueueRecord;
  /** Called after any action mutates the queue, to trigger a re-snapshot. */
  onChanged: () => void;
  client?: CellarrClient;
}

type OpenDialog = 'none' | 'remove' | 'grab' | 'category';

const QueueActions: React.FC<QueueActionsProps> = ({ record, onChanged, client = defaultApi }) => {
  const { success, error: toastError } = useToast();
  const [dialog, setDialog] = React.useState<OpenDialog>('none');
  const [busy, setBusy] = React.useState(false);

  // Remove dialog options.
  const [removeFromClient, setRemoveFromClient] = React.useState(false);
  const [blocklist, setBlocklist] = React.useState(false);

  // Change-category dialog.
  const [category, setCategory] = React.useState(record.category ?? '');

  // Manual-import picker.
  const [importPath, setImportPath] = React.useState('');
  const [contentId, setContentId] = React.useState(record.contentId ?? '');
  const [candidates, setCandidates] = React.useState<{ id: string; title: string }[]>([]);
  const [filter, setFilter] = React.useState('');

  const close = () => {
    if (busy) return;
    setDialog('none');
  };

  const remove = async () => {
    setBusy(true);
    try {
      const res = await client.removeQueueItem(record.id, { removeFromClient, blocklist });
      const extras = [
        res.removedFromClient ? 'removed from client' : null,
        res.blocklisted ? 'blocklisted' : null,
      ].filter(Boolean);
      success(`Removed ${record.title}${extras.length ? ` (${extras.join(', ')})` : ''}.`);
      setDialog('none');
      onChanged();
    } catch (err) {
      toastError(`Could not remove — ${err instanceof Error ? err.message : 'unknown error'}`);
    } finally {
      setBusy(false);
    }
  };

  const changeCategory = async () => {
    if (!category.trim()) {
      toastError('A category is required.');
      return;
    }
    setBusy(true);
    try {
      await client.updateQueueCategory(record.id, category.trim());
      success(`Category changed to "${category.trim()}".`);
      setDialog('none');
      onChanged();
    } catch (err) {
      toastError(`Could not change category — ${err instanceof Error ? err.message : 'unknown error'}`);
    } finally {
      setBusy(false);
    }
  };

  // Load library titles to pick a content match from (movies + series). Filtered
  // by the in-flight title text for convenience.
  const openImport = async () => {
    setDialog('grab');
    setCandidates([]);
    try {
      const [movies, series] = await Promise.all([
        client.listMovies().catch(() => [] as Movie[]),
        client.listSeries().catch(() => [] as Series[]),
      ]);
      const rows = [
        ...movies.map((m) => ({ id: String(m.id), title: `${m.title}${m.year ? ` (${m.year})` : ''}` })),
        ...series.map((s) => ({ id: String(s.id), title: s.title })),
      ];
      setCandidates(rows);
    } catch {
      // A failed library fetch just leaves the picker empty; the user can still
      // type a contentId directly (the field below is editable).
    }
  };

  const commitImport = async () => {
    if (!importPath.trim()) {
      toastError('The on-disk path of the completed download is required.');
      return;
    }
    setBusy(true);
    try {
      const res = await client.grabQueueItem({
        id: record.id,
        contentId: contentId.trim() || undefined,
        path: importPath.trim(),
      });
      if (res.imported) {
        success(`Imported ${res.files ?? 1} file(s).`);
        setDialog('none');
        onChanged();
      } else {
        toastError(res.message || 'Import did not complete.');
      }
    } catch (err) {
      toastError(`Manual import failed — ${err instanceof Error ? err.message : 'unknown error'}`);
    } finally {
      setBusy(false);
    }
  };

  const visibleCandidates = filter
    ? candidates.filter((c) => c.title.toLowerCase().includes(filter.toLowerCase()))
    : candidates;

  return (
    <>
      <span style={{ display: 'inline-flex', gap: '0.5ch' }}>
        <Button
          theme="DANGER"
          aria-label={`Remove ${record.title}`}
          onClick={() => {
            setRemoveFromClient(false);
            setBlocklist(false);
            setDialog('remove');
          }}
        >
          Remove
        </Button>
        <Button
          theme="SECONDARY"
          aria-label={`Manual import ${record.title}`}
          onClick={openImport}
        >
          Manual import
        </Button>
        <Button
          theme="SECONDARY"
          aria-label={`Change category for ${record.title}`}
          onClick={() => {
            setCategory(record.category ?? '');
            setDialog('category');
          }}
        >
          Change category
        </Button>
      </span>

      {dialog === 'remove' ? (
        <Modal title="Remove from queue" glyph="✗ " onClose={close}>
          <Text style={{ margin: '0.5ch 0' }}>
            Remove <strong>{record.title}</strong> from the download queue?
          </Text>
          <div style={{ margin: '1ch 0' }}>
            <Checkbox
              name="queue-remove-client"
              aria-label="Also remove from download client (deletes the downloaded data)"
              defaultChecked={removeFromClient}
              onChange={(e) => setRemoveFromClient(e.target.checked)}
            >
              Also remove from download client (deletes the downloaded data)
            </Checkbox>
          </div>
          <div style={{ margin: '1ch 0' }}>
            <Checkbox
              name="queue-remove-blocklist"
              aria-label="Blocklist this release (never re-grab it)"
              defaultChecked={blocklist}
              onChange={(e) => setBlocklist(e.target.checked)}
            >
              Blocklist this release (never re-grab it)
            </Checkbox>
          </div>
          <div style={{ display: 'flex', gap: '1ch', marginTop: '1ch' }}>
            <Button
              theme="DANGER"
              aria-label="Confirm remove from queue"
              isDisabled={busy}
              onClick={busy ? undefined : remove}
            >
              {busy ? 'Removing…' : 'Remove'}
            </Button>
            <Button theme="SECONDARY" isDisabled={busy} onClick={close}>
              Cancel
            </Button>
          </div>
        </Modal>
      ) : null}

      {dialog === 'category' ? (
        <Modal title="Change category" onClose={close}>
          <Text style={{ margin: '0.5ch 0', opacity: 0.6 }}>
            Retag <strong>{record.title}</strong> with a new download category.
          </Text>
          <div style={{ margin: '0.5ch 0' }}>
            <Text style={{ opacity: 0.6 }}>Category</Text>
            <Input
              name="queue-category"
              aria-label="Queue category"
              value={category}
              onChange={(e) => setCategory(e.target.value)}
            />
          </div>
          <div style={{ display: 'flex', gap: '1ch', marginTop: '1ch' }}>
            <Button aria-label="Save category" isDisabled={busy} onClick={busy ? undefined : changeCategory}>
              {busy ? 'Saving…' : 'Save category'}
            </Button>
            <Button theme="SECONDARY" isDisabled={busy} onClick={close}>
              Cancel
            </Button>
          </div>
        </Modal>
      ) : null}

      {dialog === 'grab' ? (
        <Modal title="Import a completed file" onClose={close}>
          <Text style={{ margin: '0.5ch 0', opacity: 0.6 }}>
            Import the completed download <strong>{record.title}</strong> onto a library item.
          </Text>

          <div style={{ margin: '0.5ch 0' }}>
            <Text style={{ opacity: 0.6 }}>Download path on disk</Text>
            <Input
              name="queue-import-path"
              aria-label="Import path"
              placeholder="/downloads/complete/The.Movie.2024.1080p.mkv"
              value={importPath}
              onChange={(e) => setImportPath(e.target.value)}
            />
          </div>

          <div style={{ margin: '0.5ch 0' }}>
            <Text style={{ opacity: 0.6 }}>Match to library item</Text>
            <Input
              name="queue-import-filter"
              aria-label="Filter library items"
              placeholder="Filter titles…"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
          </div>

          <div
            style={{
              maxHeight: '20ch',
              overflowY: 'auto',
              border: '1px solid var(--theme-border)',
              padding: '0.5ch',
              margin: '0.5ch 0',
            }}
          >
            {visibleCandidates.length ? (
              visibleCandidates.map((c) => (
                <div
                  key={c.id}
                  role="button"
                  tabIndex={0}
                  aria-label={`Select ${c.title}`}
                  onClick={() => setContentId(c.id)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' || e.key === ' ') setContentId(c.id);
                  }}
                  style={{
                    padding: '0.25ch 0.5ch',
                    cursor: 'pointer',
                    background: contentId === c.id ? 'var(--theme-focused-foreground)' : undefined,
                  }}
                >
                  {contentId === c.id ? <Badge>selected</Badge> : null} {c.title}
                </div>
              ))
            ) : (
              <Text style={{ opacity: 0.5 }}>
                No library items{filter ? ' match' : ' loaded'}. You can enter a content id below.
              </Text>
            )}
          </div>

          <div style={{ margin: '0.5ch 0' }}>
            <Text style={{ opacity: 0.6 }}>Content id</Text>
            <Input
              name="queue-import-contentid"
              aria-label="Content id"
              value={contentId}
              onChange={(e) => setContentId(e.target.value)}
            />
          </div>

          <div style={{ display: 'flex', gap: '1ch', marginTop: '1ch' }}>
            <Button aria-label="Confirm file import action" isDisabled={busy} onClick={busy ? undefined : commitImport}>
              {busy ? 'Importing…' : 'Import'}
            </Button>
            <Button theme="SECONDARY" isDisabled={busy} onClick={close}>
              Cancel
            </Button>
          </div>
        </Modal>
      ) : null}
    </>
  );
};

export default QueueActions;
