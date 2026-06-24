'use client';

// First-run onboarding wizard, rendered inside SRCL's ModalStack via the
// ModalContext (opened by a ModalTrigger). SRCL-only: Dialog (the modal frame),
// Input, Select, Checkbox, Button/ButtonGroup, Badge, Divider, Text. It walks
// the user through library + (optional) indexer + (optional) download-client
// setup and POSTs each on finish, then routes to the Library screen.
//
// Endpoints used match the rest of the app:
//   * library          POST /api/v1/libraries  (native; needs default_quality_profile)
//   * indexer          POST /api/v3/indexer     (Radarr-compatible, fields[] shape)
//   * download client  POST /api/v3/downloadclient
// The indexer / download-client fields mirror Settings > Indexers
// (name, implementation, host/baseUrl, api key, port, enabled).

import * as React from 'react';

import { useRouter } from 'next/navigation';

import Dialog from '@components/Dialog';
import Input from '@components/Input';
import Select from '@components/Select';
import ButtonGroup from '@components/ButtonGroup';
import Badge from '@components/Badge';
import Divider from '@components/Divider';
import Text from '@components/Text';

import { useModals } from '@components/page/ModalContext';

import { ApiError, CellarrClient, api as defaultApi } from '@lib/api/client';
import type {
  MediaType,
  QualityProfile,
  IndexerConfigV3,
  DownloadClientConfigV3,
} from '@lib/api/types';

import { toApiError } from '@app/settings/_components/useAsync';
import {
  ErrorBanner,
  SuccessBanner,
} from '@app/settings/_components/StatusBanners';

const MEDIA_TYPES: MediaType[] = ['movie', 'tv', 'music', 'book'];
// Mirror the implementation lists offered by Settings > Indexers / Clients.
const INDEXER_IMPLS = ['Torznab', 'Newznab', 'Prowlarr', 'Jackett'];
const CLIENT_IMPLS = ['qBittorrent', 'Transmission', 'Deluge', 'SABnzbd', 'NZBGet'];
const STEPS = ['Welcome', 'Library', 'Indexer', 'Download client', 'Finish'] as const;

// Torrent vs usenet drives the protocol/configContract the v3 shim expects.
const USENET_INDEXERS = new Set(['Newznab']);
const USENET_CLIENTS = new Set(['SABnzbd', 'NZBGet']);

function indexerProtocol(impl: string): 'usenet' | 'torrent' {
  return USENET_INDEXERS.has(impl) ? 'usenet' : 'torrent';
}

function clientProtocol(impl: string): 'usenet' | 'torrent' {
  return USENET_CLIENTS.has(impl) ? 'usenet' : 'torrent';
}

export interface WizardModalProps {
  client?: CellarrClient;
  onComplete?: () => void;
}

interface WizardState {
  libraryName: string;
  mediaType: MediaType;
  rootFolder: string;
  qualityProfile: string;
  indexerName: string;
  indexerImpl: string;
  indexerHost: string;
  indexerApiKey: string;
  clientName: string;
  clientImpl: string;
  clientHost: string;
  clientPort: string;
}

const initialState: WizardState = {
  libraryName: 'Movies',
  mediaType: 'movie',
  rootFolder: '/media/movies',
  qualityProfile: '',
  indexerName: '',
  indexerImpl: INDEXER_IMPLS[0],
  indexerHost: '',
  indexerApiKey: '',
  clientName: '',
  clientImpl: CLIENT_IMPLS[0],
  clientHost: '',
  clientPort: '',
};

