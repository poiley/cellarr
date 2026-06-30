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
import Select from '@components/Select';
import Text from '@components/Text';
import Row from '@components/Row';

import { statusColor } from '@app/_lib/status';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Divider from '@components/Divider';
import ActionButton from '@components/ActionButton';
import BreadCrumbs from '@components/BreadCrumbs';

import AppShell from '@app/_components/AppShell';
import TagInput from '@app/settings/_components/TagInput';
import { useToast } from '@app/_lib/ToastProvider';
import { api, ApiError } from '@lib/api/client';
import type { ContentNode, ContentRef, Episode, MediaFile, QualityProfile, Tag } from '@lib/api/types';
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
  setContentTags,
  setEpisodesMonitored,
  setMonitored,
  setSeasonMonitored,
  setSeriesType,
  toDetailView,
  type DetailView,
  type SeriesTypeValue,
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

/** A season/episode node carries a monitor toggle; series/movie nodes do not. */
function isToggleableKind(node: Loose): boolean {
  const kind = node.kind;
  return kind === 'season' || kind === 'episode';
}

function ContentBranch({
  node,
  byParent,
  activeId,
  isLastChild,
  isMonitored,
}: {
  node: ContentRef;
  byParent: Map<string, ContentRef[]>;
  activeId: string;
  isLastChild?: boolean;
  /** Resolved monitored state for a node (local override wins over the ref). */
  isMonitored: (node: ContentRef) => boolean;
}) {
  const loose = node as Loose;
  const children = byParent.get(node.id) ?? [];
  const isLeaf = children.length === 0;
  // A glyph marks the monitored state inline so the tree reads at a glance; the
  // actionable toggles live in the Monitoring card below (the vendored TreeView
  // title is a plain string and must not nest an interactive control).
  const glyph = isToggleableKind(loose) ? (isMonitored(node) ? '● ' : '○ ') : '';
  const label = `${glyph}${titleOf(loose)}${node.id === activeId ? '  •' : ''}`;

  return (
    <TreeView title={label} isFile={isLeaf} isLastChild={isLastChild} defaultValue>
      {children.map((child, i) => (
        <ContentBranch
          key={child.id}
          node={child}
          byParent={byParent}
          activeId={activeId}
          isLastChild={i === children.length - 1}
          isMonitored={isMonitored}
        />
      ))}
    </TreeView>
  );
}

/**
 * A season group for the monitor tree, assembled from the `GET /api/v3/episode`
 * rows (the real `list_episodes` projection): the season number, every episode
 * row beneath it (carrying the numeric episode id, `monitored`, `hasFile`, and
 * `airDate`), and — when resolvable from the structural sibling list — the season
 * *node* id the `/season/monitor` route toggles. When no season node id is known
 * the season toggle cascades over its episode ids instead (via `/episode/monitor`).
 */
interface SeasonGroup {
  seasonNumber: number;
  /** The season content-node id (from the sibling tree), for `/season/monitor`. */
  seasonId?: string;
  episodes: Episode[];
}

/** The episode's id as a string (the v3 projection is a JSON number). */
function epId(ep: Episode): string {
  return String(ep.id);
}

/** A short `SxxEyy` coordinate label for an episode row. */
function episodeCoord(ep: Episode): string {
  const s = String(ep.seasonNumber ?? 0).padStart(2, '0');
  const e = String(ep.episodeNumber ?? 0).padStart(2, '0');
  return `S${s}E${e}`;
}

/**
 * The episode's absolute number for an anime series, as ` · abs NNN` — or an
 * empty string when none is known. The v3 episode row carries
 * `absoluteEpisodeNumber` (null for standard episodes and anime episodes whose
 * absolute is not yet reconciled), so a missing/null value renders nothing. The
 * number is zero-padded to three digits to match the anime convention
 * (e.g. `abs 013`).
 */
function absoluteSuffix(ep: Episode): string {
  const abs = (ep as Record<string, unknown>).absoluteEpisodeNumber;
  if (typeof abs !== 'number' || !Number.isFinite(abs)) return '';
  return ` · abs ${String(abs).padStart(3, '0')}`;
}

