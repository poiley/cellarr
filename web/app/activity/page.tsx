'use client';

// Activity (docs/10-ui.md §screen-mapping): the operator's live "what is the
// daemon doing right now" view. It has THREE clearly separated concerns, which
// the old version conflated:
//
//   1. Downloads — the real grab lifecycle (grabbed → downloading → importing →
//      imported), driven by the queue snapshot + the live SSE `queue_progress`
//      and `import_completed` frames. A "download" here is a release a download
//      client is actually working, NOT a cron job.
//   2. Self-heal — releases that were blocklisted (a failed download the daemon
//      walled off) paired with the recovery grab that follows. Sourced from
//      /api/v3/blocklist plus the live `decision_logged` / `queue_progress`
//      frames that announce the next grab.
//   3. Scheduled tasks — the recurring scheduler jobs (MissingItemSearch /
//      RssSync / DiskSpaceCheck). These are real, but they are NOT downloads, so
//      they live in their own labelled section. The /api/v3/queue surface marks
//      them with `status: "scheduled"` / `protocol: "unknown"`.
//
// Live by default: an SSE subscription over /api/v1/stream pushes lifecycle
// transitions, and a low-frequency poll re-snapshots the queue + blocklist so
// terminal/late state (imports finishing, a blocklist clearing) reconciles even
// when a push frame is missed. Composed exclusively from vendored SRCL.

import * as React from 'react';

import AlertBanner from '@components/AlertBanner';
import Badge from '@components/Badge';
import BarLoader from '@components/BarLoader';
import BarProgress from '@components/BarProgress';
import Card from '@components/Card';
import Divider from '@components/Divider';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Table from '@components/Table';
import TableColumn from '@components/TableColumn';
import TableRow from '@components/TableRow';
import Text from '@components/Text';

import AppShell from '@app/_components/AppShell';
import { api, ApiError } from '@lib/api/client';
import type {
  BlocklistRecord,
  DomainEvent,
  Page,
  QueueRecord,
} from '@lib/api/types';

// --- live download lifecycle ------------------------------------------------

// The ordered grab lifecycle the daemon walks (crates/cellarr-core GrabStatus +
// the stream's `queue_progress.status`). Anything outside this set is shown
// verbatim but still treated as an in-flight download.
const LIFECYCLE = ['pending', 'grabbed', 'sent', 'downloading', 'completed', 'importing', 'imported'] as const;
const TERMINAL = new Set(['imported', 'failed', 'blocklisted']);

// A scheduler job (cron) rather than a real download — the v3 queue tags these.
function isScheduledTask(rec: QueueRecord): boolean {
  return rec.status === 'scheduled' || rec.protocol === 'unknown';
}

interface DownloadRow {
  id: string;
  title: string;
  status: string;
  progress?: number; // [0, 1]
  protocol?: string;
}

// One self-heal pairing: a blocklisted release and (optionally) the grab that
// recovered from it.
interface HealRow {
  id: string;
  title: string;
  reason?: string;
  indexer?: string;
  date?: number; // unix seconds
  recovery?: string; // a short note about the follow-up grab, if seen live
}

type StreamState = 'connecting' | 'live' | 'offline';

// v3 history/blocklist dates are unix seconds, not ISO; format locally.
function formatUnix(seconds?: number): string {
  if (seconds == null) return '—';
  const d = new Date(seconds * 1000);
  return Number.isNaN(d.getTime()) ? '—' : d.toLocaleString();
}

function lifecycleLabel(status: string): string {
  switch (status) {
    case 'grabbed':
    case 'sent':
      return 'grabbed';
    case 'downloading':
      return 'downloading';
    case 'completed':
    case 'importing':
      return 'importing';
    case 'imported':
      return 'imported';
    default:
      return status;
  }
}

