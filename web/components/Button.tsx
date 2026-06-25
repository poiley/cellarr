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

const Button: React.FC<ButtonProps> = ({ theme = 'PRIMARY', isDisabled, children, ...rest }) => {
  let classNames = Utilities.classNames(styles.root, styles.primary);

  if (theme === 'SECONDARY') {
    classNames = Utilities.classNames(styles.root, styles.secondary);
  }

  if (theme === 'DANGER') {
    classNames = Utilities.classNames(styles.root, styles.danger);
  }

  if (isDisabled) {
    classNames = Utilities.classNames(styles.root, styles.disabled);

    return <div className={classNames}>{children}</div>;
  }

  return (
    <button className={classNames} role="button" tabIndex={0} disabled={isDisabled} {...rest}>
      {children}
    </button>
  );
};

export default Button;
