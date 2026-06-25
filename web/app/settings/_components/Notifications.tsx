'use client';

// Settings — Notifications. Mirrors the Indexers / Download-Clients tab: a list
// of configured connectors (name, type, enabled, the events they fire on) with
// Edit / Remove (destructive confirm + danger styling), plus a form to create or
// edit one — a type dropdown, the provider's fields, the per-event toggles, an
// Enabled flag, and Test + Save with loading + toast feedback. SRCL-only: Card,
// Input, Select, Checkbox, Button, ButtonGroup, Badge, Divider, Text.
//
// The connector templates (which provider exists and what fields each one
// renders) come from GET /api/v3/notification/schema, so the form stays in step
// with the backend's advertised provider set without hand-mirroring it. List /
// create / update / delete / test all go through the Radarr/Sonarr-compatible
// /api/v3 notification routes (crates/cellarr-api/src/shim.rs): the write body is
// `{ name, implementation, on*, fields[] }` and `fields[]` projects the typed
// provider settings.

import * as React from 'react';

import Card from '@components/Card';
import Input from '@components/Input';
import Select from '@components/Select';
import Checkbox from '@components/Checkbox';
import Button from '@components/Button';
import Badge from '@components/Badge';
import Divider from '@components/Divider';
import Text from '@components/Text';

import { ApiError, CellarrClient, api as defaultApi } from '@lib/api/client';
import type {
  NotificationConfigV3,
  NotificationField,
  NotificationSchema,
} from '@lib/api/types';

import { useToast } from '@app/_lib/ToastProvider';
import { useAsync, toApiError } from '@app/settings/_components/useAsync';
import { Loading, ErrorBanner, EmptyState } from '@app/settings/_components/StatusBanners';
import ConfirmDialog from '@app/settings/_components/ConfirmDialog';

// A friendly label for each advertised implementation. The schema keys the *arr
// implementation strings (PlexServer / MediaBrowser); the UI shows the names
// users recognise (Plex / Emby).
const IMPL_LABELS: Record<string, string> = {
  Discord: 'Discord',
  Telegram: 'Telegram',
  Email: 'Email',
  CustomScript: 'Custom Script',
  Webhook: 'Webhook',
  PlexServer: 'Plex',
  Jellyfin: 'Jellyfin',
  MediaBrowser: 'Emby',
};

function implLabel(impl: string): string {
  return IMPL_LABELS[impl] ?? impl;
}

// Fields a connector genuinely cannot function without — marked with a "*" to
// match how Naming flags required tokens. Kept honest: only the endpoint a
// provider must have to deliver anything (a Webhook/Discord URL, a Custom Script
// path) is forced; everything else stays optional.
const REQUIRED_FIELD_NAMES = new Set(['url', 'path']);

function isRequiredField(field: NotificationField): boolean {
  return REQUIRED_FIELD_NAMES.has(field.name);
}

// The user-facing event toggles. `onHealthIssue` is the toggle; the backend also
// derives onHealthRestored from it (one health flag covers issue + restored).
const EVENTS: { key: EventKey; label: string }[] = [
  { key: 'onGrab', label: 'On Grab' },
  { key: 'onDownload', label: 'On Import' },
  { key: 'onUpgrade', label: 'On Upgrade' },
  { key: 'onHealthIssue', label: 'On Health Issue' },
];

type EventKey = 'onGrab' | 'onDownload' | 'onUpgrade' | 'onHealthIssue';

interface NotificationForm {
  id: string;
  name: string;
  implementation: string;
  enabled: boolean;
  events: Record<EventKey, boolean>;
  /** Provider field values, keyed by field name (mirrors the schema fields). */
  values: Record<string, string>;
}

interface TestResult {
  ok: boolean;
  message: string;
}

function defaultEvents(): Record<EventKey, boolean> {
  return { onGrab: true, onDownload: true, onUpgrade: true, onHealthIssue: true };
}

function fieldValueToString(value: unknown): string {
  if (value === undefined || value === null) return '';
  if (typeof value === 'boolean') return value ? 'true' : '';
  return String(value);
}

function blankForm(implementation: string): NotificationForm {
  return {
    id: '',
    name: '',
    implementation,
    enabled: true,
    events: defaultEvents(),
    values: {},
  };
}

