'use client';

// Settings — SECURITY. The single-user authentication panel, modelled on the
// Sonarr/Radarr "Authentication" setting minus the multi-user bits. SRCL-only:
// Card, Select, Input, Button, Badge, Divider, Text + the shared useToast /
// useAsync / StatusBanners glue.
//
// Two independent saves, each hitting one endpoint:
//   METHOD      a Select over None | Forms | Basic → PUT /api/v1/auth/config.
//               Changing it revokes all sessions, so we warn the admin they (and
//               any other client) may need to sign in again.
//   CREDENTIAL  admin username + password + confirm → POST /api/v1/auth/credential.
//               The password is Argon2id-hashed server-side; we never receive or
//               display the hash, and we never send unless password === confirm.
//
// The panel reads its current state from GET /api/v1/auth/config (which carries
// the method, whether a credential is configured, and the username — never the
// hash) and reflects it back after each save.

import * as React from 'react';

import Card from '@components/Card';
import Select from '@components/Select';
import Input from '@components/Input';
import Button from '@components/Button';
import Badge from '@components/Badge';
import Divider from '@components/Divider';
import Text from '@components/Text';

import { CellarrClient, api as defaultApi } from '@lib/api/client';
import type { AuthMethod, AuthStatus } from '@lib/api/types';

import { useToast } from '@app/_lib/ToastProvider';
import { useAsync, toApiError } from '@app/settings/_components/useAsync';
import { Loading, ErrorBanner } from '@app/settings/_components/StatusBanners';

// The dropdown labels, in display order, paired with their method value. The
// Select component speaks in label strings, so we translate at the boundary.
const METHOD_OPTIONS: { method: AuthMethod; label: string; hint: string }[] = [
  { method: 'none', label: 'None', hint: 'No login. Anyone who can reach the server can use it.' },
  { method: 'forms', label: 'Forms (Login Page)', hint: 'A username + password login page gates the UI.' },
  { method: 'basic', label: 'Basic (Browser Popup)', hint: 'The browser prompts for credentials (HTTP Basic).' },
];

const LABELS = METHOD_OPTIONS.map((o) => o.label);

function labelFor(method: AuthMethod): string {
  return METHOD_OPTIONS.find((o) => o.method === method)?.label ?? 'None';
}

function methodFor(label: string): AuthMethod {
  return METHOD_OPTIONS.find((o) => o.label === label)?.method ?? 'none';
}

function hintFor(method: AuthMethod): string {
  return METHOD_OPTIONS.find((o) => o.method === method)?.hint ?? '';
}

// Loader: fetch the current auth config, then mount the form seeded from it.
// The form is keyed on the loaded snapshot so a reload (after a save) remounts
// it with fresh defaults — the uncontrolled SRCL Select reads `defaultValue`
// only at mount, so seeding-by-key is the reliable way to reflect a new method.
const Security: React.FC<{ client?: CellarrClient }> = ({ client = defaultApi }) => {
  const load = React.useCallback(
    (signal: AbortSignal) => client.getAuthConfig(signal),
    [client]
  );
  const { data, loading, error, reload } = useAsync<AuthStatus>(load);

  if (loading) return <Loading label="Loading authentication settings" />;
  if (error) return <ErrorBanner error={error} />;
  if (!data) return <Loading label="Loading authentication settings" />;

  return (
    <SecurityForm
      key={`${data.method}:${data.username ?? ''}:${data.configured}`}
      client={client}
      initial={data}
      reload={reload}
    />
  );
};

