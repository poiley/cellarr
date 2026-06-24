'use client';

// The History screen — the append-only "what happened" stream.
//
// Two modes, both reached without ever pasting a raw uuid:
//
//   1. The GLOBAL recent feed (default): a paged stream of the most recent
//      events across every content node, read from `GET /api/v3/history`. This
//      is what you see when you open the screen — no id required.
//   2. A single node's TIMELINE (`?id=<contentNodeUuid>`): the full per-node
//      history from the native `GET /api/v1/history?content=…`, which carries
//      the pipeline run that produced each event. Deep-linkable, and reachable
//      from a row in the global feed or via the manual "node id" field.
//
// Every row that came from a pipeline run links to /decision-log?run=<runId>
// ("why this happened"). Composed exclusively from vendored SRCL primitives +
// the typed API client.

import * as React from 'react';
import Link from 'next/link';
import { useRouter, useSearchParams } from 'next/navigation';

import Accordion from '@components/Accordion';
import AlertBanner from '@components/AlertBanner';
import Badge from '@components/Badge';
import BlockLoader from '@components/BlockLoader';
import Button from '@components/Button';
import Card from '@components/Card';
import Input from '@components/Input';
import Row from '@components/Row';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Table from '@components/Table';
import TableColumn from '@components/TableColumn';
import TableRow from '@components/TableRow';
import Text from '@components/Text';

import AppShell from '@app/_components/AppShell';
import { useToast } from '@app/_lib/ToastProvider';
import { api, ApiError } from '@lib/api/client';
import type { HistoryRecord, HistoryRecordV3, Page } from '@lib/api/types';
import {
  asGlobalHistoryRow,
  asHistoryRecord,
  formatHistoryDate,
  formatTimestamp,
  historyEventLabel,
  v3EventLabel,
} from '@app/_lib/decisionlog';
import type { TypedGlobalHistoryRow, TypedHistoryRecord } from '@app/_lib/decisionlog';

type LoadState = 'idle' | 'loading' | 'loaded' | 'error';

/** A run badge that links into the decision log, or an em-dash when absent. */
const RunLink: React.FC<{ runId?: string }> = ({ runId }) => {
  if (!runId) return <>—</>;
  return (
    <Link
      href={`/decision-log?run=${encodeURIComponent(runId)}`}
      style={{ textDecoration: 'none' }}
      title={`Open the decision log for run ${runId}`}
    >
      <Badge>why · {shortId(runId)}</Badge>
    </Link>
  );
};

/** Human detail pulled out of the (event-tagged) native history event payload. */
function eventDetail(rec: TypedHistoryRecord): string {
  const e = rec.event as Record<string, unknown>;
  if (typeof e.detail === 'string') return e.detail;
  if (typeof e.reason === 'string') return e.reason;
  if (typeof e.grab_id === 'string') return `grab ${e.grab_id}`;
  return '';
}

function shortId(id: string): string {
  return id.length > 12 ? `${id.slice(0, 8)}…${id.slice(-4)}` : id;
}

// --- the global recent feed (default view) ---------------------------------

const GlobalFeedTable: React.FC<{
  rows: TypedGlobalHistoryRow[];
  onOpenNode: (contentId: string) => void;
}> = ({ rows, onOpenNode }) => (
  <Table>
    <TableRow>
      <TableColumn style={{ opacity: 0.6 }}>When</TableColumn>
      <TableColumn style={{ opacity: 0.6 }}>Event</TableColumn>
      <TableColumn style={{ opacity: 0.6 }}>Title</TableColumn>
      <TableColumn style={{ opacity: 0.6 }}>Node</TableColumn>
      <TableColumn style={{ opacity: 0.6 }}>Run</TableColumn>
    </TableRow>
    {rows.map((row, i) => (
      <TableRow key={`${String(row.date)}-${i}`}>
        <TableColumn style={{ whiteSpace: 'nowrap' }}>{formatHistoryDate(row.date)}</TableColumn>
        <TableColumn>
          <Badge>{v3EventLabel(row.eventType)}</Badge>
        </TableColumn>
        <TableColumn>{row.sourceTitle || '—'}</TableColumn>
        <TableColumn>
          {row.contentId ? (
            <Button theme="SECONDARY" onClick={() => onOpenNode(row.contentId as string)}>
              {shortId(row.contentId)}
            </Button>
          ) : (
            '—'
          )}
        </TableColumn>
        <TableColumn>
          <RunLink runId={row.runId} />
        </TableColumn>
      </TableRow>
    ))}
  </Table>
);

