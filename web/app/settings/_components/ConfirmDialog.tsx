'use client';

// A destructive-action confirm dialog (#40). Composed only from SRCL primitives:
// a bordered <Card> panel floated over the settings overlay scrim (mirroring
// CustomFormats' modal pattern), plus SRCL <Button>s and <Text>.
//
// We build the panel from Card rather than the SRCL <Dialog> because Dialog
// always renders its own neutral "OK / Cancel" row, which would sit dead beneath
// a danger action. Here the only actions are an unmistakable danger button
// (tinted with --ansi-9-red + a ✗ glyph) and a plain Cancel, so "delete" reads
// as destructive and is visually separated from a normal Save.

import * as React from 'react';

import Card from '@components/Card';
import Button from '@components/Button';
import Text from '@components/Text';

export interface ConfirmDialogProps {
  /** Short, specific title, e.g. "Delete profile". */
  title: string;
  /** Body copy spelling out what is about to happen and that it is permanent. */
  children: React.ReactNode;
  /** Label for the destructive button, e.g. "Delete profile". */
  confirmLabel: string;
  /** Label shown on the destructive button while the action is in flight. */
  pendingLabel?: string;
  /** True while the destructive action is in flight (disables both buttons). */
  pending?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

const ConfirmDialog: React.FC<ConfirmDialogProps> = ({
  title,
  children,
  confirmLabel,
  pendingLabel = 'Deleting…',
  pending = false,
  onConfirm,
  onCancel,
}) => {
  // Dismiss on Escape, matching native dialog behaviour.
  React.useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !pending) onCancel();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onCancel, pending]);

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        background: 'var(--theme-overlay)',
        zIndex: 60,
        padding: '2ch',
      }}
    >
      <div role="alertdialog" aria-modal="true" aria-label={title} style={{ maxWidth: '56ch', width: '100%' }}>
        <Card title={`✗ ${title}`} mode="left">
          <div style={{ margin: '0.5ch 0' }}>{children}</div>
          <Text style={{ opacity: 0.5, margin: '1ch 0' }}>This cannot be undone.</Text>
          <div style={{ display: 'flex', gap: '1ch', marginTop: '1ch' }}>
            <Button
              theme="DANGER"
              aria-label={confirmLabel}
              isDisabled={pending}
              onClick={pending ? undefined : onConfirm}
            >
              {pending ? pendingLabel : confirmLabel}
            </Button>
            <Button theme="SECONDARY" isDisabled={pending} onClick={pending ? undefined : onCancel}>
              Cancel
            </Button>
          </div>
        </Card>
      </div>
    </div>
  );
};

export default ConfirmDialog;
