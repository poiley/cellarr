import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

const requestV3 = vi.fn();
const push = vi.fn();

vi.mock('@lib/api/client', async () => {
  const actual = await vi.importActual<typeof import('@lib/api/client')>('@lib/api/client');
  return {
    ...actual,
    api: {
      requestV3: (...a: unknown[]) => requestV3(...a),
      // The command palette (mounted in AppShell) lists titles on open; stub so
      // the shell renders without reaching the real client.
      listMovies: () => Promise.resolve([]),
      listSeries: () => Promise.resolve([]),
    },
  };
});

vi.mock('next/navigation', () => ({
  usePathname: () => '/calendar',
  useRouter: () => ({ push }),
  useSearchParams: () => new URLSearchParams(),
}));

import {
  countEntries,
  dayHeading,
  dayOf,
  groupByDay,
  toEntry,
  type CalendarItem,
} from '@app/calendar/_lib/calendar';
import CalendarPage from '@app/calendar/page';
import { ThemeProvider } from '@lib/ThemeProvider';
import { ToastProvider } from '@app/_lib/ToastProvider';

function installMatchMedia() {
  window.matchMedia = vi.fn().mockReturnValue({
    matches: false,
    media: '(prefers-color-scheme: dark)',
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    onchange: null,
    dispatchEvent: () => true,
  }) as unknown as typeof window.matchMedia;
}

function renderPage() {
  return render(
    <ThemeProvider>
      <ToastProvider>
        <CalendarPage />
      </ToastProvider>
    </ThemeProvider>
  );
}

describe('calendar helpers', () => {
  it('dayOf extracts the day from airDate, date, or airDateUtc', () => {
    expect(dayOf({ airDate: '2026-07-04' })).toBe('2026-07-04');
    expect(dayOf({ date: '2026-07-05' })).toBe('2026-07-05');
    expect(dayOf({ airDateUtc: '2026-07-06T00:00:00Z' })).toBe('2026-07-06');
    expect(dayOf({})).toBeUndefined();
  });

  it('toEntry normalizes a row and drops undated rows', () => {
    const ok = toEntry({ id: 'x', title: 'A', airDate: '2026-07-04', monitored: true }, 0);
    expect(ok).toMatchObject({ id: 'x', title: 'A', date: '2026-07-04', monitored: true });
    expect(toEntry({ title: 'no-date' }, 0)).toBeUndefined();
    // Falls back to summary, then to a synthetic id.
    const alt = toEntry({ summary: 'S - S01E01', date: '2026-07-04' }, 3);
    expect(alt).toMatchObject({ title: 'S - S01E01', id: '2026-07-04-3' });
  });

  it('groupByDay buckets and sorts days ascending, dropping undated rows', () => {
    const items: CalendarItem[] = [
      { id: '1', title: 'B', airDate: '2026-07-05' },
      { id: '2', title: 'undated' },
      { id: '3', title: 'A', airDate: '2026-07-04' },
      { id: '4', title: 'C', airDate: '2026-07-05' },
    ];
    const days = groupByDay(items);
    expect(days.map((d) => d.date)).toEqual(['2026-07-04', '2026-07-05']);
    expect(days[1].entries.map((e) => e.id)).toEqual(['1', '4']);
    expect(countEntries(days)).toBe(3);
  });

  it('dayHeading marks today and tomorrow relative to a fixed now', () => {
    const now = new Date(2026, 6, 4); // 2026-07-04 local
    expect(dayHeading('2026-07-04', now)).toMatch(/Today$/);
    expect(dayHeading('2026-07-05', now)).toMatch(/Tomorrow$/);
    expect(dayHeading('2026-07-10', now)).not.toMatch(/Today|Tomorrow/);
  });
});

describe('Calendar screen', () => {
  beforeEach(() => {
    installMatchMedia();
    window.localStorage.clear();
    requestV3.mockReset();
    push.mockReset();
  });
  afterEach(() => cleanup());

  it('renders dated items grouped by day with deep links to content', async () => {
    requestV3.mockResolvedValue([
      { id: 'a', title: 'Movie One', airDate: '2026-07-04', monitored: true, hasFile: false },
      { id: 'b', title: 'Show - S01E02', airDate: '2026-07-04', monitored: true, hasFile: true },
      { id: 'c', title: 'Movie Two', airDate: '2026-07-10', monitored: false, hasFile: false },
    ]);

    renderPage();

    await waitFor(() => {
      expect(screen.getByText('Movie One')).toBeTruthy();
      expect(screen.getByText('Show - S01E02')).toBeTruthy();
      expect(screen.getByText('Movie Two')).toBeTruthy();
    });
    // Total badge reflects the dated count.
    expect(screen.getByText('3 items')).toBeTruthy();
    // The calendar endpoint was queried with a start/end window.
    expect(requestV3).toHaveBeenCalledWith(
      '/calendar',
      expect.objectContaining({
        query: expect.objectContaining({ start: expect.any(String), end: expect.any(String) }),
      })
    );
    // A row deep-links into the item-detail screen.
    const link = screen.getByTitle('Open Movie One') as HTMLAnchorElement;
    expect(link.getAttribute('href')).toContain('/content');
    expect(link.getAttribute('href')).toContain('id=a');
  });

  it('shows an empty state when no dated items are in the window', async () => {
    requestV3.mockResolvedValue([]);
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/Nothing scheduled/i)).toBeTruthy();
    });
  });

  it('re-queries when the window is widened', async () => {
    requestV3.mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(requestV3).toHaveBeenCalled());
    requestV3.mockClear();

    fireEvent.click(screen.getByRole('button', { name: /1 month/i }));
    await waitFor(() => expect(requestV3).toHaveBeenCalledTimes(1));
  });

  it('degrades to an empty calendar when the daemon is unreachable', async () => {
    const { ApiError } = await import('@lib/api/client');
    requestV3.mockRejectedValue(new ApiError('network_error', 'offline', 0));
    renderPage();
    await waitFor(() => {
      expect(screen.getByText(/Nothing scheduled/i)).toBeTruthy();
    });
  });
});