// --- a single node's timeline (?id=) ---------------------------------------

const NodeTimelineTable: React.FC<{ records: TypedHistoryRecord[] }> = ({ records }) => (
  <Table>
    <TableRow>
      <TableColumn style={{ opacity: 0.6 }}>When</TableColumn>
      <TableColumn style={{ opacity: 0.6 }}>Event</TableColumn>
      <TableColumn style={{ opacity: 0.6 }}>Detail</TableColumn>
      <TableColumn style={{ opacity: 0.6 }}>Run</TableColumn>
    </TableRow>
    {records.map((rec, i) => (
      <TableRow key={`${rec.at}-${i}`}>
        <TableColumn style={{ whiteSpace: 'nowrap' }}>{formatTimestamp(rec.at)}</TableColumn>
        <TableColumn>
          <Badge>{historyEventLabel(rec.event.event)}</Badge>
        </TableColumn>
        <TableColumn>{eventDetail(rec) || '—'}</TableColumn>
        <TableColumn>
          <RunLink runId={rec.run_id} />
        </TableColumn>
      </TableRow>
    ))}
  </Table>
);

function HistoryScreen() {
  const router = useRouter();
  const params = useSearchParams();
  const { info } = useToast();
  // The active node is driven by the URL (`?id=`), so deep links and the global
  // feed's "open node" buttons share one source of truth.
  const activeNode = params?.get('id')?.trim() ?? '';

  // --- global recent feed ---------------------------------------------------
  const [feed, setFeed] = React.useState<TypedGlobalHistoryRow[]>([]);
  const [feedState, setFeedState] = React.useState<LoadState>('loading');
  const [feedError, setFeedError] = React.useState<string | null>(null);

  // --- per-node timeline ----------------------------------------------------
  const [records, setRecords] = React.useState<TypedHistoryRecord[]>([]);
  const [nodeState, setNodeState] = React.useState<LoadState>('idle');
  const [nodeError, setNodeError] = React.useState<string | null>(null);

  const [nodeInput, setNodeInput] = React.useState<string>('');
  const [reloadNonce, setReloadNonce] = React.useState(0);

  // Load the global feed whenever we are in the default view.
  React.useEffect(() => {
    if (activeNode) return;
    const controller = new AbortController();
    setFeedState('loading');
    setFeedError(null);
    api
      .getHistoryV3(controller.signal)
      .then((page: Page<HistoryRecordV3>) => {
        setFeed(page.records.map(asGlobalHistoryRow));
        setFeedState('loaded');
      })
      .catch((err: unknown) => {
        if (err instanceof ApiError && err.code === 'network_error' && controller.signal.aborted) return;
        setFeedError(err instanceof Error ? err.message : 'failed to load history');
        setFeedState('error');
      });
    return () => controller.abort();
  }, [activeNode, reloadNonce]);

  // Load a single node's timeline whenever `?id=` is present.
  React.useEffect(() => {
    if (!activeNode) {
      setNodeState('idle');
      setRecords([]);
      return;
    }
    const controller = new AbortController();
    setNodeState('loading');
    setNodeError(null);
    api
      .getHistory(activeNode, controller.signal)
      .then((raw: HistoryRecord[]) => {
        setRecords(raw.map(asHistoryRecord));
        setNodeState('loaded');
      })
      .catch((err: unknown) => {
        if (err instanceof ApiError && err.code === 'network_error' && controller.signal.aborted) return;
        setNodeError(err instanceof Error ? err.message : 'failed to load history');
        setNodeState('error');
      });
    return () => controller.abort();
  }, [activeNode, reloadNonce]);

  // Keep the manual field in sync with the active node from the URL.
  React.useEffect(() => {
    setNodeInput(activeNode);
  }, [activeNode]);

  const openNode = React.useCallback(
    (id: string) => {
      const trimmed = id.trim();
      router.push(trimmed ? `/history?id=${encodeURIComponent(trimmed)}` : '/history');
    },
    [router]
  );

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = nodeInput.trim();
    if (!trimmed) return;
    info(`Opening timeline for ${shortId(trimmed)}`);
    openNode(trimmed);
  };

  return (
    <AppShell>
      <Card title="History">
        <Text style={{ opacity: 0.7 }}>
          The append-only record of everything that happened — grabs, downloads, imports, upgrades,
          and deletions. The recent feed below spans every content node; each event links to the
          decision log for the run that produced it. Open a node to see its full timeline.
        </Text>

        {activeNode ? (
          <Row style={{ marginTop: '1ch' }}>
            <Button theme="SECONDARY" onClick={() => openNode('')}>
              ▸ Back to recent feed
            </Button>
          </Row>
        ) : null}

        {/* The raw content-node id is a power-user entry point — normal users
            open a node from a row in the feed, never by pasting a uuid. Keep it
            out of the way behind an SRCL disclosure. */}
        <div style={{ marginTop: '1ch' }}>
          <Accordion title="Advanced — open a node by id" defaultValue={Boolean(activeNode)}>
            <form onSubmit={submit} style={{ width: '100%' }}>
              <RowSpaceBetween style={{ gap: '1ch', alignItems: 'flex-end', width: '100%' }}>
                <div style={{ flex: 1 }}>
                  <Input
                    label="Content node id"
                    name="node"
                    placeholder="paste a node id to view its timeline"
                    value={nodeInput}
                    onChange={(e: React.ChangeEvent<HTMLInputElement>) => setNodeInput(e.target.value)}
                  />
                </div>
                <Button type="submit" isDisabled={!nodeInput.trim()}>
                  Open node
                </Button>
              </RowSpaceBetween>
            </form>
          </Accordion>
        </div>
      </Card>

      <div style={{ marginTop: '2ch' }}>
        {activeNode ? (
          <NodeTimelineView
            node={activeNode}
            state={nodeState}
            error={nodeError}
            records={records}
            onRetry={() => setReloadNonce((n) => n + 1)}
          />
        ) : (
          <GlobalFeedView
            state={feedState}
            error={feedError}
            rows={feed}
            onOpenNode={openNode}
            onRetry={() => setReloadNonce((n) => n + 1)}
          />
        )}
      </div>
    </AppShell>
  );
}