function toForm(raw: NotificationConfigV3): NotificationForm {
  const values: Record<string, string> = {};
  for (const f of raw.fields ?? []) {
    if (f.name) values[f.name] = fieldValueToString(f.value);
  }
  return {
    id: String(raw.id ?? ''),
    name: raw.name ?? '',
    implementation: raw.implementation ?? '',
    enabled: raw.enabled !== false,
    events: {
      onGrab: raw.onGrab !== false,
      onDownload: raw.onDownload !== false,
      onUpgrade: raw.onUpgrade !== false,
      onHealthIssue: raw.onHealthIssue !== false,
    },
    values,
  };
}

/** The events a config is subscribed to, as short labels (for the list row). */
function enabledEventLabels(raw: NotificationConfigV3): string[] {
  return EVENTS.filter((e) => raw[e.key] !== false).map((e) =>
    e.label.replace(/^On /, '')
  );
}

// A field label that appends a required "*" marker (matching Naming's token
// marker) when the field cannot be left blank.
const FieldLabel: React.FC<{ field: NotificationField }> = ({ field }) => (
  <Text style={{ opacity: 0.6 }}>
    {field.label ?? field.name}
    {isRequiredField(field) ? <span aria-hidden="true"> *</span> : null}
  </Text>
);

// A single text/number/password provider field. Password-privacy fields get an
// SRCL-styled show/hide toggle (◉ shown / ○ hidden) and use a real type=password
// with new-password autocomplete until revealed.
const NotificationTextField: React.FC<{
  field: NotificationField;
  value: string;
  onChange: (next: string) => void;
}> = ({ field, value, onChange }) => {
  const isPassword = field.privacy === 'password' || field.type === 'password';
  const [show, setShow] = React.useState(false);
  const label = field.label ?? field.name;
  const required = isRequiredField(field);
  const inputType = isPassword ? (show ? 'text' : 'password') : field.type === 'number' ? 'number' : 'text';
  return (
    <div style={{ margin: '0.5ch 0' }}>
      <FieldLabel field={field} />
      <div style={{ display: 'flex', gap: '0.5ch', alignItems: 'stretch' }}>
        <div style={{ flex: 1 }}>
          <Input
            name={`notification-${field.name}`}
            aria-label={label}
            aria-required={required || undefined}
            type={inputType}
            autoComplete={isPassword ? 'new-password' : undefined}
            placeholder={field.helpText ?? ''}
            value={value}
            onChange={(e) => onChange(e.target.value)}
          />
        </div>
        {isPassword ? (
          <Button
            theme="SECONDARY"
            aria-label={show ? `Hide ${label}` : `Show ${label}`}
            aria-pressed={show}
            onClick={() => setShow((v) => !v)}
          >
            {show ? '◉ hide' : '○ show'}
          </Button>
        ) : null}
      </div>
    </div>
  );
};

export interface NotificationsProps {
  client?: CellarrClient;
}

