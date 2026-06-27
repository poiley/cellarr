'use client';

// Add / search-new screen (docs/10-ui.md §screen-mapping): find a title and add it.
// An Input drives a debounced lookup; results are split into MOVIES / TV sections,
// each ranked by relevance + popularity and capped (with a "show more" toggle) so
// the obvious hit lands first and the long tail stays out of the way. The +ADD
// ActionButton opens a Dialog (via SRCL's ModalStack/useModals) whose body lets the
// user pick LIBRARY, QUALITY PROFILE, ROOT FOLDER, MONITOR and SEARCH-ON-ADD before
// confirming. On success a toast with a "View" link points at the new item and the
// row keeps its ADDED state. Built only from vendored SRCL components + the API
// client + relative glue; empty/loading/error states are all handled and both SRCL
// themes work (all color comes from --theme-* tokens).

import * as React from 'react';

import Card from '@components/Card';
import Input from '@components/Input';
import Button from '@components/Button';
import ActionButton from '@components/ActionButton';
import Table from '@components/Table';
import TableRow from '@components/TableRow';
import TableColumn from '@components/TableColumn';
import Badge from '@components/Badge';
import Text from '@components/Text';
import Divider from '@components/Divider';
import AlertBanner from '@components/AlertBanner';
import BlockLoader from '@components/BlockLoader';
import RowSpaceBetween from '@components/RowSpaceBetween';
import Select from '@components/Select';
import Checkbox from '@components/Checkbox';
import Dialog from '@components/Dialog';
import ModalStack from '@components/ModalStack';
import { useModals } from '@components/page/ModalContext';

import { api, ApiError } from '@lib/api/client';
import type { Library, QualityProfile, RootFolder } from '@lib/api/types';

import AppShell from '@app/_components/AppShell';
import { useToast } from '@app/_lib/ToastProvider';

import {
  addContent,
  lookup,
  rankResults,
  MONITOR_OPTIONS,
  SERIES_TYPE_OPTIONS,
  type LookupResult,
  type MonitorOption,
  type SeriesType,
} from '../_search/api';

type Phase = 'idle' | 'loading' | 'ready' | 'error';

/** How many results to show per section before the "show more" toggle. */
const SECTION_PAGE = 8;

/** The add targets the dialog lets the user choose for a single add. */
interface AddSelection {
  rootFolderPath: string;
  qualityProfileId?: string;
  monitor: boolean;
  searchOnAdd: boolean;
  /**
   * For series adds: the per-episode monitoring policy (Sonarr `addOptions.monitor`).
   * Movies ignore it (they carry a single monitored flag). Defaults to `all`.
   */
  monitorOption: MonitorOption;
  /**
   * For series adds: the Sonarr `seriesType` (standard/daily/anime) — the
   * numbering model. Movies ignore it. Defaults to `standard`.
   */
  seriesType: SeriesType;
}

