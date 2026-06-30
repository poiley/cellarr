'use client';

// Library browse screen (docs/10-ui.md §screen-mapping). The library switcher is
// an inline SRCL segmented control (Movies | TV | …) above the items table, and
// for the selected library the table shows the ACTUAL items it tracks — the
// movies and series, with year, monitored + downloaded state, quality and size —
// not the sparse `/api/v1` content refs. The rich data comes from the v3
// catalogues (`listMovies()` / `listSeries()`), scoped to the library by its
// root folders. Rows are sortable, multi-selectable (with a bulk action bar),
// and clicking one drills into the item-detail screen (/content?id=…).
//
// Composed exclusively from vendored SRCL primitives + the API client + the
// theme/app glue, per the SRCL-only rule.

import * as React from 'react';
import { useRouter, useSearchParams } from 'next/navigation';

import Card from '@components/Card';
import Table from '@components/Table';
import TableRow from '@components/TableRow';
import TableColumn from '@components/TableColumn';
import Input from '@components/Input';
import Select from '@components/Select';
import Badge from '@components/Badge';
import Text from '@components/Text';

import StatusBadge from '@app/_components/StatusBadge';
import { statusColor } from '@app/_lib/status';
import Row from '@components/Row';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Divider from '@components/Divider';
import ActionButton from '@components/ActionButton';
import Button from '@components/Button';
import Checkbox from '@components/Checkbox';

import AppShell from '@app/_components/AppShell';
import { useToast } from '@app/_lib/ToastProvider';
import { api, ApiError } from '@lib/api/client';
import type { Library } from '@lib/api/types';
import {
  ariaSort,
  fileGlyph,
  fileLabel,
  formatSize,
  itemInLibrary,
  mediaTypeOf,
  movieToItem,
  seriesToItem,
  sortCaret,
  sortItems,
  type LibraryItem,
  type SortKey,
  type SortState,
} from '@app/library/format';
import { mediaCoverUrl } from '@app/content/_lib/detail';

const STATUS_OPTIONS = ['All', 'Monitored', 'Unmonitored'];

// Grid-view sort choices (the table sorts by clicking a header; the grid has no
// headers, so it offers the same keys as an explicit Select).
const SORT_OPTIONS: ReadonlyArray<{ label: string; key: SortKey }> = [
  { label: 'Title', key: 'title' },
  { label: 'Year', key: 'year' },
  { label: 'Quality', key: 'quality' },
  { label: 'Size', key: 'size' },
  { label: 'Status', key: 'status' },
];

const VIEW_STORAGE_KEY = 'cellarr.library.view';

type LoadState<T> =
  | { phase: 'idle' }
  | { phase: 'loading' }
  | { phase: 'error'; message: string }
  | { phase: 'ready'; data: T };

function errorMessage(err: unknown, fallback: string): string {
  if (err instanceof ApiError) return `${err.message} (${err.code})`;
  return err instanceof Error ? err.message : fallback;
}

/** Load the items that belong to a library, by its media type, from v3. */
async function loadLibraryItems(lib: Library, signal: AbortSignal): Promise<LibraryItem[]> {
  if (lib.media_type === 'tv') {
    const series = await api.listSeries(signal);
    return series.map(seriesToItem).filter((item) => itemInLibrary(item, lib));
  }
  const movies = await api.listMovies(signal);
  return movies.map(movieToItem).filter((item) => itemInLibrary(item, lib));
}

/** The v3 manual-search command name for a row's media kind. */
function searchCommandFor(item: LibraryItem): string {
  return item.kind === 'series' ? 'SeriesSearch' : 'MoviesSearch';
}

/** The v3 content-id field name the command body keys on for a row's kind. */
function searchIdFieldFor(item: LibraryItem): 'seriesId' | 'movieId' {
  return item.kind === 'series' ? 'seriesId' : 'movieId';
}

/**
 * A small poster thumbnail for a library row, served by the cached-artwork
 * endpoint (`GET /api/v3/mediacover/{id}/poster`). The endpoint 404s when no
 * artwork is cached, so the thumb hides itself on error (an empty fixed-size
 * frame keeps the column from jumping). An <img> is allowed for real media (the
 * SRCL-only lint governs component imports, not media tags).
 */