const Notifications: React.FC<NotificationsProps> = ({ client = defaultApi }) => {
  const loadList = React.useCallback(
    (signal: AbortSignal) => client.listNotifications(signal),
    [client]
  );
  const loadSchema = React.useCallback(
    (signal: AbortSignal) => client.getNotificationSchema(signal),
    [client]
  );
  const { data: list, loading, error, reload } = useAsync<NotificationConfigV3[]>(loadList);
  const { data: schema } = useAsync<NotificationSchema[]>(loadSchema);
  const { success, error: toastError, info } = useToast();

  const [form, setForm] = React.useState<NotificationForm | null>(null);
  const [testing, setTesting] = React.useState(false);
  const [testResult, setTestResult] = React.useState<TestResult | undefined>(undefined);
  const [saving, setSaving] = React.useState(false);
  const [saveError, setSaveError] = React.useState<ApiError | undefined>(undefined);
  const [pendingDelete, setPendingDelete] = React.useState<NotificationConfigV3 | null>(null);
  const [deleting, setDeleting] = React.useState(false);

  const configs = list ?? [];
  const templates = schema ?? [];

  // Seed the form once the schema arrives (so the type dropdown defaults to a
  // real implementation rather than an empty string).
  React.useEffect(() => {
    if (!form && templates.length) {
      setForm(blankForm(templates[0].implementation));
    }
  }, [form, templates]);

  const fieldsFor = React.useCallback(
    (implementation: string): NotificationField[] =>
      templates.find((t) => t.implementation === implementation)?.fields ?? [],
    [templates]
  );

  const edit = (raw: NotificationConfigV3) => {
    setForm(toForm(raw));
    setTestResult(undefined);
    setSaveError(undefined);
  };

  const reset = () => {
    setForm(blankForm(templates[0]?.implementation ?? 'Webhook'));
    setTestResult(undefined);
    setSaveError(undefined);
  };

  // Map the flat form to the v3 notification write body: identity + the on*
  // event toggles + a fields[] projection of the provider values.
  const toV3Body = (f: NotificationForm): Partial<NotificationConfigV3> => {
    const fields = fieldsFor(f.implementation).map((schemaField) => ({
      name: schemaField.name,
      value:
        schemaField.type === 'checkbox'
          ? f.values[schemaField.name] === 'true'
          : (f.values[schemaField.name] ?? ''),
    }));
    return {
      name: f.name,
      implementation: f.implementation,
      onGrab: f.events.onGrab,
      onDownload: f.events.onDownload,
      onUpgrade: f.events.onUpgrade,
      onRename: false,
      onHealthIssue: f.events.onHealthIssue,
      onHealthRestored: f.events.onHealthIssue,
      fields,
      tags: [],
    } as Partial<NotificationConfigV3>;
  };

  const test = async () => {
    if (!form) return;
    if (!form.name.trim()) {
      toastError('Give it a name before testing.');
      return;
    }
    setTesting(true);
    setTestResult(undefined);
    info('Sending test…', { durationMs: 2000 });
    try {
      await client.testNotification(toV3Body(form));
      setTestResult({ ok: true, message: 'Test delivered.' });
      success('Test delivered.');
    } catch (err) {
      const e = toApiError(err);
      setTestResult({ ok: false, message: `${e.code}: ${e.message}` });
      toastError(`Test failed — ${e.message}`);
    } finally {
      setTesting(false);
    }
  };

  const save = async () => {
    if (!form) return;
    if (!form.name.trim()) {
      toastError('Give it a name before saving.');
      return;
    }
    setSaving(true);
    setSaveError(undefined);
    try {
      const body = toV3Body(form);
      const numericId = Number.parseInt(form.id, 10);
      if (form.id && Number.isFinite(numericId)) {
        await client.updateNotification(numericId, body);
      } else {
        await client.createNotification(body);
      }
      success('Notification saved.');
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
    const numericId = Number(pendingDelete.id);
    if (!Number.isFinite(numericId)) {
      toastError('This entry cannot be deleted from here (no numeric id).');
      setPendingDelete(null);
      return;
    }
    setDeleting(true);
    try {
      await client.deleteNotification(numericId);
      success('Notification removed.');
      if (form?.id === String(pendingDelete.id)) reset();
      setPendingDelete(null);
      reload();
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not remove — ${e.message}`);
    } finally {
      setDeleting(false);
    }
  };

  const implOptions = templates.map((t) => t.implementation);
  const labelByImpl = React.useMemo(() => {
    const m: Record<string, string> = {};
    for (const i of implOptions) m[implLabel(i)] = i;
    return m;
  }, [implOptions]);

  return (
    <Card title="Notifications">
      {loading ? (
        <Loading label="Loading notifications" />
      ) : error ? (
        <ErrorBanner error={error} />
      ) : (
        <>
          {configs.length ? (
            <ul style={{ listStyle: 'none', padding: 0, margin: '0 0 1ch 0' }}>
              {configs.map((raw) => {
                const events = enabledEventLabels(raw);
                return (
                  <li
                    key={raw.id ?? raw.name}
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'space-between',
                      gap: '1ch',
                      padding: '0.5ch 0',
                    }}
                  >
                    <span>
                      <Badge>{raw.enabled !== false ? 'enabled' : 'disabled'}</Badge>{' '}
                      {raw.name || '(unnamed)'}{' '}
                      <span style={{ opacity: 0.5 }}>{implLabel(raw.implementation)}</span>{' '}
                      <span style={{ opacity: 0.4 }}>
                        {events.length ? events.join(', ') : 'no events'}
                      </span>
                    </span>
                    <span style={{ display: 'inline-flex', gap: '0.5ch' }}>
                      <Button
                        theme="SECONDARY"
                        aria-label={`Edit ${raw.name || 'notification'}`}
                        onClick={() => edit(raw)}
                      >
                        Edit
                      </Button>
                      <Button
                        theme="DANGER"
                        aria-label={`Remove ${raw.name || 'notification'}`}
                        onClick={() => setPendingDelete(raw)}
                      >
                        Remove
                      </Button>
                    </span>
                  </li>
                );
              })}
            </ul>
          ) : (
            <EmptyState>No notifications configured yet.</EmptyState>
          )}

          <Divider type="GRADIENT" />

          {form ? (
            <>
              <Text style={{ opacity: 0.6, margin: '1ch 0 0.5ch' }}>
                {form.id ? `Editing ${form.name || form.id}` : 'New notification'}
              </Text>

              <div style={{ margin: '0.5ch 0' }}>
                <Text style={{ opacity: 0.6 }}>Name</Text>
                <Input
                  name="notification-name"
                  aria-label="Name"
                  value={form.name}
                  onChange={(e) => setForm({ ...form, name: e.target.value })}
                />
              </div>

              <div style={{ margin: '0.5ch 0' }}>
                <Text style={{ opacity: 0.6 }}>Type</Text>
                <Select
                  name="notification-type"
                  options={implOptions.map(implLabel)}
                  defaultValue={implLabel(form.implementation)}
                  onChange={(value) =>
                    setForm({
                      ...form,
                      implementation: labelByImpl[value] ?? form.implementation,
                      // Switching provider clears the previous provider's values.
                      values: {},
                    })
                  }
                />
              </div>

              {fieldsFor(form.implementation).map((field) =>
                field.type === 'checkbox' ? (
                  <div key={field.name} style={{ margin: '0.5ch 0' }}>
                    <Checkbox
                      name={`notification-${field.name}`}
                      aria-label={field.label ?? field.name}
                      defaultChecked={form.values[field.name] === 'true'}
                      onChange={(e) =>
                        setForm({
                          ...form,
                          values: { ...form.values, [field.name]: e.target.checked ? 'true' : '' },
                        })
                      }
                    >
                      {field.label ?? field.name}
                    </Checkbox>
                  </div>
                ) : (
                  <NotificationTextField
                    key={field.name}
                    field={field}
                    value={form.values[field.name] ?? ''}
                    onChange={(next) =>
                      setForm({
                        ...form,
                        values: { ...form.values, [field.name]: next },
                      })
                    }
                  />
                )
              )}

              <Text style={{ opacity: 0.6, margin: '1ch 0 0.5ch' }}>Events</Text>
              <div style={{ display: 'flex', flexWrap: 'wrap', gap: '2ch', margin: '0.5ch 0' }}>
                {EVENTS.map((ev) => (
                  <Checkbox
                    key={ev.key}
                    name={`notification-${ev.key}`}
                    aria-label={ev.label}
                    defaultChecked={form.events[ev.key]}
                    onChange={(e) =>
                      setForm({
                        ...form,
                        events: { ...form.events, [ev.key]: e.target.checked },
                      })
                    }
                  >
                    {ev.label}
                  </Checkbox>
                ))}
              </div>

              <div style={{ margin: '0.5ch 0' }}>
                <Checkbox
                  name="notification-enabled"
                  aria-label="Enabled"
                  defaultChecked={form.enabled}
                  onChange={(e) => setForm({ ...form, enabled: e.target.checked })}
                >
                  Enabled
                </Checkbox>
              </div>

              {testResult ? (
                <div role="status" style={{ margin: '0.5ch 0' }}>
                  <Badge>{testResult.ok ? '✓ ok' : '✗ failed'}</Badge> {testResult.message}
                </div>
              ) : null}
              {saveError ? <ErrorBanner error={saveError} /> : null}

              <div style={{ marginTop: '1ch', display: 'flex', gap: '1ch', alignItems: 'center', flexWrap: 'wrap' }}>
                <Button theme="PRIMARY" isDisabled={saving} onClick={saving ? undefined : save}>
                  {saving ? 'Saving…' : 'Save'}
                </Button>
                <Button theme="SECONDARY" isDisabled={testing} onClick={testing ? undefined : test}>
                  {testing ? 'Testing…' : 'Test'}
                </Button>
                {form.id ? (
                  <Button theme="SECONDARY" onClick={reset}>
                    New
                  </Button>
                ) : null}
              </div>
            </>
          ) : (
            <Loading label="Loading providers" />
          )}

          {pendingDelete ? (
            <ConfirmDialog
              title="Remove notification"
              confirmLabel="Remove notification"
              pendingLabel="Removing…"
              pending={deleting}
              onConfirm={confirmDelete}
              onCancel={() => (deleting ? undefined : setPendingDelete(null))}
            >
              <Text>
                Remove <strong>{pendingDelete.name || 'this notification'}</strong>? cellarr will
                stop sending alerts to it.
              </Text>
            </ConfirmDialog>
          ) : null}
        </>
      )}
    </Card>
  );
};

export default Notifications;
