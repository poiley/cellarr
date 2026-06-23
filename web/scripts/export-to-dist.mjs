#!/usr/bin/env node
// Postbuild: mirror Next's static export (`out/`) into `web/dist`, which the
// daemon embeds via rust-embed (docs/09-api.md). We copy into dist rather than
// pointing distDir at it so dist stays a clean, committed-placeholder-replacing
// artifact directory.

import { cpSync, existsSync, mkdirSync, readdirSync, rmSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const WEB_ROOT = join(__dirname, '..');
const OUT = join(WEB_ROOT, 'out');
const DIST = join(WEB_ROOT, 'dist');

if (!existsSync(OUT)) {
  console.error(`export-to-dist: '${OUT}' not found — did 'next build' run with output: 'export'?`);
  process.exit(1);
}

// Clear dist (keep the directory itself) then copy the export in.
if (existsSync(DIST)) {
  for (const entry of readdirSync(DIST)) {
    rmSync(join(DIST, entry), { recursive: true, force: true });
  }
} else {
  mkdirSync(DIST, { recursive: true });
}

cpSync(OUT, DIST, { recursive: true });

const indexPath = join(DIST, 'index.html');
if (!existsSync(indexPath) || !statSync(indexPath).isFile()) {
  console.error(`export-to-dist: no index.html in '${DIST}' after copy.`);
  process.exit(1);
}

console.log(`export-to-dist OK: mirrored ${OUT} -> ${DIST}`);
