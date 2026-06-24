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

/** Sortable header cell: an SRCL TableColumn with aria-sort + a click handler. */
const SortHeader: React.FC<{
  label: string;
  col: SortKey;
  sort: SortState;
  onSort: (key: SortKey) => void;
}> = ({ label, col, sort, onSort }) => {
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
      style={{ cursor: 'pointer', userSelect: 'none', fontWeight: active ? 700 : undefined }}
      title={`Sort by ${label}`}
    >
      {label}
      {active ? ` ${sortCaret(active, sort.dir)}` : ''}
    </TableColumn>
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
  const [selected, setSelected] = React.useState<Set<string>>(new Set());
  const [busy, setBusy] = React.useState(false);

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

  // Bulk delete is intentionally NOT wired: the v3 shim exposes no
  // DELETE /movie/{id} or /series/{id} route (only GET + PUT monitored), and
  // cellarr's persistence layer has no movie/series removal seam yet. Surfacing
  // a button that no-ops or fakes success would risk the user's library, so we
  // defer it and tell the operator plainly.
  // TODO(library): wire bulk delete once the backend grows DELETE /movie/{id} +
  // /series/{id} (and a confirm dialog) — frontend selection plumbing is ready.
  const deleteSelected = () => {
    error('Delete is not available yet — the daemon has no movie/series removal endpoint.');
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
          <Row style={{ gap: '1ch', flexWrap: 'wrap', alignItems: 'flex-end' }}>
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
                  options={['All', 'Movie', 'Series']}
                  defaultValue={typeFilter}
                  placeholder="Type"
                  onChange={setTypeFilter}
                />
              </div>
            ) : null}
          </Row>

          <Divider type="GRADIENT" />

          {content.phase === 'loading' ? <Text>Loading content…</Text> : null}
          {content.phase === 'error' ? <Text>Could not load content: {content.message}</Text> : null}
          {content.phase === 'ready' && items.length === 0 ? (
            <Text>This library is empty. Add a title or run a search to populate it.</Text>
          ) : null}
          {content.phase === 'ready' && items.length > 0 ? (
            <>
              <RowSpaceBetween>
                <Text>
                  {filtered.length} of {items.length} item{items.length === 1 ? '' : 's'}
                </Text>
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
                  <Button theme="SECONDARY" onClick={deleteSelected} isDisabled={busy}>
                    ✗ Delete
                  </Button>
                  <Button theme="SECONDARY" onClick={clearSelection} isDisabled={busy}>
                    Clear
                  </Button>
                </Row>
              ) : null}

              {filtered.length > 0 ? (
                <Table>
                  <TableRow>
                    <TableColumn role="columnheader">
                      <Checkbox
                        name="select-all-visible"
                        defaultChecked={allVisibleSelected}
                        key={`all-${activeLib}-${allVisibleSelected}-${filtered.length}`}
                        onChange={toggleAllVisible}
                      >
                        <span style={{ position: 'absolute', left: '-9999px' }}>Select all rows</span>
                      </Checkbox>
                    </TableColumn>
                    <SortHeader label="Title" col="title" sort={sort} onSort={onSort} />
                    <SortHeader label="Year" col="year" sort={sort} onSort={onSort} />
                    {showType ? <TableColumn role="columnheader">Type</TableColumn> : null}
                    <SortHeader label="Quality" col="quality" sort={sort} onSort={onSort} />
                    <SortHeader label="Size" col="size" sort={sort} onSort={onSort} />
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
                          <Row style={{ gap: '1ch', alignItems: 'center' }}>
                            <PosterThumb id={item.id} title={item.title} />
                            <span>{item.title}</span>
                          </Row>
                        </TableColumn>
                        <TableColumn>{item.year ? String(item.year) : '—'}</TableColumn>
                        {showType ? <TableColumn>{item.kind}</TableColumn> : null}
                        <TableColumn>{item.quality ?? '—'}</TableColumn>
                        <TableColumn>{item.sizeOnDisk ? formatSize(item.sizeOnDisk) : '—'}</TableColumn>
                        <TableColumn>
                          <Row style={{ gap: '0.5ch', flexWrap: 'wrap', alignItems: 'center' }}>
                            {/* Glyph + emphasis so MISSING stands out beyond colour
                                alone (#17): ✓ downloaded, ✗ missing. */}
                            <span
                              aria-hidden="true"
                              style={{ fontWeight: 700, opacity: downloaded ? 0.7 : 1 }}
                            >
                              {fileGlyph(item)}
                            </span>
                            <Badge>{item.monitored ? 'MONITORED' : 'UNMONITORED'}</Badge>
                            <span style={{ fontWeight: downloaded ? 400 : 700 }}>
                              <Badge>{fileLabel(item)}</Badge>
                            </span>
                          </Row>
                        </TableColumn>
                      </TableRow>
                    );
                  })}
                </Table>
              ) : (
                <Text>No items match the current filter.</Text>
              )}
            </>
          ) : null}
        </Card>
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
