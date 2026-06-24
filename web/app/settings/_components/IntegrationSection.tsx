'use client';

// Settings — Indexers / Download Clients. Both are integration configs with the
// same shape (host/port/api-key/ssl/enabled, a Test button that surfaces an
// AlertBanner result, and Save). SRCL-only: Card, Input, Select, Checkbox,
// AlertBanner, Button.
//
// Reads GET /api/v1/<kind> (the native snake_case list). Test + Save go through
// the Radarr/Sonarr-compatible /api/v3 shim, because the native /api/v1 surface
// has NO create-test routes and no customformat/test/indexer-test endpoint:
//   * /api/v1/indexers/test, /api/v1/downloadclients/test DO NOT EXIST (they
//     404-fall-through to the SPA index.html, which silently "succeeds");
//   * the working routes are POST /api/v3/{indexer,downloadclient}/test and
//     POST /api/v3/{indexer,downloadclient} (crates/cellarr-api/src/shim.rs).
// The v3 handlers expect a Radarr-shaped body (configContract + protocol +
// fields[]), so the flat form is mapped to that shape via `toV3Body` below
// (mirroring app/first-run WizardModal). Verified against the seeded daemon:
// test returns {isValid:true}, create returns the persisted resource (200).

import * as React from 'react';

import Card from '@components/Card';
import Input from '@components/Input';
import Select from '@components/Select';
import Checkbox from '@components/Checkbox';
import Button from '@components/Button';
import ButtonGroup from '@components/ButtonGroup';
import Badge from '@components/Badge';
import Divider from '@components/Divider';
import Text from '@components/Text';

import { ApiError, CellarrClient, api as defaultApi } from '@lib/api/client';
import type {
  IndexerConfig,
  DownloadClientConfig,
  IndexerConfigV3,
  DownloadClientConfigV3,
} from '@lib/api/types';

import { useToast } from '@app/_lib/ToastProvider';
import { useAsync, toApiError } from '@app/settings/_components/useAsync';
import { Loading, ErrorBanner, EmptyState } from '@app/settings/_components/StatusBanners';
import ConfirmDialog from '@app/settings/_components/ConfirmDialog';

type IntegrationKind = 'indexers' | 'downloadclients';
type RawConfig = IndexerConfig | DownloadClientConfig;

interface IntegrationForm {
  id: string;
  name: string;
  implementation: string;
  host: string;
  port: string;
  apiKey: string;
  ssl: boolean;
  enabled: boolean;
}

interface TestResult {
  ok: boolean;
  message: string;
}

function toForm(raw: RawConfig, implementations: string[]): IntegrationForm {
  const rec = raw as Record<string, unknown>;
  return {
    id: String(rec.id ?? rec.name ?? ''),
    name: String(rec.name ?? ''),
    implementation: String(rec.implementation ?? implementations[0] ?? ''),
    host: String(rec.host ?? ''),
    port: rec.port != null ? String(rec.port) : '',
    apiKey: String(rec.api_key ?? rec.apiKey ?? ''),
    ssl: rec.ssl === true || rec.use_ssl === true,
    enabled: rec.enabled !== false,
  };
}

function blankForm(implementations: string[]): IntegrationForm {
  return {
    id: '',
    name: '',
    implementation: implementations[0] ?? '',
    host: '',
    port: '',
    apiKey: '',
    ssl: false,
    enabled: true,
  };
}

export interface IntegrationSectionProps {
  kind: IntegrationKind;
  title: string;
  implementations: string[];
  client?: CellarrClient;
}

