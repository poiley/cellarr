'use client';

// Settings — Indexers / Download Clients. Both are integration configs with the
// same shape (host/port/api-key/ssl/enabled, a Test button that surfaces an
// AlertBanner result, and Save). SRCL-only: Card, Input, Select, Checkbox,
// AlertBanner, Button. Reads GET /api/v1/<kind>; tests POST /api/v1/<kind>/test;
// writes POST /api/v1/<kind>.

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
import type { IndexerConfig, DownloadClientConfig } from '@lib/api/types';

import { useAsync, toApiError } from '@app/settings/_components/useAsync';
import {
  Loading,
  ErrorBanner,
  SuccessBanner,
  EmptyState,
} from '@app/settings/_components/StatusBanners';

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

  const [form, setForm] = React.useState<IntegrationForm>(() => blankForm(implementations));
  const [testing, setTesting] = React.useState(false);
  const [testResult, setTestResult] = React.useState<TestResult | undefined>(undefined);
  const [saving, setSaving] = React.useState(false);
  const [saved, setSaved] = React.useState(false);
  const [saveError, setSaveError] = React.useState<ApiError | undefined>(undefined);

  const configs = data ?? [];

  const edit = (raw: RawConfig) => {
    setForm(toForm(raw, implementations));
    setTestResult(undefined);
    setSaved(false);
    setSaveError(undefined);
  };

  const reset = () => {
    setForm(blankForm(implementations));
    setTestResult(undefined);
    setSaved(false);
    setSaveError(undefined);
  };

  const payload = () => ({
    id: form.id || undefined,
    name: form.name,
    implementation: form.implementation,
    host: form.host,
    port: form.port ? Number.parseInt(form.port, 10) : undefined,
    api_key: form.apiKey || undefined,
    ssl: form.ssl,
    enabled: form.enabled,
  });

  const test = async () => {
    setTesting(true);
    setTestResult(undefined);
    try {
      await client.request<unknown>(`/${kind}/test`, { method: 'POST', body: payload() });
      setTestResult({ ok: true, message: 'Connection successful.' });
    } catch (err) {
      const e = toApiError(err);
      setTestResult({ ok: false, message: `${e.code}: ${e.message}` });
    } finally {
      setTesting(false);
    }
  };

  const save = async () => {
    setSaving(true);
    setSaved(false);
    setSaveError(undefined);
    try {
      await client.request<unknown>(`/${kind}`, { method: 'POST', body: payload() });
      setSaved(true);
      reset();
      reload();
    } catch (err) {
      setSaveError(toApiError(err));
    } finally {
      setSaving(false);
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
                    <Button theme="SECONDARY" onClick={() => edit(raw)}>
                      Edit
                    </Button>
                  </li>
                );
              })}
            </ul>
          ) : (
            <EmptyState>No {title.toLowerCase()} configured yet.</EmptyState>
          )}

          <Divider type="GRADIENT" />

          <Text style={{ opacity: 0.6, margin: '1ch 0 0.5ch' }}>
            {form.id ? `Editing ${form.name || form.id}` : `New ${title.replace(/s$/, '').toLowerCase()}`}
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

          {testResult ? (
            testResult.ok ? (
              <SuccessBanner>{testResult.message}</SuccessBanner>
            ) : (
              <ErrorBanner
                error={
                  new ApiError(
                    'test_failed',
                    testResult.message,
                    0
                  )
                }
              />
            )
          ) : null}
          {saveError ? <ErrorBanner error={saveError} /> : null}
          {saved ? <SuccessBanner>Saved.</SuccessBanner> : null}

          <div style={{ marginTop: '1ch' }}>
            <ButtonGroup
              items={[
                { body: testing ? 'Testing…' : 'Test', onClick: testing ? undefined : test },
                { body: saving ? 'Saving…' : 'Save', onClick: saving ? undefined : save },
                ...(form.id ? [{ body: 'New', onClick: reset }] : []),
              ]}
            />
          </div>
        </>
      )}
    </Card>
  );
};

export default IntegrationSection;
