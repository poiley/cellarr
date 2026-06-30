'use client';

// Collections screen — the Radarr-shaped movie-collection grouping
// (`GET /api/v3/collection`), each backed by a TMDb-collection import list.
// Lists the collections with their title, member count, and a per-row monitor
// toggle that persists via `PUT /api/v3/collection/{id}`. The Sonarr/cellarr
// faces return `[]`, so the screen degrades to an explanatory empty state.
//
// Composed exclusively from vendored SRCL primitives + the API client + the
// theme/app glue, per the SRCL-only rule. The monitor toggle is an SRCL
// Checkbox (no Switch primitive exists); it updates optimistically and rolls
// back on a failed PUT, surfacing the outcome via useToast().

import * as React from 'react';

import Link from 'next/link';

import Card from '@components/Card';
import Table from '@components/Table';
import TableRow from '@components/TableRow';
import TableColumn from '@components/TableColumn';
import Input from '@components/Input';
import Badge from '@components/Badge';
import Text from '@components/Text';
import Row from '@components/Row';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Divider from '@components/Divider';
import Checkbox from '@components/Checkbox';

import AppShell from '@app/_components/AppShell';
import { useToast } from '@app/_lib/ToastProvider';
import { api, ApiError } from '@lib/api/client';
import type { Collection } from '@lib/api/types';

type LoadState<T> =
  | { phase: 'loading' }
  | { phase: 'error'; message: string }
  | { phase: 'ready'; data: T };

function errorMessage(err: unknown, fallback: string): string {
  if (err instanceof ApiError) return `${err.message} (${err.code})`;
  return err instanceof Error ? err.message : fallback;
}

/** The member count for a collection, tolerating a missing/odd `movies` field. */
function movieCount(c: Collection): number {
  return Array.isArray(c.movies) ? c.movies.length : 0;
}

