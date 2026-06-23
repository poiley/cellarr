'use client';

// Item-detail screen (docs/10-ui.md §screen-mapping). Shows one content node:
// a CardDouble header with type/status badges and ActionButtons, a TreeView of
// the series→season→episode hierarchy (for TV), and a SimpleTable of the node's
// on-disk files with quality + custom-format-score badges.
//
// Wired to GET /content/{id}, /content/{id}/files, and (to assemble the tree)
// /libraries/{id}/content. Composed exclusively from vendored SRCL primitives.

import * as React from 'react';
import Link from 'next/link';
import { useSearchParams } from 'next/navigation';

import Card from '@components/Card';
import CardDouble from '@components/CardDouble';
import SimpleTable from '@components/SimpleTable';
import TreeView from '@components/TreeView';
import Badge from '@components/Badge';
import Text from '@components/Text';
import Row from '@components/Row';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Divider from '@components/Divider';
import ActionButton from '@components/ActionButton';
import BreadCrumbs from '@components/BreadCrumbs';

import AppShell from '@app/_components/AppShell';
import { api, ApiError } from '@lib/api/client';
import type { ContentNode, ContentRef, MediaFile } from '@lib/api/types';
import {
  basename,
  coordsLabel,
  formatSize,
  kindOf,
  mediaTypeOf,
  monitoredLabel,
  qualityName,
  scoreLabel,
  titleOf,
} from '@app/library/format';

type Loose = Record<string, unknown>;

type LoadState<T> =
  | { phase: 'loading' }
  | { phase: 'error'; message: string }
  | { phase: 'ready'; data: T };

function errorMessage(err: unknown, fallback: string): string {
  if (err instanceof ApiError) return `${err.message} (${err.code})`;
  return err instanceof Error ? err.message : fallback;
}

/** Build a parent→children index from the library's flat content list. */
function indexChildren(refs: ContentRef[]): Map<string, ContentRef[]> {
  const byParent = new Map<string, ContentRef[]>();
  for (const ref of refs) {
    const parent = (ref as Loose).parent_id;
    const key = typeof parent === 'string' ? parent : '__root__';
    const bucket = byParent.get(key) ?? [];
    bucket.push(ref);
    byParent.set(key, bucket);
  }
  return byParent;
}

function ContentBranch({
  node,
  byParent,
  activeId,
  isLastChild,
}: {
  node: ContentRef;
  byParent: Map<string, ContentRef[]>;
  activeId: string;
  isLastChild?: boolean;
}) {
  const loose = node as Loose;
  const children = byParent.get(node.id) ?? [];
  const isLeaf = children.length === 0;
  const label = `${titleOf(loose)}${node.id === activeId ? '  •' : ''}`;

  return (
    <TreeView title={label} isFile={isLeaf} isLastChild={isLastChild} defaultValue>
      {children.map((child, i) => (
        <ContentBranch
          key={child.id}
          node={child}
          byParent={byParent}
          activeId={activeId}
          isLastChild={i === children.length - 1}
        />
      ))}
    </TreeView>
  );
}

