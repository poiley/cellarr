'use client';

// System / Status (docs/10-ui.md §screen-mapping): health + scheduler tasks +
// raw diagnostics. Reads /api/v1/system/status, /api/v1/commands, and the v3
// scheduler surface (/api/v3/system/task) for per-task next/last-run + a
// 'Run now' action (POST /api/v3/command).
//
// The Health and Tasks tables are the PRIMARY view; the raw status JSON now
// lives behind a 'Raw / Advanced' SRCL disclosure (Accordion) with a copy
// button. Composed only from vendored SRCL primitives; the API client + the
// shared toast hook are the lone non-component glue.

import * as React from 'react';

import Card from '@components/Card';
import Text from '@components/Text';
import Badge from '@components/Badge';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Divider from '@components/Divider';
import Table from '@components/Table';
import TableColumn from '@components/TableColumn';
import TableRow from '@components/TableRow';

import { TONE_COLOR, type Tone } from '@app/_lib/status';
import AlertBanner from '@components/AlertBanner';
import Message from '@components/Message';
import CodeBlock from '@components/CodeBlock';
import BlockLoader from '@components/BlockLoader';
import Accordion from '@components/Accordion';
import ActionButton from '@components/ActionButton';

import AppShell from '@app/_components/AppShell';
import { useToast } from '@app/_lib/ToastProvider';
import { api, ApiError } from '@lib/api/client';
import type { CommandInfo, HealthCheck, SystemStatus } from '@lib/api/types';
import {
  fetchSystemTasks,
  runTaskNow,
  formatTimestamp,
  formatCountdown,
  formatInterval,
  type SystemTask,
} from '@app/system/_lib/system';

interface LoadState {
  status: SystemStatus | null;
  commands: CommandInfo[] | null;
  tasks: SystemTask[] | null;
  health: HealthCheck[] | null;
  error: string | null;
  loading: boolean;
}

