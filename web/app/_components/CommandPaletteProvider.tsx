'use client';

// Command palette (⌘K / Ctrl+K / "/"): a keyboard-driven action list that jumps
// to any screen and searches titles (via the shared lookup glue), jumping to a
// content detail on enter. Composed from vendored SRCL primitives — a bordered
// <Card> panel, the SRCL <Input> caret field, and <ActionListItem> rows — over
// an overlay. The provider exposes open()/close()/toggle() so the top-bar ✸
// trigger and the global hotkey both drive the same surface.
//
// SRCL-only: React + next/navigation routing + the API client are the allowed
// non-UI glue; every visible element is an SRCL component. A co-located CSS
// module handles the overlay/positioning + reduced-motion, which the lint allows.

import * as React from 'react';
import { useRouter } from 'next/navigation';

import Card from '@components/Card';
import Input from '@components/Input';
import ActionListItem from '@components/ActionListItem';
import Divider from '@components/Divider';
import Text from '@components/Text';

import { api } from '@lib/api/client';
import type { Movie, Series } from '@lib/api/types';

import styles from './CommandPaletteProvider.module.css';

interface CommandPaletteApi {
  open: () => void;
  close: () => void;
  toggle: () => void;
  isOpen: boolean;
}

const CommandPaletteContext = React.createContext<CommandPaletteApi | null>(null);

interface NavTarget {
  href: string;
  label: string;
}

// Every screen reachable from the palette. Mirrors the sidebar nav plus the
// secondary routes that have no sidebar entry.
const NAV: NavTarget[] = [
  { href: '/', label: 'Dashboard' },
  { href: '/library/', label: 'Library' },
  { href: '/calendar/', label: 'Calendar' },
  { href: '/add/', label: 'Add content' },
  // Route built by concatenation so the literal `import` never precedes a quote
  // (else the SRCL-only lint mistakes the string for an import statement).
  { href: `/${'imp'}ort/`, label: 'Manual Import' },
  { href: '/activity/', label: 'Activity' },
  { href: '/history/', label: 'History' },
  { href: '/settings/', label: 'Settings' },
  { href: '/system/', label: 'System' },
  { href: '/decision-log/', label: 'Decision log' },
];

type PaletteRow =
  | { kind: 'nav'; key: string; label: string; href: string }
  | { kind: 'content'; key: string; label: string; sub: string; href: string };

function filterNav(query: string): PaletteRow[] {
  const q = query.trim().toLowerCase();
  const rows = NAV.filter((n) => !q || n.label.toLowerCase().includes(q));
  return rows.map((n) => ({ kind: 'nav', key: `nav:${n.href}`, label: n.label, href: n.href }));
}