/**
 * The Sonarr `seriesType` options the detail screen offers, in display order,
 * with labels. Mirrors the Add dialog's selector (crates/cellarr-core
 * `SeriesType`).
 */
const SERIES_TYPE_OPTIONS: ReadonlyArray<{ value: SeriesTypeValue; label: string }> = [
  { value: 'standard', label: 'Standard' },
  { value: 'daily', label: 'Daily' },
  { value: 'anime', label: 'Anime' },
];

/** The display label for a `seriesType` wire value (case-insensitive). */
function seriesTypeLabel(value: string | undefined): string {
  const v = (value ?? 'standard').trim().toLowerCase();
  return SERIES_TYPE_OPTIONS.find((o) => o.value === v)?.label ?? 'Standard';
}

/** The on-disk glyph for an episode: `✓` when a file is linked, `✗` when not. */
function fileGlyphFor(ep: Episode): '✓' | '✗' {
  return ep.hasFile ? '✓' : '✗';
}

/**
 * The per-season / per-episode monitor tree (TV nodes), sourced from
 * `GET /api/v3/episode?seriesId=…` (the real `list_episodes`). Lists each season
 * with a monitor toggle that cascades to its episodes (Sonarr behavior, via
 * `PUT /api/v3/season/monitor`), and each episode with its own toggle (via
 * `PUT /api/v3/episode/monitor`) plus an inline ●/○ monitored glyph, an on-disk
 * ✓/✗ glyph, and its air date. Renders nothing for a movie/series with no
 * episodes. Composed from SRCL primitives; toast feedback is wired by the parent.
 */
