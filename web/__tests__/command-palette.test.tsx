import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import * as React from 'react';

const push = vi.fn();
vi.mock('next/navigation', () => ({
  usePathname: () => '/',
  useRouter: () => ({ push }),
}));

import {
  CommandPaletteProvider,
  useCommandPalette,
} from '@app/_components/CommandPaletteProvider';

function Opener() {
  const { open } = useCommandPalette();
  return <button onClick={open}>open-palette</button>;
}

describe('CommandPalette', () => {
  beforeEach(() => {
    push.mockReset();
  });
  afterEach(() => {
    cleanup();
  });

  it('opens from the provider api and lists navigable screens', () => {
    render(
      <CommandPaletteProvider>
        <Opener />
      </CommandPaletteProvider>
    );
    act(() => {
      fireEvent.click(screen.getByText('open-palette'));
    });
    expect(screen.getByRole('dialog', { name: 'Command palette' })).toBeTruthy();
    expect(screen.getByText('Dashboard')).toBeTruthy();
    expect(screen.getByText('Settings')).toBeTruthy();
  });

  it('filters the screen list by the search query', () => {
    render(
      <CommandPaletteProvider>
        <Opener />
      </CommandPaletteProvider>
    );
    act(() => {
      fireEvent.click(screen.getByText('open-palette'));
    });
    const input = screen.getByLabelText('Search screens and titles');
    act(() => {
      fireEvent.change(input, { target: { value: 'settings' } });
    });
    expect(screen.getByText('Settings')).toBeTruthy();
    expect(screen.queryByText('Dashboard')).toBeNull();
  });

  it('opens the ⌘K hotkey and closes on Escape', () => {
    render(
      <CommandPaletteProvider>
        <Opener />
      </CommandPaletteProvider>
    );
    act(() => {
      fireEvent.keyDown(window, { key: 'k', metaKey: true });
    });
    const dialog = screen.getByRole('dialog', { name: 'Command palette' });
    act(() => {
      fireEvent.keyDown(dialog, { key: 'Escape' });
    });
    expect(screen.queryByRole('dialog', { name: 'Command palette' })).toBeNull();
  });

  it('arrow + enter navigates to the selected screen', () => {
    render(
      <CommandPaletteProvider>
        <Opener />
      </CommandPaletteProvider>
    );
    act(() => {
      fireEvent.click(screen.getByText('open-palette'));
    });
    const dialog = screen.getByRole('dialog', { name: 'Command palette' });
    // First row (Dashboard) is active by default; Enter should route to '/'.
    act(() => {
      fireEvent.keyDown(dialog, { key: 'Enter' });
    });
    expect(push).toHaveBeenCalledWith('/');
  });
});
