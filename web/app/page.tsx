'use client';

// Home / dashboard — a fast at-a-glance overview of the instance: library
// totals, what's downloading now, recent grabs/imports, health warnings, and a
// monitored/missing rollup. Each card links to the screen that owns the detail.
//
// Composed exclusively from vendored SRCL primitives + the typed API client and
// the pure aggregation helpers in _lib/dashboard. The only non-component glue is
// routing (next/link) and data (the API client).

import * as React from 'react';
import Link from 'next/link';

import AlertBanner from '@components/AlertBanner';
import Badge from '@components/Badge';
import BarProgress from '@components/BarProgress';
import BlockLoader from '@components/BlockLoader';
import Card from '@components/Card';
import Divider from '@components/Divider';
import Grid from '@components/Grid';
import Row from '@components/Row';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Text from '@components/Text';

import AppShell from '@app/_components/AppShell';
import { api, ApiError } from '@lib/api/client';
import { formatBytes, formatTimestamp } from '@app/_lib/decisionlog';
import {
  activeDownloads,
  downloadProgress,
  historyEventV3Label,
  notableHealth,
  recentHistory,
  summarizeLibrary,
  type MonitoredSummary,
} from '@app/_lib/dashboard';
import type {
  HealthCheck,
  HistoryRecordV3,
  Movie,
  QueueRecord,
  Series,
  SystemStatus,
} from '@lib/api/types';

interface DashboardData {
  status: SystemStatus | null;
  movies: Movie[];
  series: Series[];
  queue: QueueRecord[];
  history: HistoryRecordV3[];
  health: HealthCheck[];
}

const EMPTY: DashboardData = {
  status: null,
  movies: [],
  series: [],
  queue: [],
  history: [],
  health: [],
};

// A typed metric tile: label, value, and the screen it links into.
const Metric: React.FC<{ label: string; value: React.ReactNode; href: string }> = ({
  label,
  value,
  href,
}) => (
  <Link href={href} style={{ textDecoration: 'none' }}>
    <Card title={label} style={{ height: '100%' }}>
      <Text style={{ fontSize: '2ch', fontWeight: 600 }}>{value}</Text>
    </Card>
  </Link>
);

function shortId(id?: string): string {
  if (!id) return '—';
  return id.length > 10 ? `${id.slice(0, 8)}…` : id;
}

