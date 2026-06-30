'use client';

// A status chip: SRCL Badge tinted by the shared severity-tone map (facelift D1).
// Pure composition of the SRCL Badge primitive + a theming token — no new
// primitive. `tone` overrides the auto-derived tone when a caller knows better.

import * as React from 'react';

import Badge from '@components/Badge';

import { TONE_COLOR, toneFor, type Tone } from '@app/_lib/status';

interface StatusBadgeProps extends React.HTMLAttributes<HTMLSpanElement> {
  /** The status token; also used to derive the colour when `tone` is omitted. */
  status: string;
  /** Force a tone instead of deriving it from `status`. */
  tone?: Tone;
}

const StatusBadge: React.FC<StatusBadgeProps> = ({ status, tone, style, ...rest }) => {
  const color = TONE_COLOR[tone ?? toneFor(status)];
  return (
    <Badge style={{ color, ...style }} {...rest}>
      {status}
    </Badge>
  );
};

export default StatusBadge;
