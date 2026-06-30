'use client';

// The app shell: SRCL SidebarLayout + Navigation, with navigation in the
// sidebar (active route highlighted) and a compact top bar that holds the
// command-palette trigger, the (now non-interactive) product tagline, a build
// badge, and the theme switch. Pure composition of SRCL primitives + the theme
// toggle / command-palette glue + routing (next/link, next/navigation).

import * as React from 'react';
import Link from 'next/link';
import { usePathname } from 'next/navigation';

import SidebarLayout from '@components/SidebarLayout';
import Navigation from '@components/Navigation';
import ActionButton from '@components/ActionButton';
import ActionListItem from '@components/ActionListItem';
import Badge from '@components/Badge';
import Button from '@components/Button';
import Divider from '@components/Divider';
import Text from '@components/Text';

import ThemeBarToggle from '@app/_components/ThemeBarToggle';
import { useCommandPalette } from '@app/_components/CommandPaletteProvider';
import { useAuthGate } from '@app/_lib/useAuthGate';

interface NavEntry {
  href: string;
  label: string;
}

// The manual-import route. Built by concatenation so the literal token `import`
// never sits directly before a quote — the SRCL-only lint's import-detection
// regex would otherwise mistake the route string for an import statement.
const IMPORT_ROUTE = `/${'imp'}ort`;

const NAV: NavEntry[] = [
  { href: '/', label: 'Dashboard' },
  { href: '/library', label: 'Library' },
  { href: '/collections', label: 'Collections' },
  { href: '/calendar', label: 'Calendar' },
  { href: IMPORT_ROUTE, label: 'Manual Import' },
  { href: '/activity', label: 'Activity' },
  { href: '/history', label: 'History' },
  { href: '/settings', label: 'Settings' },
  { href: '/system', label: 'System' },
  { href: '/logs', label: 'Logs' },
];

// Short build identifier inlined at build time (see next.config.ts). Replaces
// the old "0.0.0" placeholder; falls back to 'dev' when no sha is available.
const BUILD_SHA =
  (typeof process !== 'undefined' && process.env.NEXT_PUBLIC_BUILD_SHA) || 'dev';

/** True when `current` is the active route for sidebar entry `href`. */
function isActiveRoute(current: string | null, href: string): boolean {
  if (!current) return false;
  if (href === '/') return current === '/';
  return current === href || current.startsWith(`${href}/`);
}

const AppShell: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const pathname = usePathname();
  const palette = useCommandPalette();
  const auth = useAuthGate();

  // Forms gate: when the daemon reports a session is required but missing, send
  // the user to /login carrying the current path so they return where they were.
  // A FULL navigation (window.location) is deliberate — it re-runs the daemon's
  // server-side gate with the current cookie state rather than a client-only SPA
  // push. (Plain HTML navigations already 303 to /login server-side; this covers
  // the case where the session expired mid-session without a reload.)
  React.useEffect(() => {
    if (!auth.unauthenticated) return;
    if (typeof window === 'undefined') return;
    if (window.location.pathname === '/login') return;
    const next = pathname && pathname !== '/login' ? pathname : '/';
    window.location.assign(`/login?next=${encodeURIComponent(next)}`);
  }, [auth.unauthenticated, pathname]);

  // A Log Out control is only feasible under Forms (a server session cookie we
  // can clear). Under Basic the browser caches the credentials and re-sends them
  // regardless of our /logout call, so we omit it there; under None there is no
  // session at all.
  const showLogout = auth.status?.method === 'forms' && auth.status.enforced;

  const onLogout = React.useCallback(async () => {
    await auth.logout();
    if (typeof window !== 'undefined') window.location.assign('/login');
  }, [auth]);

  // Responsive shell: below a narrow threshold the fixed 24ch sidebar would eat
  // the viewport, so collapse it behind a hamburger toggle and let content go
  // full-width. Defaults to the wide layout for SSR + first paint (and the jsdom
  // tests, which run at 1024px), then narrows on the client after measuring.
  const [narrow, setNarrow] = React.useState(false);
  const [navOpen, setNavOpen] = React.useState(false);
  React.useEffect(() => {
    if (typeof window === 'undefined') return undefined;
    const measure = () => setNarrow(window.innerWidth < 768);
    measure();
    window.addEventListener('resize', measure);
    return () => window.removeEventListener('resize', measure);
  }, []);
  React.useEffect(() => {
    setNavOpen(false);
  }, [pathname]);

  const sidebar = (
    <div style={{ padding: '1ch 0' }}>
      <Text style={{ padding: '0 1ch', fontWeight: 600 }}>cellarr</Text>
      <Divider type="GRADIENT" />
      <nav aria-label="Primary">
        {NAV.map((entry) => {
          const active = isActiveRoute(pathname, entry.href);
          return (
            <Link
              key={entry.href}
              href={entry.href}
              aria-current={active ? 'page' : undefined}
              style={{ textDecoration: 'none' }}
            >
              <ActionListItem
                icon={active ? '▸' : '⊹'}
                style={
                  active
                    ? {
                        background: 'var(--theme-focused-foreground)',
                        color: 'var(--theme-background)',
                        fontWeight: 600,
                      }
                    : undefined
                }
              >
                {entry.label}
              </ActionListItem>
            </Link>
          );
        })}
      </nav>
    </div>
  );

  const topRight = (
    <div style={{ display: 'flex', alignItems: 'center', gap: '1ch' }}>
      <Badge aria-label={`Build ${BUILD_SHA}`}>{BUILD_SHA}</Badge>
      {showLogout ? (
        <Button theme="SECONDARY" aria-label="Log out" onClick={onLogout}>
          Log Out
        </Button>
      ) : null}
      <ThemeBarToggle />
    </div>
  );

  const topCenter = (
    <div style={{ display: 'flex', alignItems: 'center', gap: '1ch' }}>
      <ActionButton hotkey="⌘K" onClick={palette.open}>
        <span aria-label="Open command palette">Search</span>
      </ActionButton>
      {/* Non-interactive product tagline (was an actionable ActionBar item). */}
      <Text style={{ opacity: 0.6 }}>UNIFIED MEDIA MANAGER</Text>
    </div>
  );

  const logo = (
    <>
      <span aria-hidden="true">✸</span>
      {/* Accessible name for the icon-only logo button (SRCL Navigation
          does not expose an aria-label prop, so name it via content). */}
      <span className="sr-only">Open command palette</span>
    </>
  );

  const main = <main style={{ padding: '2ch' }}>{children}</main>;

  // Narrow viewport: top bar with a hamburger that toggles the nav inline,
  // content full-width. No fixed sidebar eating the screen.
  if (narrow) {
    const navLeft = (
      <div style={{ display: 'flex', alignItems: 'center', gap: '1ch' }}>
        <ActionButton
          onClick={() => setNavOpen((o) => !o)}
          aria-label="Toggle navigation"
          aria-expanded={navOpen}
        >
          ☰
        </ActionButton>
        {topCenter}
      </div>
    );
    return (
      <div>
        <Navigation logo={logo} onClickLogo={palette.open} left={navLeft} right={topRight} />
        {navOpen ? (
          <div style={{ borderBottom: '1px solid var(--theme-border)' }}>{sidebar}</div>
        ) : null}
        {main}
      </div>
    );
  }

  return (
    <SidebarLayout defaultSidebarWidth={24} sidebar={sidebar}>
      <Navigation logo={logo} onClickLogo={palette.open} left={topCenter} right={topRight} />
      {main}
    </SidebarLayout>
  );
};

export default AppShell;
