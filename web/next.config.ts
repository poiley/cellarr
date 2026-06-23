import type { NextConfig } from 'next';
import { fileURLToPath } from 'node:url';
import { dirname } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));

// Static export: the daemon embeds web/dist via rust-embed (docs/09-api.md).
// Next emits the export into `out/`; a postbuild script (scripts/export-to-dist.mjs)
// mirrors it into web/dist. `images.unoptimized` is required for `output: 'export'`.
const nextConfig: NextConfig = {
  output: 'export',
  devIndicators: false,
  images: { unoptimized: true },
  trailingSlash: true,
  // Pin the workspace root to web/ so a stray sibling lockfile doesn't mislead
  // Turbopack's root inference.
  turbopack: { root: here },
};

export default nextConfig;
