'use client';

// First-run onboarding wizard, rendered inside SRCL's ModalStack via the
// ModalContext (opened by a ModalTrigger). SRCL-only: Dialog (the modal frame),
// Input, Select, Checkbox, Button/ButtonGroup, Badge, Divider, Text. It walks
// the user through library + indexer + download-client setup and POSTs each to
// the /api/v1 client on finish.

import * as React from 'react';

import Dialog from '@components/Dialog';
import Input from '@components/Input';
import Select from '@components/Select';
import Button from '@components/Button';
import ButtonGroup from '@components/ButtonGroup';
import Badge from '@components/Badge';
import Divider from '@components/Divider';
import Text from '@components/Text';

import { useModals } from '@components/page/ModalContext';

import { ApiError, CellarrClient, api as defaultApi } from '@lib/api/client';
import type { MediaType } from '@lib/api/types';

import { toApiError } from '@app/settings/_components/useAsync';
import {
  ErrorBanner,
  SuccessBanner,
} from '@app/settings/_components/StatusBanners';

const MEDIA_TYPES: MediaType[] = ['movie', 'tv', 'music', 'book'];
const STEPS = ['Welcome', 'Library', 'Indexer', 'Download client', 'Finish'] as const;

export interface WizardModalProps {
  client?: CellarrClient;
  onComplete?: () => void;
}

interface WizardState {
  libraryName: string;
  mediaType: MediaType;
  rootFolder: string;
  indexerName: string;
  indexerHost: string;
  indexerApiKey: string;
  clientName: string;
  clientHost: string;
}

const initialState: WizardState = {
  libraryName: 'Movies',
  mediaType: 'movie',
  rootFolder: '/media/movies',
  indexerName: '',
  indexerHost: '',
  indexerApiKey: '',
  clientName: '',
  clientHost: '',
};