export default function Page() {
  const modals = useModals();
  const { success, error: toastError } = useToast();

  const [term, setTerm] = React.useState('');
  const [phase, setPhase] = React.useState<Phase>('idle');
  const [results, setResults] = React.useState<LookupResult[]>([]);
  const [error, setError] = React.useState<string>('');
  const [added, setAdded] = React.useState<Set<string>>(new Set());

  const abortRef = React.useRef<AbortController | null>(null);

  // Libraries + quality profiles + root folders feed the add dialog so the user
  // can target a real root/profile per add. Held in a ref so the (memoized) add
  // handlers always read the latest values without being recreated — avoiding a
  // stale closure when the data loads after first render.
  const targetsRef = React.useRef<{
    libraries: Library[];
    rootFolders: RootFolder[];
    profiles: QualityProfile[];
  }>({ libraries: [], rootFolders: [], profiles: [] });

  React.useEffect(() => {
    const controller = new AbortController();
    void (async () => {
      try {
        const [libs, roots, profiles] = await Promise.all([
          api.listLibraries(controller.signal),
          api.listRootFolders(controller.signal),
          api.getQualityProfiles(controller.signal),
        ]);
        if (controller.signal.aborted) return;
        targetsRef.current = {
          libraries: libs ?? [],
          rootFolders: roots ?? [],
          profiles: profiles ?? [],
        };
      } catch {
        // Non-fatal: the dialog falls back to sensible defaults if data is missing.
      }
    })();
    return () => controller.abort();
  }, []);

  const runSearch = React.useCallback(async (raw: string) => {
    const q = raw.trim();
    if (!q) {
      setPhase('idle');
      setResults([]);
      setError('');
      return;
    }

    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;

    setPhase('loading');
    setError('');
    try {
      const found = await lookup(q, controller.signal);
      if (controller.signal.aborted) return;
      setResults(found ?? []);
      setPhase('ready');
    } catch (err) {
      if (controller.signal.aborted) return;
      setError(err instanceof ApiError ? `${err.code}: ${err.message}` : 'Lookup failed.');
      setResults([]);
      setPhase('error');
    }
  }, []);

  // Debounced search as the user types.
  React.useEffect(() => {
    const handle = window.setTimeout(() => {
      void runSearch(term);
    }, 350);
    return () => window.clearTimeout(handle);
  }, [term, runSearch]);

  React.useEffect(() => () => abortRef.current?.abort(), []);

  const doAdd = React.useCallback(
    async (result: LookupResult, key: string, selection: AddSelection) => {
      try {
        const created = await addContent({
          media_type: result.media_type ?? 'movie',
          title: result.title,
          title_slug: result.title_slug,
          year: result.year,
          tmdb_id: result.tmdb_id,
          tvdb_id: result.tvdb_id,
          root_folder_path: selection.rootFolderPath,
          quality_profile_id: selection.qualityProfileId,
          monitored: selection.monitor,
          search_on_add: selection.searchOnAdd,
          monitor_option:
            (result.media_type ?? 'movie') === 'tv' ? selection.monitorOption : undefined,
          series_type:
            (result.media_type ?? 'movie') === 'tv' ? selection.seriesType : undefined,
        });
        setAdded((prev) => new Set(prev).add(key));
        success(
          <span>
            Added <strong>{result.title}</strong> —{' '}
            <a href={`/content?id=${encodeURIComponent(created.id)}`}>View ▸</a>
          </span>
        );
      } catch (err) {
        const msg = err instanceof ApiError ? `${err.code}: ${err.message}` : 'Add failed.';
        toastError(
          <span>
            Could not add <strong>{result.title}</strong> — {msg}
          </span>
        );
      }
    },
    [success, toastError]
  );

  const confirmAdd = React.useCallback(
    (result: LookupResult) => {
      const key = result.foreign_id;
      // The dialog body manages its own field state and writes the latest values
      // into this ref; the Dialog's OK button reads it on confirm. (SRCL's Dialog
      // only surfaces onConfirm/onCancel, so a ref is how we lift the selection.)
      const { libraries, rootFolders, profiles } = targetsRef.current;
      const selectionRef: { current: AddSelection } = {
        current: defaultSelection(result.media_type, libraries, rootFolders),
      };
      modals.open(Dialog, {
        title: `Add "${result.title}"${result.year ? ` (${result.year})` : ''}`,
        children: (
          <AddDialogBody
            result={result}
            libraries={libraries}
            rootFolders={rootFolders}
            profiles={profiles}
            selectionRef={selectionRef}
          />
        ),
        onConfirm: () => {
          modals.close();
          void doAdd(result, key, selectionRef.current);
        },
        onCancel: () => modals.close(),
      });
    },
    [modals, doAdd]
  );

  return (
    <AppShell>
      <ModalStack />
      <Card title="Add — search for a new title">
        <RowSpaceBetween>
          <div style={{ flex: 1, minWidth: '24ch' }}>
            <Input
              label="Search"
              name="add-search"
              placeholder="Type a movie or series…"
              autoComplete="off"
              value={term}
              onChange={(e: React.ChangeEvent<HTMLInputElement>) => setTerm(e.target.value)}
            />
          </div>
          <Button
            theme="SECONDARY"
            onClick={() => void runSearch(term)}
            isDisabled={!term.trim()}
          >
            Search
          </Button>
        </RowSpaceBetween>

        <Divider type="GRADIENT" />

        <Results
          phase={phase}
          term={term}
          results={results}
          error={error}
          added={added}
          onAdd={confirmAdd}
        />
      </Card>
    </AppShell>
  );
}

