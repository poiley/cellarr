'use client';

// Manual Import screen (docs/10-ui.md §screen-mapping): map loose files on disk
// onto library content WITHOUT moving anything until the user confirms.
//
// Flow: type a folder path -> SCAN (GET /api/v3/manualimport, read-only) -> a
// table of candidate rows (file / parsed title / suggested match / quality /
// rejection reasons) with a per-row include checkbox and an editable target
// (re-map to a movie/series + season/episode via lookup). The Import button POSTs
// the included rows to the commit endpoint, which runs the existing crash-safe
// stage->verify->commit->log path; a result toast reports what landed where.
//
// SAFE BY CONSTRUCTION: the scan never touches the filesystem, and Import is gated
// on rows the user explicitly included AND that resolve to a content id. Built
// only from vendored SRCL components + the API client + relative glue; empty /
// loading / error states are handled and both SRCL themes work.

import * as React from 'react';

import Card from '@components/Card';
import Input from '@components/Input';
import Button from '@components/Button';
import Table from '@components/Table';
import TableRow from '@components/TableRow';
import TableColumn from '@components/TableColumn';
import Badge from '@components/Badge';

import StatusBadge from '@app/_components/StatusBadge';
import Text from '@components/Text';
import Divider from '@components/Divider';
import AlertBanner from '@components/AlertBanner';
import BlockLoader from '@components/BlockLoader';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Checkbox from '@components/Checkbox';
import Select from '@components/Select';
import Dialog from '@components/Dialog';
import ModalStack from '@components/ModalStack';
import { useModals } from '@components/page/ModalContext';

import { ApiError } from '@lib/api/client';

import AppShell from '@app/_components/AppShell';
import { useToast } from '@app/_lib/ToastProvider';

import {
  commitImport,
  formatSize,
  listTargets,
  lookupTargets,
  scanFolder,
  type ImportTarget,
  type ManualImportRow,
} from './_lib/manual-import';

type Phase = 'idle' | 'scanning' | 'ready' | 'error';

/** Per-row, mutable import state layered over the immutable scan result. */
interface RowState {
  /** Whether this row is included in the next Import. */
  include: boolean;
  /** The content id this file will be moved onto (suggested, or user-chosen). */
  contentId?: string;
  /** A human label for the chosen target (for the override hint). */
  targetLabel?: string;
}

