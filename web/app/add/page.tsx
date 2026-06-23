'use client';

// Add / search-new screen (docs/10-ui.md §screen-mapping): find a title and add it.
// Input to search, a Table of results, an ActionButton per row that opens a Dialog
// (via SRCL's ModalStack/useModals) to confirm the add. Built only from vendored
// SRCL components + the API client + relative glue. Empty/loading/error states are
// all handled and both SRCL themes work (all color comes from --theme-* tokens).

import * as React from 'react';

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
import Dialog from '@components/Dialog';
import ModalStack from '@components/ModalStack';
import { useModals } from '@components/page/ModalContext';

import { ApiError } from '@lib/api/client';

import AppShell from '@app/_components/AppShell';

import { addContent, lookup, type LookupResult } from '../_search/api';

type Phase = 'idle' | 'loading' | 'ready' | 'error';

export default function Page() {
  const modals = useModals();

  const [term, setTerm] = React.useState('');
  const [phase, setPhase] = React.useState<Phase>('idle');
  const [results, setResults] = React.useState<LookupResult[]>([]);
  const [error, setError] = React.useState<string>('');
  const [added, setAdded] = React.useState<Set<string>>(new Set());

  const abortRef = React.useRef<AbortController | null>(null);

  const runSearch = React.useCallback(async (raw: string) => {
    const q = raw.trim();
    if (!q) {
      setPhase('idle');
      setResults([]);
      setError('');
      return;
    }

    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;

    setPhase('loading');
    setError('');
    try {
      const found = await lookup(q, controller.signal);
      if (controller.signal.aborted) return;
      setResults(found ?? []);
      setPhase('ready');
    } catch (err) {
      if (controller.signal.aborted) return;
      setError(err instanceof ApiError ? `${err.code}: ${err.message}` : 'Lookup failed.');
      setResults([]);
      setPhase('error');
    }
  }, []);

  // Debounced search as the user types.
  React.useEffect(() => {
    const handle = window.setTimeout(() => {
      void runSearch(term);
    }, 350);
    return () => window.clearTimeout(handle);
  }, [term, runSearch]);

  React.useEffect(() => () => abortRef.current?.abort(), []);

  const confirmAdd = React.useCallback(
    (result: LookupResult) => {
      const key = result.foreign_id;
      modals.open(Dialog, {
        title: `Add "${result.title}"${result.year ? ` (${result.year})` : ''}?`,
        children: (
          <Text>
            This will start monitoring{' '}
            <strong>{result.title}</strong> and search for it on the configured
            indexers using the default quality profile.
          </Text>
        ),
        onConfirm: () => {
          modals.close();
          void doAdd(result, key);
        },
        onCancel: () => modals.close(),
      });
    },
    // doAdd is stable via the closure below; results/added state read fresh inside.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [modals]
  );

  const doAdd = React.useCallback(async (result: LookupResult, key: string) => {
    try {
      await addContent({
        // library_id is resolved server-side by media type in v1; the UI passes
        // the chosen title's identity. (Library selection lives in a later iteration.)
        library_id: result.media_type ? String(result.media_type) : '',
        foreign_id: result.foreign_id,
        title: result.title,
        search_on_add: true,
      });
      setAdded((prev) => new Set(prev).add(key));
    } catch (err) {
      const msg = err instanceof ApiError ? `${err.code}: ${err.message}` : 'Add failed.';
      modals.open(Dialog, {
        title: 'Could not add title',
        children: <Text>{msg}</Text>,
        onConfirm: () => modals.close(),
        onCancel: () => modals.close(),
      });
    }
    // modals is stable from context.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <AppShell>
      <ModalStack />
      <Card title="Add — search for a new title">
        <RowSpaceBetween>
          <div style={{ flex: 1, minWidth: '24ch' }}>
            <Input
              label="Search"
              name="add-search"
              placeholder="Type a movie, series, album, or book…"
              autoComplete="off"
              value={term}
              onChange={(e: React.ChangeEvent<HTMLInputElement>) => setTerm(e.target.value)}
            />
          </div>
          <Button
            theme="SECONDARY"
            onClick={() => void runSearch(term)}
            isDisabled={!term.trim()}
          >
            Search
          </Button>
        </RowSpaceBetween>

        <Divider type="GRADIENT" />

        <Results
          phase={phase}
          term={term}
          results={results}
          error={error}
          added={added}
          onAdd={confirmAdd}
        />
      </Card>
    </AppShell>
  );
}

const Results: React.FC<{
  phase: Phase;
  term: string;
  results: LookupResult[];
  error: string;
  added: Set<string>;
  onAdd: (r: LookupResult) => void;
}> = ({ phase, term, results, error, added, onAdd }) => {
  if (phase === 'idle') {
    return (
      <Text style={{ opacity: 0.6 }}>
        Start typing above to look up titles to add to your library.
      </Text>
    );
  }

  if (phase === 'loading') {
    return (
      <Text>
        <BlockLoader mode={1} /> Searching for “{term.trim()}”…
      </Text>
    );
  }

  if (phase === 'error') {
    return <AlertBanner>Search failed — {error}</AlertBanner>;
  }

  if (results.length === 0) {
    return (
      <Text style={{ opacity: 0.6 }}>
        No matches for “{term.trim()}”. Try a different spelling or a shorter query.
      </Text>
    );
  }

  return (
    <Table>
      <TableRow>
        <TableColumn>Title</TableColumn>
        <TableColumn>Year</TableColumn>
        <TableColumn>Type</TableColumn>
        <TableColumn>Overview</TableColumn>
        <TableColumn>Add</TableColumn>
      </TableRow>
      {results.map((r) => {
        const isAdded = added.has(r.foreign_id) || r.already_added;
        return (
          <TableRow key={r.foreign_id}>
            <TableColumn>{r.title}</TableColumn>
            <TableColumn>{r.year ?? '—'}</TableColumn>
            <TableColumn>{r.media_type ? <Badge>{String(r.media_type)}</Badge> : '—'}</TableColumn>
            <TableColumn>
              <span style={{ opacity: 0.7 }}>{truncate(r.overview)}</span>
            </TableColumn>
            <TableColumn>
              {isAdded ? (
                <Badge>added</Badge>
              ) : (
                <ActionButton hotkey="＋" onClick={() => onAdd(r)}>
                  Add
                </ActionButton>
              )}
            </TableColumn>
          </TableRow>
        );
      })}
    </Table>
  );
};

function truncate(text?: string, max = 80): string {
  if (!text) return '—';
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}