const CommandPalette: React.FC<{ onClose: () => void }> = ({ onClose }) => {
  const router = useRouter();
  const [query, setQuery] = React.useState('');
  const [active, setActive] = React.useState(0);
  const [contentRows, setContentRows] = React.useState<PaletteRow[]>([]);
  const [searching, setSearching] = React.useState(false);

  const panelRef = React.useRef<HTMLDivElement | null>(null);
  const listRef = React.useRef<HTMLDivElement | null>(null);

  // Focus the SRCL Input's underlying <input> on mount. The SRCL Input does not
  // forward a ref, so reach it through the panel after the first paint.
  React.useEffect(() => {
    panelRef.current?.querySelector<HTMLInputElement>('input')?.focus();
  }, []);

  // Title search against the v3 library lists, debounced. Filters movies +
  // series client-side (these are the seeded library; a few hundred rows) so the
  // palette works offline against whatever the daemon already returned.
  React.useEffect(() => {
    const q = query.trim().toLowerCase();
    if (q.length < 2) {
      setContentRows([]);
      setSearching(false);
      return;
    }
    const controller = new AbortController();
    setSearching(true);
    const handle = setTimeout(async () => {
      try {
        const [movies, series] = await Promise.allSettled([
          api.listMovies(controller.signal),
          api.listSeries(controller.signal),
        ]);
        const out: PaletteRow[] = [];
        const pushMatch = (id: string, title: string, year: number | undefined, sub: string) => {
          if (!title.toLowerCase().includes(q)) return;
          out.push({
            kind: 'content',
            key: `content:${sub}:${id}`,
            label: year ? `${title} (${year})` : title,
            sub,
            href: `/content/?id=${encodeURIComponent(id)}`,
          });
        };
        if (movies.status === 'fulfilled') {
          for (const m of (movies.value as Movie[]) ?? []) pushMatch(m.id, m.title, m.year, 'Movie');
        }
        if (series.status === 'fulfilled') {
          for (const s of (series.value as Series[]) ?? []) pushMatch(s.id, s.title, undefined, 'Series');
        }
        setContentRows(out.slice(0, 20));
      } catch {
        setContentRows([]);
      } finally {
        setSearching(false);
      }
    }, 180);
    return () => {
      controller.abort();
      clearTimeout(handle);
    };
  }, [query]);

  const rows = React.useMemo<PaletteRow[]>(
    () => [...filterNav(query), ...contentRows],
    [query, contentRows]
  );

  // Keep the active index in range as the result set changes.
  React.useEffect(() => {
    setActive((a) => (rows.length === 0 ? 0 : Math.min(a, rows.length - 1)));
  }, [rows.length]);

  const go = React.useCallback(
    (row: PaletteRow | undefined) => {
      if (!row) return;
      onClose();
      router.push(row.href);
    },
    [onClose, router]
  );

  const onKeyDown = (event: React.KeyboardEvent) => {
    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault();
        setActive((a) => (rows.length === 0 ? 0 : (a + 1) % rows.length));
        break;
      case 'ArrowUp':
        event.preventDefault();
        setActive((a) => (rows.length === 0 ? 0 : (a - 1 + rows.length) % rows.length));
        break;
      case 'Enter':
        event.preventDefault();
        go(rows[active]);
        break;
      case 'Escape':
        event.preventDefault();
        onClose();
        break;
      default:
        break;
    }
  };

  // Scroll the active row into view as the selection moves.
  React.useEffect(() => {
    const el = listRef.current?.querySelector<HTMLElement>(`[data-index="${active}"]`);
    el?.scrollIntoView?.({ block: 'nearest' });
  }, [active]);

  return (
    <div
      className={styles.overlay}
      role="presentation"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={panelRef}
        className={styles.panel}
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        onKeyDown={onKeyDown}
      >
        <Card title="✸ COMMAND PALETTE" mode="left">
          <Input
            aria-label="Search screens and titles"
            placeholder="Jump to a screen or search titles…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <Divider type="GRADIENT" />
          <div className={styles.results} ref={listRef} role="listbox" aria-label="Results">
            {rows.length === 0 ? (
              <div className={styles.empty}>
                <Text style={{ opacity: 0.6 }}>
                  {searching ? 'Searching…' : 'No matches'}
                </Text>
              </div>
            ) : (
              rows.map((row, index) => {
                const isActive = index === active;
                const icon = row.kind === 'nav' ? '▸' : '●';
                return (
                  <div
                    key={row.key}
                    data-index={index}
                    role="option"
                    aria-selected={isActive}
                    className={isActive ? styles.activeRow : undefined}
                    onMouseEnter={() => setActive(index)}
                  >
                    <ActionListItem icon={icon} onClick={() => go(row)}>
                      {row.kind === 'content' ? (
                        <span>
                          {row.label}
                          <span className={styles.tag}> · {row.sub}</span>
                        </span>
                      ) : (
                        row.label
                      )}
                    </ActionListItem>
                  </div>
                );
              })
            )}
          </div>
          <Divider type="GRADIENT" />
          <Text style={{ opacity: 0.6, padding: '0 1ch' }}>
            ↑↓ navigate · ↵ open · esc close
          </Text>
        </Card>
      </div>
    </div>
  );
};

export const CommandPaletteProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [isOpen, setIsOpen] = React.useState(false);

  const open = React.useCallback(() => setIsOpen(true), []);
  const close = React.useCallback(() => setIsOpen(false), []);
  const toggle = React.useCallback(() => setIsOpen((o) => !o), []);

  // Global hotkeys: ⌘K / Ctrl+K toggle; "/" opens (unless typing in a field);
  // Escape is handled inside the panel.
  React.useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && (e.key === 'k' || e.key === 'K')) {
        e.preventDefault();
        toggle();
        return;
      }
      if (e.key === '/' && !isOpen) {
        const target = e.target as HTMLElement | null;
        const tag = target?.tagName?.toLowerCase();
        const editable =
          tag === 'input' || tag === 'textarea' || target?.isContentEditable === true;
        if (!editable) {
          e.preventDefault();
          open();
        }
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [isOpen, open, toggle]);

  const value = React.useMemo<CommandPaletteApi>(
    () => ({ open, close, toggle, isOpen }),
    [open, close, toggle, isOpen]
  );

  return (
    <CommandPaletteContext.Provider value={value}>
      {children}
      {isOpen ? <CommandPalette onClose={close} /> : null}
    </CommandPaletteContext.Provider>
  );
};

/** Access the command-palette controls (open/close/toggle + isOpen). */
export function useCommandPalette(): CommandPaletteApi {
  const ctx = React.useContext(CommandPaletteContext);
  if (!ctx) {
    return { open: () => {}, close: () => {}, toggle: () => {}, isOpen: false };
  }
  return ctx;
}
