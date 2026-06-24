'use client';

// A clearly-labelled placeholder for settings sections whose backend does not
// exist yet (Naming config persistence, a Notifications system — see the task
// notes / backend deferrals). Per instructions these are NOT faked with dead
// inputs: the tab states plainly that the feature is not yet available and why.
// SRCL-only: Card, Badge, Text.

import * as React from 'react';

import Card from '@components/Card';
import Badge from '@components/Badge';
import Text from '@components/Text';

export interface ComingSoonProps {
  title: string;
  /** One-line explanation of what the section will do once the backend lands. */
  summary: string;
}

const ComingSoon: React.FC<ComingSoonProps> = ({ title, summary }) => (
  <Card title={title}>
    <div style={{ margin: '0.5ch 0' }}>
      <Badge>● coming soon</Badge>
    </div>
    <Text style={{ opacity: 0.7 }}>{summary}</Text>
    <Text style={{ opacity: 0.5, marginTop: '1ch' }}>
      No backend persistence exists for this yet, so the form is intentionally not shown rather than
      saving to nowhere.
    </Text>
  </Card>
);

export default ComingSoon;
