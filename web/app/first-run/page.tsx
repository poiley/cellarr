'use client';

// First-run onboarding screen. SRCL-only: AppShell + Card + Button +
// ModalTrigger (launches the wizard) + ModalStack (renders the modal layer).
// The wizard itself is a SRCL Dialog driven through the ModalContext.
//
// The wizard prompt only appears when there are NO libraries yet — once a
// library exists, first-run is "complete" and we surface a shortcut to the
// Library screen instead of re-running setup. Setup is fully skippable from
// both this screen (Skip for now) and inside the wizard (Skip setup).

import * as React from 'react';

import { useRouter } from 'next/navigation';

import Card from '@components/Card';
import Button from '@components/Button';
import Text from '@components/Text';
import Badge from '@components/Badge';
import Divider from '@components/Divider';
import BlockLoader from '@components/BlockLoader';
import ModalStack from '@components/ModalStack';
import ModalTrigger from '@components/ModalTrigger';

import AppShell from '@app/_components/AppShell';
import WizardModal from '@app/first-run/_components/WizardModal';
import { api, ApiError } from '@lib/api/client';
import type { Library } from '@lib/api/types';

export default function Page() {
  const router = useRouter();
  // null = still loading; [] = no libraries (show wizard); [...] = already set up.
  const [libraries, setLibraries] = React.useState<Library[] | null>(null);
  const [errored, setErrored] = React.useState(false);

  React.useEffect(() => {
    const controller = new AbortController();
    api
      .listLibraries(controller.signal)
      .then((list) => setLibraries(list))
      .catch((err: unknown) => {
        // Treat an unreachable/erroring daemon as "no libraries yet" so the
        // wizard still offers itself rather than hanging on the loader.
        if (!(err instanceof ApiError && err.code === 'network_error')) {
          setErrored(true);
        }
        setLibraries([]);
      });
    return () => controller.abort();
  }, []);

  const goToLibrary = () => router.push('/library/');

  const loading = libraries === null;
  const hasLibraries = !!libraries && libraries.length > 0;

  return (
    <AppShell>
      <Card title="Welcome to cellarr">
        {loading ? (
          <Text style={{ opacity: 0.6 }}>
            <BlockLoader mode={0} /> Checking your setup…
          </Text>
        ) : hasLibraries ? (
          <>
            <Text>
              <Badge>ready</Badge> You already have{' '}
              {libraries!.length === 1 ? 'a library' : `${libraries!.length} libraries`} set up —
              first-run setup is complete.
            </Text>
            <Divider type="GRADIENT" />
            <div style={{ marginTop: '1ch' }}>
              <Button onClick={goToLibrary}>Go to Library</Button>
            </div>
          </>
        ) : (
          <>
            <Text>
              <Badge>first run</Badge> Let&rsquo;s get your media manager set up. The wizard creates
              your first library and optionally wires an indexer and a download client. Every step is
              skippable — you can change everything later in Settings.
              {errored ? ' (The daemon was unreachable; setup will still try to save.)' : ''}
            </Text>
            <Divider type="GRADIENT" />
            <div style={{ marginTop: '1ch', display: 'flex', gap: '1ch' }}>
              <ModalTrigger modal={WizardModal}>
                <Button>Start setup</Button>
              </ModalTrigger>
              <Button theme="SECONDARY" onClick={goToLibrary}>
                Skip for now
              </Button>
            </div>
          </>
        )}
      </Card>
      <ModalStack />
    </AppShell>
  );
}
