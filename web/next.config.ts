import type { NextConfig } from 'next';
import { fileURLToPath } from 'node:url';
import { dirname } from 'node:path';
import { execSync } from 'node:child_process';

const here = dirname(fileURLToPath(import.meta.url));

// A short build identifier surfaced in the top-bar badge (replaces the old
// "0.0.0" placeholder). Prefer an explicit NEXT_PUBLIC_BUILD_SHA (set by CI),
// then the current git short sha, else 'dev'. Inlined at build time so it is
// available to client components without a runtime fetch.
function resolveBuildSha(): string {
  const fromEnv = process.env.NEXT_PUBLIC_BUILD_SHA;
  if (fromEnv && fromEnv.trim()) return fromEnv.trim();
  try {
    return execSync('git rev-parse --short HEAD', {
      cwd: here,
      stdio: ['ignore', 'pipe', 'ignore'],
    })
      .toString()
      .trim();
  } catch {
    return 'dev';
  }
}

// Static export: the daemon embeds web/dist via rust-embed (docs/09-api.md).
// Next emits the export into `out/`; a postbuild script (scripts/export-to-dist.mjs)
// mirrors it into web/dist. `images.unoptimized` is required for `output: 'export'`.
const nextConfig: NextConfig = {
  output: 'export',
  devIndicators: false,
  images: { unoptimized: true },
  trailingSlash: true,
  env: {
    NEXT_PUBLIC_BUILD_SHA: resolveBuildSha(),
  },
  // Pin the workspace root to web/ so a stray sibling lockfile doesn't mislead
  // Turbopack's root inference.
  turbopack: { root: here },
};

export default nextConfig;
