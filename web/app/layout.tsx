import '@root/global-fonts.css';
import '@root/global.css';

import * as React from 'react';

import Providers from '@app/_components/Providers';
import { PREHYDRATION_SCRIPT } from '@lib/theme';

export const metadata = {
  title: 'cellarr',
  description: 'cellarr — unified media manager',
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en-us" suppressHydrationWarning>
      {/* Default class is a safe fallback if JS is disabled; the script below
          overrides it from the stored choice / OS preference before first paint. */}
      <body className="theme-light" suppressHydrationWarning>
        {/* Pre-hydration theme class so there is no flash of the wrong theme.
            Rendered as the FIRST child of <body> (not <head>) so document.body
            exists when it runs and the theme-* class is applied before the rest
            of the body paints. Sets body.theme-light / body.theme-dark +
            color-scheme synchronously. */}
        <script dangerouslySetInnerHTML={{ __html: PREHYDRATION_SCRIPT }} />
        <Providers>{children}</Providers>
      </body>
    </html>
  );
}
