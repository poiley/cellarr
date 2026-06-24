'use client';

// Settings-screen data glue (not a UI primitive — allowed by the SRCL-only rule).
//
// The shared API client (web/lib/api/client.ts) models GET/POST/PUT/DELETE for
// remotepathmapping and a GET for rootfolder, but not the rootfolder create /
// delete routes. Rather than reach into shared infra, this helper uses the
// client's documented public `requestV3` escape hatch ("for routes not yet
// modelled") to drive POST /api/v3/rootfolder and DELETE /api/v3/rootfolder/{id}.
// Shapes confirmed by the backend:
//   POST /api/v3/rootfolder {path, name?} -> {id, path, accessible, freeSpace, unmappedFolders}
//   DELETE /api/v3/rootfolder/{id}        -> 200, idempotent

import type { CellarrClient } from '@lib/api/client';
import type { RootFolder } from '@lib/api/types';

export function createRootFolder(
  client: CellarrClient,
  body: { path: string; name?: string },
  signal?: AbortSignal
): Promise<RootFolder> {
  return client.requestV3<RootFolder>('/rootfolder', {
    method: 'POST',
    body,
    signal,
  });
}

export function deleteRootFolder(
  client: CellarrClient,
  id: number,
  signal?: AbortSignal
): Promise<void> {
  return client.requestV3<void>(`/rootfolder/${id}`, {
    method: 'DELETE',
    signal,
  });
}
