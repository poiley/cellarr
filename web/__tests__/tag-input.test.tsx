import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

import * as React from 'react';

import TagInput from '@app/settings/_components/TagInput';
import type { Tag } from '@lib/api/types';

const TAGS: Tag[] = [
  { id: 1, label: 'hd' },
  { id: 2, label: 'kids' },
];

// A controlled harness so onChange actually updates the rendered chips.
function Harness({
  initial = [],
  onCreate,
}: {
  initial?: number[];
  onCreate?: (label: string) => Promise<Tag>;
}) {
  const [value, setValue] = React.useState<number[]>(initial);
  return (
    <TagInput available={TAGS} value={value} onChange={setValue} onCreate={onCreate} label="Test tags" />
  );
}

describe('TagInput', () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('renders selected tags as removable chips and removes on click', () => {
    render(<Harness initial={[1]} />);
    // The selected chip carries a remove control.
    expect(screen.getByLabelText('Remove tag hd')).toBeTruthy();
    fireEvent.click(screen.getByLabelText('Remove tag hd'));
    // Removing it drops the chip (it reappears only as a re-add suggestion).
    expect(screen.queryByLabelText('Remove tag hd')).toBeNull();
    expect(screen.getByRole('button', { name: 'Add tag hd' })).toBeTruthy();
  });

  it('adds an existing tag from a suggestion chip', () => {
    render(<Harness initial={[]} />);
    fireEvent.click(screen.getByRole('button', { name: 'Add tag kids' }));
    // The chip now shows in the selected row.
    expect(screen.getByLabelText('Remove tag kids')).toBeTruthy();
  });

  it('mints a new tag via onCreate when the typed label is unknown', async () => {
    const onCreate = vi.fn().mockResolvedValue({ id: 9, label: 'archive' } satisfies Tag);
    render(<Harness initial={[]} onCreate={onCreate} />);

    fireEvent.change(screen.getByLabelText('Add a tag to Test tags'), {
      target: { value: 'archive' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Add tag archive' }));

    await waitFor(() => expect(onCreate).toHaveBeenCalledWith('archive'));
  });

  it('shows the empty-state hint when nothing is selected', () => {
    render(<Harness initial={[]} />);
    expect(screen.getByText(/applies everywhere/i)).toBeTruthy();
  });
});
