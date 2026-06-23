'use client';

// Theme toggle assembled from SRCL's RadioButton primitive (a controlled group).
// SRCL's RadioButtonGroup is uncontrolled (no onChange), so the controller wires
// individual SRCL RadioButtons — still SRCL primitives, no new UI component.

import * as React from 'react';

import RadioButton from '@components/RadioButton';

import { useTheme } from '@lib/ThemeProvider';
import type { ThemeChoice } from '@lib/theme';

const OPTIONS: { value: ThemeChoice; label: string }[] = [
  { value: 'system', label: 'System' },
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
];

const ThemeToggle: React.FC = () => {
  const { choice, setChoice } = useTheme();

  return (
    <div role="radiogroup" aria-label="Theme">
      {OPTIONS.map((option) => (
        <RadioButton
          key={option.value}
          name="cellarr-theme"
          value={option.value}
          selected={choice === option.value}
          onSelect={(value) => setChoice(value as ThemeChoice)}
        >
          {option.label}
        </RadioButton>
      ))}
    </div>
  );
};

export default ThemeToggle;