export default function ActivityPage() {
  const [queue, setQueue] = React.useState<QueueRecord[] | null>(null);
  const [blocklist, setBlocklist] = React.useState<BlocklistRecord[]>([]);
  const [error, setError] = React.useState<string | null>(null);
  const [stream, setStream] = React.useState<StreamState>('connecting');

  // Live overlay of grab progress keyed by grab id, fed by the SSE stream. This
  // is the source of truth for the download lifecycle between queue snapshots.
  const [live, setLive] = React.useState<Record<string, DownloadRow>>({});
  // Live recovery notes keyed off the most recent grabbed/decision frame, so a
  // freshly-blocklisted release can show "next: <grab>".
  const [recovery, setRecovery] = React.useState<string[]>([]);
  const [lastImport, setLastImport] = React.useState<string | null>(null);

  // Poll the queue + blocklist snapshots. The SSE stream carries the fast-path
  // lifecycle; this reconciles terminal/late state and survives a missed frame.
  React.useEffect(() => {
    const handle = api.poll<[Page<QueueRecord>, Page<BlocklistRecord>]>(
      (signal) => Promise.all([api.getQueueV3(signal), api.getBlocklist(signal)]),
      {
        intervalMs: 8000,
        onData: ([q, b]) => {
          setQueue(q.records);
          setBlocklist(b.records);
          setError(null);
          // Drop live overlays for grabs that have left the queue and reached a
          // terminal lifecycle state, so the table doesn't keep stale rows.
          setLive((prev) => {
            const inQueue = new Set(q.records.map((r) => r.id));
            const next: Record<string, DownloadRow> = {};
            for (const [id, row] of Object.entries(prev)) {
              if (inQueue.has(id) || !TERMINAL.has(row.status)) next[id] = row;
            }
            return next;
          });
        },
        onError: (err) => {
          if (err instanceof ApiError && err.code === 'network_error') {
            setQueue((q) => q ?? []);
            return;
          }
          setError(err instanceof Error ? err.message : 'failed to load activity');
        },
      }
    );
    return () => handle.stop();
  }, []);

  // Live updates over SSE — the push path for lifecycle transitions. The per-type
  // `on` handlers receive the full DomainEvent union, so each narrows on `type`.
  React.useEffect(() => {
    const handle = api.openStream({
      onOpen: () => setStream('live'),
      onError: () => setStream('connecting'),
      on: {
        queue_progress: (ev) => {
          if (ev.type !== 'queue_progress') return;
          setLive((prev) => ({
            ...prev,
            [ev.grab_id]: {
              id: ev.grab_id,
              title: prev[ev.grab_id]?.title ?? ev.grab_id,
              status: ev.status,
              progress: ev.progress,
              protocol: prev[ev.grab_id]?.protocol,
            },
          }));
        },
        import_completed: (ev) => {
          if (ev.type === 'import_completed') setLastImport(ev.path);
        },
        decision_logged: (ev) => {
          if (ev.type === 'decision_logged') {
            setRecovery((prev) => [ev.note, ...prev].slice(0, 5));
          }
        },
      },
    });

    // openStream returns a no-op handle when EventSource is unavailable (SSR).
    if (typeof EventSource === 'undefined') setStream('offline');

    return () => handle.close();
  }, []);

  // --- derive the three sections --------------------------------------------

  const scheduledTasks = (queue ?? []).filter(isScheduledTask);
  const queuedDownloads = (queue ?? []).filter((r) => !isScheduledTask(r));

  // Merge the real-download queue rows with the live SSE overlay (overlay wins
  // on status/progress; live-only grabs appear too).
  const liveById = new Map(Object.values(live).map((r) => [r.id, r]));
  const downloads: DownloadRow[] = [];
  for (const rec of queuedDownloads) {
    const overlay = liveById.get(rec.id);
    downloads.push({
      id: rec.id,
      title: rec.title,
      status: overlay?.status ?? rec.status,
      progress: overlay?.progress ?? (rec.size && rec.sizeleft != null && rec.size > 0
        ? (rec.size - rec.sizeleft) / rec.size
        : undefined),
      protocol: rec.protocol,
    });
    liveById.delete(rec.id);
  }
  for (const row of liveById.values()) downloads.push(row);

  const healRows: HealRow[] = blocklist.map((b, i) => ({
    id: String(b.id ?? i),
    title: b.sourceTitle ?? 'blocklisted release',
    reason: b.message,
    indexer: b.indexer,
    date: typeof b.date === 'number' ? b.date : undefined,
  }));

  const streamBadge = (
    <Badge>
      {stream === 'live' ? 'live' : stream === 'connecting' ? 'connecting…' : 'offline'}
    </Badge>
  );

  const isLoading = queue === null && !error;

  return (
    <AppShell>
      <Card title="Activity">
        <RowSpaceBetween>
          <Text>Live downloads, self-heal recovery, and scheduled work.</Text>
          {streamBadge}
        </RowSpaceBetween>

        <Divider type="GRADIENT" />

        {error ? (
          <AlertBanner style={{ marginTop: '1ch' }}>
            Could not load activity: {error}
          </AlertBanner>
        ) : null}

        {isLoading ? (
          <div style={{ marginTop: '1ch' }}>
            <Text style={{ opacity: 0.6, marginBottom: '0.5ch' }}>Loading activity…</Text>
            <BarLoader intervalRate={600} />
          </div>
        ) : null}

        {!isLoading ? (
          <>
            {/* --- Downloads ------------------------------------------------ */}
            <Text style={{ opacity: 0.6, marginTop: '1ch', marginBottom: '0.5ch' }}>
              Downloads
            </Text>
            {downloads.length === 0 ? (
              <Text style={{ opacity: 0.6 }}>
                No active downloads. Grabs appear here as they move from grabbed
                through downloading and importing.
              </Text>
            ) : (
              downloads.map((row) => (
                <div key={row.id} style={{ marginBottom: '1ch' }}>
                  <RowSpaceBetween>
                    <Text>{row.title}</Text>
                    <Badge>{lifecycleLabel(row.status)}</Badge>
                  </RowSpaceBetween>
                  {row.progress != null ? (
                    <BarProgress progress={Math.round(row.progress * 100)} />
                  ) : TERMINAL.has(row.status) ? null : (
                    <BarLoader intervalRate={700} />
                  )}
                </div>
              ))
            )}

            {lastImport ? (
              <Text style={{ opacity: 0.6, marginTop: '0.5ch' }}>
                Last import: {lastImport}
              </Text>
            ) : null}

            <Divider type="GRADIENT" />

            {/* --- Self-heal ------------------------------------------------ */}
            <Text style={{ opacity: 0.6, marginBottom: '0.5ch' }}>
              Self-heal · blocklisted releases
            </Text>
            {healRows.length === 0 ? (
              <Text style={{ opacity: 0.6 }}>
                Nothing blocklisted. When a download fails, the daemon walls off
                the bad release here and grabs the next candidate.
              </Text>
            ) : (
              <Table>
                <TableRow>
                  <TableColumn style={{ opacity: 0.6 }}>When</TableColumn>
                  <TableColumn style={{ opacity: 0.6 }}>Release</TableColumn>
                  <TableColumn style={{ opacity: 0.6 }}>Indexer</TableColumn>
                  <TableColumn style={{ opacity: 0.6 }}>Reason</TableColumn>
                </TableRow>
                {healRows.map((row) => (
                  <TableRow key={row.id}>
                    <TableColumn style={{ whiteSpace: 'nowrap' }}>
                      {formatUnix(row.date)}
                    </TableColumn>
                    <TableColumn>{row.title}</TableColumn>
                    <TableColumn>{row.indexer || '—'}</TableColumn>
                    <TableColumn>
                      <Badge>blocklisted</Badge> {row.reason || ''}
                    </TableColumn>
                  </TableRow>
                ))}
              </Table>
            )}

            {recovery.length > 0 ? (
              <div style={{ marginTop: '0.5ch' }}>
                <Text style={{ opacity: 0.6, marginBottom: '0.25ch' }}>
                  Recent recovery decisions
                </Text>
                {recovery.map((note, i) => (
                  <Text key={i} style={{ opacity: 0.8 }}>
                    · {note}
                  </Text>
                ))}
              </div>
            ) : null}

            <Divider type="GRADIENT" />

            {/* --- Scheduled tasks ----------------------------------------- */}
            <Text style={{ opacity: 0.6, marginBottom: '0.5ch' }}>
              Scheduled tasks
            </Text>
            {scheduledTasks.length === 0 ? (
              <Text style={{ opacity: 0.6 }}>No scheduled tasks registered.</Text>
            ) : (
              <Table>
                <TableRow>
                  <TableColumn style={{ opacity: 0.6 }}>Task</TableColumn>
                  <TableColumn style={{ opacity: 0.6 }}>State</TableColumn>
                </TableRow>
                {scheduledTasks.map((task) => (
                  <TableRow key={task.id}>
                    <TableColumn>{task.title}</TableColumn>
                    <TableColumn>
                      <Badge>{task.status}</Badge>
                    </TableColumn>
                  </TableRow>
                ))}
              </Table>
            )}
          </>
        ) : null}
      </Card>
    </AppShell>
  );
}
