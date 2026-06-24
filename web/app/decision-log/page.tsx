'use client';

// The Decision-log screen — cellarr's signature feature. For a pipeline run it
// shows the full ordered trail of decision-log records; each row expands
// (Accordion) to reveal the parsed fields, the CF-score breakdown, and the
// on-disk comparison that produced the verdict (DecisionDetail). Composed
// exclusively from vendored SRCL primitives + the typed API client.

import * as React from 'react';
import Link from 'next/link';
import { useSearchParams } from 'next/navigation';

import Accordion from '@components/Accordion';
import AlertBanner from '@components/AlertBanner';
import Badge from '@components/Badge';
import BlockLoader from '@components/BlockLoader';
import Button from '@components/Button';
import Card from '@components/Card';
import Divider from '@components/Divider';
import Input from '@components/Input';
import Row from '@components/Row';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Text from '@components/Text';

import AppShell from '@app/_components/AppShell';
import { useToast } from '@app/_lib/ToastProvider';
import DecisionDetail from '@app/decision-log/_components/DecisionDetail';
import { api, ApiError } from '@lib/api/client';
import type { DecisionLogRecord } from '@lib/api/types';
import {
  asDecisionRecord,
  formatTimestamp,
  transitionKindLabel,
  transitionLabel,
  verdictSummary,
} from '@app/_lib/decisionlog';
import type { TypedDecisionRecord, VerdictKind } from '@app/_lib/decisionlog';

type LoadState = 'idle' | 'loading' | 'loaded' | 'error';

const VERDICT_PILL: Record<VerdictKind, string> = {
  grab: 'GRAB',
  upgrade: 'UPGRADE',
  reject: 'REJECT',
};

function recordTitle(rec: TypedDecisionRecord): string {
  const head = `${transitionLabel(rec.transition)} · ${transitionKindLabel(rec.transition.kind)}`;
  if (rec.decision) return `${head} — ${verdictSummary(rec.decision.verdict)}`;
  if (rec.note) return `${head} — ${rec.note}`;
  return head;
}

const RecordAccordion: React.FC<{ record: TypedDecisionRecord; index: number }> = ({ record, index }) => {
  const verdict = record.decision?.verdict.verdict;
  return (
    <div style={{ marginBottom: '0.5ch' }}>
      <RowSpaceBetween style={{ gap: '1ch', alignItems: 'baseline' }}>
        <Text style={{ opacity: 0.5, minWidth: '4ch' }}>#{index + 1}</Text>
        <div style={{ flex: 1, minWidth: 0 }}>
          <Accordion title={recordTitle(record)} defaultValue={index === 0}>
            <DecisionDetail record={record} />
          </Accordion>
        </div>
        <Row style={{ gap: '1ch', alignItems: 'center' }}>
          {verdict ? <Badge>{VERDICT_PILL[verdict]}</Badge> : <Badge>{record.transition.kind.toUpperCase()}</Badge>}
          <Text style={{ opacity: 0.6, whiteSpace: 'nowrap' }}>{formatTimestamp(record.at)}</Text>
        </Row>
      </RowSpaceBetween>
    </div>
  );
};

function Summary({ records }: { records: TypedDecisionRecord[] }) {
  const counts = { grab: 0, upgrade: 0, reject: 0, other: 0 };
  for (const r of records) {
    const v = r.decision?.verdict.verdict;
    if (v === 'grab') counts.grab += 1;
    else if (v === 'upgrade') counts.upgrade += 1;
    else if (v === 'reject') counts.reject += 1;
    else counts.other += 1;
  }
  return (
    <Row style={{ gap: '1ch', flexWrap: 'wrap' }}>
      <Badge>{records.length} records</Badge>
      {counts.grab ? <Badge>{counts.grab} grab</Badge> : null}
      {counts.upgrade ? <Badge>{counts.upgrade} upgrade</Badge> : null}
      {counts.reject ? <Badge>{counts.reject} reject</Badge> : null}
      {counts.other ? <Badge>{counts.other} transitions</Badge> : null}
    </Row>
  );
}

function shortId(id: string): string {
  return id.length > 12 ? `${id.slice(0, 8)}…${id.slice(-4)}` : id;
}

