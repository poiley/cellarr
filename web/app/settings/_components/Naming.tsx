'use client';

// Settings — Media Management / Naming. Three sections, SRCL-only:
//
//   NAMING       editable naming-format inputs (movie file / series folder /
//                season folder / episode file), a click-to-insert TOKEN
//                REFERENCE sourced from GET /api/v3/config/naming/tokens, and a
//                LIVE PREVIEW that re-renders via POST /api/v3/config/naming/preview
//                as each format changes (debounced). Save → PUT /config/naming.
//   PERMISSIONS  chmod folder/file (octal text) + chown (user:group), applied
//                AFTER the media commit. Save → PUT /config/mediamanagement.
//   EXTRA FILES  an "Import extra files" toggle + an editable extensions list
//                (srt, nfo, …). Save → PUT /config/mediamanagement.
//
// Composed entirely from vendored SRCL primitives (Card, Input, Button,
// ButtonGroup, Badge, Divider, Text) plus the shared useToast() / useAsync()
// glue. Permissions + extra-files persist into the MediaManagement settings
// blob and apply only after a successful media commit — never rolling the
// imported media back on failure (that contract lives in the import path).

import * as React from 'react';

import Card from '@components/Card';
import Input from '@components/Input';
import Button from '@components/Button';
import ButtonGroup from '@components/ButtonGroup';
import Badge from '@components/Badge';
import Divider from '@components/Divider';
import Text from '@components/Text';

import { CellarrClient, api as defaultApi } from '@lib/api/client';
import type {
  MediaManagement,
  NamingConfig,
  NamingTarget,
  NamingTokens,
} from '@lib/api/types';

import { useToast } from '@app/_lib/ToastProvider';
import { useAsync, toApiError } from '@app/settings/_components/useAsync';
import { Loading, ErrorBanner } from '@app/settings/_components/StatusBanners';

// The four naming targets, in display order, mapped to their config field +
// label. seasonFolder may legitimately be empty (a flat, season-less layout).
const TARGETS: {
  target: NamingTarget;
  field: keyof Pick<
    NamingConfig,
    'movieFileFormat' | 'seriesFolderFormat' | 'seasonFolderFormat' | 'episodeFileFormat'
  >;
  label: string;
  allowEmpty: boolean;
}[] = [
  { target: 'movieFile', field: 'movieFileFormat', label: 'Movie file', allowEmpty: false },
  { target: 'seriesFolder', field: 'seriesFolderFormat', label: 'Series folder', allowEmpty: false },
  { target: 'seasonFolder', field: 'seasonFolderFormat', label: 'Season folder (empty = flat)', allowEmpty: true },
  { target: 'episodeFile', field: 'episodeFileFormat', label: 'Episode file', allowEmpty: false },
];

type Formats = Record<NamingTarget, string>;

function formatsFrom(cfg: NamingConfig): Formats {
  return {
    movieFile: cfg.movieFileFormat ?? '',
    seriesFolder: cfg.seriesFolderFormat ?? '',
    seasonFolder: cfg.seasonFolderFormat ?? '',
    episodeFile: cfg.episodeFileFormat ?? '',
  };
}

/** One naming-format row: label, input, token palette, and a live preview line. */
const FormatRow: React.FC<{
  client: CellarrClient;
  target: NamingTarget;
  label: string;
  allowEmpty: boolean;
  value: string;
  tokens: NamingTokens['targets'][number]['tokens'];
  onChange: (next: string) => void;
}> = ({ client, target, label, allowEmpty, value, tokens, onChange }) => {
  const [preview, setPreview] = React.useState<string>('');
  const [previewError, setPreviewError] = React.useState<string | null>(null);

  // Re-render the preview as the format changes, debounced so we don't POST on
  // every keystroke. An empty allow-empty format renders as a flat "(none)".
  React.useEffect(() => {
    if (allowEmpty && value.trim() === '') {
      setPreview('(none — flat layout)');
      setPreviewError(null);
      return;
    }
    const controller = new AbortController();
    const handle = setTimeout(() => {
      client
        .previewNaming({ format: value, target }, controller.signal)
        .then((res) => {
          setPreview(res.rendered);
          setPreviewError(null);
        })
        .catch((err) => {
          const e = toApiError(err);
          if (e.code === 'network_error' && controller.signal.aborted) return;
          setPreview('');
          setPreviewError(e.message);
        });
    }, 250);
    return () => {
      clearTimeout(handle);
      controller.abort();
    };
  }, [client, target, value, allowEmpty]);

  return (
    <div style={{ margin: '1ch 0' }}>
      <Text style={{ opacity: 0.6 }}>{label}</Text>
      <Input
        name={`naming-${target}`}
        aria-label={`${label} format`}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />

      <div style={{ display: 'flex', flexWrap: 'wrap', gap: '0.5ch', margin: '0.5ch 0' }}>
        {tokens.map((t) => (
          <Button
            key={t.token}
            theme="SECONDARY"
            aria-label={`Insert ${t.name} token into ${label} format`}
            title={`${t.label} — e.g. ${t.example}${t.required ? ' (required)' : ''}`}
            onClick={() => onChange(`${value}${t.token}`)}
          >
            {t.token}
            {t.required ? ' *' : ''}
          </Button>
        ))}
      </div>

      <Text style={{ opacity: 0.55 }} aria-label={`${label} preview`}>
        {previewError ? (
          <span style={{ color: 'var(--ansi-9-red)' }}>✗ {previewError}</span>
        ) : (
          <>
            <Badge>preview</Badge> <code>{preview}</code>
          </>
        )}
      </Text>
    </div>
  );
};

