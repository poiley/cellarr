'use client';

// Settings — the shared "managed by config" badge + a helper for disabling the
// edit/delete controls of an entity the config-as-code reconciler owns.
//
// Several /api/v3 resources (and the native library list) now carry an additive,
// read-only `managed: true` flag, derived purely from the managed-config ledger
// (crates/cellarr-api/src/shim.rs `with_managed`, native.rs `library_with_managed`).
// When it is true the entity is owned by `cellarr managed-config` — editing or
// deleting it from the UI would be reverted on the next reconcile — so every
// settings list renders this badge and locks its row's Edit/Delete affordances.
//
// SRCL-only: an SRCL Badge, wrapped in the SRCL HoverComponentTrigger so a real
// SRCL Tooltip explains the lock on hover/focus. The badge carries an explicit
// `aria-label` (and a native `title`) so its purpose is announced even without
// the hover tooltip.

import * as React from 'react';

import Badge from '@components/Badge';
import HoverComponentTrigger from '@components/HoverComponentTrigger';

/** The single source of truth for the badge's visible + accessible label. */
export const MANAGED_BADGE_LABEL = 'managed by config';

/** The explanatory hint shown in the tooltip + announced via aria. */
export const MANAGED_BADGE_HINT =
  'This entry is managed by config-as-code. Edits made here would be reverted on the next reconcile, so its controls are read-only.';

/**
 * Whether a resource object carries the additive read-only `managed: true` flag.
 * Tolerant of the flag being absent (older daemon / a resource that never carries
 * it) — only an explicit `true` locks the row.
 */
export function isManaged(entity: { managed?: boolean } | null | undefined): boolean {
  return entity?.managed === true;
}

export interface ManagedBadgeProps {
  /**
   * A noun describing the entity (e.g. "root folder", "quality profile"), folded
   * into the accessible label so each badge announces what it locks. Optional —
   * defaults to a generic phrasing.
   */
  entityLabel?: string;
}

/**
 * The read-only "managed by config" chip. Shown next to a config-owned entity's
 * name; its Edit/Delete controls are disabled alongside it.
 */
const ManagedBadge: React.FC<ManagedBadgeProps> = ({ entityLabel }) => {
  const ariaLabel = entityLabel
    ? `${entityLabel} is ${MANAGED_BADGE_LABEL}. ${MANAGED_BADGE_HINT}`
    : `${MANAGED_BADGE_LABEL}. ${MANAGED_BADGE_HINT}`;

  return (
    <HoverComponentTrigger component="tooltip" text={MANAGED_BADGE_HINT}>
      <Badge role="status" aria-label={ariaLabel} title={MANAGED_BADGE_HINT}>
        {MANAGED_BADGE_LABEL}
      </Badge>
    </HoverComponentTrigger>
  );
};

export default ManagedBadge;