export default function HomePage() {
  const [data, setData] = React.useState<DashboardData>(EMPTY);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState<string | null>(null);

  React.useEffect(() => {
    const controller = new AbortController();
    const { signal } = controller;

    // Each panel degrades independently: one failing endpoint must not blank the
    // whole dashboard. settle() folds a rejected call into a fallback value.
    const settle = <T,>(p: Promise<T>, fallback: T): Promise<T> =>
      p.catch((err: unknown) => {
        if (err instanceof ApiError && err.code === 'network_error') return fallback;
        // Surface the first real error but keep rendering the rest.
        setError((prev) => prev ?? (err instanceof Error ? err.message : 'load failed'));
        return fallback;
      });

    Promise.all([
      settle(api.systemStatus(signal), null as SystemStatus | null),
      settle(api.listMovies(signal), [] as Movie[]),
      settle(api.listSeries(signal), [] as Series[]),
      settle(api.getQueueV3(signal), null).then((page) => page?.records ?? []),
      settle(api.getHistoryV3(signal), null).then((page) => page?.records ?? []),
      settle(api.health(signal), [] as HealthCheck[]),
    ])
      .then(([status, movies, series, queue, history, health]) => {
        if (signal.aborted) return;
        setData({ status, movies, series, queue, history, health });
      })
      .finally(() => {
        if (!signal.aborted) setLoading(false);
      });

    return () => controller.abort();
  }, []);

  const summary: MonitoredSummary = React.useMemo(
    () => summarizeLibrary(data.movies, data.series),
    [data.movies, data.series]
  );
  const downloads = React.useMemo(() => activeDownloads(data.queue), [data.queue]);
  const recent = React.useMemo(() => recentHistory(data.history), [data.history]);
  const warnings = React.useMemo(() => notableHealth(data.health), [data.health]);

  return (
    <AppShell>
      <RowSpaceBetween>
        <Text style={{ fontWeight: 600 }}>
          {data.status?.app_name ?? 'cellarr'}
        </Text>
        {data.status ? <Badge>{data.status.version}</Badge> : null}
      </RowSpaceBetween>

      {error ? (
        <AlertBanner style={{ marginTop: '1ch' }}>
          Some data could not be loaded: {error}
        </AlertBanner>
      ) : null}

      {loading ? (
        <Row style={{ marginTop: '1ch' }}>
          <BlockLoader mode={1} />
          <Text style={{ marginLeft: '1ch' }}>Loading overview…</Text>
        </Row>
      ) : null}

      {/* Top-line counts, each linking to the owning screen. */}
      <Grid style={{ marginTop: '1ch' }}>
        <Metric label="Library items" value={summary.total} href="/library" />
        <Metric label="Monitored" value={summary.monitored} href="/library" />
        <Metric label="Missing" value={summary.missing} href="/library" />
        <Metric label="On disk" value={formatBytes(summary.sizeOnDisk)} href="/library" />
        <Metric label="Downloading" value={downloads.length} href="/activity" />
        <Metric
          label="Health"
          value={warnings.length === 0 ? 'OK' : warnings.length}
          href="/system"
        />
      </Grid>

      {/* Health warnings — only shown when there is something to say. */}
      {warnings.length > 0 ? (
        <Card title="Health warnings" style={{ marginTop: '1ch' }}>
          {warnings.map((c, i) => (
            <React.Fragment key={`${c.source ?? c.type ?? 'h'}-${i}`}>
              {i > 0 ? <Divider type="GRADIENT" /> : null}
              <RowSpaceBetween>
                <Text>{c.message ?? c.source ?? 'health check'}</Text>
                <Badge>{(c.type ?? 'warning').toUpperCase()}</Badge>
              </RowSpaceBetween>
            </React.Fragment>
          ))}
        </Card>
      ) : null}

      {/* Filesystem warnings come from the v1 status payload. */}
      {data.status?.filesystem_warnings && data.status.filesystem_warnings.length > 0 ? (
        <Card title="Filesystem warnings" style={{ marginTop: '1ch' }}>
          {data.status.filesystem_warnings.map((w, i) => (
            <Row key={i}>
              <Text>{w}</Text>
            </Row>
          ))}
        </Card>
      ) : null}

      <Grid style={{ marginTop: '1ch' }}>
        {/* What's downloading now. */}
        <Card title="Downloading now">
          {downloads.length === 0 ? (
            <Text style={{ opacity: 0.6 }}>Nothing in flight.</Text>
          ) : (
            downloads.slice(0, 6).map((d, i) => {
              const progress = downloadProgress(d);
              return (
                <React.Fragment key={d.id ?? i}>
                  {i > 0 ? <Divider type="GRADIENT" /> : null}
                  <RowSpaceBetween>
                    <Text>{d.title ?? shortId(d.id)}</Text>
                    <Badge>{(d.status ?? 'queued').toUpperCase()}</Badge>
                  </RowSpaceBetween>
                  {progress !== undefined ? (
                    <BarProgress progress={progress * 100} />
                  ) : null}
                  {typeof d.sizeleft === 'number' && d.sizeleft > 0 ? (
                    <Text style={{ opacity: 0.6 }}>
                      {formatBytes(d.sizeleft)} left
                      {d.timeleft ? ` · ${d.timeleft}` : ''}
                    </Text>
                  ) : null}
                </React.Fragment>
              );
            })
          )}
          <Divider type="GRADIENT" />
          <Link href="/activity" style={{ textDecoration: 'none' }}>
            <Text style={{ opacity: 0.6 }}>View activity →</Text>
          </Link>
        </Card>

        {/* Recent grabs / imports. */}
        <Card title="Recent activity">
          {recent.length === 0 ? (
            <Text style={{ opacity: 0.6 }}>No recent history.</Text>
          ) : (
            recent.map((r, i) => (
              <React.Fragment key={r.id ?? i}>
                {i > 0 ? <Divider type="GRADIENT" /> : null}
                <RowSpaceBetween>
                  <Text>{r.sourceTitle ?? shortId(r.id)}</Text>
                  <Badge>{historyEventV3Label(r.eventType).toUpperCase()}</Badge>
                </RowSpaceBetween>
                {r.date ? (
                  <Text style={{ opacity: 0.6 }}>{formatTimestamp(r.date)}</Text>
                ) : null}
              </React.Fragment>
            ))
          )}
          <Divider type="GRADIENT" />
          <Link href="/history" style={{ textDecoration: 'none' }}>
            <Text style={{ opacity: 0.6 }}>View history →</Text>
          </Link>
        </Card>
      </Grid>
    </AppShell>
  );
}
