'use client';

// Home / dashboard. Composed from SRCL primitives; reads system status via the
// typed API client. Other screens are filled in the Screens phase.

import * as React from 'react';

import Card from '@components/Card';
import Row from '@components/Row';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Badge from '@components/Badge';
import Text from '@components/Text';

import AppShell from '@app/_components/AppShell';
import { api, ApiError } from '@lib/api/client';
import type { SystemStatus } from '@lib/api/types';

export default function HomePage() {
  const [status, setStatus] = React.useState<SystemStatus | null>(null);
  const [error, setError] = React.useState<string | null>(null);

  React.useEffect(() => {
    const controller = new AbortController();
    api
      .systemStatus(controller.signal)
      .then(setStatus)
      .catch((err: unknown) => {
        if (err instanceof ApiError && err.code === 'network_error') return;
        setError(err instanceof Error ? err.message : 'failed to load status');
      });
    return () => controller.abort();
  }, []);

  return (
    <AppShell>
      <Card title="Dashboard">
        {error ? <Text>Could not reach the API: {error}</Text> : null}
        {!status && !error ? <Text>Loading system status…</Text> : null}
        {status ? (
          <>
            <RowSpaceBetween>
              <Text>{status.app_name}</Text>
              <Badge>{status.version}</Badge>
            </RowSpaceBetween>
            <Row>
              <Text>Libraries: {status.library_count}</Text>
            </Row>
            <Row>
              <Text>Indexers: {status.indexer_count}</Text>
            </Row>
            <Row>
              <Text>Download clients: {status.download_client_count}</Text>
            </Row>
            <Row>
              <Text>Auth: {status.auth_enabled ? 'enabled' : 'open'}</Text>
            </Row>
          </>
        ) : null}
      </Card>
    </AppShell>
  );
}