const SecurityForm: React.FC<{
  client: CellarrClient;
  initial: AuthStatus;
  reload: () => void;
}> = ({ client, initial, reload }) => {
  const { success, error: toastError } = useToast();

  // Local edit state, seeded directly from the loaded config at mount.
  const [method, setMethod] = React.useState<AuthMethod>(initial.method);
  const [username, setUsername] = React.useState(initial.username ?? '');
  const [password, setPassword] = React.useState('');
  const [confirm, setConfirm] = React.useState('');
  const [savingMethod, setSavingMethod] = React.useState(false);
  const [savingCred, setSavingCred] = React.useState(false);

  const configured = initial.configured;
  const methodChanged = method !== initial.method;

  const saveMethod = async () => {
    // Switching to a gated method with no credential set yet would lock the user
    // out; require a credential first.
    if (method !== 'none' && !configured) {
      toastError('Set an admin username and password before enabling a login method.');
      return;
    }
    setSavingMethod(true);
    try {
      await client.setAuthMethod(method);
      success(
        method === 'none'
          ? 'Authentication disabled.'
          : 'Authentication method saved. You may need to sign in again.'
      );
      reload();
    } catch (err) {
      toastError(`Could not save method — ${toApiError(err).message}`);
    } finally {
      setSavingMethod(false);
    }
  };

  const saveCredential = async () => {
    const u = username.trim();
    if (!u || !password) {
      toastError('Enter both a username and a password.');
      return;
    }
    if (password !== confirm) {
      toastError('Password and confirmation do not match.');
      return;
    }
    setSavingCred(true);
    try {
      await client.setCredential({ username: u, password });
      setPassword('');
      setConfirm('');
      success('Admin credentials saved. You may need to sign in again.');
      reload();
    } catch (err) {
      toastError(`Could not save credentials — ${toApiError(err).message}`);
    } finally {
      setSavingCred(false);
    }
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '1ch' }}>
      <Card title="Authentication Method">
        <Text style={{ opacity: 0.6 }}>
          Controls how the web UI and its private API are protected. The{' '}
          <Badge>/api/v3</Badge> compatibility API is always key-authenticated and
          is never affected by this setting.
        </Text>
        <Divider type="GRADIENT" />

        <div style={{ margin: '1ch 0' }}>
          <Text style={{ opacity: 0.6 }}>Method</Text>
          <Select
            name="auth-method"
            options={LABELS}
            defaultValue={labelFor(method)}
            onChange={(value) => setMethod(methodFor(value))}
          />
          <Text style={{ opacity: 0.6, marginTop: '0.5ch' }}>{hintFor(method)}</Text>
        </div>

        {methodChanged ? (
          <Text role="status" style={{ color: 'var(--ansi-3-yellow)' }}>
            ⚠ Changing the method revokes all sessions — you may need to sign in
            again.
          </Text>
        ) : null}

        <div style={{ marginTop: '1ch' }}>
          <Button onClick={saveMethod} isDisabled={savingMethod}>
            {savingMethod ? 'Saving…' : 'Save method'}
          </Button>
        </div>
      </Card>

      <Card title="Admin Credentials">
        <Text style={{ opacity: 0.6 }}>
          The single administrator account.{' '}
          {configured ? (
            <Badge>configured</Badge>
          ) : (
            <Badge>not set</Badge>
          )}{' '}
          Passwords are hashed and never displayed.
        </Text>
        <Divider type="GRADIENT" />

        <div style={{ margin: '1ch 0' }}>
          <Text style={{ opacity: 0.6 }}>Username</Text>
          <Input
            name="auth-username"
            aria-label="Admin username"
            autoComplete="username"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
          />
        </div>

        <div style={{ margin: '1ch 0' }}>
          <Text style={{ opacity: 0.6 }}>Password</Text>
          <Input
            name="auth-password"
            type="password"
            aria-label="Admin password"
            autoComplete="new-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
        </div>

        <div style={{ margin: '1ch 0' }}>
          <Text style={{ opacity: 0.6 }}>Confirm password</Text>
          <Input
            name="auth-password-confirm"
            type="password"
            aria-label="Confirm admin password"
            autoComplete="new-password"
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
          />
        </div>

        <Text role="status" style={{ color: 'var(--ansi-3-yellow)' }}>
          ⚠ Saving new credentials revokes all sessions — you may need to sign in
          again.
        </Text>

        <div style={{ marginTop: '1ch' }}>
          <Button onClick={saveCredential} isDisabled={savingCred}>
            {savingCred ? 'Saving…' : 'Save credentials'}
          </Button>
        </div>
      </Card>
    </div>
  );
};

export default Security;
