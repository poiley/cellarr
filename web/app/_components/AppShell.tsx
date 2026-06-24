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
import Divider from '@components/Divider';
import Text from '@components/Text';

import ThemeBarToggle from '@app/_components/ThemeBarToggle';
import { useCommandPalette } from '@app/_components/CommandPaletteProvider';

interface NavEntry {
  href: string;
  label: string;
}

const NAV: NavEntry[] = [
  { href: '/', label: 'Dashboard' },
  { href: '/library', label: 'Library' },
  { href: '/calendar', label: 'Calendar' },
  { href: '/activity', label: 'Activity' },
  { href: '/history', label: 'History' },
  { href: '/settings', label: 'Settings' },
  { href: '/system', label: 'System' },
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

  return (
    <SidebarLayout defaultSidebarWidth={24} sidebar={sidebar}>
      <Navigation
        logo={
          <>
            <span aria-hidden="true">✸</span>
            {/* Accessible name for the icon-only logo button (SRCL Navigation
                does not expose an aria-label prop, so name it via content). */}
            <span className="sr-only">Open command palette</span>
          </>
        }
        onClickLogo={palette.open}
        left={topCenter}
        right={topRight}
      />
      <main style={{ padding: '2ch' }}>{children}</main>
    </SidebarLayout>
  );
};

export default AppShell;
