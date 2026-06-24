import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import * as React from 'react';

import { ToastProvider, useToast } from '@app/_lib/ToastProvider';

function Harness() {
  const { success, error, dismiss } = useToast();
  return (
    <div>
      <button onClick={() => success('saved profile')}>fire-success</button>
      <button onClick={() => error('grab failed')}>fire-error</button>
      <button onClick={() => dismiss()}>dismiss-last</button>
    </div>
  );
}

describe('ToastProvider / useToast', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
    cleanup();
  });

  it('renders a success toast with an aria-live region and the message', () => {
    render(
      <ToastProvider>
        <Harness />
      </ToastProvider>
    );
    act(() => {
      fireEvent.click(screen.getByText('fire-success'));
    });
    expect(screen.getByText('saved profile')).toBeTruthy();
    const region = screen.getByRole('region', { name: 'Notifications' });
    expect(region.querySelector('[aria-live="polite"]')).toBeTruthy();
  });

  it('auto-dismisses after the variant duration', () => {
    render(
      <ToastProvider>
        <Harness />
      </ToastProvider>
    );
    act(() => {
      fireEvent.click(screen.getByText('fire-success'));
    });
    expect(screen.queryByText('saved profile')).toBeTruthy();
    act(() => {
      vi.advanceTimersByTime(4000);
    });
    expect(screen.queryByText('saved profile')).toBeNull();
  });

  it('manual dismiss removes the most recent toast immediately', () => {
    render(
      <ToastProvider>
        <Harness />
      </ToastProvider>
    );
    act(() => {
      fireEvent.click(screen.getByText('fire-error'));
    });
    expect(screen.queryByText('grab failed')).toBeTruthy();
    act(() => {
      fireEvent.click(screen.getByText('dismiss-last'));
    });
    expect(screen.queryByText('grab failed')).toBeNull();
  });

  it('useToast outside a provider is a safe no-op', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    function Lonely() {
      const { success } = useToast();
      return <button onClick={() => success('x')}>go</button>;
    }
    render(<Lonely />);
    act(() => {
      fireEvent.click(screen.getByText('go'));
    });
    expect(screen.queryByText('x')).toBeNull();
    warn.mockRestore();
  });
});
