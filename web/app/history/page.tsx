'use client';

// The History screen — the append-only "what happened" stream for a content
// node: grabs, completions, failures, imports, upgrades, deletes, holds. Each
// row carries the pipeline run that produced it and links into the Decision-log
// screen ("why"). The daemon indexes history per content node (no global scan),
// so the screen queries by content id. Composed exclusively from vendored SRCL
// primitives + the typed API client.

import * as React from 'react';
import Link from 'next/link';

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
import { api, ApiError } from '@lib/api/client';
import type { HistoryRecord } from '@lib/api/types';
import {
  asHistoryRecord,
  formatTimestamp,
  historyEventLabel,
} from '@app/_lib/decisionlog';
import type { TypedHistoryRecord } from '@app/_lib/decisionlog';

type LoadState = 'idle' | 'loading' | 'loaded' | 'error';

/** Human detail pulled out of the (event-tagged) history event payload. */
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

const HistoryTable: React.FC<{ records: TypedHistoryRecord[] }> = ({ records }) => (
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
          {rec.run_id ? (
            <Link
              href={`/decision-log?run=${encodeURIComponent(rec.run_id)}`}
              style={{ textDecoration: 'none' }}
              title={`Open the decision log for run ${rec.run_id}`}
            >
              <Badge>why · {shortId(rec.run_id)}</Badge>
            </Link>
          ) : (
            '—'
          )}
        </TableColumn>
      </TableRow>
    ))}
  </Table>
);

export default function HistoryPage() {
  const [contentInput, setContentInput] = React.useState<string>('');
  const [activeContent, setActiveContent] = React.useState<string>('');
  const [records, setRecords] = React.useState<TypedHistoryRecord[]>([]);
  const [state, setState] = React.useState<LoadState>('idle');
  const [error, setError] = React.useState<string | null>(null);
  const [reloadNonce, setReloadNonce] = React.useState(0);

  React.useEffect(() => {
    if (!activeContent) {
      setState('idle');
      setRecords([]);
      return;
    }
    const controller = new AbortController();
    setState('loading');
    setError(null);
    api
      .getHistory(activeContent, controller.signal)
      .then((raw: HistoryRecord[]) => {
        setRecords(raw.map(asHistoryRecord));
        setState('loaded');
      })
      .catch((err: unknown) => {
        if (err instanceof ApiError && err.code === 'network_error' && controller.signal.aborted) return;
        setError(err instanceof Error ? err.message : 'failed to load history');
        setState('error');
      });
    return () => controller.abort();
  }, [activeContent, reloadNonce]);

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    setActiveContent(contentInput.trim());
  };

  return (
    <AppShell>
      <Card title="History">
        <Text style={{ opacity: 0.7 }}>
          The append-only record of everything that happened to a content node — grabs, downloads,
          imports, upgrades, and deletions. Each event links to the decision log for the run that
          produced it. History is indexed per content node, so pick one to view its timeline.
        </Text>

        <form onSubmit={submit} style={{ marginTop: '1ch' }}>
          <RowSpaceBetween style={{ gap: '1ch', alignItems: 'flex-end' }}>
            <div style={{ flex: 1 }}>
              <Input
                label="Content id"
                name="content"
                placeholder="content node uuid"
                value={contentInput}
                onChange={(e: React.ChangeEvent<HTMLInputElement>) => setContentInput(e.target.value)}
              />
            </div>
            <Button type="submit" isDisabled={!contentInput.trim()}>
              Load
            </Button>
          </RowSpaceBetween>
        </form>
      </Card>

      <div style={{ marginTop: '2ch' }}>
        {state === 'idle' ? (
          <Card title="No content selected">
            <Text>Enter a content node id above to view its history timeline.</Text>
          </Card>
        ) : null}

        {state === 'loading' ? (
          <Card title="Loading">
            <Row style={{ gap: '1ch', alignItems: 'center' }}>
              <BlockLoader mode={1} />
              <Text>Loading history for {activeContent}…</Text>
            </Row>
          </Card>
        ) : null}

        {state === 'error' ? (
          <Card title="Could not load">
            <AlertBanner>Failed to load history: {error}</AlertBanner>
            <Row style={{ marginTop: '1ch' }}>
              <Button theme="SECONDARY" onClick={() => setReloadNonce((n) => n + 1)}>
                Retry
              </Button>
            </Row>
          </Card>
        ) : null}

        {state === 'loaded' && records.length === 0 ? (
          <Card title="No history">
            <Text>No events recorded for this content node yet.</Text>
          </Card>
        ) : null}

        {state === 'loaded' && records.length > 0 ? (
          <Card title={`${records.length} events`}>
            <HistoryTable records={records} />
          </Card>
        ) : null}
      </div>
    </AppShell>
  );
}