export default function Page() {
  const modals = useModals();
  const { success, error: toastError, info } = useToast();

  const [folder, setFolder] = React.useState('');
  const [phase, setPhase] = React.useState<Phase>('idle');
  const [rows, setRows] = React.useState<ManualImportRow[]>([]);
  const [rowStates, setRowStates] = React.useState<Record<string, RowState>>({});
  const [error, setError] = React.useState('');
  const [importing, setImporting] = React.useState(false);

  // The library's existing movies/series, loaded once, used to seed the re-map
  // picker so the common "this belongs to something I already track" case needs
  // no typed lookup.
  const targetsRef = React.useRef<ImportTarget[]>([]);
  React.useEffect(() => {
    const controller = new AbortController();
    void (async () => {
      try {
        const t = await listTargets(controller.signal);
        if (!controller.signal.aborted) targetsRef.current = t;
      } catch {
        // Non-fatal: the picker falls back to typed lookup only.
      }
    })();
    return () => controller.abort();
  }, []);

  const scanAbort = React.useRef<AbortController | null>(null);
  React.useEffect(() => () => scanAbort.current?.abort(), []);

  const runScan = React.useCallback(async () => {
    const f = folder.trim();
    if (!f) {
      setPhase('idle');
      setRows([]);
      setError('');
      return;
    }
    scanAbort.current?.abort();
    const controller = new AbortController();
    scanAbort.current = controller;

    setPhase('scanning');
    setError('');
    try {
      const found = await scanFolder(f, controller.signal);
      if (controller.signal.aborted) return;
      setRows(found);
      // Seed per-row state: pre-include rows that the scan both identified AND
      // did not reject (the safe default); leave rejected/unidentified rows for
      // the user to opt in + fix.
      const seeded: Record<string, RowState> = {};
      for (const r of found) {
        seeded[r.path] = {
          include: !r.rejected && !!r.contentId,
          contentId: r.contentId,
        };
      }
      setRowStates(seeded);
      setPhase('ready');
    } catch (err) {
      if (controller.signal.aborted) return;
      setRows([]);
      setError(err instanceof ApiError ? `${err.code}: ${err.message}` : 'Scan failed.');
      setPhase('error');
    }
  }, [folder]);

  const setRowState = React.useCallback((path: string, patch: Partial<RowState>) => {
    setRowStates((prev) => ({ ...prev, [path]: { ...prev[path], ...patch } }));
  }, []);

  // Re-map a single row onto a different content node via a lookup dialog.
  const editTarget = React.useCallback(
    (row: ManualImportRow) => {
      const chosenRef: { current: ImportTarget | undefined } = { current: undefined };
      modals.open(Dialog, {
        title: `Match "${row.name}"`,
        children: (
          <TargetPicker
            seed={targetsRef.current}
            onPick={(t) => {
              chosenRef.current = t;
            }}
          />
        ),
        onConfirm: () => {
          modals.close();
          const t = chosenRef.current;
          if (t) {
            setRowState(row.path, {
              contentId: t.id,
              targetLabel: t.year ? `${t.title} (${t.year})` : t.title,
              include: true,
            });
          }
        },
        onCancel: () => modals.close(),
      });
    },
    [modals, setRowState]
  );

  const includable = rows.filter((r) => rowStates[r.path]?.include && rowStates[r.path]?.contentId);

  const runImport = React.useCallback(async () => {
    const files = rows
      .map((r) => ({ row: r, st: rowStates[r.path] }))
      .filter((x) => x.st?.include && x.st?.contentId)
      .map((x) => ({ path: x.row.path, contentId: x.st.contentId as string }));

    if (files.length === 0) {
      toastError('Select at least one file with a match to import.');
      return;
    }
    setImporting(true);
    info(`Importing ${files.length} file${files.length === 1 ? '' : 's'}…`, { durationMs: 2000 });
    try {
      const result = await commitImport(files);
      if (result.message && result.imported.length === 0 && result.errors.length === 0) {
        // No pipeline/library ready — nothing was moved.
        info(result.message);
      } else if (result.errors.length > 0 && result.imported.length === 0) {
        toastError(`Import failed: ${result.errors.join('; ')}`);
      } else {
        const ok = result.imported.length;
        const failed = result.errors.length;
        success(
          <span>
            Imported <strong>{ok}</strong> file{ok === 1 ? '' : 's'}
            {failed > 0 ? <> · {failed} error{failed === 1 ? '' : 's'}</> : null}
          </span>
        );
        // Drop the imported rows from the table; keep any that errored.
        const importedPaths = new Set(result.imported.map((i) => i.sourcePath));
        setRows((prev) => prev.filter((r) => !importedPaths.has(r.path)));
      }
    } catch (err) {
      toastError(err instanceof ApiError ? `${err.code}: ${err.message}` : 'Import failed.');
    } finally {
      setImporting(false);
    }
  }, [rows, rowStates, success, toastError, info]);

  return (
    <AppShell>
      <ModalStack />
      <Card title="Manual import — map loose files to library content">
        <Text style={{ opacity: 0.6 }}>
          Scan a folder of loose files; nothing moves until you press Import.
        </Text>
        <div style={{ height: '1ch' }} />
        <RowSpaceBetween>
          <div style={{ flex: 1, minWidth: '24ch' }}>
            <Input
              label="Folder"
              name="import-folder"
              placeholder="/downloads/complete/…"
              autoComplete="off"
              value={folder}
              onChange={(e: React.ChangeEvent<HTMLInputElement>) => setFolder(e.target.value)}
              onKeyDown={(e: React.KeyboardEvent) => {
                if (e.key === 'Enter') void runScan();
              }}
            />
          </div>
          <Button theme="SECONDARY" onClick={() => void runScan()} isDisabled={!folder.trim()}>
            Scan
          </Button>
        </RowSpaceBetween>

        <Divider type="GRADIENT" />

        <ScanResults phase={phase} folder={folder} rows={rows} error={error} />

        {phase === 'ready' && rows.length > 0 ? (
          <ImportTable
            rows={rows}
            rowStates={rowStates}
            onToggle={(path, include) => setRowState(path, { include })}
            onEdit={editTarget}
          />
        ) : null}

        {phase === 'ready' && rows.length > 0 ? (
          <>
            <Divider type="GRADIENT" />
            <RowSpaceBetween>
              <Text style={{ opacity: 0.7 }}>
                {includable.length} of {rows.length} selected
              </Text>
              <Button
                theme="PRIMARY"
                onClick={() => void runImport()}
                isDisabled={importing || includable.length === 0}
              >
                {importing ? 'Importing…' : `Import ${includable.length} ▸`}
              </Button>
            </RowSpaceBetween>
          </>
        ) : null}
      </Card>
    </AppShell>
  );
}

