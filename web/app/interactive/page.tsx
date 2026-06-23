'use client';

// Manual / interactive-search screen (docs/10-ui.md §screen-mapping): pick a release,
// see scores. A Table of candidate releases with quality + CF-score Badges, a Popover
// (via HoverComponentTrigger) explaining how a score was reached, and an ActionButton
// to grab. Built only from vendored SRCL components + the API client + relative glue;
// all color comes from --theme-* tokens so both SRCL themes render correctly.

import * as React from 'react';
import { useSearchParams } from 'next/navigation';

import Card from '@components/Card';
import Input from '@components/Input';
import Button from '@components/Button';
import ActionButton from '@components/ActionButton';
import Table from '@components/Table';
import TableRow from '@components/TableRow';
import TableColumn from '@components/TableColumn';
import Badge from '@components/Badge';
import Text from '@components/Text';
import Divider from '@components/Divider';
import AlertBanner from '@components/AlertBanner';
import BlockLoader from '@components/BlockLoader';
import RowSpaceBetween from '@components/RowSpaceBetween';
import HoverComponentTrigger from '@components/HoverComponentTrigger';

import { ApiError } from '@lib/api/client';

import AppShell from '@app/_components/AppShell';

import {
  formatSize,
  grabRelease,
  searchReleases,
  type CandidateRelease,
} from '../_search/api';

type Phase = 'idle' | 'loading' | 'ready' | 'error';
type GrabState = 'idle' | 'grabbing' | 'grabbed' | 'failed';

function InteractiveSearch() {
  const params = useSearchParams();
  const initialContent = params.get('content') ?? '';

  const [contentId, setContentId] = React.useState(initialContent);
  const [phase, setPhase] = React.useState<Phase>('idle');
  const [releases, setReleases] = React.useState<CandidateRelease[]>([]);
  const [error, setError] = React.useState('');
  const [grabs, setGrabs] = React.useState<Record<string, GrabState>>({});

  const abortRef = React.useRef<AbortController | null>(null);

  const runSearch = React.useCallback(async (id: string) => {
    const cid = id.trim();
    if (!cid) {
      setPhase('idle');
      setReleases([]);
      setError('');
      return;
    }

    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;

    setPhase('loading');
    setError('');
    try {
      const found = await searchReleases(cid, controller.signal);
      if (controller.signal.aborted) return;
      setReleases(found ?? []);
      setPhase('ready');
    } catch (err) {
      if (controller.signal.aborted) return;
      setError(err instanceof ApiError ? `${err.code}: ${err.message}` : 'Release search failed.');
      setReleases([]);
      setPhase('error');
    }
  }, []);

  // Auto-search when arriving with ?content=… from an item detail screen.
  React.useEffect(() => {
    if (initialContent) void runSearch(initialContent);
  }, [initialContent, runSearch]);

  React.useEffect(() => () => abortRef.current?.abort(), []);

  const grab = React.useCallback(
    async (release: CandidateRelease) => {
      const cid = contentId.trim();
      if (!cid) return;
      setGrabs((prev) => ({ ...prev, [release.guid]: 'grabbing' }));
      try {
        await grabRelease(release.guid, cid);
        setGrabs((prev) => ({ ...prev, [release.guid]: 'grabbed' }));
      } catch {
        setGrabs((prev) => ({ ...prev, [release.guid]: 'failed' }));
      }
    },
    [contentId]
  );

  return (
    <AppShell>
      <Card title="Interactive search — pick a release">
        <RowSpaceBetween>
          <div style={{ flex: 1, minWidth: '24ch' }}>
            <Input
              label="Content id"
              name="content-id"
              placeholder="content id to search releases for…"
              autoComplete="off"
              value={contentId}
              onChange={(e: React.ChangeEvent<HTMLInputElement>) => setContentId(e.target.value)}
            />
          </div>
          <Button
            theme="SECONDARY"
            onClick={() => void runSearch(contentId)}
            isDisabled={!contentId.trim()}
          >
            Search releases
          </Button>
        </RowSpaceBetween>

        <Divider type="GRADIENT" />

        <ReleaseTable
          phase={phase}
          contentId={contentId}
          releases={releases}
          error={error}
          grabs={grabs}
          onGrab={grab}
        />
      </Card>
    </AppShell>
  );
}