// ---------------------------------------------------------------------------
// Results — split into MOVIES / TV sections, ranked + capped per section.
// ---------------------------------------------------------------------------

const Results: React.FC<{
  phase: Phase;
  term: string;
  results: LookupResult[];
  error: string;
  added: Set<string>;
  onAdd: (r: LookupResult) => void;
}> = ({ phase, term, results, error, added, onAdd }) => {
  if (phase === 'idle') {
    return (
      <Text style={{ opacity: 0.6 }}>
        Start typing above to look up titles to add to your library.
      </Text>
    );
  }

  if (phase === 'loading') {
    return (
      <Text>
        <BlockLoader mode={1} /> Searching for “{term.trim()}”…
      </Text>
    );
  }

  if (phase === 'error') {
    return <AlertBanner>Search failed — {error}</AlertBanner>;
  }

  if (results.length === 0) {
    return (
      <Text style={{ opacity: 0.6 }}>
        No matches for “{term.trim()}”. Try a different spelling or a shorter query.
      </Text>
    );
  }

  const movies = rankResults(
    results.filter((r) => r.media_type !== 'tv'),
    term
  );
  const tv = rankResults(
    results.filter((r) => r.media_type === 'tv'),
    term
  );

  return (
    <>
      <ResultSection title="MOVIES" results={movies} added={added} onAdd={onAdd} />
      {movies.length > 0 && tv.length > 0 ? <Divider /> : null}
      <ResultSection title="TV" results={tv} added={added} onAdd={onAdd} />
    </>
  );
};

const ResultSection: React.FC<{
  title: string;
  results: LookupResult[];
  added: Set<string>;
  onAdd: (r: LookupResult) => void;
}> = ({ title, results, added, onAdd }) => {
  const [expanded, setExpanded] = React.useState(false);

  if (results.length === 0) return null;

  const shown = expanded ? results : results.slice(0, SECTION_PAGE);
  const hiddenCount = results.length - shown.length;

  return (
    <section style={{ marginBottom: '1ch' }}>
      <Text style={{ fontWeight: 600, opacity: 0.85 }}>
        <span aria-hidden="true">▸ </span>
        <span>{title}</span> <Badge>{results.length}</Badge>
      </Text>
      <Table>
        <TableRow>
          <TableColumn>Title</TableColumn>
          <TableColumn>Year</TableColumn>
          <TableColumn>Popularity</TableColumn>
          <TableColumn>Overview</TableColumn>
          <TableColumn>Add</TableColumn>
        </TableRow>
        {shown.map((r) => {
          const isAdded = added.has(r.foreign_id) || r.already_added;
          return (
            <TableRow key={r.foreign_id}>
              <TableColumn>{r.title}</TableColumn>
              <TableColumn>{r.year ?? '—'}</TableColumn>
              <TableColumn>
                <span style={{ opacity: 0.7 }}>{disambiguation(r)}</span>
              </TableColumn>
              <TableColumn>
                <span style={{ opacity: 0.7 }}>{truncate(r.overview)}</span>
              </TableColumn>
              <TableColumn>
                {isAdded ? (
                  <Badge>added</Badge>
                ) : (
                  <ActionButton hotkey="＋" onClick={() => onAdd(r)}>
                    Add
                  </ActionButton>
                )}
              </TableColumn>
            </TableRow>
          );
        })}
      </Table>
      {hiddenCount > 0 ? (
        <Button theme="SECONDARY" onClick={() => setExpanded(true)}>
          Show {hiddenCount} more ▾
        </Button>
      ) : null}
      {expanded && results.length > SECTION_PAGE ? (
        <Button theme="SECONDARY" onClick={() => setExpanded(false)}>
          Show fewer ▴
        </Button>
      ) : null}
    </section>
  );
};

// ---------------------------------------------------------------------------
// Add dialog body — Library / Quality profile / Root folder / Monitor / Search.
// ---------------------------------------------------------------------------