export default function SystemPage() {
  const { success, error: toastError, info } = useToast();

  const [state, setState] = React.useState<LoadState>({
    status: null,
    commands: null,
    tasks: null,
    health: null,
    error: null,
    loading: true,
  });
  // Which task's 'Run now' is currently in flight (keyed by taskName).
  const [running, setRunning] = React.useState<string | null>(null);
  const [copied, setCopied] = React.useState(false);

  React.useEffect(() => {
    const controller = new AbortController();
    Promise.all([
      api.systemStatus(controller.signal),
      api.getCommands(controller.signal),
      // The scheduler surface is best-effort: an older daemon without it should
      // not blank the whole screen, so swallow its failure into a null list.
      fetchSystemTasks(api, controller.signal).catch(() => null),
      // The broader health surface (/api/v3/health) is also best-effort; a
      // failure here should not blank status/tasks.
      api.health(controller.signal).catch(() => null),
    ])
      .then(([status, commands, tasks, health]) => {
        setState({ status, commands, tasks, health, error: null, loading: false });
      })
      .catch((err: unknown) => {
        if (err instanceof ApiError && err.code === 'network_error') {
          setState((prev) => ({ ...prev, loading: false }));
          return;
        }
        setState((prev) => ({
          ...prev,
          loading: false,
          error: err instanceof Error ? err.message : 'failed to load system status',
        }));
      });
    return () => controller.abort();
  }, []);

  const { status, commands, tasks, health, error, loading } = state;

  // A small health check derived from the status snapshot: zero indexers means
  // nothing can be searched, which is the canonical first-run warning.
  const healthWarning =
    status && status.indexer_count === 0
      ? 'No indexers configured — searches will return nothing until you add one.'
      : null;

  const healthRows: { label: string; value: string; tone: Tone }[] = status
    ? [
        { label: 'Application', value: 'ACTIVE', tone: 'ok' },
        {
          label: 'Authentication',
          value: status.auth_enabled ? 'enabled' : 'open',
          tone: status.auth_enabled ? 'ok' : 'warn',
        },
        {
          label: 'Indexers',
          value: status.indexer_count > 0 ? 'ACTIVE' : 'none configured',
          tone: status.indexer_count > 0 ? 'ok' : 'warn',
        },
        {
          label: 'Download clients',
          value: status.download_client_count > 0 ? 'ACTIVE' : 'none configured',
          tone: status.download_client_count > 0 ? 'ok' : 'warn',
        },
        {
          label: 'Libraries',
          value: String(status.library_count),
          tone: status.library_count > 0 ? 'ok' : 'neutral',
        },
      ]
    : [];

  const rawDetails = status ? JSON.stringify(status, null, 2) : '';

  // 'Run now': POST the task's command, surface the result via the shared toast.
  const handleRunNow = React.useCallback(
    async (task: SystemTask) => {
      if (running) return; // serialize while one is in flight
      setRunning(task.taskName);
      info(`Queuing ${task.name}…`);
      try {
        await runTaskNow(api, task.taskName);
        success(`${task.name} queued.`);
        // Optimistically reset the countdown so the row reads as just-run.
        setState((prev) => {
          if (!prev.tasks) return prev;
          const now = new Date().toISOString();
          return {
            ...prev,
            tasks: prev.tasks.map((t) =>
              t.taskName === task.taskName ? { ...t, lastExecution: now } : t
            ),
          };
        });
      } catch (err) {
        const message =
          err instanceof ApiError ? err.message : 'failed to queue task';
        toastError(`${task.name} failed: ${message}`);
      } finally {
        setRunning(null);
      }
    },
    [running, info, success, toastError]
  );

  const handleCopyRaw = React.useCallback(async () => {
    try {
      if (typeof navigator !== 'undefined' && navigator.clipboard) {
        await navigator.clipboard.writeText(rawDetails);
        setCopied(true);
        success('Raw status copied to clipboard.');
        window.setTimeout(() => setCopied(false), 2000);
      } else {
        toastError('Clipboard is unavailable in this context.');
      }
    } catch {
      toastError('Could not copy to clipboard.');
    }
  }, [rawDetails, success, toastError]);

  return (
    <AppShell>
      <Card title="System / Status">
        <RowSpaceBetween>
          <Text>Health, scheduled tasks, and diagnostics.</Text>
          {status ? <Badge>{status.version}</Badge> : null}
        </RowSpaceBetween>

        <Divider type="GRADIENT" />

        {loading ? (
          <Text style={{ marginTop: '1ch', opacity: 0.6 }}>
            <BlockLoader mode={0} /> Loading system status…
          </Text>
        ) : null}

        {error ? (
          <AlertBanner style={{ marginTop: '1ch' }}>
            Could not reach the API: {error}
          </AlertBanner>
        ) : null}

        {!loading && !error && !status ? (
          <Message>
            The daemon is not reachable. System status will appear once the API is up.
          </Message>
        ) : null}

        {healthWarning ? (
          <AlertBanner style={{ marginTop: '1ch' }}>{healthWarning}</AlertBanner>
        ) : null}

        {status ? (
          <>
            <Text style={{ marginTop: '1ch', opacity: 0.6, marginBottom: '0.5ch' }}>
              Health
            </Text>
            <Table>
              <TableRow>
                <TableColumn style={{ opacity: 0.6 }}>Check</TableColumn>
                <TableColumn style={{ opacity: 0.6 }}>Status</TableColumn>
              </TableRow>
              {healthRows.map((r) => (
                <TableRow key={r.label}>
                  <TableColumn>{r.label}</TableColumn>
                  <TableColumn>
                    <span style={{ color: TONE_COLOR[r.tone] }}>{r.value}</span>
                  </TableColumn>
                </TableRow>
              ))}
            </Table>
          </>
        ) : null}

        {status ? (
          <>
            <Divider type="GRADIENT" />
            <Text style={{ opacity: 0.6, marginBottom: '0.5ch' }}>Health checks</Text>
            <HealthChecks health={health} />
          </>
        ) : null}

        {status ? (
          <>
            <Divider type="GRADIENT" />
            <Text style={{ opacity: 0.6, marginBottom: '0.5ch' }}>Scheduled tasks</Text>
            <TaskTable
              tasks={tasks}
              commands={commands}
              running={running}
              onRunNow={handleRunNow}
            />
          </>
        ) : null}

        {status ? (
          <>
            <Divider type="GRADIENT" />
            <Accordion title="Raw / Advanced">
              <div style={{ width: '100%' }}>
                <RowSpaceBetween style={{ marginBottom: '0.5ch' }}>
                  <Text style={{ opacity: 0.6 }}>Raw status JSON</Text>
                  <ActionButton onClick={handleCopyRaw} hotkey={copied ? '✓' : '⧉'}>
                    {copied ? 'Copied' : 'Copy'}
                  </ActionButton>
                </RowSpaceBetween>
                <CodeBlock>{rawDetails}</CodeBlock>
              </div>
            </Accordion>
          </>
        ) : null}
      </Card>
    </AppShell>
  );
}

// ---------------------------------------------------------------------------
// Tasks table — composed from SRCL rows (SimpleTable is string-only, so it can
// not host the per-row 'Run now' action). Each row shows cadence, the live
// countdown to the next run, the derived last run, and a Run-now ActionButton.
// Falls back to the native command catalogue when the scheduler surface is
// absent (older daemon).
// ---------------------------------------------------------------------------

interface TaskTableProps {
  tasks: SystemTask[] | null;
  commands: CommandInfo[] | null;
  running: string | null;
  onRunNow: (task: SystemTask) => void;
}