export default function Page() {
  // useSearchParams requires a Suspense boundary under the App Router.
  return (
    <React.Suspense
      fallback={
        <AppShell>
          <Card title="Interactive search — pick a release">
            <Text>
              <BlockLoader mode={1} /> Loading…
            </Text>
          </Card>
        </AppShell>
      }
    >
      <InteractiveSearch />
    </React.Suspense>
  );
}

const ReleaseTable: React.FC<{
  phase: Phase;
  contentId: string;
  releases: CandidateRelease[];
  error: string;
  grabs: Record<string, GrabState>;
  onGrab: (r: CandidateRelease) => void;
}> = ({ phase, contentId, releases, error, grabs, onGrab }) => {
  if (phase === 'idle') {
    return (
      <Text style={{ opacity: 0.6 }}>
        Enter a content id (or open this screen from an item) to search indexers for
        candidate releases.
      </Text>
    );
  }

  if (phase === 'loading') {
    return (
      <Text>
        <BlockLoader mode={1} /> Searching indexers for {contentId.trim()}…
      </Text>
    );
  }

  if (phase === 'error') {
    return <AlertBanner>Release search failed — {error}</AlertBanner>;
  }

  if (releases.length === 0) {
    return (
      <Text style={{ opacity: 0.6 }}>
        No candidate releases found. The indexers may have nothing, or none passed the
        profile’s minimum.
      </Text>
    );
  }

  return (
    <Table>
      <TableRow>
        <TableColumn>Release</TableColumn>
        <TableColumn>Quality</TableColumn>
        <TableColumn>Score</TableColumn>
        <TableColumn>Size</TableColumn>
        <TableColumn>Peers</TableColumn>
        <TableColumn>Grab</TableColumn>
      </TableRow>
      {releases.map((r) => (
        <TableRow key={r.guid}>
          <TableColumn>
            <span title={r.title}>{r.title}</span>
            {r.indexer ? (
              <span style={{ opacity: 0.6 }}> · {r.indexer}</span>
            ) : null}
            {r.rejected ? (
              <>
                {' '}
                <HoverComponentTrigger
                  component="tooltip"
                  text={r.rejection_reason || 'Rejected by the current quality profile.'}
                >
                  <Badge>rejected</Badge>
                </HoverComponentTrigger>
              </>
            ) : null}
          </TableColumn>
          <TableColumn>{r.quality ? <Badge>{r.quality}</Badge> : <Badge>unknown</Badge>}</TableColumn>
          <TableColumn>
            <HoverComponentTrigger component="popover" text={scoreReason(r)}>
              <Badge>{formatScore(r.cf_score)}</Badge>
            </HoverComponentTrigger>
          </TableColumn>
          <TableColumn>{formatSize(r.size)}</TableColumn>
          <TableColumn>{r.protocol === 'usenet' ? '—' : r.seeders ?? '—'}</TableColumn>
          <TableColumn>
            <GrabCell state={grabs[r.guid] ?? 'idle'} onGrab={() => onGrab(r)} />
          </TableColumn>
        </TableRow>
      ))}
    </Table>
  );
};

const GrabCell: React.FC<{ state: GrabState; onGrab: () => void }> = ({ state, onGrab }) => {
  if (state === 'grabbing') {
    return (
      <Text>
        <BlockLoader mode={0} /> grabbing…
      </Text>
    );
  }
  if (state === 'grabbed') return <Badge>grabbed</Badge>;
  return (
    <>
      <ActionButton hotkey="⏷" onClick={onGrab}>
        Grab
      </ActionButton>
      {state === 'failed' ? (
        <span style={{ opacity: 0.7 }}>
          {' '}
          <Badge>retry</Badge>
        </span>
      ) : null}
    </>
  );
};

function formatScore(score?: number): string {
  if (score === undefined || score === null) return 'n/a';
  return score > 0 ? `+${score}` : String(score);
}

function scoreReason(r: CandidateRelease): string {
  if (r.score_reason) return r.score_reason;
  const parts: string[] = [];
  parts.push(`Custom-format score: ${formatScore(r.cf_score)}.`);
  if (r.quality) parts.push(`Quality parsed as ${r.quality}.`);
  if (r.flags && r.flags.length) parts.push(`Flags: ${r.flags.join(', ')}.`);
  parts.push('Score is the sum of matching custom-format weights under the active profile.');
  return parts.join(' ');
}
