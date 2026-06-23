'use client';

// Theme context + hook. Glue around the SRCL body classes (lib/theme.ts) —
// introduces no UI primitives. Consumers read the current choice / resolved
// theme and call setChoice() from a SRCL-built toggle.

import * as React from 'react';

import {
  applyTheme,
  readStoredChoice,
  resolveTheme,
  subscribeSystem,
  writeStoredChoice,
  type ResolvedTheme,
  type ThemeChoice,
} from '@lib/theme';

interface ThemeContextValue {
  choice: ThemeChoice;
  resolved: ResolvedTheme;
  setChoice: (choice: ThemeChoice) => void;
}

const ThemeContext = React.createContext<ThemeContextValue | null>(null);

export const ThemeProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [choice, setChoiceState] = React.useState<ThemeChoice>('system');
  const [resolved, setResolved] = React.useState<ResolvedTheme>('light');

  // On mount, adopt the persisted choice (the pre-hydration script already set
  // the class for first paint; this syncs React state to it).
  React.useEffect(() => {
    const stored = readStoredChoice();
    const next = resolveTheme(stored);
    setChoiceState(stored);
    setResolved(next);
    applyTheme(next);
  }, []);

  // While on "System", follow the OS live.
  React.useEffect(() => {
    if (choice !== 'system') return;
    return subscribeSystem((prefersDark) => {
      const next: ResolvedTheme = prefersDark ? 'dark' : 'light';
      setResolved(next);
      applyTheme(next);
    });
  }, [choice]);

  const setChoice = React.useCallback((next: ThemeChoice) => {
    writeStoredChoice(next);
    const nextResolved = resolveTheme(next);
    setChoiceState(next);
    setResolved(nextResolved);
    applyTheme(nextResolved);
  }, []);

  const value = React.useMemo<ThemeContextValue>(
    () => ({ choice, resolved, setChoice }),
    [choice, resolved, setChoice]
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
};

export function useTheme(): ThemeContextValue {
  const ctx = React.useContext(ThemeContext);
  if (!ctx) {
    throw new Error('useTheme must be used within a ThemeProvider');
  }
  return ctx;
}