// --- view sections ---------------------------------------------------------

const GlobalFeedView: React.FC<{
  state: LoadState;
  error: string | null;
  rows: TypedGlobalHistoryRow[];
  onOpenNode: (id: string) => void;
  onRetry: () => void;
}> = ({ state, error, rows, onOpenNode, onRetry }) => {
  if (state === 'loading') {
    return (
      <Card title="Loading">
        <Row style={{ gap: '1ch', alignItems: 'center' }}>
          <BlockLoader mode={1} />
          <Text>Loading recent history…</Text>
        </Row>
      </Card>
    );
  }
  if (state === 'error') {
    return (
      <Card title="Could not load">
        <AlertBanner>Failed to load history: {error}</AlertBanner>
        <Row style={{ marginTop: '1ch' }}>
          <Button theme="SECONDARY" onClick={onRetry}>
            Retry
          </Button>
        </Row>
      </Card>
    );
  }
  if (rows.length === 0) {
    return (
      <Card title="No history yet">
        <Text>
          No events have been recorded. As the daemon grabs, imports, and upgrades content, the
          activity shows up here.
        </Text>
      </Card>
    );
  }
  return (
    <Card title={`Recent activity · ${rows.length} events`}>
      <GlobalFeedTable rows={rows} onOpenNode={onOpenNode} />
    </Card>
  );
};

const NodeTimelineView: React.FC<{
  node: string;
  state: LoadState;
  error: string | null;
  records: TypedHistoryRecord[];
  onRetry: () => void;
}> = ({ node, state, error, records, onRetry }) => {
  if (state === 'loading') {
    return (
      <Card title="Loading">
        <Row style={{ gap: '1ch', alignItems: 'center' }}>
          <BlockLoader mode={1} />
          <Text>Loading history for {node}…</Text>
        </Row>
      </Card>
    );
  }
  if (state === 'error') {
    return (
      <Card title="Could not load">
        <AlertBanner>Failed to load history: {error}</AlertBanner>
        <Row style={{ marginTop: '1ch' }}>
          <Button theme="SECONDARY" onClick={onRetry}>
            Retry
          </Button>
        </Row>
      </Card>
    );
  }
  if (records.length === 0) {
    return (
      <Card title="No history">
        <Text>No events recorded for this content node yet.</Text>
      </Card>
    );
  }
  return (
    <Card title={`${records.length} events`}>
      <NodeTimelineTable records={records} />
    </Card>
  );
};

export default function HistoryPage() {
  return (
    <React.Suspense
      fallback={
        <AppShell>
          <Card title="History">
            <Text>Loading…</Text>
          </Card>
        </AppShell>
      }
    >
      <HistoryScreen />
    </React.Suspense>
  );
}
