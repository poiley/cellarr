'use client';

// Library browse screen (docs/10-ui.md §screen-mapping). Lists the daemon's
// libraries, and for the selected library shows its monitored content in a
// SRCL DataTable with a filter Input and a status Select. Selecting an item in
// the "Open item" Select navigates to the item-detail screen (/content?id=…).
//
// Composed exclusively from vendored SRCL primitives + the API client + the
// theme/app glue, per the SRCL-only rule.

import * as React from 'react';
import Link from 'next/link';
import { useRouter, useSearchParams } from 'next/navigation';

import Card from '@components/Card';
import DataTable from '@components/DataTable';
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
import type { ContentRef, Library } from '@lib/api/types';
import {
  coordsLabel,
  kindOf,
  mediaTypeOf,
  monitoredLabel,
  titleOf,
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

function LibraryBrowser() {
  const router = useRouter();
  const params = useSearchParams();
  const activeLib = params.get('lib') ?? undefined;

  const [libs, setLibs] = React.useState<LoadState<Library[]>>({ phase: 'loading' });
  const [content, setContent] = React.useState<LoadState<ContentRef[]>>({ phase: 'idle' });
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

  // Load content whenever the active library changes.
  React.useEffect(() => {
    if (!activeLib) {
      setContent({ phase: 'idle' });
      return;
    }
    const controller = new AbortController();
    setContent({ phase: 'loading' });
    api
      .listContent(activeLib, controller.signal)
      .then((data) => setContent({ phase: 'ready', data }))
      .catch((err: unknown) => {
        if (err instanceof ApiError && err.code === 'network_error') {
          setContent({ phase: 'ready', data: [] });
          return;
        }
        setContent({ phase: 'error', message: errorMessage(err, 'failed to load content') });
      });
    return () => controller.abort();
  }, [activeLib]);

  const libraries = libs.phase === 'ready' ? libs.data : [];
  const selectedLibrary = libraries.find((l) => l.id === activeLib);

  const items = content.phase === 'ready' ? content.data : [];

  const filtered = React.useMemo(() => {
    const q = filter.trim().toLowerCase();
    return items.filter((item) => {
      const node = item as Record<string, unknown>;
      if (status === 'Monitored' && monitoredLabel(node) !== 'MONITORED') return false;
      if (status === 'Unmonitored' && monitoredLabel(node) !== 'UNMONITORED') return false;
      if (!q) return true;
      const hay = `${titleOf(node)} ${coordsLabel(node.coords) ?? ''} ${kindOf(node) ?? ''}`.toLowerCase();
      return hay.includes(q);
    });
  }, [items, filter, status]);

  const tableData = React.useMemo<string[][]>(() => {
    const header = ['Title', 'Type', 'Detail', 'Status'];
    const rows = filtered.map((item) => {
      const node = item as Record<string, unknown>;
      return [
        titleOf(node),
        kindOf(node) ?? mediaTypeOf(node),
        coordsLabel(node.coords) ?? '—',
        monitoredLabel(node),
      ];
    });
    return [header, ...rows];
  }, [filtered]);

  const openOptions = React.useMemo(() => filtered.map((item) => titleOf(item as Record<string, unknown>)), [filtered]);

  const onOpen = (chosenTitle: string) => {
    const match = filtered.find((item) => titleOf(item as Record<string, unknown>) === chosenTitle);
    if (match) router.push(`/content/?id=${encodeURIComponent(match.id)}`);
  };

  return (
    <AppShell>
      <Card title="Library">
        <RowSpaceBetween>
          <Text>Browse monitored items across your libraries.</Text>
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
                placeholder="Filter by title or detail…"
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
                <div style={{ flex: '0 0 28ch', minWidth: '20ch' }}>
                  <Select
                    name="open-item"
                    options={openOptions}
                    placeholder="Open item…"
                    onChange={onOpen}
                  />
                </div>
              </RowSpaceBetween>
              {filtered.length > 0 ? (
                <DataTable data={tableData} />
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