const WizardModal: React.FC<WizardModalProps> = ({ client = defaultApi, onComplete }) => {
  const { close } = useModals();
  const [step, setStep] = React.useState(0);
  const [state, setState] = React.useState<WizardState>(initialState);
  const [submitting, setSubmitting] = React.useState(false);
  const [done, setDone] = React.useState(false);
  const [error, setError] = React.useState<ApiError | undefined>(undefined);

  const set = (patch: Partial<WizardState>) => setState((s) => ({ ...s, ...patch }));

  const canAdvance = (): boolean => {
    if (step === 1) return state.libraryName.trim() !== '' && state.rootFolder.trim() !== '';
    return true;
  };

  const finish = async () => {
    setSubmitting(true);
    setError(undefined);
    try {
      await client.request<unknown>('/libraries', {
        method: 'POST',
        body: {
          name: state.libraryName,
          media_type: state.mediaType,
          root_folders: [state.rootFolder],
        },
      });
      if (state.indexerHost.trim()) {
        await client.request<unknown>('/indexers', {
          method: 'POST',
          body: {
            name: state.indexerName || 'Indexer',
            host: state.indexerHost,
            api_key: state.indexerApiKey || undefined,
            enabled: true,
          },
        });
      }
      if (state.clientHost.trim()) {
        await client.request<unknown>('/downloadclients', {
          method: 'POST',
          body: {
            name: state.clientName || 'Download client',
            host: state.clientHost,
            enabled: true,
          },
        });
      }
      setDone(true);
      onComplete?.();
    } catch (err) {
      setError(toApiError(err));
    } finally {
      setSubmitting(false);
    }
  };

  const body = (() => {
    if (done) {
      return (
        <>
          <SuccessBanner>Setup complete — your library is ready.</SuccessBanner>
          <div style={{ marginTop: '1ch' }}>
            <ButtonGroup items={[{ body: 'Close', onClick: () => close() }]} />
          </div>
        </>
      );
    }

    switch (step) {
      case 0:
        return (
          <Text>
            Welcome to cellarr. This quick wizard sets up your first library, an indexer, and a
            download client. You can change everything later in Settings.
          </Text>
        );
      case 1:
        return (
          <>
            <div style={{ margin: '1ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Library name</Text>
              <Input
                name="wiz-library-name"
                aria-label="Library name"
                value={state.libraryName}
                onChange={(e) => set({ libraryName: e.target.value })}
              />
            </div>
            <div style={{ margin: '1ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Media type</Text>
              <Select
                name="wiz-media-type"
                options={MEDIA_TYPES as string[]}
                defaultValue={state.mediaType as string}
                onChange={(value) => set({ mediaType: value })}
              />
            </div>
            <div style={{ margin: '1ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Root folder</Text>
              <Input
                name="wiz-root-folder"
                aria-label="Root folder"
                value={state.rootFolder}
                onChange={(e) => set({ rootFolder: e.target.value })}
              />
            </div>
          </>
        );
      case 2:
        return (
          <>
            <Text style={{ opacity: 0.6 }}>Optional — add an indexer now (or skip).</Text>
            <div style={{ margin: '1ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Name</Text>
              <Input
                name="wiz-indexer-name"
                aria-label="Indexer name"
                value={state.indexerName}
                onChange={(e) => set({ indexerName: e.target.value })}
              />
            </div>
            <div style={{ margin: '1ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Host URL</Text>
              <Input
                name="wiz-indexer-host"
                aria-label="Indexer host"
                placeholder="http://localhost:9117"
                value={state.indexerHost}
                onChange={(e) => set({ indexerHost: e.target.value })}
              />
            </div>
            <div style={{ margin: '1ch 0' }}>
              <Text style={{ opacity: 0.6 }}>API key</Text>
              <Input
                name="wiz-indexer-key"
                aria-label="Indexer API key"
                type="password"
                value={state.indexerApiKey}
                onChange={(e) => set({ indexerApiKey: e.target.value })}
              />
            </div>
          </>
        );
      case 3:
        return (
          <>
            <Text style={{ opacity: 0.6 }}>Optional — add a download client now (or skip).</Text>
            <div style={{ margin: '1ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Name</Text>
              <Input
                name="wiz-client-name"
                aria-label="Download client name"
                value={state.clientName}
                onChange={(e) => set({ clientName: e.target.value })}
              />
            </div>
            <div style={{ margin: '1ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Host URL</Text>
              <Input
                name="wiz-client-host"
                aria-label="Download client host"
                placeholder="http://localhost:8080"
                value={state.clientHost}
                onChange={(e) => set({ clientHost: e.target.value })}
              />
            </div>
          </>
        );
      case 4:
        return (
          <>
            <Text>Review and create your setup:</Text>
            <ul style={{ listStyle: 'none', padding: 0 }}>
              <li>
                <Badge>library</Badge> {state.libraryName} ({state.mediaType}) → {state.rootFolder}
              </li>
              <li>
                <Badge>indexer</Badge>{' '}
                {state.indexerHost.trim() ? `${state.indexerName || 'Indexer'} @ ${state.indexerHost}` : 'skipped'}
              </li>
              <li>
                <Badge>client</Badge>{' '}
                {state.clientHost.trim() ? `${state.clientName || 'Client'} @ ${state.clientHost}` : 'skipped'}
              </li>
            </ul>
            {error ? <ErrorBanner error={error} /> : null}
            {submitting ? <Text role="status">Creating…</Text> : null}
          </>
        );
      default:
        return null;
    }
  })();

  const footer = done ? null : (
    <div style={{ marginTop: '1ch' }}>
      <ButtonGroup
        items={[
          ...(step > 0 ? [{ body: 'Back', onClick: () => setStep((s) => s - 1) }] : []),
          ...(step < STEPS.length - 1
            ? [
                {
                  body: 'Next',
                  onClick: canAdvance() ? () => setStep((s) => s + 1) : undefined,
                },
              ]
            : [
                {
                  body: submitting ? 'Creating…' : 'Create library',
                  onClick: submitting ? undefined : finish,
                },
              ]),
          { body: 'Cancel', onClick: () => close() },
        ]}
      />
    </div>
  );

  // Dialog already supplies OK/Cancel buttons; we drive navigation with our own
  // ButtonGroup, so we leave Dialog's confirm/cancel as Close handlers.
  return (
    <div role="document">
      <Dialog
        title={
          <span>
            First-run setup <Badge>{STEPS[step]}</Badge>{' '}
            <span style={{ opacity: 0.5 }}>
              {done ? '' : `step ${step + 1} of ${STEPS.length}`}
            </span>
          </span>
        }
        onConfirm={() => close()}
        onCancel={() => close()}
        style={{ maxWidth: '64ch', width: '100%' }}
      >
        {body}
        <Divider type="GRADIENT" />
        {footer}
      </Dialog>
    </div>
  );
};

export default WizardModal;
