'use client';

// First-run onboarding screen. SRCL-only: AppShell + Card + Button +
// ModalTrigger (launches the wizard) + ModalStack (renders the modal layer).
// The wizard itself is a SRCL Dialog driven through the ModalContext.

import * as React from 'react';

import Card from '@components/Card';
import Button from '@components/Button';
import Text from '@components/Text';
import Badge from '@components/Badge';
import Divider from '@components/Divider';
import ModalStack from '@components/ModalStack';
import ModalTrigger from '@components/ModalTrigger';

import AppShell from '@app/_components/AppShell';
import WizardModal from '@app/first-run/_components/WizardModal';

export default function Page() {
  return (
    <AppShell>
      <Card title="Welcome to cellarr">
        <Text>
          <Badge>first run</Badge> Let&rsquo;s get your media manager set up. The wizard creates your
          first library and optionally wires an indexer and a download client.
        </Text>
        <Divider type="GRADIENT" />
        <div style={{ marginTop: '1ch' }}>
          <ModalTrigger modal={WizardModal}>
            <Button>Start setup</Button>
          </ModalTrigger>
        </div>
      </Card>
      <ModalStack />
    </AppShell>
  );
}
