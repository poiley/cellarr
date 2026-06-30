'use client';

// Home / dashboard — a fast at-a-glance overview of the instance: a compact
// stat strip (library totals, monitored/missing, on-disk size, in-flight
// downloads, health), what's downloading now, recent grabs/imports, recently
// added titles, and (when the backend has dated items) an upcoming calendar.
// Every stat is a deep link into the screen that owns the detail.
//
// Composed exclusively from vendored SRCL primitives + the typed API client and
// the pure aggregation helpers in _lib/dashboard. The only non-component glue is
// routing (next/link), data (the API client), and the shared toast.

import * as React from 'react';
import Link from 'next/link';

import AlertBanner from '@components/AlertBanner';
import Badge from '@components/Badge';
import BarProgress from '@components/BarProgress';
import BlockLoader from '@components/BlockLoader';

import { statusColor } from '@app/_lib/status';
import Card from '@components/Card';
import Divider from '@components/Divider';
import Grid from '@components/Grid';
import Row from '@components/Row';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Text from '@components/Text';

import AppShell from '@app/_components/AppShell';
import { useToast } from '@app/_lib/ToastProvider';
import { api, ApiError } from '@lib/api/client';
import { formatBytes, formatTimestamp } from '@app/_lib/decisionlog';
import {
  activeDownloads,
  downloadProgress,
  fetchCalendar,
  healthSummary,
  historyEventV3Label,
  notableHealth,
  recentHistory,
  recentlyAdded,
  summarizeLibrary,
  upcomingItems,
  type CalendarItem,
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
  calendar: CalendarItem[];
}

const EMPTY: DashboardData = {
  status: null,
  movies: [],
  series: [],
  queue: [],
  history: [],
  health: [],
  calendar: [],
};

