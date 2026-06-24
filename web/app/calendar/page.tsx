'use client';

// Calendar screen — an upcoming/calendar view of the library's dated items
// (movie release dates + TV episode air dates), grouped by day. Reads the daemon's
// JSON calendar feed (`GET /api/v3/calendar?start&end`, see
// crates/cellarr-api/src/calendar.rs) over a forward window and renders one SRCL
// Card per day, each row deep-linking into the item-detail screen.
//
// Composed exclusively from vendored SRCL primitives + the typed API client and
// the pure grouping helpers in _lib/calendar. The only non-component glue is
// routing (next/link), data (the API client), and the shared toast.

import * as React from 'react';
import Link from 'next/link';

import AlertBanner from '@components/AlertBanner';
import Badge from '@components/Badge';
import BlockLoader from '@components/BlockLoader';
import Button from '@components/Button';
import Card from '@components/Card';
import Divider from '@components/Divider';
import Row from '@components/Row';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Text from '@components/Text';

import AppShell from '@app/_components/AppShell';
import { useToast } from '@app/_lib/ToastProvider';
import { api, ApiError } from '@lib/api/client';
import {
  countEntries,
  dayHeading,
  groupByDay,
  isoDate,
  type CalendarDay,
  type CalendarItem,
} from './_lib/calendar';

// The forward window the screen requests, in days. A generous default so a sparse
// library still surfaces something; users can widen it.
const WINDOW_OPTIONS = [
  { label: '2 weeks', days: 14 },
  { label: '1 month', days: 30 },
  { label: '3 months', days: 90 },
];

type LoadState =
  | { phase: 'loading' }
  | { phase: 'error'; message: string }
  | { phase: 'ready'; days: CalendarDay[] };

function errorMessage(err: unknown, fallback: string): string {
  if (err instanceof ApiError) return `${err.message} (${err.code})`;
  return err instanceof Error ? err.message : fallback;
}

/** Fetch the calendar window [today, today+days] via the v3 escape hatch. */
async function fetchWindow(days: number, signal: AbortSignal): Promise<CalendarItem[]> {
  const now = new Date();
  const end = new Date(now.getTime() + days * 24 * 60 * 60 * 1000);
  return api.requestV3<CalendarItem[]>('/calendar', {
    query: { start: isoDate(now), end: isoDate(end) },
    signal,
  });
}

function CalendarScreen() {
  const [windowDays, setWindowDays] = React.useState(14);
  const [state, setState] = React.useState<LoadState>({ phase: 'loading' });
  const { error: toastError } = useToast();

  React.useEffect(() => {
    const controller = new AbortController();
    setState({ phase: 'loading' });
    fetchWindow(windowDays, controller.signal)
      .then((items) => setState({ phase: 'ready', days: groupByDay(items) }))
      .catch((err: unknown) => {
        if (controller.signal.aborted) return;
        // An unreachable daemon degrades to an empty calendar (matches the
        // dashboard's behaviour); other failures surface in a banner + toast.
        if (err instanceof ApiError && err.code === 'network_error') {
          setState({ phase: 'ready', days: [] });
          return;
        }
        const message = errorMessage(err, 'failed to load calendar');
        setState({ phase: 'error', message });
        toastError(`Could not load the calendar: ${message}`);
      });
    return () => controller.abort();
  }, [windowDays, toastError]);

  const days = state.phase === 'ready' ? state.days : [];
  const total = countEntries(days);

  return (
    <AppShell>
      <Card title="Calendar">
        <RowSpaceBetween>
          <Text>Upcoming movie releases and episode air dates, grouped by day.</Text>
          {state.phase === 'ready' ? (
            <Badge>
              {total} item{total === 1 ? '' : 's'}
            </Badge>
          ) : null}
        </RowSpaceBetween>

        <Divider type="GRADIENT" />

        {/* Window switcher — an inline segmented control over the forward range. */}
        <Row role="group" aria-label="Window" style={{ gap: '1ch', flexWrap: 'wrap' }}>
          {WINDOW_OPTIONS.map((opt) => {
            const isActive = opt.days === windowDays;
            return (
              <Button
                key={opt.days}
                theme={isActive ? 'PRIMARY' : 'SECONDARY'}
                onClick={() => setWindowDays(opt.days)}
                aria-pressed={isActive}
              >
                {isActive ? '● ' : '○ '}
                {opt.label}
              </Button>
            );
          })}
        </Row>
      </Card>

      {state.phase === 'loading' ? (
        <Row style={{ marginTop: '1ch' }}>
          <BlockLoader mode={1} />
          <Text style={{ marginLeft: '1ch' }}>Loading calendar…</Text>
        </Row>
      ) : null}

      {state.phase === 'error' ? (
        <AlertBanner style={{ marginTop: '1ch' }}>
          Could not load the calendar: {state.message}
        </AlertBanner>
      ) : null}

      {state.phase === 'ready' && days.length === 0 ? (
        <Card title="Nothing scheduled" style={{ marginTop: '2ch' }}>
          <Text style={{ opacity: 0.6 }}>
            No dated items in this window. Movie release dates and episode air dates
            appear here once the identify pipeline has resolved them.
          </Text>
          <Divider type="GRADIENT" />
          <Link href="/library" style={{ textDecoration: 'none' }}>
            <Text style={{ opacity: 0.6 }}>Browse the library →</Text>
          </Link>
        </Card>
      ) : null}

      {/* One Card per day, each with its dated rows. */}
      {days.map((day) => (
        <Card key={day.date} title={dayHeading(day.date)} style={{ marginTop: '2ch' }}>
          {day.entries.map((entry, i) => (
            <React.Fragment key={entry.id}>
              {i > 0 ? <Divider type="GRADIENT" /> : null}
              <Link
                href={`/content/?id=${encodeURIComponent(entry.id)}`}
                style={{ textDecoration: 'none', display: 'block' }}
                title={`Open ${entry.title}`}
              >
                <RowSpaceBetween>
                  <Text>
                    <span aria-hidden="true">{entry.hasFile ? '✓' : '●'}</span>{' '}
                    {entry.title}
                  </Text>
                  <Row style={{ gap: '0.5ch', flexWrap: 'wrap' }}>
                    <Badge>{entry.monitored ? 'MONITORED' : 'UNMONITORED'}</Badge>
                    <Badge>{entry.hasFile ? 'DOWNLOADED' : 'MISSING'}</Badge>
                  </Row>
                </RowSpaceBetween>
              </Link>
            </React.Fragment>
          ))}
        </Card>
      ))}
    </AppShell>
  );
}

export default function Page() {
  return <CalendarScreen />;
}