const PosterThumb: React.FC<{ id: string; title: string }> = ({ id, title }) => {
  const [ok, setOk] = React.useState<boolean>(false);
  React.useEffect(() => setOk(false), [id]);
  return (
    <span
      aria-hidden="true"
      style={{
        display: 'inline-block',
        width: '3ch',
        height: '4.5ch',
        flex: '0 0 auto',
        border: ok ? '1px solid var(--theme-border, var(--theme-text))' : 'none',
        overflow: 'hidden',
        verticalAlign: 'middle',
      }}
    >
      <img
        src={mediaCoverUrl('poster', id)}
        alt={`${title} poster`}
        onLoad={() => setOk(true)}
        onError={() => setOk(false)}
        style={{
          width: '100%',
          height: '100%',
          objectFit: 'cover',
          display: ok ? 'block' : 'none',
        }}
      />
    </span>
  );
};

/**
 * A full-bleed poster for a grid card: the cached-artwork endpoint at a 2:3
 * aspect, with an ASCII placeholder (▦, the terminal/OLED aesthetic) while it
 * loads or when no artwork is cached. Mirrors the detail screen's <Poster>
 * load-state reconcile (a *cached* image can already be `complete` before React
 * attaches onLoad), scaled to fill the card width.
 */
const PosterCardImage: React.FC<{ id: string; title: string }> = ({ id, title }) => {
  const [state, setState] = React.useState<'loading' | 'ok' | 'error'>('loading');
  const imgRef = React.useRef<HTMLImageElement | null>(null);
  React.useEffect(() => {
    setState('loading');
    const img = imgRef.current;
    if (img && img.complete) setState(img.naturalWidth > 0 ? 'ok' : 'error');
  }, [id]);
  return (
    <div
      aria-hidden="true"
      style={{
        width: '100%',
        aspectRatio: '2 / 3',
        borderBottom: '1px solid var(--theme-border, var(--theme-text))',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        overflow: 'hidden',
        background: 'var(--theme-background)',
      }}
    >
      <img
        ref={imgRef}
        src={mediaCoverUrl('poster', id)}
        alt={`${title} poster`}
        onLoad={() => setState('ok')}
        onError={() => setState('error')}
        style={{
          width: '100%',
          height: '100%',
          objectFit: 'cover',
          display: state === 'ok' ? 'block' : 'none',
        }}
      />
      {state !== 'ok' ? (
        <Text style={{ fontSize: '3ch', opacity: 0.4 }} aria-hidden="true">
          ▦
        </Text>
      ) : null}
    </div>
  );
};

/**
 * One poster card in the grid view: a bordered cell with the poster on top and a
 * compact meta footer (title, year · quality, monitored + on-disk status with a
 * ✓/✗ glyph) below, plus a select checkbox overlaid top-left. The poster and the
 * title both drill into the item; the checkbox stops propagation so selecting a
 * card never opens it. Same selection/status semantics as a table row — only the
 * shape differs.
 */
const LibraryGridCard: React.FC<{
  item: LibraryItem;
  selected: boolean;
  onToggle: (id: string) => void;
  onOpen: (id: string) => void;
}> = ({ item, selected, onToggle, onOpen }) => {
  const open = () => onOpen(item.id);
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      open();
    }
  };
  return (
    <div
      style={{
        position: 'relative',
        border: '1px solid var(--theme-border, var(--theme-text))',
        outline: selected ? '1px solid var(--theme-text)' : undefined,
        background: selected ? 'var(--theme-focused-foreground)' : undefined,
        display: 'flex',
        flexDirection: 'column',
      }}
    >
      {/* Select checkbox — overlaid on the poster, on its own opaque chip so it
          stays legible over any artwork; stops propagation so it never opens. */}
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          position: 'absolute',
          top: '0.5ch',
          left: '0.5ch',
          zIndex: 1,
          background: 'var(--theme-background)',
          border: '1px solid var(--theme-border, var(--theme-text))',
          padding: '0 0.25ch',
          lineHeight: 1,
        }}
      >
        <Checkbox
          name={`select-${item.id}`}
          aria-label={`Select ${item.title}`}
          defaultChecked={selected}
          key={`grid-${item.id}-${selected}`}
          onChange={() => onToggle(item.id)}
        >
          <span style={{ position: 'absolute', left: '-9999px' }}>Select {item.title}</span>
        </Checkbox>
      </div>

      <div role="link" tabIndex={0} onClick={open} onKeyDown={onKey} style={{ cursor: 'pointer' }} title={`Open ${item.title}`}>
        <PosterCardImage id={item.id} title={item.title} />
      </div>

      <div style={{ padding: '0.75ch', display: 'flex', flexDirection: 'column', gap: '0.5ch', flex: '1 1 auto' }}>
        <span
          role="link"
          tabIndex={0}
          onClick={open}
          onKeyDown={onKey}
          title={`Open ${item.title}`}
          style={{ cursor: 'pointer', fontWeight: 600, lineHeight: 1.2 }}
        >
          {item.title}
        </span>
        <Row style={{ gap: '0.75ch', alignItems: 'center', flexWrap: 'wrap' }}>
          <span style={{ opacity: 0.6, fontVariantNumeric: 'tabular-nums' }}>
            {item.year ? String(item.year) : '—'}
          </span>
          {item.quality ? <Badge>{item.quality}</Badge> : null}
        </Row>
        <Row style={{ gap: '0.5ch', alignItems: 'center', flexWrap: 'wrap', marginTop: 'auto' }}>
          <span aria-hidden="true" style={{ fontWeight: 700, color: statusColor(fileLabel(item)) }}>
            {fileGlyph(item)}
          </span>
          <StatusBadge status={item.monitored ? 'MONITORED' : 'UNMONITORED'} />
          <StatusBadge status={fileLabel(item)} />
        </Row>
      </div>
    </div>
  );
};

