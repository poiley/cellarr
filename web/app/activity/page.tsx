'use client';

// Activity / Queue (docs/10-ui.md §screen-mapping): the in-progress queue, a
// DataTable of the scheduler's in-flight/pending jobs, with a BarProgress per
// active download. Subscribed to the live SSE stream at /api/v1/stream so queue
// progress, imports, and decision-log entries update without polling.
//
// Composed only from vendored SRCL primitives. The single non-component pieces
// are routing/data glue: the API client (queue snapshot) and the browser-native
// EventSource (the live stream — a global, not a UI primitive).

import * as React from 'react';

import Card from '@components/Card';
import Text from '@components/Text';
import Badge from '@components/Badge';
import Row from '@components/Row';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Divider from '@components/Divider';
import DataTable from '@components/DataTable';
import BarProgress from '@components/BarProgress';
import BarLoader from '@components/BarLoader';
import AlertBanner from '@components/AlertBanner';

import AppShell from '@app/_components/AppShell';
import { api, ApiError, resolveBaseUrl } from '@lib/api/client';
import type { QueueEntry } from '@lib/api/types';

// --- live-stream payloads (mirror crates/cellarr-api/src/events.rs) ----------
// Local shapes for the SSE frames; not UI primitives, so no import needed.

interface QueueProgressEvent {
  grab_id: string;
  status: string;
  progress?: number; // [0, 1]
}

interface ImportCompletedEvent {
  content_id: string;
  path: string;
}

type StreamState = 'connecting' | 'live' | 'offline';

interface ActivityItem {
  id: string;
  status: string;
  progress?: number; // [0, 1]
}

const STREAM_PATH = '/api/v1/stream';

export default function ActivityPage() {
  const [queue, setQueue] = React.useState<QueueEntry[] | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [stream, setStream] = React.useState<StreamState>('connecting');
  // Live overlay of grab progress keyed by grab id, fed by the SSE stream.
  const [live, setLive] = React.useState<Record<string, ActivityItem>>({});
  const [lastImport, setLastImport] = React.useState<ImportCompletedEvent | null>(null);

  // Initial snapshot of the queue from the REST surface.
  React.useEffect(() => {
    const controller = new AbortController();
    api
      .getQueue(controller.signal)
      .then((entries) => {
        setQueue(entries);
        setError(null);
      })
      .catch((err: unknown) => {
        if (err instanceof ApiError && err.code === 'network_error') {
          setQueue([]);
          return;
        }
        setError(err instanceof Error ? err.message : 'failed to load the queue');
      });
    return () => controller.abort();
  }, []);

  // Live updates over SSE — no polling timer (docs/09-api.md).
  React.useEffect(() => {
    if (typeof window === 'undefined' || typeof EventSource === 'undefined') {
      setStream('offline');
      return;
    }

    const source = new EventSource(`${resolveBaseUrl()}${STREAM_PATH}`);

    const onProgress = (ev: MessageEvent) => {
      try {
        const data = JSON.parse(ev.data) as QueueProgressEvent;
        setLive((prev) => ({
          ...prev,
          [data.grab_id]: {
            id: data.grab_id,
            status: data.status,
            progress: data.progress,
          },
        }));
      } catch {
        // Ignore malformed frames rather than tearing down the view.
      }
    };

    const onImport = (ev: MessageEvent) => {
      try {
        setLastImport(JSON.parse(ev.data) as ImportCompletedEvent);
      } catch {
        // Ignore malformed frames.
      }
    };

    source.addEventListener('open', () => setStream('live'));
    source.addEventListener('queue_progress', onProgress as EventListener);
    source.addEventListener('import_completed', onImport as EventListener);
    source.addEventListener('error', () => {
      // EventSource auto-reconnects; reflect the gap until it reopens.
      setStream(source.readyState === EventSource.CLOSED ? 'offline' : 'connecting');
    });

    return () => source.close();
  }, []);

  const streamBadge = (
    <Badge>
      {stream === 'live' ? 'live' : stream === 'connecting' ? 'connecting…' : 'offline'}
    </Badge>
  );

  // Merge the REST snapshot with the live progress overlay into table rows.
  const liveItems = Object.values(live);
  const liveById = new Map(liveItems.map((item) => [item.id, item]));

  const rows: ActivityItem[] = [];
  for (const entry of queue ?? []) {
    const overlay = liveById.get(entry.id);
    rows.push({
      id: entry.id,
      status: overlay?.status ?? entry.state,
      progress: overlay?.progress,
    });
    liveById.delete(entry.id);
  }
  // Grabs that only exist on the live stream (not yet in the snapshot).
  for (const item of liveById.values()) rows.push(item);

  const commandFor = (id: string) =>
    (queue ?? []).find((entry) => entry.id === id)?.command ?? 'download';

  const tableData: string[][] = [
    ['Item', 'State', 'Progress'],
    ...rows.map((row) => [
      commandFor(row.id),
      row.status,
      row.progress != null ? `${Math.round(row.progress * 100)}%` : '—',
    ]),
  ];

  const isLoading = queue === null && !error;
  const isEmpty = !isLoading && !error && rows.length === 0;

  return (
    <AppShell>
      <Card title="Activity / Queue">
        <RowSpaceBetween>
          <Text>In-progress downloads and scheduled work.</Text>
          {streamBadge}
        </RowSpaceBetween>

        <Divider type="GRADIENT" />

        {error ? (
          <AlertBanner style={{ marginTop: '1ch' }}>
            Could not load the queue: {error}
          </AlertBanner>
        ) : null}

        {isLoading ? (
          <div style={{ marginTop: '1ch' }}>
            <Text style={{ opacity: 0.6, marginBottom: '0.5ch' }}>Loading the queue…</Text>
            <BarLoader intervalRate={600} />
          </div>
        ) : null}

        {isEmpty ? (
          <Text style={{ marginTop: '1ch', opacity: 0.6 }}>
            {stream === 'live'
              ? 'Nothing in the queue. New grabs appear here as they start.'
              : 'Nothing in the queue yet.'}
          </Text>
        ) : null}

        {!isLoading && !error && rows.length > 0 ? (
          <>
            <div style={{ marginTop: '1ch' }}>
              <DataTable data={tableData} />
            </div>

            <Divider type="GRADIENT" />

            <Text style={{ opacity: 0.6, marginBottom: '0.5ch' }}>Active downloads</Text>
            {rows.map((row) => (
              <div key={row.id} style={{ marginBottom: '1ch' }}>
                <RowSpaceBetween>
                  <Text>{commandFor(row.id)}</Text>
                  <Badge>{row.status}</Badge>
                </RowSpaceBetween>
                {row.progress != null ? (
                  <BarProgress progress={Math.round(row.progress * 100)} />
                ) : (
                  <BarLoader intervalRate={700} />
                )}
              </div>
            ))}
          </>
        ) : null}

        {lastImport ? (
          <Row>
            <Text style={{ opacity: 0.6 }}>
              Last import: {lastImport.path}
            </Text>
          </Row>
        ) : null}
      </Card>
    </AppShell>
  );
}
