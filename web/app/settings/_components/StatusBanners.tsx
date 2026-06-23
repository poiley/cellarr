'use client';

// Loading / error / empty / success feedback assembled from SRCL primitives
// (AlertBanner, BlockLoader, Text, Badge). Success/error variants are tinted
// with SRCL's own --ansi-* theme tokens, so both themes stay correct.

import * as React from 'react';

import AlertBanner from '@components/AlertBanner';
import BlockLoader from '@components/BlockLoader';
import Text from '@components/Text';
import Badge from '@components/Badge';

import type { ApiError } from '@lib/api/client';

export const Loading: React.FC<{ label?: string }> = ({ label = 'Loading' }) => (
  <Text role="status" aria-live="polite">
    <BlockLoader mode={1} /> {label}…
  </Text>
);

export const ErrorBanner: React.FC<{ error: ApiError }> = ({ error }) => (
  <div role="alert">
    <AlertBanner style={{ background: 'var(--ansi-9-red)', color: 'var(--ansi-15-white)' }}>
      <Badge>{error.code}</Badge> {error.message}
    </AlertBanner>
  </div>
);

export const SuccessBanner: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div role="status">
    <AlertBanner style={{ background: 'var(--ansi-2-green)', color: 'var(--ansi-15-white)' }}>
      {children}
    </AlertBanner>
  </div>
);

export const EmptyState: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div role="status">
    <AlertBanner>{children}</AlertBanner>
  </div>
);