const IntegrationSection: React.FC<IntegrationSectionProps> = ({
  kind,
  title,
  implementations,
  client = defaultApi,
}) => {
  const load = React.useCallback(
    (signal: AbortSignal) =>
      kind === 'indexers' ? client.listIndexers(signal) : client.listDownloadClients(signal),
    [client, kind]
  );
  const { data, loading, error, reload } = useAsync<RawConfig[]>(load);
  const { success, error: toastError, info } = useToast();

  const singular = title.replace(/s$/, '');

  const [form, setForm] = React.useState<IntegrationForm>(() => blankForm(implementations));
  const [testing, setTesting] = React.useState(false);
  const [testResult, setTestResult] = React.useState<TestResult | undefined>(undefined);
  const [saving, setSaving] = React.useState(false);
  const [saveError, setSaveError] = React.useState<ApiError | undefined>(undefined);
  const [pendingDelete, setPendingDelete] = React.useState<IntegrationForm | null>(null);
  const [deleting, setDeleting] = React.useState(false);

  const configs = data ?? [];

  const edit = (raw: RawConfig) => {
    setForm(toForm(raw, implementations));
    setTestResult(undefined);
    setSaveError(undefined);
  };

  const reset = () => {
    setForm(blankForm(implementations));
    setTestResult(undefined);
    setSaveError(undefined);
  };

  // Torrent vs usenet drives the protocol/configContract the v3 shim validates
  // against (mirrors app/first-run WizardModal's mapping).
  const protocolFor = (impl: string): 'usenet' | 'torrent' =>
    /newznab|usenet|sab|nzb/i.test(impl) ? 'usenet' : 'torrent';

  // Map the flat form to the Radarr/Sonarr-shaped body the /api/v3 test + create
  // handlers expect: configContract + protocol + a fields[] array. An indexer's
  // endpoint lives under `baseUrl`; a download client's under `host`/`port`.
  const toV3Body = (): Partial<IndexerConfigV3> & Partial<DownloadClientConfigV3> => {
    const port = form.port ? Number.parseInt(form.port, 10) : undefined;
    const fields =
      kind === 'indexers'
        ? [
            { name: 'baseUrl', value: form.host },
            ...(form.apiKey ? [{ name: 'apiKey', value: form.apiKey }] : []),
          ]
        : [
            { name: 'host', value: form.host },
            ...(port !== undefined ? [{ name: 'port', value: port }] : []),
            { name: 'useSsl', value: form.ssl },
          ];
    return {
      name: form.name,
      implementation: form.implementation,
      configContract: `${form.implementation}Settings`,
      protocol: protocolFor(form.implementation),
      ...(kind === 'indexers'
        ? {
            enableRss: form.enabled,
            enableAutomaticSearch: form.enabled,
            enableInteractiveSearch: form.enabled,
          }
        : { enable: form.enabled }),
      fields,
      tags: [],
    } as Partial<IndexerConfigV3> & Partial<DownloadClientConfigV3>;
  };

  const test = async () => {
    if (!form.name.trim()) {
      toastError('Give it a name before testing.');
      return;
    }
    setTesting(true);
    setTestResult(undefined);
    info('Testing connection…', { durationMs: 2000 });
    try {
      const body = toV3Body();
      if (kind === 'indexers') await client.testIndexer(body);
      else await client.testDownloadClient(body);
      setTestResult({ ok: true, message: 'Connection successful.' });
      success('Connection successful.');
    } catch (err) {
      const e = toApiError(err);
      setTestResult({ ok: false, message: `${e.code}: ${e.message}` });
      toastError(`Test failed — ${e.message}`);
    } finally {
      setTesting(false);
    }
  };

  const save = async () => {
    if (!form.name.trim()) {
      toastError('Give it a name before saving.');
      return;
    }
    setSaving(true);
    setSaveError(undefined);
    try {
      const body = toV3Body();
      if (kind === 'indexers') await client.createIndexer(body);
      else await client.createDownloadClient(body);
      success(`${singular} saved.`);
      reset();
      reload();
    } catch (err) {
      const e = toApiError(err);
      setSaveError(e);
      toastError(`Could not save — ${e.message}`);
    } finally {
      setSaving(false);
    }
  };

  const confirmDelete = async () => {
    if (!pendingDelete) return;
    const numericId = Number.parseInt(pendingDelete.id, 10);
    if (!Number.isFinite(numericId)) {
      // The native list keys configs by an opaque id; the v3 delete route is
      // addressed by a numeric id. Surface this rather than firing a request
      // that cannot resolve.
      toastError('This entry cannot be deleted from here (no numeric id).');
      setPendingDelete(null);
      return;
    }
    setDeleting(true);
    try {
      if (kind === 'indexers') await client.deleteIndexer(numericId);
      else await client.deleteDownloadClient(numericId);
      success(`${singular} removed.`);
      if (form.id === pendingDelete.id) reset();
      setPendingDelete(null);
      reload();
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not remove — ${e.message}`);
    } finally {
      setDeleting(false);
    }
  };

  return (
    <Card title={title}>
      {loading ? (
        <Loading label={`Loading ${title.toLowerCase()}`} />
      ) : error ? (
        <ErrorBanner error={error} />
      ) : (
        <>
          {configs.length ? (
            <ul style={{ listStyle: 'none', padding: 0, margin: '0 0 1ch 0' }}>
              {configs.map((raw) => {
                const f = toForm(raw, implementations);
                return (
                  <li
                    key={f.id || f.name}
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'space-between',
                      gap: '1ch',
                      padding: '0.5ch 0',
                    }}
                  >
                    <span>
                      <Badge>{f.enabled ? 'enabled' : 'disabled'}</Badge> {f.name || '(unnamed)'}{' '}
                      <span style={{ opacity: 0.5 }}>{f.implementation}</span>
                    </span>
                    <span style={{ display: 'inline-flex', gap: '0.5ch' }}>
                      <Button theme="SECONDARY" aria-label={`Edit ${f.name || singular}`} onClick={() => edit(raw)}>
                        Edit
                      </Button>
                      <Button
                        theme="SECONDARY"
                        aria-label={`Remove ${f.name || singular}`}
                        onClick={() => setPendingDelete(f)}
                      >
                        Remove
                      </Button>
                    </span>
                  </li>
                );
              })}
            </ul>
          ) : (
            <EmptyState>No {title.toLowerCase()} configured yet.</EmptyState>
          )}

          <Divider type="GRADIENT" />

          <Text style={{ opacity: 0.6, margin: '1ch 0 0.5ch' }}>
            {form.id ? `Editing ${form.name || form.id}` : `New ${singular.toLowerCase()}`}
          </Text>

          <div style={{ margin: '0.5ch 0' }}>
            <Text style={{ opacity: 0.6 }}>Name</Text>
            <Input
              name={`${kind}-name`}
              aria-label="Name"
              value={form.name}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
            />
          </div>

          <div style={{ margin: '0.5ch 0' }}>
            <Text style={{ opacity: 0.6 }}>Implementation</Text>
            <Select
              name={`${kind}-impl`}
              options={implementations}
              defaultValue={form.implementation}
              onChange={(value) => setForm({ ...form, implementation: value })}
            />
          </div>

          <div style={{ display: 'flex', gap: '1ch' }}>
            <div style={{ flex: 2, margin: '0.5ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Host</Text>
              <Input
                name={`${kind}-host`}
                aria-label="Host"
                placeholder="localhost"
                value={form.host}
                onChange={(e) => setForm({ ...form, host: e.target.value })}
              />
            </div>
            <div style={{ flex: 1, margin: '0.5ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Port</Text>
              <Input
                name={`${kind}-port`}
                aria-label="Port"
                type="number"
                placeholder="9117"
                value={form.port}
                onChange={(e) => setForm({ ...form, port: e.target.value })}
              />
            </div>
          </div>

          <div style={{ margin: '0.5ch 0' }}>
            <Text style={{ opacity: 0.6 }}>API key</Text>
            <Input
              name={`${kind}-apikey`}
              aria-label="API key"
              type="password"
              value={form.apiKey}
              onChange={(e) => setForm({ ...form, apiKey: e.target.value })}
            />
          </div>

          <div style={{ display: 'flex', gap: '2ch', margin: '0.5ch 0' }}>
            <Checkbox
              name={`${kind}-ssl`}
              defaultChecked={form.ssl}
              onChange={(e) => setForm({ ...form, ssl: e.target.checked })}
            >
              Use SSL
            </Checkbox>
            <Checkbox
              name={`${kind}-enabled`}
              defaultChecked={form.enabled}
              onChange={(e) => setForm({ ...form, enabled: e.target.checked })}
            >
              Enabled
            </Checkbox>
          </div>

          {/* Inline test result stays near the form as a persistent indicator;
              the same outcome is also announced via toast. */}
          {testResult ? (
            <div role="status" style={{ margin: '0.5ch 0' }}>
              <Badge>{testResult.ok ? '✓ ok' : '✗ failed'}</Badge> {testResult.message}
            </div>
          ) : null}
          {saveError ? <ErrorBanner error={saveError} /> : null}

          <div style={{ marginTop: '1ch' }}>
            <ButtonGroup
              items={[
                { body: testing ? 'Testing…' : 'Test', onClick: testing ? undefined : test },
                { body: saving ? 'Saving…' : 'Save', onClick: saving ? undefined : save },
                ...(form.id ? [{ body: 'New', onClick: reset }] : []),
              ]}
            />
          </div>

          {pendingDelete ? (
            <ConfirmDialog
              title={`Remove ${singular.toLowerCase()}`}
              confirmLabel={`Remove ${singular.toLowerCase()}`}
              pendingLabel="Removing…"
              pending={deleting}
              onConfirm={confirmDelete}
              onCancel={() => (deleting ? undefined : setPendingDelete(null))}
            >
              <Text>
                Remove <strong>{pendingDelete.name || singular.toLowerCase()}</strong>? cellarr will
                stop using this {singular.toLowerCase()}.
              </Text>
            </ConfirmDialog>
          ) : null}
        </>
      )}
    </Card>
  );
};

export default IntegrationSection;
