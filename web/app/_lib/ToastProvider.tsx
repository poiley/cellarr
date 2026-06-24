'use client';

// Shared toast / status system. A provider hangs an aria-live region off the
// document and renders a stack of auto-dismissing notices; useToast() hands any
// screen a tiny, typed API for action feedback (grab queued, profile saved,
// search failed, ...).
//
// SRCL-only: each toast is a vendored SRCL <Card> (bordered, monospace) with an
// ASCII status glyph (✓ / ✗ / ● ) — no emoji, no CSS framework, no new icon
// lib. The only non-SRCL bits are React state, the live-region semantics, and a
// co-located CSS module (positioning + reduced-motion), which the SRCL-only
// lint allows.

import * as React from 'react';

import Card from '@components/Card';

import styles from './ToastProvider.module.css';

/** Visual + semantic variant of a toast. */
export type ToastVariant = 'success' | 'error' | 'info';

/** Options accepted when pushing a toast. */
export interface ToastOptions {
  /** Variant drives the accent colour + ASCII glyph. Defaults to 'info'. */
  variant?: ToastVariant;
  /**
   * Milliseconds before auto-dismiss. Clamped to [2000, 8000]; defaults to
   * 4000 for info/success and 5000 for error. Pass 0 to disable auto-dismiss.
   */
  durationMs?: number;
}

/** The public toast API handed to screens via useToast(). */
export interface ToastApi {
  /** Push a toast; returns its id so callers can dismiss it early. */
  toast: (message: React.ReactNode, options?: ToastOptions) => string;
  /** Convenience: a success toast (✓). */
  success: (message: React.ReactNode, options?: Omit<ToastOptions, 'variant'>) => string;
  /** Convenience: an error toast (✗); defaults to a longer 5s dwell. */
  error: (message: React.ReactNode, options?: Omit<ToastOptions, 'variant'>) => string;
  /** Convenience: an info toast (●). */
  info: (message: React.ReactNode, options?: Omit<ToastOptions, 'variant'>) => string;
  /** Dismiss a specific toast (or the most recent when id is omitted). */
  dismiss: (id?: string) => void;
}

interface ToastItem {
  id: string;
  message: React.ReactNode;
  variant: ToastVariant;
}

const GLYPH: Record<ToastVariant, string> = {
  success: '✓',
  error: '✗',
  info: '●',
};

const TITLE: Record<ToastVariant, string> = {
  success: 'OK',
  error: 'ERROR',
  info: 'INFO',
};

const DEFAULT_DURATION: Record<ToastVariant, number> = {
  success: 4000,
  error: 5000,
  info: 4000,
};

const MIN_DURATION = 2000;
const MAX_DURATION = 8000;

const ToastContext = React.createContext<ToastApi | null>(null);

export const ToastProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [items, setItems] = React.useState<ToastItem[]>([]);
  // Track auto-dismiss timers so we can clear them on manual dismiss / unmount.
  const timers = React.useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());

  const dismiss = React.useCallback((id?: string) => {
    setItems((prev) => {
      if (prev.length === 0) return prev;
      const targetId = id ?? prev[prev.length - 1].id;
      const timer = timers.current.get(targetId);
      if (timer) {
        clearTimeout(timer);
        timers.current.delete(targetId);
      }
      return prev.filter((t) => t.id !== targetId);
    });
  }, []);

  const toast = React.useCallback(
    (message: React.ReactNode, options?: ToastOptions): string => {
      const variant = options?.variant ?? 'info';
      const id = `toast-${Date.now()}-${Math.random().toString(36).slice(2, 9)}`;
      setItems((prev) => [...prev, { id, message, variant }]);

      const requested = options?.durationMs;
      const duration =
        requested === 0
          ? 0
          : Math.min(MAX_DURATION, Math.max(MIN_DURATION, requested ?? DEFAULT_DURATION[variant]));
      if (duration > 0) {
        const timer = setTimeout(() => dismiss(id), duration);
        timers.current.set(id, timer);
      }
      return id;
    },
    [dismiss]
  );

  const success = React.useCallback<ToastApi['success']>(
    (message, options) => toast(message, { ...options, variant: 'success' }),
    [toast]
  );
  const error = React.useCallback<ToastApi['error']>(
    (message, options) => toast(message, { ...options, variant: 'error' }),
    [toast]
  );
  const info = React.useCallback<ToastApi['info']>(
    (message, options) => toast(message, { ...options, variant: 'info' }),
    [toast]
  );

  // Clear any pending timers if the provider unmounts.
  React.useEffect(() => {
    const map = timers.current;
    return () => {
      for (const t of map.values()) clearTimeout(t);
      map.clear();
    };
  }, []);

  const value = React.useMemo<ToastApi>(
    () => ({ toast, success, error, info, dismiss }),
    [toast, success, error, info, dismiss]
  );

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className={styles.region} role="region" aria-label="Notifications">
        {/* aria-live="polite" so screen readers announce toasts without
            interrupting; the visual stack mirrors the same nodes. */}
        <div aria-live="polite" aria-atomic="false">
          {items.map((item) => (
            <div key={item.id} className={styles.item} data-variant={item.variant}>
              <Card title={`${GLYPH[item.variant]} ${TITLE[item.variant]}`} mode="left">
                <div className={styles.row}>
                  <span className={styles.message}>{item.message}</span>
                  <button
                    type="button"
                    className={styles.dismiss}
                    aria-label="Dismiss notification"
                    onClick={() => dismiss(item.id)}
                  >
                    ✕
                  </button>
                </div>
              </Card>
            </div>
          ))}
        </div>
      </div>
    </ToastContext.Provider>
  );
};

/**
 * Access the shared toast API. Must be called under <ToastProvider> (mounted in
 * the app's Providers tree). Falls back to no-ops with a console warning if used
 * outside the provider so a stray call never crashes a screen.
 */
export function useToast(): ToastApi {
  const ctx = React.useContext(ToastContext);
  if (!ctx) {
    if (typeof console !== 'undefined') {
      console.warn('useToast() called outside <ToastProvider>; toasts will be dropped.');
    }
    const noop = () => '';
    return { toast: noop, success: noop, error: noop, info: noop, dismiss: () => {} };
  }
  return ctx;
}
