'use client';

// A placeholder screen body for routes the Screens phase fills in. Composed only
// from SRCL primitives inside the shared AppShell.

import * as React from 'react';

import Card from '@components/Card';
import Text from '@components/Text';
import Badge from '@components/Badge';

import AppShell from '@app/_components/AppShell';

const ScreenPlaceholder: React.FC<{ title: string; note?: string }> = ({ title, note }) => (
  <AppShell>
    <Card title={title}>
      <Text>
        <Badge>placeholder</Badge> This screen is wired into the shell and routing,
        and will be built from SRCL components in the Screens phase.
      </Text>
      {note ? <Text>{note}</Text> : null}
    </Card>
  </AppShell>
);

export default ScreenPlaceholder;