const SeasonMonitoring: React.FC<{
  seasons: SeasonGroup[];
  /** Resolved monitored state for an episode id (local override wins). */
  isEpisodeMonitored: (id: string) => boolean;
  /** Whether every episode in a season group is currently monitored. */
  isSeasonMonitored: (group: SeasonGroup) => boolean;
  isToggling: (id: string) => boolean;
  onToggleSeason: (group: SeasonGroup) => void;
  onToggleEpisode: (ep: Episode) => void;
  /** When true (anime series), each episode row shows its absolute number. */
  showAbsolute: boolean;
}> = ({ seasons, isEpisodeMonitored, isSeasonMonitored, isToggling, onToggleSeason, onToggleEpisode, showAbsolute }) => {
  if (seasons.length === 0) return null;
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '1ch' }}>
      {seasons.map((group) => {
        const seasonOn = isSeasonMonitored(group);
        const seasonLabel =
          group.seasonNumber === 0 ? 'Specials' : `Season ${group.seasonNumber}`;
        // The toggle key is the season node id when known, else a stable synthetic
        // key so the in-flight guard still tracks the cascade.
        const seasonKey = group.seasonId ?? `season:${group.seasonNumber}`;
        return (
          <div key={seasonKey}>
            <RowSpaceBetween style={{ gap: '2ch', alignItems: 'center' }}>
              <Text style={{ fontWeight: 600 }}>
                {seasonOn ? '● ' : '○ '}
                {seasonLabel}
              </Text>
              <Button
                theme="SECONDARY"
                isDisabled={isToggling(seasonKey)}
                onClick={() => onToggleSeason(group)}
                aria-pressed={seasonOn}
                aria-label={`${seasonOn ? 'Unmonitor' : 'Monitor'} ${seasonLabel}`}
              >
                {seasonOn ? '● Monitored' : '○ Not monitored'}
              </Button>
            </RowSpaceBetween>
            {group.episodes.length > 0 ? (
              <div style={{ paddingLeft: '2ch', marginTop: '0.5ch' }}>
                {group.episodes.map((ep) => {
                  const id = epId(ep);
                  const epOn = isEpisodeMonitored(id);
                  const label = ep.title?.length ? ep.title : episodeCoord(ep);
                  // Anime rows show the absolute number after the SxxEyy coord.
                  const coord = `${episodeCoord(ep)}${showAbsolute ? absoluteSuffix(ep) : ''}`;
                  return (
                    <RowSpaceBetween key={id} style={{ gap: '2ch', alignItems: 'center' }}>
                      <Row style={{ gap: '1ch', alignItems: 'center' }}>
                        <Text style={{ opacity: 0.5 }} aria-hidden="true">
                          {epOn ? '●' : '○'}
                        </Text>
                        <Text style={{ opacity: 0.8 }}>
                          {coord} {label}
                        </Text>
                        <Badge>
                          {fileGlyphFor(ep)}
                          {ep.hasFile ? ' file' : ' missing'}
                        </Badge>
                        {ep.airDate ? (
                          <Text style={{ opacity: 0.45 }}>{String(ep.airDate)}</Text>
                        ) : null}
                      </Row>
                      <Button
                        theme="SECONDARY"
                        isDisabled={isToggling(id)}
                        onClick={() => onToggleEpisode(ep)}
                        aria-pressed={epOn}
                        aria-label={`${epOn ? 'Unmonitor' : 'Monitor'} ${episodeCoord(ep)} ${label}`}
                      >
                        {epOn ? '● Monitored' : '○ Not monitored'}
                      </Button>
                    </RowSpaceBetween>
                  );
                })}
              </div>
            ) : null}
            <Divider />
          </div>
        );
      })}
    </div>
  );
};

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
  const imgRef = React.useRef<HTMLImageElement | null>(null);
  // Reset when the id changes so a re-navigation re-attempts the fetch, then
  // immediately reconcile against the element: a *cached* image can already be
  // `complete` before React attaches `onLoad`, so `onLoad` never fires and the
  // card would be stuck on "Loading…" until a refresh. Reading `complete` /
  // `naturalWidth` after the commit covers that race (and re-checks on a new id).
  React.useEffect(() => {
    setState('loading');
    const img = imgRef.current;
    if (img && img.complete) {
      setState(img.naturalWidth > 0 ? 'ok' : 'error');
    }
  }, [id]);

  const frame: React.CSSProperties = {
    width: '30ch',
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
        ref={imgRef}
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
  const statusText = (detail?.status ?? (detail?.hasFile ? 'Downloaded' : 'Missing')).toUpperCase();
  const path = detail?.path;
  const tmdbId = detail?.tmdbId;
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '0.5ch' }}>
      {year !== undefined ? <MetaRow label="Year" value={year} /> : null}
      {runtime ? <MetaRow label="Runtime" value={runtime} /> : null}
      {detail?.genres && detail.genres.length > 0 ? (
        <MetaRow
          label="Genres"
          value={
            <span style={{ display: 'inline-flex', gap: '0.5ch', flexWrap: 'wrap' }}>
              {detail.genres.map((g) => (
                <Badge key={g}>{g}</Badge>
              ))}
            </span>
          }
        />
      ) : null}
      {detail?.rating !== undefined ? (
        <MetaRow
          label="Rating"
          value={
            <span>
              <span style={{ color: 'var(--ansi-11-yellow)' }}>★</span> {detail.rating.toFixed(1)}/10
              {detail.ratingVotes ? (
                <span style={{ opacity: 0.6 }}> ({detail.ratingVotes.toLocaleString()} votes)</span>
              ) : null}
            </span>
          }
        />
      ) : null}
      <MetaRow
        label="Quality profile"
        value={profileName ?? (detail?.qualityProfileId ? '—' : 'Unassigned')}
      />
      <MetaRow
        label="Total size"
        value={sizeBytes !== undefined && sizeBytes > 0 ? formatSize(sizeBytes) : 'Not downloaded'}
      />
      {/* Status casing matches the header badge (both uppercased), now coloured by
          the shared severity tone. */}
      <MetaRow
        label="Status"
        value={<span style={{ color: statusColor(statusText) }}>{statusText}</span>}
      />
      {path ? (
        <MetaRow
          label="Path"
          value={<span style={{ opacity: 0.85, wordBreak: 'break-all' }}>{path}</span>}
        />
      ) : null}
      {tmdbId ? (
        <MetaRow
          label="TMDB"
          value={
            <a
              href={`https://www.themoviedb.org/movie/${tmdbId}`}
              target="_blank"
              rel="noreferrer"
              style={{ color: 'var(--ansi-12-blue)' }}
            >
              #{tmdbId} ↗
            </a>
          }
        />
      ) : null}
      <RowSpaceBetween style={{ gap: '2ch', alignItems: 'center' }}>
        <Text style={{ opacity: 0.6 }}>Monitored</Text>
        {/* Actionable toggle (#21): a SECONDARY SRCL Button that flips the
            flag and surfaces a toast. The label is the CURRENT state with an
            ASCII status glyph; clicking sets the opposite. Wrapped in an
            inline-block span so the SRCL Button sizes to its label instead of
            spanning the (now narrower) right column. */}
        <span style={{ display: 'inline-block', maxWidth: 'max-content' }}>
          <Button
            theme="SECONDARY"
            isDisabled={toggling}
            onClick={onToggleMonitored}
            aria-pressed={monitored}
          >
            {monitored ? '● Monitored' : '○ Not monitored'}
          </Button>
        </span>
      </RowSpaceBetween>
      <div style={{ marginTop: '1ch' }}>
        <Text style={{ opacity: 0.6 }}>Overview</Text>
        {overview ? (
          <Text>{overview}</Text>
        ) : (
          // A subtle placeholder rather than an empty gap, so an absent overview
          // doesn't leave the card looking broken.
          <Text style={{ opacity: 0.4, fontStyle: 'italic' }}>No overview available.</Text>
        )}
      </div>
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
  // The series' episode rows (GET /api/v3/episode?seriesId=…) that back the
  // per-season/episode monitor tree — the real `list_episodes` projection
  // carrying each episode's numeric id, monitored flag, on-disk state, air date.
  const [episodes, setEpisodes] = React.useState<Episode[]>([]);
  // In-flight guard for the monitored toggle (#21).
  const [toggling, setToggling] = React.useState(false);
  // In-flight guard for the series-type change (anime/daily/standard).
  const [changingType, setChangingType] = React.useState(false);
  // The tag catalogue (GET /api/v3/tag) + this node's current tag-id set, which
  // the tag editor renders as removable chips and PUTs back on change.
  const [tags, setTags] = React.useState<Tag[]>([]);
  const [contentTags, setContentTags_] = React.useState<number[]>([]);
  const [savingTags, setSavingTags] = React.useState(false);
  // Per-node monitored overrides for the season/episode toggles: a node id ->
  // its locally-applied monitored flag (wins over the loaded value until a
  // reload), plus the set of nodes whose toggle is mid-flight.
  const [monitorOverrides, setMonitorOverrides] = React.useState<Record<string, boolean>>({});
  const [togglingNodes, setTogglingNodes] = React.useState<Set<string>>(new Set());

  React.useEffect(() => {
    if (!id) return;
    const controller = new AbortController();
    setNode({ phase: 'loading' });
    setFiles({ phase: 'loading' });
    setSiblings([]);
    setCatalogueTitle(undefined);
    setDetail(undefined);
    setLibraryName(undefined);
    setEpisodes([]);
    setMonitorOverrides({});
    setTogglingNodes(new Set());
    setContentTags_([]);

    // Quality-profile catalogue — used to name the detail's qualityProfileId.
    api
      .getQualityProfiles(controller.signal)
      .then(setProfiles)
      .catch(() => setProfiles([]));

    // Tag catalogue — the choices the tag editor offers. Best-effort.
    api
      .listTags(controller.signal)
      .then(setTags)
      .catch(() => setTags([]));

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
          .then((d) => {
            const view = toDetailView(d);
            setDetail(view);
            if (view) setContentTags_(view.tags);
          })
          .catch(() => setDetail(undefined));

        // For a TV node, fetch its episode rows from the v3 episode endpoint
        // (the real `list_episodes`) to drive the per-season/episode monitor
        // tree. Best-effort: a non-series id / empty series yields [], which
        // renders no Monitoring card. The id sent is the same one the Library
        // screen drills in with (a full content id or numeric projection); the
        // endpoint accepts both as `seriesId`.
        if (detailKindFor(data as Loose) === 'series') {
          api
            .listEpisodes(id, controller.signal)
            .then(setEpisodes)
            .catch(() => setEpisodes([]));
        }

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
   * Change a series' `seriesType` (standard/daily/anime). PUTs ONLY `seriesType`
   * to the v3 series resource (so the monitored flag + tags are untouched) and
   * reflects the refreshed value back into the metadata block. A no-op when the
   * value is unchanged. Toast feedback on success and failure.
   */
  const changeSeriesType = (next: SeriesTypeValue) => {
    if (!id || !data || changingType) return;
    const current = (detail?.seriesType ?? 'standard').trim().toLowerCase();
    if (current === next) return;
    setChangingType(true);
    info(`Setting series type to ${seriesTypeLabel(next)}…`, { durationMs: 2000 });
    setSeriesType(id, next)
      .then((refreshed) => {
        const view = toDetailView(refreshed);
        if (view) setDetail(view);
        success(`Series type set to ${seriesTypeLabel(next)}`);
      })
      .catch((err: unknown) => toastError(errorMessage(err, 'failed to update series type')))
      .finally(() => setChangingType(false));
  };

  /**
   * Rewrite this node's tag set (#tags). PUTs the full id set to the v3 detail
   * resource (an empty array clears every tag) and reflects the refreshed value
   * back, with toast feedback. Optimistically applies the new set so the chips
   * update immediately; a failure reloads the server's truth.
   */
  const saveContentTags = (next: number[]) => {
    if (!id || !data || savingTags) return;
    const kind = detailKindFor(data as Loose);
    const prev = contentTags;
    setContentTags_(next);
    setSavingTags(true);
    setContentTags(kind, id, next)
      .then((refreshed) => {
        const view = toDetailView(refreshed);
        if (view) {
          setDetail(view);
          setContentTags_(view.tags);
        }
        success('Tags updated');
      })
      .catch((err: unknown) => {
        setContentTags_(prev);
        toastError(errorMessage(err, 'failed to update tags'));
      })
      .finally(() => setSavingTags(false));
  };

  /**
   * Mint a brand-new tag from a typed label (the tag editor's "+ new" path).
   * Resolves to the created tag so the editor can select it, and folds it into
   * the local catalogue so its chip renders with the real label.
   */
  const createContentTag = (label: string): Promise<Tag> =>
    api.createTag({ label }).then((tag) => {
      setTags((prev) => (prev.some((t) => t.id === tag.id) ? prev : [...prev, tag]));
      return tag;
    });

  /** Resolve a structural node's monitored flag (the inline glyph in the tree). */
  const nodeMonitored = React.useCallback(
    (n: ContentRef): boolean => {
      if (n.id in monitorOverrides) return monitorOverrides[n.id];
      return (n as Loose).monitored === true;
    },
    [monitorOverrides]
  );

  /**
   * Resolve an episode's monitored flag from the episode-endpoint rows: a local
   * override (applied optimistically on toggle) wins over the loaded value.
   */
  const episodeMonitored = React.useCallback(
    (id: string): boolean => {
      if (id in monitorOverrides) return monitorOverrides[id];
      const ep = episodes.find((e) => String(e.id) === id);
      return ep?.monitored === true;
    },
    [monitorOverrides, episodes]
  );

  const markToggling = (nodeId: string, on: boolean) =>
    setTogglingNodes((prev) => {
      const next = new Set(prev);
      if (on) next.add(nodeId);
      else next.delete(nodeId);
      return next;
    });

  /**
   * Toggle a whole season's monitoring. When the season's content-node id is
   * known (resolved from the structural sibling tree) the `/season/monitor` route
   * cascades to its episodes server-side; otherwise we fall back to a bulk
   * `/episode/monitor` over the season's episode ids. Either way every episode
   * override flips locally on success so the tree stays in sync without a reload.
   */
  const toggleSeasonMonitored = (group: SeasonGroup) => {
    const key = group.seasonId ?? `season:${group.seasonNumber}`;
    if (togglingNodes.has(key)) return;
    const seasonOn = group.episodes.length > 0 && group.episodes.every((e) => episodeMonitored(epId(e)));
    const next = !seasonOn;
    const epIds = group.episodes.map(epId);
    const seasonLabel = group.seasonNumber === 0 ? 'Specials' : `Season ${group.seasonNumber}`;
    markToggling(key, true);

    const apply = (count: number) => {
      setMonitorOverrides((prev) => {
        const merged = { ...prev };
        if (group.seasonId) merged[group.seasonId] = next;
        for (const id of epIds) merged[id] = next;
        return merged;
      });
      success(
        next
          ? `Monitoring ${seasonLabel} (${count} episode${count === 1 ? '' : 's'})`
          : `Stopped monitoring ${seasonLabel}`
      );
    };

    const req = group.seasonId
      ? setSeasonMonitored(group.seasonId, next).then((res) => res.episodesUpdated)
      : setEpisodesMonitored(epIds, next).then((res) => res.updated);

    req
      .then((count) => apply(count))
      .catch((err: unknown) => toastError(errorMessage(err, 'failed to update season monitoring')))
      .finally(() => markToggling(key, false));
  };

  /** Toggle a single episode's monitoring via the episode-monitor route. */
  const toggleEpisodeMonitored = (ep: Episode) => {
    const id = epId(ep);
    if (togglingNodes.has(id)) return;
    const next = !episodeMonitored(id);
    const label = ep.title?.length ? ep.title : episodeCoord(ep);
    markToggling(id, true);
    setEpisodesMonitored([id], next)
      .then(() => {
        setMonitorOverrides((prev) => ({ ...prev, [id]: next }));
        success(next ? `Monitoring ${label}` : `Stopped monitoring ${label}`);
      })
      .catch((err: unknown) => toastError(errorMessage(err, 'failed to update episode monitoring')))
      .finally(() => markToggling(id, false));
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

  // Season -> episodes grouping for the per-season/episode monitor tree, built
  // from the `GET /api/v3/episode` rows (the real `list_episodes`): group the
  // episode rows by `seasonNumber`, sort seasons then episodes in numbering
  // order, and resolve each season's content-node id from the structural sibling
  // tree (by matching `coords.season`) so the `/season/monitor` route has a
  // season id to toggle. Seasons with no resolvable node id still render and
  // toggle (the handler falls back to a bulk episode-monitor cascade).
  const seasons = React.useMemo<SeasonGroup[]>(() => {
    if (episodes.length === 0) return [];
    // Resolve a season's content-node id from the structural sibling list so the
    // `/season/monitor` route has a node id to toggle. The `/api/v1/libraries/
    // {id}/content` projection carries only `id`/`media_type`/`coords` (no `kind`
    // / `parent_id`), so identify a season node by its coordinate shape: same
    // `season` number with `episode` 0 / absent (an episode leaf carries a
    // non-zero `episode`). Fall back to the `kind` field when a richer projection
    // does carry it.
    const seasonNodeId = (n: number): string | undefined => {
      const match = siblings.find((s) => {
        const loose = s as Loose;
        const coords = loose.coords as Loose | undefined;
        if (coords?.season !== n) return false;
        if (loose.kind === 'season') return true;
        if (loose.kind === 'episode') return false;
        // No `kind`: a season node has no (or a zero) episode coordinate.
        const ep = coords?.episode;
        return ep === undefined || ep === null || ep === 0;
      });
      return match?.id;
    };
    const byNumber = new Map<number, Episode[]>();
    for (const ep of episodes) {
      const n = typeof ep.seasonNumber === 'number' ? ep.seasonNumber : 0;
      const bucket = byNumber.get(n) ?? [];
      bucket.push(ep);
      byNumber.set(n, bucket);
    }
    return [...byNumber.entries()]
      .sort(([a], [b]) => a - b)
      .map(([seasonNumber, eps]) => ({
        seasonNumber,
        seasonId: seasonNodeId(seasonNumber),
        episodes: eps
          .slice()
          .sort((a, b) => (a.episodeNumber ?? 0) - (b.episodeNumber ?? 0)),
      }));
  }, [episodes, siblings]);

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

  // Series-type display + whether this node is a series at all (the seriesType
  // card + absolute-number column only apply to a series). `seriesType` lives on
  // the v3 series detail resource; absent for a movie.
  const isSeries = loose ? detailKindFor(loose) === 'series' : false;
  const seriesType = (detail?.seriesType ?? 'standard').trim().toLowerCase();
  const isAnime = seriesType === 'anime';

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

            {/* Metadata block (#20): a two-column layout — the cached poster in
                a fixed ~30ch LEFT column, the rich detail + Monitored toggle +
                Overview in a RIGHT column that fills the rest and wraps below the
                poster on a narrow viewport (flex-wrap). */}
            <div style={{ display: 'flex', gap: '2ch', alignItems: 'flex-start', flexWrap: 'wrap' }}>
              {/* Left column: fixed-width poster column. */}
              <div style={{ flex: '0 0 30ch' }}>
                <Poster id={id} title={title} />
              </div>
              {/* Right column: metadata, the Monitored toggle, and the Overview.
                  Fills the space beside the poster and wraps below it when the
                  viewport is too narrow to fit both. */}
              <div style={{ flex: '1 1 32ch' }}>
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
            </div>

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

      {data && isSeries ? (
        <Card title="Series type" style={{ marginTop: '2ch' }}>
          <Text style={{ opacity: 0.6 }}>
            How cellarr numbers this series. <strong>Anime</strong> uses absolute
            numbering (with the live absolute → season/episode scene-remap) and the
            anime episode-file naming format; <strong>Daily</strong> keys episodes
            by air date; <strong>Standard</strong> uses native season/episode
            numbering.
          </Text>
          <Divider type="GRADIENT" />
          <RowSpaceBetween style={{ gap: '2ch', alignItems: 'center' }}>
            <Row style={{ gap: '1ch', alignItems: 'center' }}>
              <Text style={{ opacity: 0.6 }}>Type</Text>
              <Badge>{seriesTypeLabel(detail?.seriesType)}</Badge>
            </Row>
            <div style={{ minWidth: '20ch' }}>
              <Select
                key={`series-type-${seriesType}`}
                name="series-type"
                aria-label="Series type"
                options={SERIES_TYPE_OPTIONS.map((o) => o.label)}
                defaultValue={seriesTypeLabel(detail?.seriesType)}
                placeholder="Choose a series type"
                onChange={(label) => {
                  if (changingType) return;
                  const opt = SERIES_TYPE_OPTIONS.find((o) => o.label === label);
                  if (opt) changeSeriesType(opt.value);
                }}
              />
            </div>
          </RowSpaceBetween>
          <Text style={{ opacity: 0.5, marginTop: '0.5ch' }}>
            Fansub-group preferences (required / preferred / ignored terms, scoped by
            tag) are configured in Settings ▸ Release Profiles.
          </Text>
        </Card>
      ) : null}

      {/* The structure tree is only meaningful when there's a hierarchy to show
          (a series' seasons/episodes). For a movie it's a single node rendered as
          its raw "#id" — pure noise — so hide it when the root has no children. */}
      {data && root && (byParent.get(root.id)?.length ?? 0) > 0 ? (
        <Card title="Structure" style={{ marginTop: '2ch' }}>
          <ContentBranch
            node={root}
            byParent={byParent}
            activeId={data.id}
            isLastChild
            isMonitored={nodeMonitored}
          />
        </Card>
      ) : null}

      {data && seasons.length > 0 ? (
        <Card title="Monitoring" style={{ marginTop: '2ch' }}>
          <Text style={{ opacity: 0.6 }}>
            Toggle which seasons and episodes cellarr watches for. Toggling a season
            cascades to its episodes. ● monitored · ○ not · ✓ on disk · ✗ missing.
          </Text>
          <Divider type="GRADIENT" />
          <SeasonMonitoring
            seasons={seasons}
            isEpisodeMonitored={episodeMonitored}
            isSeasonMonitored={(group) =>
              group.episodes.length > 0 &&
              group.episodes.every((e) => episodeMonitored(epId(e)))
            }
            isToggling={(nodeId) => togglingNodes.has(nodeId)}
            onToggleSeason={toggleSeasonMonitored}
            onToggleEpisode={toggleEpisodeMonitored}
            showAbsolute={isAnime}
          />
        </Card>
      ) : null}

      {data ? (
        <Card title="Tags" style={{ marginTop: '2ch' }}>
          <Text style={{ opacity: 0.6 }}>
            Tag this item to route it through tag-scoped delay profiles, indexers,
            download clients, and notifications. Manage the tag list in Settings ▸ Tags.
          </Text>
          <Divider type="GRADIENT" />
          <TagInput
            available={tags}
            value={contentTags}
            onChange={saveContentTags}
            onCreate={createContentTag}
            label={`Tags for ${title}`}
            disabled={savingTags}
          />
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
