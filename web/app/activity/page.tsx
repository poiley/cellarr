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
//      RssSync / DiskSpaceCheck), read from the dedicated /api/v3/system/task
//      surface so each row shows a real next-run countdown, last run, last
//      status, and a "Run now" action (POST /api/v3/command). These are NOT
//      downloads, so they live in their own labelled section; any cron rows the
//      /api/v3/queue still tags with `status:"scheduled"`/`protocol:"unknown"`
//      are recognised only to keep them OUT of the download list.
//
// Live by default: an SSE subscription over /api/v1/stream pushes lifecycle
// transitions, and a low-frequency poll re-snapshots the queue + blocklist so
// terminal/late state (imports finishing, a blocklist clearing) reconciles even
// when a push frame is missed. Composed exclusively from vendored SRCL.

import * as React from 'react';

import AlertBanner from '@components/AlertBanner';
import Badge from '@components/Badge';
import BarLoader from '@components/BarLoader';

import StatusBadge from '@app/_components/StatusBadge';
import BarProgress from '@components/BarProgress';
import Button from '@components/Button';
import Card from '@components/Card';
import Divider from '@components/Divider';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Table from '@components/Table';
import TableColumn from '@components/TableColumn';
import TableRow from '@components/TableRow';
import Text from '@components/Text';

import AppShell from '@app/_components/AppShell';
import QueueActions from '@app/activity/_components/QueueActions';
import {
  formatCountdown,
  formatIso,
  getSystemTasks,
  lastStatusGlyph,
  runTaskNow,
  type SystemTaskV3,
} from '@app/_lib/activity';
import { toneFor } from '@app/_lib/status';
import { useToast } from '@app/_lib/ToastProvider';
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
  /** The backing queue record (present for real queue rows; absent for live-only
   *  SSE overlays that have not yet been re-snapshotted). Carries the ids the
   *  remove / manual-import / change-category actions need. */
  record?: QueueRecord;
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

