'use client';

// Compact top-bar theme switch (System / Light / Dark) as a segmented control
// built from SRCL's ActionButton primitive — each segment is an ActionButton
// with isSelected reflecting the active choice. More compact than the sidebar's
// RadioButton group, so it fits the top bar while reclaiming the sidebar for
// navigation. Pure SRCL composition + the theme controller glue.

import * as React from 'react';

import ActionButton from '@components/ActionButton';

import { useTheme } from '@lib/ThemeProvider';
import type { ThemeChoice } from '@lib/theme';

const OPTIONS: { value: ThemeChoice; label: string; glyph: string }[] = [
  { value: 'system', label: 'System', glyph: '◑' },
  { value: 'light', label: 'Light', glyph: '○' },
  { value: 'dark', label: 'Dark', glyph: '●' },
];

const ThemeBarToggle: React.FC = () => {
  const { choice, setChoice } = useTheme();

  return (
    <div role="group" aria-label="Theme" style={{ display: 'flex', gap: '0.5ch' }}>
      {OPTIONS.map((option) => {
        const selected = choice === option.value;
        return (
          <ActionButton key={option.value} isSelected={selected} onClick={() => setChoice(option.value)}>
            <span aria-label={`${option.label} theme`} aria-pressed={selected}>
              {option.glyph} {option.label}
            </span>
          </ActionButton>
        );
      })}
    </div>
  );
};

export default ThemeBarToggle;
