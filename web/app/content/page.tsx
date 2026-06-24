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
import { useRouter, useSearchParams } from 'next/navigation';

import Card from '@components/Card';
import CardDouble from '@components/CardDouble';
import SimpleTable from '@components/SimpleTable';
import TreeView from '@components/TreeView';
import Badge from '@components/Badge';
import Button from '@components/Button';
import Text from '@components/Text';
import Row from '@components/Row';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Divider from '@components/Divider';
import ActionButton from '@components/ActionButton';
import BreadCrumbs from '@components/BreadCrumbs';

import AppShell from '@app/_components/AppShell';
import { useToast } from '@app/_lib/ToastProvider';
import { api, ApiError } from '@lib/api/client';
import type { ContentNode, ContentRef, MediaFile, QualityProfile } from '@lib/api/types';
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
import {
  detailKindFor,
  getDetail,
  mediaCoverUrl,
  setMonitored,
  toDetailView,
  type DetailView,
} from './_lib/detail';

type Loose = Record<string, unknown>;

type LoadState<T> =
  | { phase: 'loading' }
  | { phase: 'error'; message: string }
  | { phase: 'ready'; data: T };

function errorMessage(err: unknown, fallback: string): string {
  if (err instanceof ApiError) return `${err.message} (${err.code})`;
  return err instanceof Error ? err.message : fallback;
}

/**
 * Resolve the human title for a content node.
 *
 * The `/api/v1/content/{id}` node carries no `title` (only a `title_id` and the
 * structural `coords`/`kind`), so `titleOf` would otherwise fall back to the raw
 * `#shortid`. The readable title lives in the v3 catalogues (`/api/v3/movie`,
 * `/api/v3/series`), keyed by the SAME id the Library screen drills in with.
 * Prefer any title already on the node, then the catalogue match, then the
 * structural fallback.
 */
