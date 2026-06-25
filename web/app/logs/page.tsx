'use client';

// Logs screen: lists the daemon's log files, and on selection shows the recent
// lines in a terminal/monospace SRCL view with a level filter, a line-count
// selector, and a manual refresh. SRCL-only: AppShell + Card / Table / Button /
// ButtonGroup / Select / Badge / Divider / Text / BlockLoader / AlertBanner /
// Message, over the API client + the screen-local log parser (data glue).

import * as React from 'react';

import Card from '@components/Card';
import Table from '@components/Table';
import TableRow from '@components/TableRow';
import TableColumn from '@components/TableColumn';
import Button from '@components/Button';
import ButtonGroup from '@components/ButtonGroup';
import Select from '@components/Select';
import Badge from '@components/Badge';
import Divider from '@components/Divider';
import Text from '@components/Text';
import BlockLoader from '@components/BlockLoader';
import AlertBanner from '@components/AlertBanner';
import Message from '@components/Message';

import AppShell from '@app/_components/AppShell';
import { useToast } from '@app/_lib/ToastProvider';
import { api, ApiError } from '@lib/api/client';
import type { LogFile } from '@lib/api/types';
import {
  parseLogLines,
  filterByLevel,
  LOG_LEVELS,
  type LogLevel,
} from '@app/logs/_lib/logs';

const LINE_COUNTS = [100, 250, 500, 1000, 5000];
const DEFAULT_LINES = 500;

// Per-level tint from SRCL's own ansi tokens, so both themes stay correct.
const LEVEL_COLOR: Record<LogLevel, string> = {
  TRACE: 'var(--ansi-8-bright-black, #888)',
  DEBUG: 'var(--ansi-6-cyan)',
  INFO: 'var(--ansi-2-green)',
  WARN: 'var(--ansi-3-yellow)',
  ERROR: 'var(--ansi-9-red)',
};

function formatTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const pad = (n: number) => String(n).padStart(2, '0');
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ` +
    `${pad(d.getHours())}:${pad(d.getMinutes())}`
  );
}

export default function LogsPage() {
  const { error: toastError } = useToast();

  const [files, setFiles] = React.useState<LogFile[] | null>(null);
  const [filesError, setFilesError] = React.useState<string | null>(null);
  const [filesLoading, setFilesLoading] = React.useState(true);

  const [selected, setSelected] = React.useState<string | null>(null);
  const [content, setContent] = React.useState<string>('');
  const [contentLoading, setContentLoading] = React.useState(false);
  const [contentError, setContentError] = React.useState<string | null>(null);

  const [level, setLevel] = React.useState<LogLevel | null>(null);
  const [lines, setLines] = React.useState<number>(DEFAULT_LINES);

  // Load the file list once.
  React.useEffect(() => {
    const controller = new AbortController();
    api
      .listLogFiles(controller.signal)
      .then((list) => {
        setFiles(list);
        // Auto-select the most recently written file for a useful first view.
        if (list && list.length && selected === null) {
          const newest = [...list].sort(
            (a, b) => new Date(b.lastWriteTime).getTime() - new Date(a.lastWriteTime).getTime()
          )[0];
          setSelected(newest.filename);
        }
      })
      .catch((err: unknown) => {
        if (err instanceof ApiError && err.code === 'network_error' && controller.signal.aborted) {
          return;
        }
        setFilesError(err instanceof Error ? err.message : 'failed to load log files');
      })
      .finally(() => setFilesLoading(false));
    return () => controller.abort();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const loadContent = React.useCallback(
    (name: string, count: number) => {
      const controller = new AbortController();
      setContentLoading(true);
      setContentError(null);
      api
        .getLogFile(name, count, controller.signal)
        .then((text) => setContent(text))
        .catch((err: unknown) => {
          if (err instanceof ApiError && err.code === 'network_error' && controller.signal.aborted) {
            return;
          }
          const message = err instanceof Error ? err.message : 'failed to load log';
          setContentError(message);
          toastError(`Could not load ${name} — ${message}`);
        })
        .finally(() => setContentLoading(false));
      return () => controller.abort();
    },
    [toastError]
  );

  // (Re)load whenever the selected file or the line-count changes.
  React.useEffect(() => {
    if (!selected) return;
    const cancel = loadContent(selected, lines);
    return cancel;
  }, [selected, lines, loadContent]);

  const parsed = React.useMemo(() => parseLogLines(content), [content]);
  const visible = React.useMemo(() => filterByLevel(parsed, level), [parsed, level]);

  const refresh = () => {
    if (selected) loadContent(selected, lines);
  };

  return (
    <AppShell>
      <Card title="Logs">
        <Text style={{ opacity: 0.6 }}>
          Recent daemon log output. Pick a file, then filter by level or adjust how many trailing
          lines to show.
        </Text>

        <Divider type="GRADIENT" />

        {filesLoading ? (
          <Text role="status" aria-live="polite" style={{ opacity: 0.6 }}>
            <BlockLoader mode={0} /> Loading log files…
          </Text>
        ) : null}

        {filesError ? (
          <AlertBanner style={{ background: 'var(--ansi-9-red)', color: 'var(--ansi-15-white)' }}>
            Could not load log files: {filesError}
          </AlertBanner>
        ) : null}

        {!filesLoading && !filesError && files && files.length === 0 ? (
          <Message>No log files are available yet.</Message>
        ) : null}

        {files && files.length ? (
          <>
            <Text style={{ opacity: 0.6, marginBottom: '0.5ch' }}>Log files</Text>
            <Table>
              <TableRow>
                <TableColumn>File</TableColumn>
                <TableColumn>Last write</TableColumn>
                <TableColumn> </TableColumn>
              </TableRow>
              {files.map((f) => (
                <TableRow key={String(f.id)}>
                  <TableColumn>
                    <code>{f.filename}</code>
                    {selected === f.filename ? (
                      <span style={{ marginLeft: '1ch' }}>
                        <Badge>viewing</Badge>
                      </span>
                    ) : null}
                  </TableColumn>
                  <TableColumn>{formatTime(f.lastWriteTime)}</TableColumn>
                  <TableColumn>
                    <Button
                      theme={selected === f.filename ? undefined : 'SECONDARY'}
                      aria-label={`View log ${f.filename}`}
                      onClick={() => setSelected(f.filename)}
                    >
                      View
                    </Button>
                  </TableColumn>
                </TableRow>
              ))}
            </Table>
          </>
        ) : null}

        {selected ? (
          <>
            <Divider type="GRADIENT" />

            <div
              style={{
                display: 'flex',
                gap: '2ch',
                alignItems: 'flex-end',
                flexWrap: 'wrap',
                marginBottom: '1ch',
              }}
            >
              <div>
                <Text style={{ opacity: 0.6 }}>Level</Text>
                <Select
                  name="log-level"
                  aria-label="Filter by log level"
                  options={['All', ...LOG_LEVELS]}
                  defaultValue="All"
                  onChange={(v) => setLevel(v === 'All' ? null : (v as LogLevel))}
                />
              </div>
              <div>
                <Text style={{ opacity: 0.6 }}>Lines</Text>
                <Select
                  name="log-lines"
                  aria-label="Lines to show"
                  options={LINE_COUNTS.map(String)}
                  defaultValue={String(DEFAULT_LINES)}
                  onChange={(v) => setLines(Number(v))}
                />
              </div>
              <ButtonGroup
                items={[
                  {
                    body: contentLoading ? 'Refreshing…' : 'Refresh',
                    onClick: contentLoading ? undefined : refresh,
                  },
                ]}
              />
            </div>

            {contentLoading ? (
              <Text role="status" aria-live="polite" style={{ opacity: 0.6 }}>
                <BlockLoader mode={0} /> Loading {selected}…
              </Text>
            ) : null}

            {contentError ? (
              <AlertBanner style={{ background: 'var(--ansi-9-red)', color: 'var(--ansi-15-white)' }}>
                Could not load {selected}: {contentError}
              </AlertBanner>
            ) : null}

            {!contentLoading && !contentError ? (
              visible.length ? (
                <pre
                  aria-label={`Log contents for ${selected}`}
                  style={{
                    margin: 0,
                    padding: '1ch',
                    background: 'var(--theme-background-subdued, rgba(0,0,0,0.2))',
                    border: '1px solid var(--theme-border)',
                    fontFamily: 'var(--font-family-mono, monospace)',
                    fontSize: 'var(--font-size, 0.875rem)',
                    lineHeight: 1.5,
                    overflowX: 'auto',
                    maxHeight: '60vh',
                    whiteSpace: 'pre-wrap',
                    wordBreak: 'break-word',
                  }}
                >
                  {visible.map((line) => (
                    <div
                      key={line.index}
                      data-level={line.level ?? 'NONE'}
                      style={{ color: line.level ? LEVEL_COLOR[line.level] : undefined }}
                    >
                      {line.text || ' '}
                    </div>
                  ))}
                </pre>
              ) : (
                <Message>
                  {parsed.length
                    ? 'No lines match the selected level.'
                    : 'This log file is empty.'}
                </Message>
              )
            ) : null}
          </>
        ) : null}
      </Card>
    </AppShell>
  );
}