function ItemDetail() {
  const params = useSearchParams();
  const id = params.get('id') ?? undefined;

  const [node, setNode] = React.useState<LoadState<ContentNode>>({ phase: 'loading' });
  const [files, setFiles] = React.useState<LoadState<MediaFile[]>>({ phase: 'loading' });
  const [siblings, setSiblings] = React.useState<ContentRef[]>([]);
  const [command, setCommand] = React.useState<string | null>(null);

  React.useEffect(() => {
    if (!id) return;
    const controller = new AbortController();
    setNode({ phase: 'loading' });
    setFiles({ phase: 'loading' });
    setSiblings([]);

    api
      .getContent(id, controller.signal)
      .then((data) => {
        setNode({ phase: 'ready', data });
        // Once we know the library, fetch its content to assemble the tree.
        const libId = (data as Loose).library_id;
        if (typeof libId === 'string') {
          api
            .listContent(libId, controller.signal)
            .then(setSiblings)
            .catch(() => setSiblings([]));
        }
      })
      .catch((err: unknown) => {
        if (err instanceof ApiError && err.code === 'network_error') {
          setNode({ phase: 'error', message: 'API unreachable' });
          return;
        }
        setNode({ phase: 'error', message: errorMessage(err, 'failed to load item') });
      });

    api
      .listContentFiles(id, controller.signal)
      .then((data) => setFiles({ phase: 'ready', data }))
      .catch((err: unknown) => {
        if (err instanceof ApiError && err.code === 'network_error') {
          setFiles({ phase: 'ready', data: [] });
          return;
        }
        setFiles({ phase: 'error', message: errorMessage(err, 'failed to load files') });
      });

    return () => controller.abort();
  }, [id]);

  const runCommand = (name: string) => {
    if (!id) return;
    setCommand(null);
    api
      .runCommand(name, id)
      .then((res) => setCommand(`${name} accepted (${res.status})`))
      .catch((err: unknown) => setCommand(errorMessage(err, `${name} failed`)));
  };

  if (!id) {
    return (
      <AppShell>
        <Card title="Item detail">
          <Text>No item selected.</Text>
          <Link href="/library/" style={{ textDecoration: 'none' }}>
            <Text>Go to the Library to pick one.</Text>
          </Link>
        </Card>
      </AppShell>
    );
  }

  const data = node.phase === 'ready' ? node.data : undefined;
  const loose = data as Loose | undefined;

  const breadcrumbs = [
    { name: 'Library', url: '/library/' },
    ...(loose && typeof loose.library_id === 'string'
      ? [{ name: 'Content', url: `/library/?lib=${encodeURIComponent(loose.library_id)}` }]
      : []),
    { name: data ? titleOf(loose ?? {}) : 'Item' },
  ];

  // Assemble the hierarchy: find this node's root, render the tree from there.
  const byParent = indexChildren(siblings);
  const root = (() => {
    if (!data) return undefined;
    let current: ContentRef | undefined = siblings.find((s) => s.id === data.id);
    if (!current) return undefined;
    const byId = new Map(siblings.map((s) => [s.id, s] as const));
    let parentId = (current as Loose).parent_id;
    while (typeof parentId === 'string' && byId.has(parentId)) {
      current = byId.get(parentId)!;
      parentId = (current as Loose).parent_id;
    }
    return current;
  })();

  const fileRows = files.phase === 'ready' ? files.data : [];
  const fileTable: string[][] = [
    ['File', 'Quality', 'Score', 'Size'],
    ...fileRows.map((f) => {
      const lf = f as Loose;
      return [basename(lf.path), qualityName(lf), scoreLabel(lf) ?? '—', formatSize(lf.size)];
    }),
  ];

  return (
    <AppShell>
      <BreadCrumbs items={breadcrumbs} />

      <CardDouble title={data ? titleOf(loose ?? {}) : 'Item detail'}>
        {node.phase === 'loading' ? <Text>Loading item…</Text> : null}
        {node.phase === 'error' ? <Text>Could not load item: {node.message}</Text> : null}

        {data ? (
          <>
            <RowSpaceBetween>
              <Row style={{ gap: '0.5ch', flexWrap: 'wrap' }}>
                <Badge>{kindOf(loose ?? {}) ?? mediaTypeOf(loose ?? {})}</Badge>
                <Badge>{monitoredLabel(loose ?? {})}</Badge>
                {coordsLabel((loose ?? {}).coords) ? (
                  <Badge>{coordsLabel((loose ?? {}).coords)}</Badge>
                ) : null}
              </Row>
            </RowSpaceBetween>

            <Divider type="GRADIENT" />

            <Row style={{ gap: '1ch', flexWrap: 'wrap' }}>
              <ActionButton hotkey="⌘R" onClick={() => runCommand('RefreshContent')}>
                Refresh
              </ActionButton>
              <ActionButton hotkey="⌘F" onClick={() => runCommand('ManualSearch')}>
                Search
              </ActionButton>
            </Row>
            {command ? <Text>{command}</Text> : null}
          </>
        ) : null}
      </CardDouble>

      {data && root ? (
        <Card title="Structure" style={{ marginTop: '2ch' }}>
          <ContentBranch node={root} byParent={byParent} activeId={data.id} isLastChild />
        </Card>
      ) : null}

      <Card title="Files" style={{ marginTop: '2ch' }}>
        {files.phase === 'loading' ? <Text>Loading files…</Text> : null}
        {files.phase === 'error' ? <Text>Could not load files: {files.message}</Text> : null}
        {files.phase === 'ready' && fileRows.length === 0 ? (
          <Text>No files on disk yet. Nothing has been imported for this item.</Text>
        ) : null}
        {files.phase === 'ready' && fileRows.length > 0 ? (
          <>
            <SimpleTable data={fileTable} align={['left', 'left', 'right', 'right']} />
            <Divider type="GRADIENT" />
            <Row style={{ gap: '0.5ch', flexWrap: 'wrap' }}>
              {fileRows.map((f) => {
                const lf = f as Loose;
                const score = scoreLabel(lf);
                return (
                  <Badge key={String(lf.id)}>
                    {qualityName(lf)}
                    {score ? ` · ${score}` : ''}
                  </Badge>
                );
              })}
            </Row>
          </>
        ) : null}
      </Card>
    </AppShell>
  );
}

export default function Page() {
  return (
    <React.Suspense
      fallback={
        <AppShell>
          <Card title="Item detail">
            <Text>Loading…</Text>
          </Card>
        </AppShell>
      }
    >
      <ItemDetail />
    </React.Suspense>
  );
}