function resolveTitle(node: Loose | undefined, catalogueTitle: string | undefined): string {
  if (!node) return 'Item';
  const direct = node.title ?? node.name;
  if (typeof direct === 'string' && direct.length) return direct;
  if (catalogueTitle) return catalogueTitle;
  return titleOf(node);
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

/**
 * A short status token for the header badge. Prefers the v3 detail's `status`
 * (e.g. `released` / `continuing`), falling back to the file-state derived
 * MONITORED/UNMONITORED token the screen always had.
 */
function statusLabel(detail: DetailView | undefined, loose: Loose | undefined): string {
  if (detail?.status) return detail.status.toUpperCase();
  return monitoredLabel(loose ?? {});
}

/** Format a v3 runtime (minutes) as `Nh Mm` / `Mm`, or undefined when unknown. */
function formatRuntime(minutes: number | undefined): string | undefined {
  if (minutes === undefined || minutes <= 0) return undefined;
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  if (h > 0) return m > 0 ? `${h}h ${m}m` : `${h}h`;
  return `${m}m`;
}

/**
 * The item poster (#20): an `<img>` pointed at the cached-artwork endpoint
 * (`GET /api/v3/mediacover/{id}/poster`), which 404s when no artwork is cached.
 * Rather than show a broken-image glyph, we track the load state and swap in an
 * ASCII placeholder card (the terminal/OLED aesthetic) on error or while there is
 * no id. The frame keeps a 2:3 poster aspect so the layout doesn't jump.
 */
function Poster({ id, title }: { id: string; title: string }) {
  // 'loading' until the image resolves; 'error' when the endpoint 404s / fails.
  const [state, setState] = React.useState<'loading' | 'ok' | 'error'>('loading');
  // Reset when the id changes so a re-navigation re-attempts the fetch.
  React.useEffect(() => setState('loading'), [id]);

  const frame: React.CSSProperties = {
    width: '20ch',
    aspectRatio: '2 / 3',
    flex: '0 0 auto',
    border: '1px solid var(--theme-border, var(--theme-text))',
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    overflow: 'hidden',
    background: 'var(--theme-background)',
  };

  if (state === 'error') {
    return (
      <div style={frame} aria-label={`No poster for ${title}`} role="img">
        <div style={{ textAlign: 'center', padding: '1ch' }}>
          <Text style={{ fontSize: '3ch', opacity: 0.5 }} aria-hidden="true">
            ▦
          </Text>
          <Text style={{ opacity: 0.5 }}>No poster</Text>
        </div>
      </div>
    );
  }

  return (
    <div style={frame}>
      {/* Real artwork: an <img> is allowed for media (the SRCL-only lint governs
          component imports, not media tags). Hidden until it actually loads so a
          half-loaded/404 frame never flashes; onError swaps in the placeholder. */}
      <img
        src={mediaCoverUrl('poster', id)}
        alt={`${title} poster`}
        onLoad={() => setState('ok')}
        onError={() => setState('error')}
        style={{
          width: '100%',
          height: '100%',
          objectFit: 'cover',
          display: state === 'ok' ? 'block' : 'none',
        }}
      />
      {state === 'loading' ? <Text style={{ opacity: 0.5 }}>Loading…</Text> : null}
    </div>
  );
}

/** One labelled metadata row (`Label  value`) in the detail block. */
function MetaRow({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <RowSpaceBetween style={{ gap: '2ch' }}>
      <Text style={{ opacity: 0.6 }}>{label}</Text>
      <Text style={{ textAlign: 'right' }}>{value}</Text>
    </RowSpaceBetween>
  );
}

/**
 * The metadata block above Files (#20): title / year / overview / quality
 * profile / total size / status, plus the actionable Monitored toggle (#21).
 * All values come from the rich v3 detail resource with structural fallbacks;
 * absent fields (e.g. the backend-deferred year/overview) simply don't render.
 */
function MetadataBlock({
  detail,
  year,
  runtime,
  overview,
  profileName,
  monitored,
  toggling,
  onToggleMonitored,
}: {
  detail: DetailView | undefined;
  year: number | undefined;
  runtime: string | undefined;
  overview: string | undefined;
  profileName: string | undefined;
  monitored: boolean;
  toggling: boolean;
  onToggleMonitored: () => void;
}) {
  const sizeBytes = detail?.sizeOnDisk;
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '0.5ch' }}>
      {year !== undefined ? <MetaRow label="Year" value={year} /> : null}
      {runtime ? <MetaRow label="Runtime" value={runtime} /> : null}
      <MetaRow
        label="Quality profile"
        value={profileName ?? (detail?.qualityProfileId ? '—' : 'Unassigned')}
      />
      <MetaRow
        label="Total size"
        value={sizeBytes !== undefined && sizeBytes > 0 ? formatSize(sizeBytes) : '—'}
      />
      <MetaRow
        label="Status"
        value={detail?.status ?? (detail?.hasFile ? 'Downloaded' : 'Missing')}
      />
      <RowSpaceBetween style={{ gap: '2ch', alignItems: 'center' }}>
        <Text style={{ opacity: 0.6 }}>Monitored</Text>
        {/* Actionable toggle (#21): a SECONDARY SRCL Button that flips the
            flag and surfaces a toast. The label is the CURRENT state with an
            ASCII status glyph; clicking sets the opposite. */}
        <Button
          theme="SECONDARY"
          isDisabled={toggling}
          onClick={onToggleMonitored}
          aria-pressed={monitored}
        >
          {monitored ? '● Monitored' : '○ Not monitored'}
        </Button>
      </RowSpaceBetween>
      {overview ? (
        <div style={{ marginTop: '1ch' }}>
          <Text style={{ opacity: 0.6 }}>Overview</Text>
          <Text>{overview}</Text>
        </div>
      ) : null}
    </div>
  );
}