/** Sortable header cell: an SRCL TableColumn with aria-sort + a click handler. */
const SortHeader: React.FC<{
  label: string;
  col: SortKey;
  sort: SortState;
  onSort: (key: SortKey) => void;
  style?: React.CSSProperties;
}> = ({ label, col, sort, onSort, style }) => {
  const active = sort.key === col;
  return (
    <TableColumn
      aria-sort={ariaSort(active, sort.dir)}
      role="columnheader"
      tabIndex={0}
      onClick={() => onSort(col)}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onSort(col);
        }
      }}
      style={{ cursor: 'pointer', userSelect: 'none', fontWeight: active ? 700 : undefined, ...style }}
      title={`Sort by ${label}`}
    >
      {label}
      {active ? ` ${sortCaret(active, sort.dir)}` : ''}
    </TableColumn>
  );
};

/**
 * The bulk-delete confirm dialog: a destructive-action gate before the
 * library-destroying DELETE fan-out. Composed only from SRCL primitives — a
 * bordered <Card> floated over the theme overlay scrim (mirroring the settings
 * ConfirmDialog), an unmistakable danger button (tinted with --ansi-9-red + a ✗
 * glyph), and two opt-in <Checkbox>es that both default OFF (safe: remove only
 * the records, keep the files). Dismisses on Escape. It does not delete by
 * itself — it only surfaces the choice and calls back.
 */
const BulkDeleteDialog: React.FC<{
  items: LibraryItem[];
  deleteFiles: boolean;
  addExclusion: boolean;
  pending: boolean;
  onToggleDeleteFiles: (next: boolean) => void;
  onToggleAddExclusion: (next: boolean) => void;
  onConfirm: () => void;
  onCancel: () => void;
}> = ({
  items,
  deleteFiles,
  addExclusion,
  pending,
  onToggleDeleteFiles,
  onToggleAddExclusion,
  onConfirm,
  onCancel,
}) => {
  const count = items.length;

  React.useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !pending) onCancel();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onCancel, pending]);

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
      <div
        role="alertdialog"
        aria-modal="true"
        aria-label={`Delete ${count} item${count === 1 ? '' : 's'}`}
        style={{ maxWidth: '64ch', width: '100%' }}
      >
        <Card title={`✗ Delete ${count} item${count === 1 ? '' : 's'}`} mode="left">
          <Text style={{ margin: '0.5ch 0' }}>
            The following will be removed from the library:
          </Text>
          {/* What's about to go — capped so a huge selection can't blow out the
              dialog; the leftover count is summarised. */}
          <ul style={{ margin: '1ch 0', paddingLeft: '2ch', maxHeight: '24ch', overflow: 'auto' }}>
            {items.slice(0, 12).map((item) => (
              <li key={item.id}>
                {item.title}
                {item.year ? ` (${item.year})` : ''} — {item.kind}
              </li>
            ))}
            {count > 12 ? <li>…and {count - 12} more</li> : null}
          </ul>

          <div style={{ margin: '1ch 0' }}>
            <Checkbox
              name="bulk-delete-files"
              aria-label="Also delete files from disk"
              defaultChecked={deleteFiles}
              onChange={(e) => onToggleDeleteFiles(e.target.checked)}
            >
              Also delete files from disk
            </Checkbox>
            <Checkbox
              name="bulk-delete-exclusion"
              aria-label="Add to import-exclusion list"
              defaultChecked={addExclusion}
              onChange={(e) => onToggleAddExclusion(e.target.checked)}
            >
              Add to import-exclusion list (don&rsquo;t re-add on the next sync)
            </Checkbox>
          </div>

          <Text style={{ opacity: 0.5, margin: '1ch 0' }}>
            {deleteFiles
              ? 'Files are recycled into the configured recycle bin when set, otherwise unlinked. This cannot be undone.'
              : 'Only the library records are removed; the media files stay on disk.'}
          </Text>

          <div style={{ display: 'flex', gap: '1ch', marginTop: '1ch' }}>
            <Button
              theme="DANGER"
              aria-label={`Delete ${count} item${count === 1 ? '' : 's'}`}
              isDisabled={pending}
              onClick={pending ? undefined : onConfirm}
            >
              {pending ? 'Deleting…' : `✗ Delete ${count} item${count === 1 ? '' : 's'}`}
            </Button>
            <Button theme="SECONDARY" isDisabled={pending} onClick={pending ? undefined : onCancel}>
              Cancel
            </Button>
          </div>
        </Card>
      </div>
    </div>
  );
};