// ---------------------------------------------------------------------------
// Scan-state messaging (idle / scanning / error / empty).
// ---------------------------------------------------------------------------

const ScanResults: React.FC<{
  phase: Phase;
  folder: string;
  rows: ManualImportRow[];
  error: string;
}> = ({ phase, folder, rows, error }) => {
  if (phase === 'idle') {
    return (
      <Text style={{ opacity: 0.6 }}>
        Enter a folder path above and press Scan to find files to import.
      </Text>
    );
  }
  if (phase === 'scanning') {
    return (
      <Text>
        <BlockLoader mode={1} /> Scanning “{folder.trim()}”…
      </Text>
    );
  }
  if (phase === 'error') {
    return <AlertBanner>Scan failed — {error}</AlertBanner>;
  }
  if (rows.length === 0) {
    return (
      <Text style={{ opacity: 0.6 }}>
        No importable files found in “{folder.trim()}”. Check the path, or that a library is ready.
      </Text>
    );
  }
  return null;
};

// ---------------------------------------------------------------------------
// The candidate table: file / parsed title / match / quality / rejections, with
// a per-row include checkbox + a re-map button.
// ---------------------------------------------------------------------------

const ImportTable: React.FC<{
  rows: ManualImportRow[];
  rowStates: Record<string, RowState>;
  onToggle: (path: string, include: boolean) => void;
  onEdit: (row: ManualImportRow) => void;
}> = ({ rows, rowStates, onToggle, onEdit }) => (
  <Table>
    <TableRow>
      <TableColumn>Include</TableColumn>
      <TableColumn>File</TableColumn>
      <TableColumn>Parsed title</TableColumn>
      <TableColumn>Match</TableColumn>
      <TableColumn>Quality</TableColumn>
      <TableColumn>Status</TableColumn>
      <TableColumn>Size</TableColumn>
    </TableRow>
    {rows.map((r) => {
      const st = rowStates[r.path] ?? { include: false };
      const matched = !!st.contentId;
      const coords =
        r.seasonNumber !== undefined && r.episodeNumber !== undefined
          ? ` · S${pad(r.seasonNumber)}E${pad(r.episodeNumber)}`
          : '';
      return (
        <TableRow key={r.path}>
          <TableColumn>
            <Checkbox
              name={`include-${r.path}`}
              aria-label={`Include ${r.name}`}
              defaultChecked={st.include}
              onChange={(e) => onToggle(r.path, e.target.checked)}
            >
              <span className="sr-only">Include {r.name}</span>
            </Checkbox>
          </TableColumn>
          <TableColumn>{r.name}</TableColumn>
          <TableColumn>
            <span style={{ opacity: 0.8 }}>{r.parsedTitle ?? '—'}</span>
          </TableColumn>
          <TableColumn>
            <span style={{ opacity: 0.85 }}>
              {st.targetLabel ?? (matched ? `#${shortId(st.contentId!)}${coords}` : 'Unmatched')}
            </span>{' '}
            <Button theme="SECONDARY" onClick={() => onEdit(r)}>
              {matched ? 'Change' : 'Pick'}
            </Button>
          </TableColumn>
          <TableColumn>{r.quality ?? '—'}</TableColumn>
          <TableColumn>
            {r.rejected ? (
              <span title={r.rejections.join('; ')}>
                <StatusBadge status="rejected" />{' '}
                <span style={{ opacity: 0.7 }}>{r.rejections.join('; ') || 'rejected'}</span>
              </span>
            ) : (
              <StatusBadge status="ok" />
            )}
          </TableColumn>
          <TableColumn>{formatSize(r.size)}</TableColumn>
        </TableRow>
      );
    })}
  </Table>
);