function DecisionLogScreen() {
  const params = useSearchParams();
  const { info } = useToast();
  const initialRun = params?.get('run') ?? '';

  const [runInput, setRunInput] = React.useState<string>(initialRun);
  const [activeRun, setActiveRun] = React.useState<string>(initialRun);
  const [records, setRecords] = React.useState<TypedDecisionRecord[]>([]);
  const [state, setState] = React.useState<LoadState>(initialRun ? 'loading' : 'idle');
  const [error, setError] = React.useState<string | null>(null);
  const [reloadNonce, setReloadNonce] = React.useState(0);

  React.useEffect(() => {
    if (!activeRun) {
      setState('idle');
      setRecords([]);
      return;
    }
    const controller = new AbortController();
    setState('loading');
    setError(null);
    api
      .getDecisionLog(activeRun, controller.signal)
      .then((raw: DecisionLogRecord[]) => {
        setRecords(raw.map(asDecisionRecord));
        setState('loaded');
      })
      .catch((err: unknown) => {
        if (err instanceof ApiError && err.code === 'network_error') {
          // Aborted or offline — leave the prior state untouched on abort.
          if (controller.signal.aborted) return;
        }
        setError(err instanceof Error ? err.message : 'failed to load the decision log');
        setState('error');
      });
    return () => controller.abort();
  }, [activeRun, reloadNonce]);

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = runInput.trim();
    if (!trimmed) return;
    info(`Loading decision log for run ${shortId(trimmed)}`);
    setActiveRun(trimmed);
  };

  return (
    <AppShell>
      <Card title="Decision log">
        <Text style={{ opacity: 0.7 }}>
          Why cellarr grabbed, upgraded, or rejected each candidate in a pipeline run. Expand any
          record to see the parsed fields, the custom-format score breakdown, and the on-disk
          comparison behind the verdict. Open a decision log from a row in{' '}
          <Link href="/history" style={{ textDecoration: 'underline' }}>
            History
          </Link>{' '}
          — every event that came from a run carries a “why · …” link straight to its trail.
        </Text>

        {/* Pasting a raw run uuid is a power-user path — the primary entry is the
            "why · …" deep-link from History. Keep the manual field behind an SRCL
            disclosure, opened by default when a run is already in the URL. */}
        <div style={{ marginTop: '1ch' }}>
          <Accordion title="Advanced — load a run by id" defaultValue={Boolean(initialRun)}>
            <form onSubmit={submit} style={{ width: '100%' }}>
              <RowSpaceBetween style={{ gap: '1ch', alignItems: 'flex-end', width: '100%' }}>
                <div style={{ flex: 1 }}>
                  <Input
                    label="Run id"
                    name="run"
                    placeholder="pipeline run uuid"
                    value={runInput}
                    onChange={(e: React.ChangeEvent<HTMLInputElement>) => setRunInput(e.target.value)}
                  />
                </div>
                <Button type="submit" isDisabled={!runInput.trim()}>
                  Load
                </Button>
              </RowSpaceBetween>
            </form>
          </Accordion>
        </div>
      </Card>

      <div style={{ marginTop: '2ch' }}>
        {state === 'idle' ? (
          <Card title="No run selected">
            <Text>
              Enter a pipeline run id above, or open the decision log for a run from the History
              screen.
            </Text>
          </Card>
        ) : null}

        {state === 'loading' ? (
          <Card title="Loading">
            <Row style={{ gap: '1ch', alignItems: 'center' }}>
              <BlockLoader mode={1} />
              <Text>Loading decision log for run {activeRun}…</Text>
            </Row>
          </Card>
        ) : null}

        {state === 'error' ? (
          <Card title="Could not load">
            <AlertBanner>Failed to load the decision log: {error}</AlertBanner>
            <Row style={{ marginTop: '1ch' }}>
              <Button theme="SECONDARY" onClick={() => setReloadNonce((n) => n + 1)}>
                Retry
              </Button>
            </Row>
          </Card>
        ) : null}

        {state === 'loaded' && records.length === 0 ? (
          <Card title="No records">
            <Text>This run produced no decision-log records, or the run id is unknown.</Text>
          </Card>
        ) : null}

        {state === 'loaded' && records.length > 0 ? (
          <Card title={`Run ${activeRun}`}>
            <Summary records={records} />
            <Divider type="GRADIENT" style={{ margin: '1ch 0' }} />
            {records.map((rec, i) => (
              <RecordAccordion key={`${rec.at}-${i}`} record={rec} index={i} />
            ))}
          </Card>
        ) : null}
      </div>
    </AppShell>
  );
}

export default function DecisionLogPage() {
  return (
    <React.Suspense
      fallback={
        <AppShell>
          <Card title="Decision log">
            <Text>Loading…</Text>
          </Card>
        </AppShell>
      }
    >
      <DecisionLogScreen />
    </React.Suspense>
  );
}