function LibraryBrowser() {
  const router = useRouter();
  const params = useSearchParams();
  const requestedLib = params.get('lib') ?? undefined;
  const { success, error, info } = useToast();

  const [libs, setLibs] = React.useState<LoadState<Library[]>>({ phase: 'loading' });
  const [content, setContent] = React.useState<LoadState<LibraryItem[]>>({ phase: 'idle' });
  const [filter, setFilter] = React.useState('');
  const [status, setStatus] = React.useState<string>('All');
  const [typeFilter, setTypeFilter] = React.useState<string>('All');
  const [sort, setSort] = React.useState<SortState>({ key: 'title', dir: 'asc' });
  // Grid is the default — the library reads as a wall of posters (a media app),
  // with the dense sortable table one click away. The choice persists per browser.
  const [view, setView] = React.useState<'grid' | 'list'>('grid');
  const [selected, setSelected] = React.useState<Set<string>>(new Set());

  // Restore the saved view once on mount (localStorage is client-only; the
  // initial render stays 'grid' on both server and client so hydration matches).
  React.useEffect(() => {
    try {
      const saved = window.localStorage.getItem(VIEW_STORAGE_KEY);
      if (saved === 'grid' || saved === 'list') setView(saved);
    } catch {
      /* ignore unavailable storage */
    }
  }, []);

  const changeView = (next: 'grid' | 'list') => {
    setView(next);
    try {
      window.localStorage.setItem(VIEW_STORAGE_KEY, next);
    } catch {
      /* ignore unavailable storage */
    }
  };
  const [busy, setBusy] = React.useState(false);
  // Bulk-delete confirm dialog: null when closed, otherwise the rows it targets.
  // The dialog is the only path to a delete — the library-destroying action is
  // never one click away. `deleteFiles`/`addImportExclusion` start OFF (the safe
  // default: remove only the records, keep the files on disk).
  const [deleteDialog, setDeleteDialog] = React.useState<LibraryItem[] | null>(null);
  const [deleteFiles, setDeleteFiles] = React.useState(false);
  const [addExclusion, setAddExclusion] = React.useState(false);

  // Load the library list once.
  React.useEffect(() => {
    const controller = new AbortController();
    setLibs({ phase: 'loading' });
    api
      .listLibraries(controller.signal)
      .then((data) => setLibs({ phase: 'ready', data }))
      .catch((err: unknown) => {
        if (err instanceof ApiError && err.code === 'network_error') {
          setLibs({ phase: 'ready', data: [] });
          return;
        }
        setLibs({ phase: 'error', message: errorMessage(err, 'failed to load libraries') });
      });
    return () => controller.abort();
  }, []);

  const libraries = libs.phase === 'ready' ? libs.data : [];

  // Auto-select the first library on load so items render immediately, while the
  // URL stays the source of truth for an explicit pick. If the requested `lib`
  // doesn't resolve (stale/bad id), we also fall back to the first one so the
  // screen is never stuck showing only a switcher.
  const explicitLibrary = requestedLib
    ? libraries.find((l) => l.id === requestedLib)
    : undefined;
  const selectedLibrary = explicitLibrary ?? libraries[0];
  const activeLib = selectedLibrary?.id;

  // Load the selected library's items whenever it changes (and once the library
  // list is available, so we know its media type + root folders).
  React.useEffect(() => {
    if (!activeLib || !selectedLibrary) {
      setContent({ phase: 'idle' });
      return;
    }
    const controller = new AbortController();
    setContent({ phase: 'loading' });
    setSelected(new Set());
    setTypeFilter('All');
    loadLibraryItems(selectedLibrary, controller.signal)
      .then((data) => setContent({ phase: 'ready', data }))
      .catch((err: unknown) => {
        if (err instanceof ApiError && err.code === 'network_error') {
          setContent({ phase: 'ready', data: [] });
          return;
        }
        setContent({ phase: 'error', message: errorMessage(err, 'failed to load content') });
      });
    return () => controller.abort();
  }, [activeLib, selectedLibrary]);

  const items = content.phase === 'ready' ? content.data : [];

  // Whether to show the "Type" filter + column: only for an all-types view (a
  // library holding more than one distinct media kind). A single-type library —
  // every library in this data model is single media type — never needs it.
  const showType = React.useMemo(() => new Set(items.map((i) => i.kind)).size > 1, [items]);

  const filtered = React.useMemo(() => {
    const q = filter.trim().toLowerCase();
    const matched = items.filter((item) => {
      if (status === 'Monitored' && !item.monitored) return false;
      if (status === 'Unmonitored' && item.monitored) return false;
      if (typeFilter === 'Movie' && item.kind !== 'movie') return false;
      if (typeFilter === 'Series' && item.kind !== 'series') return false;
      if (!q) return true;
      const hay = `${item.title} ${item.year ?? ''} ${item.kind}`.toLowerCase();
      return hay.includes(q);
    });
    return sortItems(matched, sort);
  }, [items, filter, status, typeFilter, sort]);

  // Keep the selection scoped to rows that are still visible after filtering, so
  // a bulk action never targets a hidden row the user can't see.
  const visibleSelected = React.useMemo(
    () => filtered.filter((item) => selected.has(item.id)),
    [filtered, selected]
  );
  const allVisibleSelected = filtered.length > 0 && visibleSelected.length === filtered.length;

  const onSort = (key: SortKey) => {
    setSort((prev) =>
      prev.key === key ? { key, dir: prev.dir === 'asc' ? 'desc' : 'asc' } : { key, dir: 'asc' }
    );
  };

  const toggleRow = (id: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const toggleAllVisible = () => {
    setSelected((prev) => {
      if (allVisibleSelected) {
        const next = new Set(prev);
        for (const item of filtered) next.delete(item.id);
        return next;
      }
      const next = new Set(prev);
      for (const item of filtered) next.add(item.id);
      return next;
    });
  };

  const clearSelection = () => setSelected(new Set());

  const onOpen = (id: string) => {
    router.push(`/content/?id=${encodeURIComponent(id)}`);
  };

  // Bulk: search for releases of the selected rows. Backed by the real v3
  // command surface — one ManualSearch per row (the command body addresses a
  // single content id), so we fan out and report a single summary toast.
  const searchSelected = async () => {
    if (visibleSelected.length === 0 || busy) return;
    setBusy(true);
    info(`Searching for ${visibleSelected.length} item${visibleSelected.length === 1 ? '' : 's'}…`);
    let ok = 0;
    let failed = 0;
    for (const item of visibleSelected) {
      try {
        await api.runCommandV3({
          name: searchCommandFor(item),
          [searchIdFieldFor(item)]: item.id,
        });
        ok += 1;
      } catch {
        failed += 1;
      }
    }
    setBusy(false);
    if (failed === 0) {
      success(`Queued a search for ${ok} item${ok === 1 ? '' : 's'}.`);
      clearSelection();
    } else if (ok === 0) {
      error(`Could not queue any searches (${failed} failed).`);
    } else {
      error(`Queued ${ok}, but ${failed} search${failed === 1 ? '' : 'es'} failed.`);
    }
  };

  // Open the bulk-delete confirm dialog for the current selection. The dialog
  // (not this opener) carries the destructive gate + the two opt-in toggles, so
  // a stray click on the bar's Delete only surfaces the dialog — it never
  // deletes. Toggles reset to their safe default each time the dialog opens.
  const openDeleteDialog = () => {
    if (visibleSelected.length === 0 || busy) return;
    setDeleteFiles(false);
    setAddExclusion(false);
    setDeleteDialog(visibleSelected);
  };

  const closeDeleteDialog = () => {
    if (busy) return;
    setDeleteDialog(null);
  };

  // Confirmed bulk delete, backed by the real v3 removal routes (DELETE
  // /movie/{id} + /series/{id}). The dialog's toggles decide whether the media
  // files are recycled/unlinked (deleteFiles) and whether an import-exclusion is
  // written so a sync cannot re-add the title (addImportExclusion). We fan out
  // one DELETE per row and report a single summary toast, then drop the deleted
  // rows from the local view.
  const confirmDelete = async () => {
    const targets = deleteDialog;
    if (!targets || targets.length === 0 || busy) return;
    const count = targets.length;
    const opts = { deleteFiles, addImportExclusion: addExclusion };
    setBusy(true);
    info(`Deleting ${count} item${count === 1 ? '' : 's'}…`);
    const deletedIds: string[] = [];
    let failed = 0;
    for (const item of targets) {
      try {
        if (item.kind === 'series') {
          await api.deleteSeries(item.id, opts);
        } else {
          await api.deleteMovie(item.id, opts);
        }
        deletedIds.push(item.id);
      } catch {
        failed += 1;
      }
    }
    setBusy(false);
    setDeleteDialog(null);
    // Drop the deleted rows from the local view so the table reflects reality
    // without a full reload.
    if (deletedIds.length > 0) {
      const gone = new Set(deletedIds);
      setContent((prev) =>
        prev.phase === 'ready'
          ? { phase: 'ready', data: prev.data.filter((i) => !gone.has(i.id)) }
          : prev
      );
      setSelected((prev) => {
        const next = new Set(prev);
        for (const id of deletedIds) next.delete(id);
        return next;
      });
    }
    if (failed === 0) {
      success(`Deleted ${deletedIds.length} item${deletedIds.length === 1 ? '' : 's'}.`);
    } else if (deletedIds.length === 0) {
      error(`Could not delete any items (${failed} failed).`);
    } else {
      error(`Deleted ${deletedIds.length}, but ${failed} failed.`);
    }
  };

  return (
    <AppShell>
      <Card title="Library">
        <RowSpaceBetween>
          <Text>Browse the movies and series across your libraries.</Text>
          <Badge>{libraries.length} libraries</Badge>
        </RowSpaceBetween>

        <Divider type="GRADIENT" />

        {/* Library switcher — an inline SRCL segmented control (one segment per
            library) composed from ActionButton, replacing the old persistent
            list panel. The active segment is marked selected; clicking one
            deep-links via ?lib= so the URL stays the source of truth. */}
        {libs.phase === 'loading' ? <Text>Loading libraries…</Text> : null}
        {libs.phase === 'error' ? <Text>Could not load libraries: {libs.message}</Text> : null}
        {libs.phase === 'ready' && libraries.length === 0 ? (
          <Text>No libraries yet. Add a Movies or TV library to get started.</Text>
        ) : null}
        {libraries.length > 0 ? (
          <Row role="tablist" aria-label="Libraries" style={{ gap: '1ch', flexWrap: 'wrap' }}>
            {libraries.map((lib) => {
              const isActive = lib.id === activeLib;
              return (
                <ActionButton
                  key={lib.id}
                  isSelected={isActive}
                  onClick={() => router.push(`/library/?lib=${encodeURIComponent(lib.id)}`)}
                >
                  <span role="tab" aria-selected={isActive}>
                    {isActive ? '● ' : '▸ '}
                    {lib.name} — {mediaTypeOf(lib)}
                  </span>
                </ActionButton>
              );
            })}
          </Row>
        ) : null}
      </Card>

      {activeLib ? (
        <Card title={selectedLibrary ? selectedLibrary.name : 'Content'} style={{ marginTop: '2ch' }}>
          {/* Filter input + status (and optional type) selects on ONE flex row,
              side by side. The SRCL <Row> renders display:block, so an explicit
              flex container is used here to keep the controls inline (wrapping on
              narrow viewports via flexWrap + per-control flex-basis). */}
          <div style={{ display: 'flex', gap: '1ch', alignItems: 'center', flexWrap: 'wrap' }}>
            <div style={{ flex: '1 1 24ch', minWidth: '20ch' }}>
              <Input
                name="content-filter"
                label="Filter"
                placeholder="Filter by title or year…"
                value={filter}
                onChange={(e) => setFilter(e.target.value)}
              />
            </div>
            <div style={{ flex: '0 0 22ch', minWidth: '18ch' }}>
              <Select
                name="content-status"
                aria-label="Filter by status"
                options={STATUS_OPTIONS}
                defaultValue={status}
                placeholder="Status"
                onChange={setStatus}
              />
            </div>
            {/* Type filter only for an all-types (mixed-kind) view (#18). A
                single-media-type library never shows it. */}
            {showType ? (
              <div style={{ flex: '0 0 18ch', minWidth: '16ch' }}>
                <Select
                  name="content-type"
                  aria-label="Filter by type"
                  options={['All', 'Movie', 'Series']}
                  defaultValue={typeFilter}
                  placeholder="Type"
                  onChange={setTypeFilter}
                />
              </div>
            ) : null}

            {/* Pushed to the right edge: a grid-only Sort select (the table sorts
                via its headers), then the Grid|List view toggle. */}
            <div style={{ marginLeft: 'auto', display: 'flex', gap: '1ch', alignItems: 'flex-end' }}>
              {view === 'grid' ? (
                <div style={{ flex: '0 0 16ch', minWidth: '14ch' }}>
                  <Select
                    name="grid-sort"
                    aria-label="Sort by"
                    options={SORT_OPTIONS.map((o) => o.label)}
                    defaultValue={SORT_OPTIONS.find((o) => o.key === sort.key)?.label ?? 'Title'}
                    placeholder="Sort by"
                    onChange={(label) => {
                      const opt = SORT_OPTIONS.find((o) => o.label === label);
                      if (opt) setSort({ key: opt.key, dir: 'asc' });
                    }}
                  />
                </div>
              ) : null}
              <Row role="group" aria-label="View" style={{ gap: '0.5ch', flexWrap: 'nowrap' }}>
                <ActionButton isSelected={view === 'grid'} onClick={() => changeView('grid')}>
                  ▦ Grid
                </ActionButton>
                <ActionButton isSelected={view === 'list'} onClick={() => changeView('list')}>
                  ≡ List
                </ActionButton>
              </Row>
            </div>
          </div>

          <Divider type="GRADIENT" />

          {content.phase === 'loading' ? <Text>Loading content…</Text> : null}
          {content.phase === 'error' ? <Text>Could not load content: {content.message}</Text> : null}
          {content.phase === 'ready' && items.length === 0 ? (
            <Text>This library is empty. Add a title or run a search to populate it.</Text>
          ) : null}
          {content.phase === 'ready' && items.length > 0 ? (
            <>
              <RowSpaceBetween>
                <Row style={{ gap: '1ch', alignItems: 'center' }}>
                  {/* Grid view has no table header, so the select-all lives here. */}
                  {view === 'grid' && filtered.length > 0 ? (
                    <Checkbox
                      name="select-all-visible"
                      aria-label="Select all"
                      defaultChecked={allVisibleSelected}
                      key={`gall-${activeLib}-${allVisibleSelected}-${filtered.length}`}
                      onChange={toggleAllVisible}
                    >
                      <span style={{ position: 'absolute', left: '-9999px' }}>Select all</span>
                    </Checkbox>
                  ) : null}
                  <Text>
                    {filtered.length} of {items.length} item{items.length === 1 ? '' : 's'}
                  </Text>
                </Row>
              </RowSpaceBetween>

              {/* Bulk action bar — appears when rows are selected (#19). */}
              {visibleSelected.length > 0 ? (
                <Row
                  role="group"
                  aria-label="Bulk actions"
                  style={{ gap: '1ch', flexWrap: 'wrap', alignItems: 'center', marginBottom: '1ch' }}
                >
                  <Badge>
                    {visibleSelected.length} selected
                  </Badge>
                  <Button
                    theme="PRIMARY"
                    isDisabled={busy}
                    onClick={() => {
                      void searchSelected();
                    }}
                  >
                    ▸ Search missing
                  </Button>
                  <Button
                    theme="DANGER"
                    onClick={openDeleteDialog}
                    isDisabled={busy}
                  >
                    ✗ Delete
                  </Button>
                  <Button theme="SECONDARY" onClick={clearSelection} isDisabled={busy}>
                    Clear
                  </Button>
                </Row>
              ) : null}

              {filtered.length === 0 ? (
                <Text>No items match the current filter.</Text>
              ) : view === 'grid' ? (
                <div
                  style={{
                    display: 'grid',
                    gridTemplateColumns: 'repeat(auto-fill, minmax(20ch, 1fr))',
                    gap: '1.5ch',
                    marginTop: '1ch',
                  }}
                >
                  {filtered.map((item) => (
                    <LibraryGridCard
                      key={item.id}
                      item={item}
                      selected={selected.has(item.id)}
                      onToggle={toggleRow}
                      onOpen={onOpen}
                    />
                  ))}
                </div>
              ) : (
                <Table>
                  <TableRow>
                    <TableColumn role="columnheader">
                      <Checkbox
                        name="select-all-visible"
                        aria-label="Select all rows"
                        defaultChecked={allVisibleSelected}
                        key={`all-${activeLib}-${allVisibleSelected}-${filtered.length}`}
                        onChange={toggleAllVisible}
                      >
                        <span style={{ position: 'absolute', left: '-9999px' }}>Select all rows</span>
                      </Checkbox>
                    </TableColumn>
                    <SortHeader label="Title" col="title" sort={sort} onSort={onSort} />
                    <SortHeader
                      label="Year"
                      col="year"
                      sort={sort}
                      onSort={onSort}
                      style={{ textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}
                    />
                    {showType ? <TableColumn role="columnheader">Type</TableColumn> : null}
                    <SortHeader label="Quality" col="quality" sort={sort} onSort={onSort} />
                    <SortHeader
                      label="Size"
                      col="size"
                      sort={sort}
                      onSort={onSort}
                      style={{ textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}
                    />
                    <SortHeader label="Status" col="status" sort={sort} onSort={onSort} />
                  </TableRow>
                  {filtered.map((item) => {
                    const isSelected = selected.has(item.id);
                    const downloaded = item.hasFile;
                    return (
                      <TableRow key={item.id} style={{ cursor: 'default' }}>
                        <TableColumn
                          onClick={(e) => e.stopPropagation()}
                        >
                          <Checkbox
                            name={`select-${item.id}`}
                            aria-label={`Select ${item.title}`}
                            defaultChecked={isSelected}
                            key={`row-${item.id}-${isSelected}`}
                            onChange={() => toggleRow(item.id)}
                          >
                            <span style={{ position: 'absolute', left: '-9999px' }}>
                              Select {item.title}
                            </span>
                          </Checkbox>
                        </TableColumn>
                        <TableColumn
                          onClick={() => onOpen(item.id)}
                          onKeyDown={(e) => {
                            if (e.key === 'Enter' || e.key === ' ') {
                              e.preventDefault();
                              onOpen(item.id);
                            }
                          }}
                          role="link"
                          tabIndex={0}
                          style={{ cursor: 'pointer' }}
                          title={`Open ${item.title}`}
                        >
                          {/* Thumbnail + title vertically centered. SRCL <Row>
                              renders display:block (so alignItems is inert);
                              an explicit flex container centers the title against
                              the poster thumbnail. */}
                          <div style={{ display: 'flex', gap: '1ch', alignItems: 'center' }}>
                            <PosterThumb id={item.id} title={item.title} />
                            <span>{item.title}</span>
                          </div>
                        </TableColumn>
                        <TableColumn style={{ textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                          {item.year ? String(item.year) : '—'}
                        </TableColumn>
                        {showType ? <TableColumn>{item.kind}</TableColumn> : null}
                        <TableColumn>{item.quality ?? '—'}</TableColumn>
                        <TableColumn style={{ textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                          {item.sizeOnDisk ? formatSize(item.sizeOnDisk) : '—'}
                        </TableColumn>
                        <TableColumn>
                          <Row style={{ gap: '0.5ch', flexWrap: 'wrap', alignItems: 'center' }}>
                            {/* Glyph + colour so MISSING stands out beyond colour
                                alone (#17): ✓ downloaded, ✗ missing. */}
                            <span
                              aria-hidden="true"
                              style={{ fontWeight: 700, color: statusColor(fileLabel(item)) }}
                            >
                              {fileGlyph(item)}
                            </span>
                            <StatusBadge status={item.monitored ? 'MONITORED' : 'UNMONITORED'} />
                            <StatusBadge status={fileLabel(item)} />
                          </Row>
                        </TableColumn>
                      </TableRow>
                    );
                  })}
                </Table>
              )}
            </>
          ) : null}
        </Card>
      ) : null}

      {deleteDialog ? (
        <BulkDeleteDialog
          items={deleteDialog}
          deleteFiles={deleteFiles}
          addExclusion={addExclusion}
          pending={busy}
          onToggleDeleteFiles={setDeleteFiles}
          onToggleAddExclusion={setAddExclusion}
          onConfirm={() => {
            void confirmDelete();
          }}
          onCancel={closeDeleteDialog}
        />
      ) : null}
    </AppShell>
  );
}

export default function Page() {
  // useSearchParams requires a Suspense boundary under static export.
  return (
    <React.Suspense fallback={<AppShell><Card title="Library"><Text>Loading…</Text></Card></AppShell>}>
      <LibraryBrowser />
    </React.Suspense>
  );
}
