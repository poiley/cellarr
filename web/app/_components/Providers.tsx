'use client';

// Client provider wrapper (mirrors SRCL's components/Providers.tsx so the SRCL
// context providers run as Client Components) plus cellarr's ThemeProvider.

import * as React from 'react';

import { HotkeysProvider } from '@modules/hotkeys';
import { ModalProvider } from '@components/page/ModalContext';

import { ThemeProvider } from '@lib/ThemeProvider';
import { ToastProvider } from '@app/_lib/ToastProvider';
import { CommandPaletteProvider } from '@app/_components/CommandPaletteProvider';

const Providers: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  return (
    <ThemeProvider>
      <HotkeysProvider>
        <ModalProvider>
          <ToastProvider>
            <CommandPaletteProvider>{children}</CommandPaletteProvider>
          </ToastProvider>
        </ModalProvider>
      </HotkeysProvider>
    </ThemeProvider>
  );
};

export default Providers;
