'use client';

// Library browse screen (docs/10-ui.md §screen-mapping). Lists the daemon's
// libraries, and for the selected library shows the ACTUAL items it tracks —
// the movies and series, with year, monitored + downloaded state, quality and
// size — not the sparse `/api/v1` content refs. The rich data comes from the v3
// catalogues (`listMovies()` / `listSeries()`), scoped to the library by its
// root folders. Selecting a row drills into the item-detail screen
// (/content?id=…); the v3 ids resolve there through `/api/v1/content/{id}`.
//
// Composed exclusively from vendored SRCL primitives + the API client + the
// theme/app glue, per the SRCL-only rule.

import * as React from 'react';
import Link from 'next/link';
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
import ActionListItem from '@components/ActionListItem';

import AppShell from '@app/_components/AppShell';
import { api, ApiError } from '@lib/api/client';
import type { Library } from '@lib/api/types';
import {
  fileLabel,
  formatSize,
  itemInLibrary,
  mediaTypeOf,
  movieToItem,
  seriesToItem,
  type LibraryItem,
} from '@app/library/format';

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

function LibraryBrowser() {
  const router = useRouter();
  const params = useSearchParams();
  const requestedLib = params.get('lib') ?? undefined;

  const [libs, setLibs] = React.useState<LoadState<Library[]>>({ phase: 'loading' });
  const [content, setContent] = React.useState<LoadState<LibraryItem[]>>({ phase: 'idle' });
  const [filter, setFilter] = React.useState('');
  const [status, setStatus] = React.useState<string>('All');

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
  // screen is never stuck showing only library names.
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

  const filtered = React.useMemo(() => {
    const q = filter.trim().toLowerCase();
    return items.filter((item) => {
      if (status === 'Monitored' && !item.monitored) return false;
      if (status === 'Unmonitored' && item.monitored) return false;
      if (!q) return true;
      const hay = `${item.title} ${item.year ?? ''} ${item.kind}`.toLowerCase();
      return hay.includes(q);
    });
  }, [items, filter, status]);

  const onOpen = (id: string) => {
    router.push(`/content/?id=${encodeURIComponent(id)}`);
  };

  return (
    <AppShell>
      <Card title="Library">
        <RowSpaceBetween>
          <Text>Browse the movies and series across your libraries.</Text>
          <Badge>{libraries.length} libraries</Badge>
        </RowSpaceBetween>

        <Divider type="GRADIENT" />

        {/* Library picker — rendered as SRCL ActionListItems linked by route. */}
        {libs.phase === 'loading' ? <Text>Loading libraries…</Text> : null}
        {libs.phase === 'error' ? <Text>Could not load libraries: {libs.message}</Text> : null}
        {libs.phase === 'ready' && libraries.length === 0 ? (
          <Text>No libraries yet. Add a Movies or TV library to get started.</Text>
        ) : null}
        {libraries.map((lib) => (
          <Link key={lib.id} href={`/library/?lib=${encodeURIComponent(lib.id)}`} style={{ textDecoration: 'none' }}>
            <ActionListItem icon={lib.id === activeLib ? '◆' : '◇'}>
              {lib.name} — {mediaTypeOf(lib)}
            </ActionListItem>
          </Link>
        ))}
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

              {filtered.length > 0 ? (
                <Table>
                  <TableRow>
                    <TableColumn>Title</TableColumn>
                    <TableColumn>Year</TableColumn>
                    <TableColumn>Type</TableColumn>
                    <TableColumn>Quality</TableColumn>
                    <TableColumn>Size</TableColumn>
                    <TableColumn>Status</TableColumn>
                  </TableRow>
                  {filtered.map((item) => (
                    <TableRow
                      key={item.id}
                      onClick={() => onOpen(item.id)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter' || e.key === ' ') {
                          e.preventDefault();
                          onOpen(item.id);
                        }
                      }}
                      role="link"
                      style={{ cursor: 'pointer' }}
                      title={`Open ${item.title}`}
                    >
                      <TableColumn>{item.title}</TableColumn>
                      <TableColumn>{item.year ? String(item.year) : '—'}</TableColumn>
                      <TableColumn>{item.kind}</TableColumn>
                      <TableColumn>{item.quality ?? '—'}</TableColumn>
                      <TableColumn>{item.sizeOnDisk ? formatSize(item.sizeOnDisk) : '—'}</TableColumn>
                      <TableColumn>
                        <Row style={{ gap: '0.5ch', flexWrap: 'wrap' }}>
                          <Badge>{item.monitored ? 'MONITORED' : 'UNMONITORED'}</Badge>
                          <Badge>{fileLabel(item)}</Badge>
                        </Row>
                      </TableColumn>
                    </TableRow>
                  ))}
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