function ItemDetail() {
  const router = useRouter();
  const params = useSearchParams();
  const id = params.get('id') ?? undefined;
  const { success, error: toastError, info } = useToast();

  const [node, setNode] = React.useState<LoadState<ContentNode>>({ phase: 'loading' });
  const [files, setFiles] = React.useState<LoadState<MediaFile[]>>({ phase: 'loading' });
  const [siblings, setSiblings] = React.useState<ContentRef[]>([]);
  const [catalogueTitle, setCatalogueTitle] = React.useState<string | undefined>(undefined);
  // The rich v3 detail resource (title/year/overview/size/profile/status) that
  // backs the metadata block. Loaded best-effort alongside the structural node.
  const [detail, setDetail] = React.useState<DetailView | undefined>(undefined);
  const [profiles, setProfiles] = React.useState<QualityProfile[]>([]);
  const [libraryName, setLibraryName] = React.useState<string | undefined>(undefined);
  // In-flight guard for the monitored toggle (#21).
  const [toggling, setToggling] = React.useState(false);

  React.useEffect(() => {
    if (!id) return;
    const controller = new AbortController();
    setNode({ phase: 'loading' });
    setFiles({ phase: 'loading' });
    setSiblings([]);
    setCatalogueTitle(undefined);
    setDetail(undefined);
    setLibraryName(undefined);

    // Quality-profile catalogue — used to name the detail's qualityProfileId.
    api
      .getQualityProfiles(controller.signal)
      .then(setProfiles)
      .catch(() => setProfiles([]));

    // Resolve the readable title from the v3 catalogues. The content node itself
    // carries no title (only a title_id), so look this id up among movies/series
    // — the same ids the Library screen drills in with. Best-effort: failures
    // just leave the structural fallback in place.
    Promise.allSettled([
      api.listMovies(controller.signal),
      api.listSeries(controller.signal),
    ]).then(([movies, series]) => {
      const pool = [
        ...(movies.status === 'fulfilled' ? movies.value : []),
        ...(series.status === 'fulfilled' ? series.value : []),
      ];
      const match = pool.find((m) => m.id === id);
      const title = match ? (match as Loose).title : undefined;
      if (typeof title === 'string' && title.length) setCatalogueTitle(title);
    });

    api
      .getContent(id, controller.signal)
      .then((data) => {
        setNode({ phase: 'ready', data });

        // Fetch the rich v3 detail resource for the metadata block (#20). The
        // catalogue (movie vs series) is inferred from the node's media type.
        getDetail(detailKindFor(data as Loose), id, controller.signal)
          .then((d) => setDetail(toDetailView(d)))
          .catch(() => setDetail(undefined));

        // Once we know the library, fetch its content to assemble the tree, and
        // its name for the breadcrumb middle crumb (#24).
        const libId = (data as Loose).library_id;
        if (typeof libId === 'string') {
          api
            .listContent(libId, controller.signal)
            .then(setSiblings)
            .catch(() => setSiblings([]));
          api
            .getLibrary(libId, controller.signal)
            .then((lib) => setLibraryName(lib.name))
            .catch(() => setLibraryName(undefined));
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
    api
      .runCommand(name, id)
      .then((res) => success(`${name} accepted (${res.status})`))
      .catch((err: unknown) => toastError(errorMessage(err, `${name} failed`)));
  };

  /**
   * Flip the node's `monitored` flag (#21). PUTs to the v3 detail resource and
   * reflects the refreshed value back into the metadata block + the node so the
   * badge/toggle stay in sync. Toast feedback on success and failure.
   */
  const toggleMonitored = () => {
    if (!id || !data || toggling) return;
    const next = !(detail?.monitored ?? (data as Loose).monitored === true);
    setToggling(true);
    info(next ? 'Enabling monitoring…' : 'Disabling monitoring…', { durationMs: 2000 });
    setMonitored(detailKindFor(data as Loose), id, next)
      .then((refreshed) => {
        const view = toDetailView(refreshed);
        if (view) setDetail(view);
        // Mirror onto the structural node so the header badge updates too.
        setNode((prev) =>
          prev.phase === 'ready'
            ? { phase: 'ready', data: { ...prev.data, monitored: next } }
            : prev
        );
        success(next ? 'Monitoring enabled' : 'Monitoring disabled');
      })
      .catch((err: unknown) => toastError(errorMessage(err, 'failed to update monitoring')))
      .finally(() => setToggling(false));
  };

  /**
   * The content-scoped refresh command name the backend accepts.
   *
   * The native command catalogue (crates/cellarr-api/src/commands.rs
   * `kind_for_command`) has NO `RefreshContent` command — sending it 400s with
   * "unknown command". It DOES accept `refreshmovie` / `refreshseries` (both map
   * to `JobKind::MetadataRefresh`), so pick the one matching this node's media
   * type. Confirmed against the live daemon: both return 200 (queued).
   * Series sub-nodes (season/episode) refresh through the series command.
   */
  const refreshCommandFor = (n: Loose | undefined): string =>
    (n && (n.media_type === 'tv' || n.kind === 'series' || n.kind === 'season' || n.kind === 'episode'))
      ? 'RefreshSeries'
      : 'RefreshMovie';

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

  // The header / breadcrumb title: resolved from the v3 catalogue rather than
  // the raw `#shortid` the content node alone would yield.
  const title = resolveTitle(loose, catalogueTitle);

  // Middle crumb shows the LIBRARY name (e.g. 'Movies') once resolved (#24),
  // falling back to 'Content' only until the library load lands.
  const breadcrumbs = [
    { name: 'Library', url: '/library/' },
    ...(loose && typeof loose.library_id === 'string'
      ? [
          {
            name: libraryName ?? 'Content',
            url: `/library/?lib=${encodeURIComponent(loose.library_id)}`,
          },
        ]
      : []),
    { name: data ? title : 'Item' },
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
  // Hide the Score column entirely when no file carries a custom-format score
  // (#23) — an all-'—' column is pure noise. Otherwise keep it and show '—' for
  // the unscored rows.
  const anyScored = fileRows.some((f) => scoreLabel(f as Loose) !== undefined);
  const fileTable: string[][] = [
    anyScored ? ['File', 'Quality', 'Score', 'Size'] : ['File', 'Quality', 'Size'],
    ...fileRows.map((f) => {
      const lf = f as Loose;
      const base = [basename(lf.path), qualityName(lf)];
      if (anyScored) base.push(scoreLabel(lf) ?? '—');
      base.push(formatSize(lf.size));
      return base;
    }),
  ];
  const fileAlign: Array<'left' | 'right'> = anyScored
    ? ['left', 'left', 'right', 'right']
    : ['left', 'left', 'right'];

  // Quality-profile display name for the metadata block (#20).
  const profileName = detail?.qualityProfileId
    ? profiles.find((p) => p.id === detail.qualityProfileId)?.name
    : undefined;

  // Detail fields with structural-node fallbacks. `year` + `overview` are
  // currently backend-deferred (content_detail() returns them empty/zero — see
  // crates/cellarr-api/src/native.rs TODO), so they simply don't render until
  // the identify pipeline persists per-item metadata.
  const year = detail?.year && detail.year > 0 ? detail.year : undefined;
  const runtime = formatRuntime(detail?.runtime);
  const overview = detail?.overview;
  const isMonitored = detail?.monitored ?? (loose ?? {}).monitored === true;

  return (
    <AppShell>
      <BreadCrumbs items={breadcrumbs} />

      <CardDouble title={data ? title : 'Item detail'}>
        {node.phase === 'loading' ? <Text>Loading item…</Text> : null}
        {node.phase === 'error' ? <Text>Could not load item: {node.message}</Text> : null}

        {data ? (
          <>
            <RowSpaceBetween>
              <Row style={{ gap: '0.5ch', flexWrap: 'wrap' }}>
                <Badge>{kindOf(loose ?? {}) ?? mediaTypeOf(loose ?? {})}</Badge>
                <Badge>{statusLabel(detail, loose)}</Badge>
                {coordsLabel((loose ?? {}).coords) ? (
                  <Badge>{coordsLabel((loose ?? {}).coords)}</Badge>
                ) : null}
              </Row>
            </RowSpaceBetween>

            <Divider type="GRADIENT" />

            {/* Metadata block (#20): the cached poster beside the rich detail.
                The two stack on a narrow viewport (flex-wrap) and sit side by
                side otherwise; the metadata column flexes to fill the rest. */}
            <Row style={{ gap: '2ch', flexWrap: 'wrap', alignItems: 'flex-start' }}>
              <Poster id={id} title={title} />
              <div style={{ flex: '1 1 32ch', minWidth: '28ch' }}>
                <MetadataBlock
                  detail={detail}
                  year={year}
                  runtime={runtime}
                  overview={overview}
                  profileName={profileName}
                  monitored={isMonitored}
                  toggling={toggling}
                  onToggleMonitored={toggleMonitored}
                />
              </div>
            </Row>

            <Divider type="GRADIENT" />

            {/* Action hierarchy (#22): Search (find & grab) is the primary CTA;
                Refresh + History are secondary. */}
            <Row style={{ gap: '1ch', flexWrap: 'wrap', alignItems: 'center' }}>
              <Button
                theme="PRIMARY"
                onClick={() =>
                  router.push(
                    `/interactive?id=${encodeURIComponent(id)}&content=${encodeURIComponent(id)}`
                  )
                }
              >
                Search ▸
              </Button>
              <ActionButton hotkey="⌘R" onClick={() => runCommand(refreshCommandFor(loose))}>
                Refresh
              </ActionButton>
              <ActionButton
                hotkey="⌘H"
                onClick={() => router.push(`/history?id=${encodeURIComponent(id)}`)}
              >
                History
              </ActionButton>
            </Row>
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
            <SimpleTable data={fileTable} align={fileAlign} />
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