const AddDialogBody: React.FC<{
  result: LookupResult;
  libraries: Library[];
  rootFolders: RootFolder[];
  profiles: QualityProfile[];
  selectionRef: { current: AddSelection };
}> = ({ result, libraries, rootFolders, profiles, selectionRef }) => {
  // The library matching this title's media type is the natural default target;
  // it seeds the root folder + quality profile choices.
  const defaultLib = React.useMemo(
    () => libraries.find((l) => l.media_type === result.media_type) ?? libraries[0],
    [libraries, result.media_type]
  );

  // Root-folder options: every configured library root + every standalone root
  // folder, de-duplicated. Falls back to a sensible placeholder if none exist.
  const rootOptions = React.useMemo(() => {
    const paths = new Set<string>();
    for (const l of libraries) for (const p of l.root_folders ?? []) paths.add(p);
    for (const rf of rootFolders) if (rf.path) paths.add(rf.path);
    return Array.from(paths);
  }, [libraries, rootFolders]);

  const profileOptions = React.useMemo(() => profiles.map((p) => p.name), [profiles]);
  const profileByName = React.useMemo(() => {
    const m = new Map<string, string>();
    for (const p of profiles) m.set(p.name, p.id);
    return m;
  }, [profiles]);

  const defaultRoot =
    defaultLib?.root_folders?.[0] ?? rootOptions[0] ?? rootFolders[0]?.path ?? '';
  const defaultProfileId = defaultLib?.default_quality_profile;
  const defaultProfileName =
    profiles.find((p) => p.id === defaultProfileId)?.name ?? profileOptions[0] ?? '';

  // Seed the shared selection ref with the defaults so a straight OK (no edits)
  // still adds with a real target.
  React.useEffect(() => {
    selectionRef.current = {
      rootFolderPath: defaultRoot,
      qualityProfileId: profileByName.get(defaultProfileName) ?? defaultProfileId,
      monitor: selectionRef.current.monitor,
      searchOnAdd: selectionRef.current.searchOnAdd,
      monitorOption: selectionRef.current.monitorOption,
      seriesType: selectionRef.current.seriesType,
    };
    // Run once on mount with the resolved defaults.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '1ch', minWidth: '40ch' }}>
      <Text>
        Add <strong>{result.title}</strong>
        {result.year ? ` (${result.year})` : ''} to your library.
      </Text>

      {libraries.length > 1 ? (
        <Field label="Library">
          <Select
            name="add-library"
            options={libraries.map((l) => l.name)}
            defaultValue={defaultLib?.name ?? ''}
            placeholder="Choose a library"
            onChange={(name) => {
              const lib = libraries.find((l) => l.name === name);
              if (!lib) return;
              const root = lib.root_folders?.[0] ?? selectionRef.current.rootFolderPath;
              const profId = lib.default_quality_profile ?? selectionRef.current.qualityProfileId;
              selectionRef.current = {
                ...selectionRef.current,
                rootFolderPath: root,
                qualityProfileId: profId,
              };
            }}
          />
        </Field>
      ) : null}

      <Field label="Quality profile">
        {profileOptions.length > 0 ? (
          <Select
            name="add-profile"
            options={profileOptions}
            defaultValue={defaultProfileName}
            placeholder="Choose a quality profile"
            onChange={(name) => {
              selectionRef.current = {
                ...selectionRef.current,
                qualityProfileId: profileByName.get(name) ?? selectionRef.current.qualityProfileId,
              };
            }}
          />
        ) : (
          <Text style={{ opacity: 0.6 }}>No quality profiles configured — using library default.</Text>
        )}
      </Field>

      <Field label="Root folder">
        {rootOptions.length > 0 ? (
          <Select
            name="add-root"
            options={rootOptions}
            defaultValue={defaultRoot}
            placeholder="Choose a root folder"
            onChange={(path) => {
              selectionRef.current = { ...selectionRef.current, rootFolderPath: path };
            }}
          />
        ) : (
          <Text style={{ opacity: 0.6 }}>No root folders configured.</Text>
        )}
      </Field>

      {result.media_type === 'tv' ? (
        // Series type (standard/daily/anime): the numbering model. Anime turns on
        // absolute-numbering + scene-remap and the anime episode-file naming
        // format. Sent as `seriesType` on the create body.
        <Field label="Series type">
          <Select
            name="add-series-type"
            aria-label="Series type"
            options={SERIES_TYPE_OPTIONS.map((o) => o.label)}
            defaultValue={
              SERIES_TYPE_OPTIONS.find((o) => o.value === selectionRef.current.seriesType)?.label ??
              SERIES_TYPE_OPTIONS[0].label
            }
            placeholder="Choose a series type"
            onChange={(label) => {
              const opt = SERIES_TYPE_OPTIONS.find((o) => o.label === label);
              if (!opt) return;
              selectionRef.current = { ...selectionRef.current, seriesType: opt.value };
            }}
          />
          <Text style={{ opacity: 0.55, marginTop: '0.5ch' }}>
            Anime uses absolute numbering and the anime episode-file naming format.
            Fansub-group preferences (required / preferred / ignored terms) live in
            Settings ▸ Release Profiles.
          </Text>
        </Field>
      ) : null}

      {result.media_type === 'tv' ? (
        // Series: the per-episode monitoring policy (Sonarr addOptions.monitor).
        // The dropdown drives both the policy AND the root monitored flag (`none`
        // adds the series unmonitored), so the bare Monitor checkbox is hidden.
        <Field label="Monitor">
          <Select
            name="add-monitor-option"
            options={MONITOR_OPTIONS.map((o) => o.label)}
            defaultValue={
              MONITOR_OPTIONS.find((o) => o.value === selectionRef.current.monitorOption)?.label ??
              MONITOR_OPTIONS[0].label
            }
            placeholder="Choose what to monitor"
            onChange={(label) => {
              const opt = MONITOR_OPTIONS.find((o) => o.label === label);
              if (!opt) return;
              selectionRef.current = {
                ...selectionRef.current,
                monitorOption: opt.value,
                monitor: opt.value !== 'none',
              };
            }}
          />
        </Field>
      ) : (
        <Checkbox
          name="add-monitor"
          defaultChecked={selectionRef.current.monitor}
          onChange={(e) => {
            selectionRef.current = { ...selectionRef.current, monitor: e.target.checked };
          }}
        >
          Monitor
        </Checkbox>
      )}

      <Checkbox
        name="add-search-on-add"
        defaultChecked={selectionRef.current.searchOnAdd}
        onChange={(e) => {
          selectionRef.current = { ...selectionRef.current, searchOnAdd: e.target.checked };
        }}
      >
        Search for it on add
      </Checkbox>
    </div>
  );
};

const Field: React.FC<{ label: string; children: React.ReactNode }> = ({ label, children }) => (
  <div>
    <Text style={{ opacity: 0.7, marginBottom: '0.5ch' }}>{label}</Text>
    {children}
  </div>
);

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/** The selection a dialog starts from before the user edits anything. */
function defaultSelection(
  mediaType: string | undefined,
  libraries: Library[],
  rootFolders: RootFolder[]
): AddSelection {
  const lib = libraries.find((l) => l.media_type === mediaType) ?? libraries[0];
  const rootFolderPath =
    lib?.root_folders?.[0] ?? rootFolders[0]?.path ?? '';
  return {
    rootFolderPath,
    qualityProfileId: lib?.default_quality_profile,
    monitor: true,
    searchOnAdd: true,
    monitorOption: 'all',
    seriesType: 'standard',
  };
}

/**
 * A compact disambiguation aid for a result row — popularity / rating / runtime,
 * whichever the metadata source provided, so identically-named titles can be told
 * apart at a glance. Falls back to an em dash when nothing is known.
 */
function disambiguation(r: LookupResult): string {
  const bits: string[] = [];
  if (r.popularity !== undefined) bits.push(`pop ${Math.round(r.popularity)}`);
  if (r.vote_average !== undefined) bits.push(`★ ${r.vote_average.toFixed(1)}`);
  if (r.runtime !== undefined && r.runtime > 0) bits.push(`${r.runtime}m`);
  return bits.length > 0 ? bits.join(' · ') : '—';
}

function truncate(text?: string, max = 80): string {
  if (!text) return '—';
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}