// ---------------------------------------------------------------------------
// Target picker dialog body: choose an existing library item or look one up.
// ---------------------------------------------------------------------------

const TargetPicker: React.FC<{
  seed: ImportTarget[];
  onPick: (target: ImportTarget) => void;
}> = ({ seed, onPick }) => {
  const [term, setTerm] = React.useState('');
  const [results, setResults] = React.useState<ImportTarget[]>(seed);
  const [searching, setSearching] = React.useState(false);

  // Debounced typed lookup; falls back to the seed list when the box is empty.
  React.useEffect(() => {
    const q = term.trim();
    if (q.length < 2) {
      setResults(seed);
      setSearching(false);
      return;
    }
    const controller = new AbortController();
    setSearching(true);
    const handle = window.setTimeout(async () => {
      try {
        const found = await lookupTargets(q, controller.signal);
        if (!controller.signal.aborted) setResults(found);
      } catch {
        if (!controller.signal.aborted) setResults([]);
      } finally {
        if (!controller.signal.aborted) setSearching(false);
      }
    }, 300);
    return () => {
      controller.abort();
      window.clearTimeout(handle);
    };
  }, [term, seed]);

  const options = React.useMemo(
    () => results.map((t) => labelFor(t)),
    [results]
  );
  const byLabel = React.useMemo(() => {
    const m = new Map<string, ImportTarget>();
    for (const t of results) m.set(labelFor(t), t);
    return m;
  }, [results]);

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '1ch', minWidth: '40ch' }}>
      <Text style={{ opacity: 0.7 }}>
        Pick the movie or series this file belongs to.
      </Text>
      <Input
        name="target-search"
        label="Search"
        placeholder="Filter library or look up a title…"
        autoComplete="off"
        value={term}
        onChange={(e: React.ChangeEvent<HTMLInputElement>) => setTerm(e.target.value)}
      />
      {options.length > 0 ? (
        <Select
          name="target-select"
          aria-label="Choose a match"
          options={options}
          placeholder="Choose a match"
          onChange={(label) => {
            const t = byLabel.get(label);
            if (t) onPick(t);
          }}
        />
      ) : (
        <Text style={{ opacity: 0.6 }}>{searching ? 'Searching…' : 'No matches.'}</Text>
      )}
    </div>
  );
};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

function labelFor(t: ImportTarget): string {
  const kind = t.mediaType === 'tv' ? 'TV' : 'Movie';
  return `${t.title}${t.year ? ` (${t.year})` : ''} · ${kind}`;
}

function pad(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

/** A short, glanceable form of a content id (uuid or numeric projection). */
function shortId(id: string): string {
  return id.length > 8 ? id.slice(0, 8) : id;
}