// A single dense stat tile in the bento strip: a glyph-or-value over a label,
// the whole cell a deep link into the screen that owns the detail. Tiles share
// a bordered grid cell (the strip draws the borders) so the row reads as one
// compact panel rather than six stacked cards.
const StatTile: React.FC<{
  label: string;
  value: React.ReactNode;
  href: string;
  hint?: string;
  emphasis?: boolean;
}> = ({ label, value, href, hint, emphasis }) => (
  <Link
    href={href}
    style={{ textDecoration: 'none', display: 'block' }}
    title={hint ?? `Open ${label}`}
  >
    <div
      style={{
        border: '1px solid var(--theme-border, var(--theme-text))',
        padding: '0.75ch 1ch',
        minWidth: '14ch',
        height: '100%',
        display: 'flex',
        flexDirection: 'column',
        gap: '0.25ch',
      }}
    >
      <Text
        style={{
          fontSize: '2.2rem',
          fontWeight: 700,
          lineHeight: 1.1,
          color: emphasis ? 'var(--ansi-9-red)' : 'inherit',
        }}
      >
        {emphasis ? (
          <span aria-hidden="true" style={{ fontSize: '0.7em' }}>
            ▲{' '}
          </span>
        ) : null}
        {value}
      </Text>
      <Text style={{ opacity: 0.6, textTransform: 'uppercase', fontSize: '0.75em', letterSpacing: '0.04em' }}>
        {label}
      </Text>
    </div>
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
  const { error: toastError } = useToast();

  React.useEffect(() => {
    const controller = new AbortController();
    const { signal } = controller;

    // Each panel degrades independently: one failing endpoint must not blank the
    // whole dashboard. settle() folds a rejected call into a fallback value, and
    // surfaces the first non-network failure both in the banner and as a toast.
    const settle = <T,>(p: Promise<T>, fallback: T): Promise<T> =>
      p.catch((err: unknown) => {
        if (err instanceof ApiError && err.code === 'network_error') return fallback;
        const message = err instanceof Error ? err.message : 'load failed';
        setError((prev) => {
          if (prev) return prev;
          // First real failure: also nudge the user via the shared toast.
          toastError(`Some dashboard data failed to load: ${message}`);
          return message;
        });
        return fallback;
      });

    Promise.all([
      settle(api.systemStatus(signal), null as SystemStatus | null),
      settle(api.listMovies(signal), [] as Movie[]),
      settle(api.listSeries(signal), [] as Series[]),
      settle(api.getQueueV3(signal), null).then((page) => page?.records ?? []),
      settle(api.getHistoryV3(signal), null).then((page) => page?.records ?? []),
      settle(api.health(signal), [] as HealthCheck[]),
      settle(fetchCalendar(api, 14, signal), [] as CalendarItem[]),
    ])
      .then(([status, movies, series, queue, history, health, calendar]) => {
        if (signal.aborted) return;
        setData({ status, movies, series, queue, history, health, calendar });
      })
      .finally(() => {
        if (!signal.aborted) setLoading(false);
      });

    return () => controller.abort();
    // toastError is stable from the provider; intentionally run once on mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const summary: MonitoredSummary = React.useMemo(
    () => summarizeLibrary(data.movies, data.series),
    [data.movies, data.series]
  );
  const downloads = React.useMemo(() => activeDownloads(data.queue), [data.queue]);
  const recent = React.useMemo(() => recentHistory(data.history), [data.history]);
  const warnings = React.useMemo(() => notableHealth(data.health), [data.health]);
  const health = React.useMemo(() => healthSummary(data.health), [data.health]);
  const added = React.useMemo(
    () => recentlyAdded(data.movies, data.series),
    [data.movies, data.series]
  );
  const upcoming = React.useMemo(() => upcomingItems(data.calendar), [data.calendar]);

  return (
    <AppShell>
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

      {/* Compact bento stat strip — high density, no scroll. Each tile is a deep
          link into the screen that owns the detail. Health is a glyph + word. */}
      <Card title="Overview" style={{ marginTop: '1ch' }}>
        {data.status ? (
          <RowSpaceBetween style={{ marginBottom: '0.5ch' }}>
            <span />
            <Badge>{data.status.version}</Badge>
          </RowSpaceBetween>
        ) : null}
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fit, minmax(14ch, 1fr))',
            gap: '0.5ch',
          }}
        >
          <StatTile label="Items" value={summary.total} href="/library" />
          <StatTile label="Monitored" value={summary.monitored} href="/library" />
          <StatTile
            label="Missing"
            value={summary.missing}
            href="/library"
            emphasis={summary.missing > 0}
            hint="Monitored items with no file — open Library"
          />
          <StatTile
            label="On disk"
            value={formatBytes(summary.sizeOnDisk)}
            href="/library"
          />
          <StatTile
            label="Downloading"
            value={downloads.length}
            href="/activity"
            hint="In-flight downloads — open Activity"
          />
          <StatTile
            label="Health"
            value={
              <span style={{ color: statusColor(health.word) }}>
                <span aria-hidden="true">{health.glyph}</span>{' '}
                {health.word}
              </span>
            }
            href="/system"
            hint={
              health.hasWarnings
                ? `${health.count} health ${health.count === 1 ? 'warning' : 'warnings'} — open System`
                : 'All health checks OK — open System'
            }
          />
        </div>
      </Card>

      {/* Health warnings — only shown when there is something to say. Glyphed,
          never colour-only. */}
      {warnings.length > 0 ? (
        <Card title="Health warnings" style={{ marginTop: '1ch' }}>
          {warnings.map((c, i) => (
            <React.Fragment key={`${c.source ?? c.type ?? 'h'}-${i}`}>
              {i > 0 ? <Divider type="GRADIENT" /> : null}
              <RowSpaceBetween>
                <Text>
                  <span aria-hidden="true">▲</span>{' '}
                  {c.message ?? c.source ?? 'health check'}
                </Text>
                <Badge>{(c.type ?? 'warning').toUpperCase()}</Badge>
              </RowSpaceBetween>
            </React.Fragment>
          ))}
          <Divider type="GRADIENT" />
          <Link href="/system" style={{ textDecoration: 'none' }}>
            <Text style={{ opacity: 0.6 }}>View system →</Text>
          </Link>
        </Card>
      ) : null}

      {/* Filesystem warnings come from the v1 status payload. */}
      {data.status?.filesystem_warnings && data.status.filesystem_warnings.length > 0 ? (
        <Card title="Filesystem warnings" style={{ marginTop: '1ch' }}>
          {data.status.filesystem_warnings.map((w, i) => (
            <Row key={i}>
              <Text>
                <span aria-hidden="true">▲</span> {w}
              </Text>
            </Row>
          ))}
        </Card>
      ) : null}

      <div
        style={{
          display: 'flex',
          flexWrap: 'wrap',
          gap: '2ch',
          marginTop: '1ch',
        }}
      >
        {/* What's downloading now. */}
        <Card title="Downloading now" style={{ flex: '1 1 32ch' }}>
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
        <Card title="Recent activity" style={{ flex: '1 1 32ch' }}>
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
      </div>

      <Grid style={{ marginTop: '1ch' }}>
        {/* Recently added monitored titles. */}
        <Card title="Recently added">
          {added.length === 0 ? (
            <Text style={{ opacity: 0.6 }}>No monitored items yet.</Text>
          ) : (
            added.map((item, i) => (
              <React.Fragment key={item.id}>
                {i > 0 ? <Divider type="GRADIENT" /> : null}
                <Link
                  href={`/content/?id=${encodeURIComponent(item.id)}`}
                  style={{ textDecoration: 'none', display: 'block' }}
                >
                  <RowSpaceBetween>
                    <Text>
                      {item.hasFile ? (
                        <>
                          <span aria-hidden="true">✓</span>{' '}
                        </>
                      ) : null}
                      {item.title}{' '}
                      <span aria-hidden="true" style={{ opacity: 0.6 }}>
                        →
                      </span>
                    </Text>
                    <Badge>{item.kind === 'movie' ? 'MOVIE' : 'SERIES'}</Badge>
                  </RowSpaceBetween>
                  {item.added ? (
                    <Text style={{ opacity: 0.6 }}>{formatTimestamp(item.added)}</Text>
                  ) : null}
                </Link>
              </React.Fragment>
            ))
          )}
          <Divider type="GRADIENT" />
          <Link href="/library" style={{ textDecoration: 'none' }}>
            <Text style={{ opacity: 0.6 }}>View library →</Text>
          </Link>
        </Card>

        {/* Upcoming — only shown when the backend calendar has dated items.
            Today only TV daily-coded episodes carry a self-contained date, so
            most libraries yield an empty calendar (the data-model limitation
            noted by the backend); we then simply omit this panel.
            TODO: when the identify pipeline persists per-item air/release dates
            for movies + standard episodes, this panel will populate for them too
            (no frontend change needed — it already reads the same shape). */}
        {upcoming.length > 0 ? (
          <Card title="Upcoming">
            {upcoming.map((item, i) => {
              const when = item.airDate ?? item.date ?? '';
              const label = item.title ?? item.summary ?? shortId(item.id);
              return (
                <React.Fragment key={item.id ?? `${label}-${i}`}>
                  {i > 0 ? <Divider type="GRADIENT" /> : null}
                  <RowSpaceBetween>
                    <Text>
                      <span aria-hidden="true">{item.hasFile ? '✓' : '●'}</span>{' '}
                      {label}
                    </Text>
                    {when ? <Badge>{when}</Badge> : null}
                  </RowSpaceBetween>
                </React.Fragment>
              );
            })}
          </Card>
        ) : null}
      </Grid>
    </AppShell>
  );
}
