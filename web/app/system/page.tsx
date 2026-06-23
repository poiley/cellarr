'use client';

// System / Status (docs/10-ui.md §screen-mapping): health + scheduler tasks +
// raw details. Reads /api/v1/system/status and /api/v1/commands. A SimpleTable
// for the health/tasks rows, an AlertBanner for any health advisory, a Message
// for empty/explanatory copy, and a CodeBlock for the raw status JSON.
//
// Composed only from vendored SRCL primitives; the API client is the lone
// non-component data glue.

import * as React from 'react';

import Card from '@components/Card';
import Text from '@components/Text';
import Badge from '@components/Badge';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Divider from '@components/Divider';
import SimpleTable from '@components/SimpleTable';
import AlertBanner from '@components/AlertBanner';
import Message from '@components/Message';
import CodeBlock from '@components/CodeBlock';
import BlockLoader from '@components/BlockLoader';

import AppShell from '@app/_components/AppShell';
import { api, ApiError } from '@lib/api/client';
import type { CommandInfo, SystemStatus } from '@lib/api/types';

interface LoadState {
  status: SystemStatus | null;
  commands: CommandInfo[] | null;
  error: string | null;
  loading: boolean;
}

export default function SystemPage() {
  const [state, setState] = React.useState<LoadState>({
    status: null,
    commands: null,
    error: null,
    loading: true,
  });

  React.useEffect(() => {
    const controller = new AbortController();
    Promise.all([
      api.systemStatus(controller.signal),
      api.getCommands(controller.signal),
    ])
      .then(([status, commands]) => {
        setState({ status, commands, error: null, loading: false });
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

  const { status, commands, error, loading } = state;

  // A small health check derived from the status snapshot: zero indexers means
  // nothing can be searched, which is the canonical first-run warning.
  const healthWarning =
    status && status.indexer_count === 0
      ? 'No indexers configured — searches will return nothing until you add one.'
      : null;

  const healthData: string[][] = status
    ? [
        ['Check', 'Status'],
        ['Application', 'ACTIVE'],
        ['Authentication', status.auth_enabled ? 'enabled' : 'open'],
        ['Indexers', status.indexer_count > 0 ? 'ACTIVE' : 'none configured'],
        [
          'Download clients',
          status.download_client_count > 0 ? 'ACTIVE' : 'none configured',
        ],
        ['Libraries', String(status.library_count)],
      ]
    : [];

  const taskData: string[][] = commands
    ? [['Task', 'Description'], ...commands.map((c) => [c.name, c.description])]
    : [];

  const rawDetails = status ? JSON.stringify(status, null, 2) : '';

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
            <SimpleTable data={healthData} />
          </>
        ) : null}

        {commands ? (
          <>
            <Divider type="GRADIENT" />
            <Text style={{ opacity: 0.6, marginBottom: '0.5ch' }}>Scheduled tasks</Text>
            {commands.length > 0 ? (
              <SimpleTable data={taskData} />
            ) : (
              <Message>No tasks are registered.</Message>
            )}
          </>
        ) : null}

        {status ? (
          <>
            <Divider type="GRADIENT" />
            <Text style={{ opacity: 0.6, marginBottom: '0.5ch' }}>Raw status</Text>
            <CodeBlock>{rawDetails}</CodeBlock>
          </>
        ) : null}
      </Card>
    </AppShell>
  );
}