const WizardModal: React.FC<WizardModalProps> = ({ client = defaultApi, onComplete }) => {
  const { close } = useModals();
  const router = useRouter();
  const [step, setStep] = React.useState(0);
  const [state, setState] = React.useState<WizardState>(initialState);
  const [profiles, setProfiles] = React.useState<QualityProfile[]>([]);
  const [submitting, setSubmitting] = React.useState(false);
  const [done, setDone] = React.useState(false);
  const [error, setError] = React.useState<ApiError | undefined>(undefined);

  const set = (patch: Partial<WizardState>) => setState((s) => ({ ...s, ...patch }));

  // Load quality profiles once: the library POST requires a default profile id,
  // and we let the user pick a human-readable name (defaulting to the first).
  React.useEffect(() => {
    const controller = new AbortController();
    let active = true;
    client
      .getQualityProfiles(controller.signal)
      .then((list) => {
        if (!active) return;
        setProfiles(list);
        if (list.length) {
          setState((s) => (s.qualityProfile ? s : { ...s, qualityProfile: list[0].id }));
        }
      })
      .catch(() => {
        // A missing profile list is non-fatal here; the finish step surfaces the
        // resulting API error if the daemon rejects the create.
      });
    return () => {
      active = false;
      controller.abort();
    };
  }, [client]);

  const profileNames = profiles.map((p) => p.name);
  const idForName = (name: string): string =>
    profiles.find((p) => p.name === name)?.id ?? state.qualityProfile;
  const nameForId = (id: string): string => profiles.find((p) => p.id === id)?.name ?? '';

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
          default_quality_profile: state.qualityProfile,
        },
      });

      if (state.indexerHost.trim()) {
        const protocol = indexerProtocol(state.indexerImpl);
        const body: Partial<IndexerConfigV3> = {
          name: state.indexerName || state.indexerImpl,
          implementation: state.indexerImpl,
          configContract: `${state.indexerImpl}Settings`,
          protocol,
          priority: 25,
          enableRss: true,
          enableAutomaticSearch: true,
          enableInteractiveSearch: true,
          fields: [
            { name: 'baseUrl', value: state.indexerHost },
            ...(state.indexerApiKey.trim()
              ? [{ name: 'apiKey', value: state.indexerApiKey }]
              : []),
          ],
          tags: [],
        };
        await client.createIndexer(body);
      }

      if (state.clientHost.trim()) {
        const protocol = clientProtocol(state.clientImpl);
        const body: Partial<DownloadClientConfigV3> = {
          name: state.clientName || state.clientImpl,
          implementation: state.clientImpl,
          configContract: `${state.clientImpl}Settings`,
          protocol,
          priority: 1,
          enable: true,
          fields: [
            { name: 'host', value: state.clientHost },
            ...(state.clientPort.trim()
              ? [{ name: 'port', value: Number.parseInt(state.clientPort, 10) }]
              : []),
          ],
          tags: [],
        };
        await client.createDownloadClient(body);
      }

      setDone(true);
      onComplete?.();
    } catch (err) {
      setError(toApiError(err));
    } finally {
      setSubmitting(false);
    }
  };

  const goToLibrary = () => {
    close();
    router.push('/library/');
  };

  const body = (() => {
    if (done) {
      return (
        <>
          <SuccessBanner>Setup complete — your library is ready.</SuccessBanner>
          <div style={{ marginTop: '1ch' }}>
            <ButtonGroup
              items={[
                { body: 'Go to Library', onClick: goToLibrary },
                { body: 'Close', onClick: () => close() },
              ]}
            />
          </div>
        </>
      );
    }

    switch (step) {
      case 0:
        return (
          <Text>
            Welcome to cellarr. This quick wizard sets up your first library, and optionally an
            indexer and a download client. The indexer and download client are skippable — you can
            change everything later in Settings.
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
            {profileNames.length ? (
              <div style={{ margin: '1ch 0' }}>
                <Text style={{ opacity: 0.6 }}>Quality profile</Text>
                <Select
                  name="wiz-quality-profile"
                  options={profileNames}
                  defaultValue={nameForId(state.qualityProfile)}
                  onChange={(value) => set({ qualityProfile: idForName(value) })}
                />
              </div>
            ) : null}
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
              <Text style={{ opacity: 0.6 }}>Implementation</Text>
              <Select
                name="wiz-indexer-impl"
                options={INDEXER_IMPLS}
                defaultValue={state.indexerImpl}
                onChange={(value) => set({ indexerImpl: value })}
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
              <Text style={{ opacity: 0.6 }}>Implementation</Text>
              <Select
                name="wiz-client-impl"
                options={CLIENT_IMPLS}
                defaultValue={state.clientImpl}
                onChange={(value) => set({ clientImpl: value })}
              />
            </div>
            <div style={{ display: 'flex', gap: '1ch' }}>
              <div style={{ flex: 2, margin: '1ch 0' }}>
                <Text style={{ opacity: 0.6 }}>Host</Text>
                <Input
                  name="wiz-client-host"
                  aria-label="Download client host"
                  placeholder="localhost"
                  value={state.clientHost}
                  onChange={(e) => set({ clientHost: e.target.value })}
                />
              </div>
              <div style={{ flex: 1, margin: '1ch 0' }}>
                <Text style={{ opacity: 0.6 }}>Port</Text>
                <Input
                  name="wiz-client-port"
                  aria-label="Download client port"
                  type="number"
                  placeholder="8080"
                  value={state.clientPort}
                  onChange={(e) => set({ clientPort: e.target.value })}
                />
              </div>
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
                {nameForId(state.qualityProfile)
                  ? ` · ${nameForId(state.qualityProfile)}`
                  : ''}
              </li>
              <li>
                <Badge>indexer</Badge>{' '}
                {state.indexerHost.trim()
                  ? `${state.indexerName || state.indexerImpl} @ ${state.indexerHost}`
                  : 'skipped'}
              </li>
              <li>
                <Badge>client</Badge>{' '}
                {state.clientHost.trim()
                  ? `${state.clientName || state.clientImpl} @ ${state.clientHost}`
                  : 'skipped'}
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

  // Dialog supplies its own OK/Cancel; we drive navigation with our own
  // ButtonGroup, so Dialog's confirm/cancel just close the modal.
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
