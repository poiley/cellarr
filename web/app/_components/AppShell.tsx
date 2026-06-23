'use client';

// The app shell: SRCL SidebarLayout + Navigation + ActionBar, with the theme
// toggle wired into the sidebar. Pure composition of SRCL primitives + the
// theme toggle (itself built from SRCL RadioButton) + routing glue (next/link).

import * as React from 'react';
import Link from 'next/link';

import SidebarLayout from '@components/SidebarLayout';
import Navigation from '@components/Navigation';
import ActionBar from '@components/ActionBar';
import ActionListItem from '@components/ActionListItem';
import Divider from '@components/Divider';
import Text from '@components/Text';

import ThemeToggle from '@app/_components/ThemeToggle';

interface NavEntry {
  href: string;
  label: string;
}

const NAV: NavEntry[] = [
  { href: '/', label: 'Dashboard' },
  { href: '/library', label: 'Library' },
  { href: '/activity', label: 'Activity' },
  { href: '/history', label: 'History' },
  { href: '/settings', label: 'Settings' },
  { href: '/system', label: 'System' },
];

const AppShell: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const sidebar = (
    <div style={{ padding: '1ch 0' }}>
      <Text style={{ padding: '0 1ch', fontWeight: 600 }}>cellarr</Text>
      <Divider type="GRADIENT" />
      <nav aria-label="Primary">
        {NAV.map((entry) => (
          <Link key={entry.href} href={entry.href} style={{ textDecoration: 'none' }}>
            <ActionListItem icon={`⊹`}>{entry.label}</ActionListItem>
          </Link>
        ))}
      </nav>
      <Divider type="GRADIENT" />
      <div style={{ padding: '1ch' }}>
        <Text style={{ opacity: 0.6, marginBottom: '0.5ch' }}>Theme</Text>
        <ThemeToggle />
      </div>
    </div>
  );

  const actions = [
    { hotkey: '⌘1', body: 'cellarr' },
    { body: 'Unified media manager' },
  ];

  return (
    <SidebarLayout defaultSidebarWidth={24} sidebar={sidebar}>
      <Navigation
        logo="✸"
        right={<ActionBar items={actions} />}
      />
      <main style={{ padding: '2ch' }}>{children}</main>
    </SidebarLayout>
  );
};

export default AppShell;
