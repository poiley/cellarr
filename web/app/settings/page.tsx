'use client';

// Settings hub. SRCL-only: AppShell + ButtonGroup section switcher + the
// settings sections (quality profiles, custom formats, indexers, download
// clients, root folders, remote path mappings, naming, notifications), each
// composed entirely from vendored SRCL primitives and wired to the API client.

import * as React from 'react';

import ButtonGroup from '@components/ButtonGroup';
import Divider from '@components/Divider';

import AppShell from '@app/_components/AppShell';
import QualityProfiles from '@app/settings/_components/QualityProfiles';
import CustomFormats from '@app/settings/_components/CustomFormats';
import DelayProfiles from '@app/settings/_components/DelayProfiles';
import IntegrationSection from '@app/settings/_components/IntegrationSection';
import ImportLists from '@app/settings/_components/ImportLists';
import RootFolders from '@app/settings/_components/RootFolders';
import RemotePathMappings from '@app/settings/_components/RemotePathMappings';
import Notifications from '@app/settings/_components/Notifications';
import Naming from '@app/settings/_components/Naming';

type Section =
  | 'profiles'
  | 'formats'
  | 'delays'
  | 'indexers'
  | 'clients'
  | 'importlists'
  | 'rootfolders'
  | 'remotepaths'
  | 'naming'
  | 'notifications';

const TABS: { id: Section; label: string }[] = [
  { id: 'profiles', label: 'Quality Profiles' },
  { id: 'formats', label: 'Custom Formats' },
  { id: 'delays', label: 'Delay Profiles' },
  { id: 'indexers', label: 'Indexers' },
  { id: 'clients', label: 'Download Clients' },
  { id: 'importlists', label: 'Import Lists' },
  { id: 'rootfolders', label: 'Root Folders' },
  { id: 'remotepaths', label: 'Remote Path Mappings' },
  { id: 'naming', label: 'Naming' },
  { id: 'notifications', label: 'Notifications' },
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
        {section === 'delays' ? <DelayProfiles /> : null}
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
        {section === 'importlists' ? <ImportLists /> : null}
        {section === 'rootfolders' ? <RootFolders /> : null}
        {section === 'remotepaths' ? <RemotePathMappings /> : null}
        {section === 'naming' ? <Naming /> : null}
        {section === 'notifications' ? <Notifications /> : null}
      </div>
    </AppShell>
  );
}