// The live-stream connection lifecycle, surfaced verbatim in the header badge:
//   connecting   — opening the SSE socket, not yet confirmed open
//   live         — socket open, frames flowing
//   disconnected — socket dropped after having been live (browser is retrying)
//   offline      — no EventSource at all (SSR / unsupported runtime)
type StreamState = 'connecting' | 'live' | 'disconnected' | 'offline';

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
  const { success, error: toastError, info } = useToast();
  const [queue, setQueue] = React.useState<QueueRecord[] | null>(null);
  const [blocklist, setBlocklist] = React.useState<BlocklistRecord[]>([]);
  const [tasks, setTasks] = React.useState<SystemTaskV3[] | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [stream, setStream] = React.useState<StreamState>('connecting');
  // Tasks currently mid "Run now" POST, keyed by task id (string), so each row's
  // button can disable itself without blocking the others.
  const [running, setRunning] = React.useState<Set<string>>(new Set());
  // A ticking clock so the next-run countdowns re-render once a second without a
  // network round-trip; the task list itself is re-polled far less often.
  const [nowMs, setNowMs] = React.useState(() => Date.now());

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

  // An on-demand re-snapshot of the queue + blocklist, fired after a queue action
  // (remove / manual-import / change-category) so the row reflects the mutation
  // immediately rather than waiting for the next 8s poll tick.
  const refreshQueue = React.useCallback(async () => {
    try {
      const [q, b] = await Promise.all([api.getQueueV3(), api.getBlocklist()]);
      setQueue(q.records);
      setBlocklist(b.records);
    } catch {
      // Non-fatal: the background poll will reconcile on its next tick.
    }
  }, []);

  // Live updates over SSE — the push path for lifecycle transitions. The per-type
  // `on` handlers receive the full DomainEvent union, so each narrows on `type`.
  React.useEffect(() => {
    // EventSource fires `error` both before the first open (still connecting) and
    // whenever an established socket drops (it then auto-retries). Reflect the
    // REAL transport state: only claim "live" on a confirmed open; show
    // "disconnected" when a previously-open stream drops, "connecting" otherwise.
    let everOpen = false;
    const handle = api.openStream({
      onOpen: () => {
        everOpen = true;
        setStream('live');
      },
      onError: () => setStream(everOpen ? 'disconnected' : 'connecting'),
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

  // Poll the real scheduler tasks (`/api/v3/system/task`) for next-run, last-run
  // and last-status. Slower cadence than the live channel — the per-second
  // countdown ticker below keeps the displayed times fresh between polls.
  React.useEffect(() => {
    const handle = api.poll<SystemTaskV3[]>((signal) => getSystemTasks(signal), {
      intervalMs: 15000,
      onData: (rows) => setTasks(rows ?? []),
      onError: (err) => {
        // A missing task surface shouldn't blank the whole screen; degrade to an
        // empty task list (the section renders its own one-line empty state).
        if (err instanceof ApiError) setTasks((t) => t ?? []);
      },
    });
    return () => handle.stop();
  }, []);

  // Re-render countdowns once a second without re-fetching.
  React.useEffect(() => {
    const id = setInterval(() => setNowMs(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  // --- run a scheduled task on demand ---------------------------------------

  const onRunTask = React.useCallback(
    async (task: SystemTaskV3) => {
      const key = String(task.id);
      setRunning((prev) => new Set(prev).add(key));
      info(`Queuing ${task.name}…`);
      try {
        await runTaskNow(task.taskName);
        success(`${task.name} queued`);
        // Refresh tasks so next/last-run reflects the just-triggered command.
        try {
          setTasks(await getSystemTasks());
        } catch {
          // Non-fatal: the 15s poll will reconcile shortly.
        }
      } catch (err) {
        const message =
          err instanceof ApiError ? err.message : err instanceof Error ? err.message : 'unknown error';
        toastError(`Could not run ${task.name}: ${message}`);
      } finally {
        setRunning((prev) => {
          const next = new Set(prev);
          next.delete(key);
          return next;
        });
      }
    },
    [info, success, toastError]
  );

  // --- derive the three sections --------------------------------------------

  // Scheduler jobs now come from the dedicated /system/task surface (real
  // next/last-run + status). The legacy queue-tagged cron rows are still
  // recognised and excluded from the download list so they never masquerade as
  // downloads, but they are no longer the source for the Scheduled section.
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
      record: rec,
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

  // The blocklist surface frequently carries no indexer, leaving the Indexer
  // column a wall of "—". Only render that column when at least one row actually
  // names an indexer; otherwise drop the header + cells entirely.
  const hasIndexer = healRows.some((row) => {
    const v = row.indexer?.trim();
    return !!v && v !== '—';
  });

  // The badge reflects the REAL SSE transport state — it only says "live" when
  // the socket is confirmed open, and switches to "disconnected" if it drops.
  const STREAM_LABEL: Record<StreamState, string> = {
    live: '● live',
    connecting: '● connecting…',
    disconnected: '✗ disconnected',
    offline: '✗ offline',
  };
  const streamBadge = <Badge>{STREAM_LABEL[stream]}</Badge>;

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
              <Text style={{ opacity: 0.6 }}>No active downloads.</Text>
            ) : (
              downloads.map((row) => (
                <div key={row.id} style={{ marginBottom: '1ch' }}>
                  <RowSpaceBetween>
                    <Text>{row.title}</Text>
                    <span style={{ display: 'inline-flex', gap: '1ch', alignItems: 'center' }}>
                      {row.record?.timeleft && !TERMINAL.has(row.status) ? (
                        <Text style={{ opacity: 0.6 }}>ETA {row.record.timeleft}</Text>
                      ) : null}
                      <StatusBadge status={lifecycleLabel(row.status)} />
                      {row.record ? (
                        <QueueActions record={row.record} onChanged={refreshQueue} />
                      ) : null}
                    </span>
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
              <Text style={{ opacity: 0.6 }}>Nothing blocklisted.</Text>
            ) : (
              <Table>
                <TableRow>
                  <TableColumn style={{ opacity: 0.6 }}>When</TableColumn>
                  <TableColumn style={{ opacity: 0.6 }}>Release</TableColumn>
                  {hasIndexer ? (
                    <TableColumn style={{ opacity: 0.6 }}>Indexer</TableColumn>
                  ) : null}
                  <TableColumn style={{ opacity: 0.6 }}>Reason</TableColumn>
                </TableRow>
                {healRows.map((row) => (
                  <TableRow key={row.id}>
                    <TableColumn style={{ whiteSpace: 'nowrap' }}>
                      {formatUnix(row.date)}
                    </TableColumn>
                    <TableColumn>{row.title}</TableColumn>
                    {hasIndexer ? (
                      <TableColumn>{row.indexer || '—'}</TableColumn>
                    ) : null}
                    <TableColumn>
                      <StatusBadge status="blocklisted" /> {row.reason || ''}
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
            {tasks === null ? (
              <BarLoader intervalRate={700} />
            ) : tasks.length === 0 ? (
              <Text style={{ opacity: 0.6 }}>No scheduled tasks registered.</Text>
            ) : (
              <Table>
                <TableRow>
                  <TableColumn style={{ opacity: 0.6 }}>Task</TableColumn>
                  <TableColumn style={{ opacity: 0.6 }}>Next run</TableColumn>
                  <TableColumn style={{ opacity: 0.6 }}>Last run</TableColumn>
                  <TableColumn style={{ opacity: 0.6 }}>Status &amp; action</TableColumn>
                </TableRow>
                {tasks.map((task) => {
                  const key = String(task.id);
                  const isRunning = running.has(key);
                  const last = lastStatusGlyph(task.lastStatus);
                  return (
                    <TableRow key={key}>
                      <TableColumn>{task.name}</TableColumn>
                      <TableColumn style={{ whiteSpace: 'nowrap' }}>
                        {formatCountdown(task.nextExecution, nowMs)}
                      </TableColumn>
                      <TableColumn style={{ whiteSpace: 'nowrap' }}>
                        {formatIso(task.lastExecution)}
                      </TableColumn>
                      <TableColumn style={{ whiteSpace: 'nowrap' }}>
                        {/* Status chip + action read together so the button is
                            adjacent to its row rather than stranded in a far
                            column. The chip is tone-coloured (QUEUED → blue) via
                            StatusBadge; the glyph + label text is preserved. */}
                        <span
                          style={{
                            display: 'inline-flex',
                            gap: '1ch',
                            alignItems: 'center',
                          }}
                        >
                          <StatusBadge
                            status={`${last.glyph} ${last.label}`}
                            tone={toneFor(task.lastStatus)}
                          />
                          <Button
                            theme="SECONDARY"
                            isDisabled={isRunning}
                            onClick={() => onRunTask(task)}
                          >
                            {isRunning ? 'Running…' : 'Run now'}
                          </Button>
                        </span>
                      </TableColumn>
                    </TableRow>
                  );
                })}
              </Table>
            )}
          </>
        ) : null}
      </Card>
    </AppShell>
  );
}