const TaskTable: React.FC<TaskTableProps> = ({ tasks, commands, running, onRunNow }) => {
  // Prefer the rich scheduler tasks; otherwise synthesize rows from the native
  // command catalogue so 'Run now' still works (no schedule metadata, though).
  const rows: SystemTask[] =
    tasks && tasks.length
      ? tasks
      : (commands ?? []).map((c) => ({
          id: c.name,
          name: c.name,
          taskName: c.name,
          interval: 0,
          nextExecution: '',
          lastExecution: null,
          lastStatus: c.description,
        }));

  if (rows.length === 0) {
    return <Message>No tasks are registered.</Message>;
  }

  return (
    <div role="table" aria-label="Scheduled tasks">
      <RowSpaceBetween
        role="row"
        style={{ opacity: 0.6, borderBottom: '1px solid var(--theme-border)', paddingBottom: '0.25ch' }}
      >
        <span style={{ flex: 2 }}>Task</span>
        <span style={{ flex: 1 }}>Interval</span>
        <span style={{ flex: 1 }}>Next run</span>
        <span style={{ flex: 1 }}>Last run</span>
        <span style={{ flex: 1, textAlign: 'right' }}>Action</span>
      </RowSpaceBetween>
      {rows.map((task) => {
        const isRunning = running === task.taskName;
        const statusGlyph =
          task.lastStatus && /fail|error/i.test(task.lastStatus)
            ? '✗'
            : task.lastExecution
              ? '✓'
              : '●';
        return (
          <RowSpaceBetween
            key={String(task.id)}
            role="row"
            style={{ padding: '0.5ch 0', alignItems: 'center' }}
          >
            <span style={{ flex: 2 }}>
              <span aria-hidden style={{ opacity: 0.6, marginRight: '0.5ch' }}>
                {statusGlyph}
              </span>
              {task.name}
            </span>
            <span style={{ flex: 1, opacity: 0.7 }}>
              {task.interval ? formatInterval(task.interval) : '—'}
            </span>
            <span style={{ flex: 1, opacity: 0.7 }} title={formatTimestamp(task.nextExecution)}>
              {task.nextExecution ? formatCountdown(task.nextExecution) : '—'}
            </span>
            <span style={{ flex: 1, opacity: 0.7 }} title={formatTimestamp(task.lastExecution)}>
              {formatTimestamp(task.lastExecution)}
            </span>
            <span style={{ flex: 1, display: 'flex', justifyContent: 'flex-end' }}>
              <ActionButton
                onClick={isRunning ? undefined : () => onRunNow(task)}
                hotkey={isRunning ? '●' : '▸'}
              >
                {isRunning ? 'Running…' : 'Run now'}
              </ActionButton>
            </span>
          </RowSpaceBetween>
        );
      })}
    </div>
  );
};

// ---------------------------------------------------------------------------
// Health checks — the broader /api/v3/health surface. Each entry carries a
// severity (warning|error) + message; the absence of any entry is the canonical
// 'all healthy' state. Composed from SRCL Badge / Text / Message + a row layout.
// ---------------------------------------------------------------------------

interface HealthChecksProps {
  health: HealthCheck[] | null;
}

const SEVERITY_STYLE: Record<string, { glyph: string; color: string; label: string }> = {
  error: { glyph: '✗', color: 'var(--ansi-9-red)', label: 'error' },
  warning: { glyph: '▲', color: 'var(--ansi-3-yellow)', label: 'warning' },
};

const HealthChecks: React.FC<HealthChecksProps> = ({ health }) => {
  // A null list means the health surface was unreachable (best-effort fetch);
  // an empty list means every check passed.
  if (health === null) {
    return (
      <Message>
        The health surface is unavailable. Individual checks will appear once the daemon exposes
        them.
      </Message>
    );
  }

  if (health.length === 0) {
    return (
      <div role="status" aria-live="polite">
        <AlertBanner style={{ background: 'var(--ansi-2-green)', color: 'var(--ansi-15-white)' }}>
          ✓ All health checks passed — no warnings or errors.
        </AlertBanner>
      </div>
    );
  }

  return (
    <div role="list" aria-label="Health checks">
      {health.map((check, index) => {
        const severity = (check.type ?? 'warning').toLowerCase();
        const style = SEVERITY_STYLE[severity] ?? SEVERITY_STYLE.warning;
        return (
          <div
            key={`${check.source ?? 'check'}-${index}`}
            role="listitem"
            style={{
              display: 'flex',
              alignItems: 'baseline',
              gap: '1ch',
              padding: '0.5ch 0',
              borderBottom: '1px solid var(--theme-border)',
            }}
          >
            <span aria-hidden style={{ color: style.color }}>
              {style.glyph}
            </span>
            <Badge>{style.label}</Badge>
            {check.source ? <Text style={{ opacity: 0.7 }}>{check.source}</Text> : null}
            <Text style={{ flex: 1 }}>{check.message ?? '(no message)'}</Text>
          </div>
        );
      })}
    </div>
  );
};
