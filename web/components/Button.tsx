'use client';

import styles from '@components/Button.module.css';

import * as React from 'react';
import * as Utilities from '@common/utilities';

interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  // PRIMARY  — the inverse (white/black) full-width affordance for the one main
  //            action of a view (Save, Import).
  // SECONDARY— a bordered, low-emphasis action (Cancel, Edit, row actions).
  // DANGER   — a destructive action (Delete/Remove). Outlined in red with a red
  //            label, going solid-red only on hover/focus, so it reads as
  //            dangerous yet stays subordinate to the PRIMARY action above it.
  theme?: 'PRIMARY' | 'SECONDARY' | 'DANGER';
  isDisabled?: boolean;
  children?: React.ReactNode;
}

const Button: React.FC<ButtonProps> = ({ theme = 'PRIMARY', isDisabled, children, style, ...rest }) => {
  const themeClass =
    theme === 'SECONDARY' ? styles.secondary : theme === 'DANGER' ? styles.danger : styles.primary;

  if (isDisabled) {
    // Keep the THEME class on the disabled element so a disabled DANGER button
    // retains `.danger { width: auto }` (intrinsic width) instead of falling back
    // to the full-bleed `.root { width: 100% }`, and forward `style` so callers can
    // still size/space it. `.disabled` is defined after the theme classes, so it
    // still wins on the overlapping colour properties and reads as disabled.
    return (
      <div className={Utilities.classNames(styles.root, themeClass, styles.disabled)} style={style}>
        {children}
      </div>
    );
  }

  return (
    <button
      className={Utilities.classNames(styles.root, themeClass)}
      style={style}
      role="button"
      tabIndex={0}
      disabled={isDisabled}
      {...rest}
    >
      {children}
    </button>
  );
};

export default Button;