function CollectionsBrowser() {
  const { success, error } = useToast();

  const [state, setState] = React.useState<LoadState<Collection[]>>({ phase: 'loading' });
  const [filter, setFilter] = React.useState('');
  // The set of collection ids with an in-flight monitor PUT, so a row's toggle
  // is disabled (and the row marked busy) until its write resolves.
  const [pending, setPending] = React.useState<Set<number>>(new Set());

  React.useEffect(() => {
    const controller = new AbortController();
    setState({ phase: 'loading' });
    api
      .listCollections(controller.signal)
      .then((data) => setState({ phase: 'ready', data }))
      .catch((err: unknown) => {
        // A reachable-but-empty surface (Sonarr/cellarr face, or no collections)
        // is the common case; a network error degrades to an empty list too so
        // the screen is never stuck on a spinner offline.
        if (err instanceof ApiError && err.code === 'network_error') {
          setState({ phase: 'ready', data: [] });
          return;
        }
        setState({ phase: 'error', message: errorMessage(err, 'failed to load collections') });
      });
    return () => controller.abort();
  }, []);

  const collections = state.phase === 'ready' ? state.data : [];

  const filtered = React.useMemo(() => {
    const q = filter.trim().toLowerCase();
    const matched = q
      ? collections.filter((c) => c.title.toLowerCase().includes(q))
      : collections.slice();
    return matched.sort((a, b) => a.title.localeCompare(b.title));
  }, [collections, filter]);

  const monitoredCount = React.useMemo(
    () => collections.filter((c) => c.monitored).length,
    [collections]
  );

  // Optimistically flip a collection's monitor flag, PUT it, and roll back on
  // failure. The toggle is the only writer here, so a single source of truth
  // (the loaded list) is patched in place rather than refetching.
  const toggleMonitored = React.useCallback(
    async (c: Collection, next: boolean) => {
      if (pending.has(c.id)) return;
      setPending((prev) => new Set(prev).add(c.id));
      setState((prev) =>
        prev.phase === 'ready'
          ? {
              phase: 'ready',
              data: prev.data.map((row) =>
                row.id === c.id ? { ...row, monitored: next } : row
              ),
            }
          : prev
      );
      try {
        await api.updateCollection(c.id, { monitored: next });
        success(`${next ? 'Monitoring' : 'Stopped monitoring'} “${c.title}”.`);
      } catch (err: unknown) {
        // Roll the row back to its prior state and tell the user it didn't take.
        setState((prev) =>
          prev.phase === 'ready'
            ? {
                phase: 'ready',
                data: prev.data.map((row) =>
                  row.id === c.id ? { ...row, monitored: !next } : row
                ),
              }
            : prev
        );
        error(`Could not update “${c.title}”: ${errorMessage(err, 'update failed')}`);
      } finally {
        setPending((prev) => {
          const set = new Set(prev);
          set.delete(c.id);
          return set;
        });
      }
    },
    [pending, success, error]
  );

  return (
    <AppShell>
      <Card title="Collections">
        <RowSpaceBetween>
          <Text>
            Movie collections grouped from your import lists. Toggle monitoring to
            control whether a collection&rsquo;s missing members are searched.
          </Text>
          <Badge aria-label={`${collections.length} collections, ${monitoredCount} monitored`}>
            {monitoredCount}/{collections.length} monitored
          </Badge>
        </RowSpaceBetween>

        <Divider type="GRADIENT" />

        {state.phase === 'loading' ? <Text>Loading collections…</Text> : null}
        {state.phase === 'error' ? (
          <Text>Could not load collections: {state.message}</Text>
        ) : null}
        {state.phase === 'ready' && collections.length === 0 ? (
          <>
            <Text>
              No collections. Collections come from Radarr-style TMDb collection
              import lists — add one to a Movies library to populate this view.
            </Text>
            <Link href="/settings" style={{ textDecoration: 'none' }}>
              <Text style={{ opacity: 0.6 }}>→ Add an import list in Settings</Text>
            </Link>
          </>
        ) : null}

        {state.phase === 'ready' && collections.length > 0 ? (
          <>
            <Row style={{ gap: '1ch', flexWrap: 'wrap', alignItems: 'flex-end' }}>
              <div style={{ flex: '1 1 24ch', minWidth: '20ch' }}>
                <Input
                  name="collection-filter"
                  label="Filter"
                  placeholder="Filter by title…"
                  value={filter}
                  onChange={(e) => setFilter(e.target.value)}
                />
              </div>
            </Row>

            <Divider type="GRADIENT" />

            <RowSpaceBetween>
              <Text>
                {filtered.length} of {collections.length} collection
                {collections.length === 1 ? '' : 's'}
              </Text>
            </RowSpaceBetween>

            {filtered.length > 0 ? (
              <Table>
                <TableRow>
                  <TableColumn role="columnheader">Title</TableColumn>
                  <TableColumn role="columnheader">Movies</TableColumn>
                  <TableColumn role="columnheader">Monitored</TableColumn>
                </TableRow>
                {filtered.map((c) => {
                  const count = movieCount(c);
                  const busy = pending.has(c.id);
                  return (
                    <TableRow key={c.id}>
                      <TableColumn>{c.title}</TableColumn>
                      <TableColumn>
                        <Badge aria-label={`${count} movie${count === 1 ? '' : 's'}`}>
                          {count} movie{count === 1 ? '' : 's'}
                        </Badge>
                      </TableColumn>
                      <TableColumn onClick={(e) => e.stopPropagation()}>
                        <Row style={{ gap: '0.5ch', alignItems: 'center' }}>
                          <Checkbox
                            name={`monitor-${c.id}`}
                            aria-label={`Monitor ${c.title}`}
                            defaultChecked={c.monitored}
                            // Re-key so the controlled DOM reflects an
                            // optimistic flip / rollback rather than the
                            // checkbox's own uncontrolled state.
                            key={`monitor-${c.id}-${c.monitored}`}
                            onChange={(e) => {
                              void toggleMonitored(c, e.target.checked);
                            }}
                          >
                            <span style={{ position: 'absolute', left: '-9999px' }}>
                              Monitor {c.title}
                            </span>
                          </Checkbox>
                          <Badge>
                            {busy
                              ? 'SAVING…'
                              : c.monitored
                                ? 'MONITORED'
                                : 'UNMONITORED'}
                          </Badge>
                        </Row>
                      </TableColumn>
                    </TableRow>
                  );
                })}
              </Table>
            ) : (
              <Text>No collections match the current filter.</Text>
            )}
          </>
        ) : null}
      </Card>
    </AppShell>
  );
}

export default function Page() {
  return <CollectionsBrowser />;
}