const Naming: React.FC<{ client?: CellarrClient }> = ({ client = defaultApi }) => {
  const { success, error: toastError } = useToast();

  const loadNaming = React.useCallback(
    (signal: AbortSignal) => client.getNamingConfig(signal),
    [client]
  );
  const loadTokens = React.useCallback(
    (signal: AbortSignal) => client.getNamingTokens(signal),
    [client]
  );
  const loadMm = React.useCallback(
    (signal: AbortSignal) => client.getMediaManagement(signal),
    [client]
  );

  const naming = useAsync<NamingConfig>(loadNaming);
  const tokens = useAsync<NamingTokens>(loadTokens);
  const mm = useAsync<MediaManagement>(loadMm);

  // Local form state hydrated once the loaders resolve.
  const [formats, setFormats] = React.useState<Formats | null>(null);
  const [savingNaming, setSavingNaming] = React.useState(false);

  const [chmodFolder, setChmodFolder] = React.useState('');
  const [chmodFile, setChmodFile] = React.useState('');
  const [chown, setChown] = React.useState('');
  const [savingPerms, setSavingPerms] = React.useState(false);

  const [extraEnabled, setExtraEnabled] = React.useState(false);
  const [extensions, setExtensions] = React.useState<string[]>([]);
  const [newExt, setNewExt] = React.useState('');
  const [savingExtra, setSavingExtra] = React.useState(false);

  React.useEffect(() => {
    if (naming.data && !formats) setFormats(formatsFrom(naming.data));
  }, [naming.data, formats]);

  React.useEffect(() => {
    if (!mm.data) return;
    setChmodFolder(mm.data.permissions?.chmodFolder ?? '');
    setChmodFile(mm.data.permissions?.chmodFile ?? '');
    setChown(mm.data.permissions?.chown ?? '');
    setExtraEnabled(mm.data.extraFiles?.enabled ?? false);
    setExtensions(mm.data.extraFiles?.extensions ?? []);
  }, [mm.data]);

  if (naming.loading || tokens.loading || mm.loading) return <Loading label="Loading naming config" />;
  if (naming.error) return <ErrorBanner error={naming.error} />;
  if (tokens.error) return <ErrorBanner error={tokens.error} />;
  if (mm.error) return <ErrorBanner error={mm.error} />;
  if (!formats || !tokens.data) return <Loading label="Loading naming config" />;

  const tokensFor = (target: NamingTarget) =>
    tokens.data?.targets.find((t) => t.target === target)?.tokens ?? [];

  const setFormat = (target: NamingTarget, next: string) =>
    setFormats((prev) => (prev ? { ...prev, [target]: next } : prev));

  const saveNaming = async () => {
    if (!formats) return;
    setSavingNaming(true);
    try {
      const updated = await client.updateNamingConfig({
        movieFileFormat: formats.movieFile,
        seriesFolderFormat: formats.seriesFolder,
        seasonFolderFormat: formats.seasonFolder,
        episodeFileFormat: formats.episodeFile,
      });
      setFormats(formatsFrom(updated));
      success('Naming formats saved.');
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not save naming — ${e.message}`);
    } finally {
      setSavingNaming(false);
    }
  };

  const savePermissions = async () => {
    setSavingPerms(true);
    try {
      await client.updateMediaManagement({
        permissions: {
          chmodFolder: chmodFolder.trim() || undefined,
          chmodFile: chmodFile.trim() || undefined,
          chown: chown.trim() || undefined,
        },
      });
      success('Permissions saved.');
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not save permissions — ${e.message}`);
    } finally {
      setSavingPerms(false);
    }
  };

  const saveExtraFiles = async () => {
    setSavingExtra(true);
    try {
      await client.updateMediaManagement({
        extraFiles: { enabled: extraEnabled, extensions },
      });
      success('Extra-files config saved.');
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not save extra files — ${e.message}`);
    } finally {
      setSavingExtra(false);
    }
  };

  const addExtension = () => {
    const ext = newExt.trim().replace(/^\./, '').toLowerCase();
    if (!ext) return;
    if (extensions.includes(ext)) {
      toastError(`Extension "${ext}" is already in the list.`);
      setNewExt('');
      return;
    }
    setExtensions((prev) => [...prev, ext]);
    setNewExt('');
  };

  const removeExtension = (ext: string) =>
    setExtensions((prev) => prev.filter((e) => e !== ext));

  return (
    <div>
      <Card title="Naming">
        <Text style={{ opacity: 0.6 }}>
          How imported movie and episode files are renamed and laid out on disk. Click a token to
          insert it; the preview below each field re-renders as you type.
        </Text>

        <Divider type="GRADIENT" />

        {TARGETS.map((t) => (
          <FormatRow
            key={t.target}
            client={client}
            target={t.target}
            label={t.label}
            allowEmpty={t.allowEmpty}
            value={formats[t.target]}
            tokens={tokensFor(t.target)}
            onChange={(next) => setFormat(t.target, next)}
          />
        ))}

        <div style={{ marginTop: '1ch' }}>
          <ButtonGroup
            items={[
              {
                body: savingNaming ? 'Saving…' : 'Save naming',
                onClick: savingNaming ? undefined : saveNaming,
              },
            ]}
          />
        </div>
      </Card>

      <div style={{ marginTop: '1ch' }}>
        <Card title="Permissions">
          <Text style={{ opacity: 0.6 }}>
            File ownership and modes applied <em>after</em> a media file is committed. A failure here
            is logged and skipped — it never rolls back or corrupts the imported media. Unix only.
          </Text>

          <Divider type="GRADIENT" />

          <div style={{ display: 'flex', gap: '1ch', flexWrap: 'wrap' }}>
            <div style={{ flex: 1, minWidth: '14ch' }}>
              <Text style={{ opacity: 0.6 }}>chmod folder (octal)</Text>
              <Input
                name="perm-chmod-folder"
                aria-label="chmod folder"
                placeholder="755"
                value={chmodFolder}
                onChange={(e) => setChmodFolder(e.target.value)}
              />
            </div>
            <div style={{ flex: 1, minWidth: '14ch' }}>
              <Text style={{ opacity: 0.6 }}>chmod file (octal)</Text>
              <Input
                name="perm-chmod-file"
                aria-label="chmod file"
                placeholder="644"
                value={chmodFile}
                onChange={(e) => setChmodFile(e.target.value)}
              />
            </div>
            <div style={{ flex: 1, minWidth: '14ch' }}>
              <Text style={{ opacity: 0.6 }}>chown (user:group)</Text>
              <Input
                name="perm-chown"
                aria-label="chown"
                placeholder="media:media"
                value={chown}
                onChange={(e) => setChown(e.target.value)}
              />
            </div>
          </div>

          <div style={{ marginTop: '1ch' }}>
            <ButtonGroup
              items={[
                {
                  body: savingPerms ? 'Saving…' : 'Save permissions',
                  onClick: savingPerms ? undefined : savePermissions,
                },
              ]}
            />
          </div>
        </Card>
      </div>

      <div style={{ marginTop: '1ch' }}>
        <Card title="Extra Files">
          <Text style={{ opacity: 0.6 }}>
            Import sidecar files (subtitles, metadata) found alongside the media. Like permissions,
            these are handled after the media commit and never put the import at risk.
          </Text>

          <Divider type="GRADIENT" />

          <div style={{ display: 'flex', alignItems: 'center', gap: '1ch', margin: '0.5ch 0' }}>
            <Text style={{ opacity: 0.6 }}>Import extra files</Text>
            <Button
              theme={extraEnabled ? 'PRIMARY' : 'SECONDARY'}
              role="switch"
              aria-checked={extraEnabled}
              aria-label="Import extra files"
              onClick={() => setExtraEnabled((v) => !v)}
            >
              {extraEnabled ? '● on' : '○ off'}
            </Button>
          </div>

          <Text style={{ opacity: 0.6, marginTop: '0.5ch' }}>Extensions</Text>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: '0.5ch', margin: '0.5ch 0' }}>
            {extensions.length ? (
              extensions.map((ext) => (
                <Button
                  key={ext}
                  theme="SECONDARY"
                  aria-label={`Remove extension ${ext}`}
                  onClick={() => removeExtension(ext)}
                >
                  {ext} ✕
                </Button>
              ))
            ) : (
              <Text style={{ opacity: 0.5 }}>No extensions yet.</Text>
            )}
          </div>

          <div style={{ display: 'flex', gap: '1ch', alignItems: 'flex-end' }}>
            <div style={{ flex: 1 }}>
              <Input
                name="extra-files-new"
                aria-label="New extension"
                placeholder="srt"
                value={newExt}
                onChange={(e) => setNewExt(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    e.preventDefault();
                    addExtension();
                  }
                }}
              />
            </div>
            <ButtonGroup items={[{ body: 'Add', onClick: addExtension }]} />
          </div>

          <div style={{ marginTop: '1ch' }}>
            <ButtonGroup
              items={[
                {
                  body: savingExtra ? 'Saving…' : 'Save extra files',
                  onClick: savingExtra ? undefined : saveExtraFiles,
                },
              ]}
            />
          </div>
        </Card>
      </div>
    </div>
  );
};

export default Naming;
