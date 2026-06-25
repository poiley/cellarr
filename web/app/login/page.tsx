'use client';

// Login screen (Forms auth). A bare, shell-less route — the AppShell gate
// redirects here with a 303 when an unauthenticated navigation hits a gated
// page under the Forms method. SRCL-only: a centred Card holding username +
// password Inputs and a Log In Button, with an inline AlertBanner for a failed
// attempt. On success the daemon has set the HttpOnly session cookie, so we just
// route into the app (default: the dashboard, or `?next=` if present).
//
// This page is only meaningful under Forms: under Basic the browser's native
// prompt handles auth, and under None there is no gate. If someone lands here
// while already authenticated / not enforced, we bounce them straight in.

import * as React from 'react';

import { useRouter, useSearchParams } from 'next/navigation';

import Card from '@components/Card';
import Input from '@components/Input';
import Button from '@components/Button';
import Text from '@components/Text';
import Divider from '@components/Divider';
import AlertBanner from '@components/AlertBanner';

import { api, ApiError } from '@lib/api/client';

// Only allow same-origin relative redirect targets, so a crafted `?next=` can
// never bounce the freshly-authenticated user to another origin.
function safeNext(raw: string | null): string {
  if (!raw) return '/';
  if (!raw.startsWith('/') || raw.startsWith('//')) return '/';
  return raw;
}

// The form reads `useSearchParams`, which forces a client bail-out during
// static prerendering — so it must live inside a <Suspense> boundary (provided
// by the default-exported Page below).
function LoginForm() {
  const router = useRouter();
  const search = useSearchParams();
  const next = safeNext(search.get('next'));

  const [username, setUsername] = React.useState('');
  const [password, setPassword] = React.useState('');
  const [error, setError] = React.useState<string | null>(null);
  const [submitting, setSubmitting] = React.useState(false);

  const submit = React.useCallback(
    async (e?: React.FormEvent) => {
      e?.preventDefault();
      if (submitting) return;
      const u = username.trim();
      if (!u || !password) {
        setError('Enter both a username and a password.');
        return;
      }
      setSubmitting(true);
      setError(null);
      try {
        await api.login({ username: u, password });
        // The session cookie is now set; hand off to the requested page. Use a
        // full navigation so the server re-evaluates the gate with the cookie.
        router.replace(next);
        router.refresh();
      } catch (err) {
        const code = err instanceof ApiError ? err.code : 'unknown_error';
        setError(
          code === 'unauthorized'
            ? 'Incorrect username or password.'
            : err instanceof ApiError
              ? err.message
              : 'Login failed. Please try again.'
        );
        setSubmitting(false);
      }
    },
    [api, username, password, submitting, router, next]
  );

  return (
    <main
      style={{
        minHeight: '100vh',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: '2ch',
      }}
    >
      <div style={{ width: '48ch', maxWidth: '100%' }}>
        <Card title="cellarr — sign in">
          <form onSubmit={submit} aria-label="Sign in">
            <Text style={{ opacity: 0.6 }}>
              This server requires you to sign in.
            </Text>
            <Divider type="GRADIENT" />

            <div style={{ margin: '1ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Username</Text>
              <Input
                name="username"
                aria-label="Username"
                autoComplete="username"
                autoFocus
                value={username}
                onChange={(ev) => setUsername(ev.target.value)}
              />
            </div>

            <div style={{ margin: '1ch 0' }}>
              <Text style={{ opacity: 0.6 }}>Password</Text>
              <Input
                name="password"
                type="password"
                aria-label="Password"
                autoComplete="current-password"
                value={password}
                onChange={(ev) => setPassword(ev.target.value)}
              />
            </div>

            {error ? (
              <div role="alert" style={{ margin: '1ch 0' }}>
                <AlertBanner
                  style={{
                    background: 'var(--ansi-9-red)',
                    color: 'var(--ansi-15-white)',
                  }}
                >
                  {error}
                </AlertBanner>
              </div>
            ) : null}

            <div style={{ marginTop: '1ch' }}>
              <Button type="submit" isDisabled={submitting}>
                {submitting ? 'Signing in…' : 'Log In'}
              </Button>
            </div>
          </form>
        </Card>
      </div>
    </main>
  );
}

export default function Page() {
  // Suspense boundary required around the useSearchParams-using form so the
  // route can be statically prerendered (it bails to client rendering at the
  // boundary instead of failing the build).
  return (
    <React.Suspense fallback={null}>
      <LoginForm />
    </React.Suspense>
  );
}
