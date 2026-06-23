'use client';

// Settings hub. SRCL-only: AppShell + ButtonGroup section switcher + the four
// settings sections, each composed entirely from vendored SRCL primitives and
// wired to the /api/v1 client.

import * as React from 'react';

import ButtonGroup from '@components/ButtonGroup';
import Divider from '@components/Divider';

import AppShell from '@app/_components/AppShell';
import QualityProfiles from '@app/settings/_components/QualityProfiles';
import CustomFormats from '@app/settings/_components/CustomFormats';
import IntegrationSection from '@app/settings/_components/IntegrationSection';

type Section = 'profiles' | 'formats' | 'indexers' | 'clients';

const TABS: { id: Section; label: string }[] = [
  { id: 'profiles', label: 'Quality Profiles' },
  { id: 'formats', label: 'Custom Formats' },
  { id: 'indexers', label: 'Indexers' },
  { id: 'clients', label: 'Download Clients' },
];

const INDEXER_IMPLS = ['Torznab', 'Newznab', 'Prowlarr', 'Jackett'];
const CLIENT_IMPLS = ['qBittorrent', 'Transmission', 'Deluge', 'SABnzbd', 'NZBGet'];

export default function Page() {
  const [section, setSection] = React.useState<Section>('profiles');

  return (
    <AppShell>
      <ButtonGroup
        items={TABS.map((t) => ({
          body: t.label,
          selected: section === t.id,
          onClick: () => setSection(t.id),
        }))}
      />
      <Divider type="GRADIENT" />
      <div style={{ marginTop: '1ch' }}>
        {section === 'profiles' ? <QualityProfiles /> : null}
        {section === 'formats' ? <CustomFormats /> : null}
        {section === 'indexers' ? (
          <IntegrationSection
            kind="indexers"
            title="Indexers"
            implementations={INDEXER_IMPLS}
          />
        ) : null}
        {section === 'clients' ? (
          <IntegrationSection
            kind="downloadclients"
            title="Download Clients"
            implementations={CLIENT_IMPLS}
          />
        ) : null}
      </div>
    </AppShell>
  );
}
