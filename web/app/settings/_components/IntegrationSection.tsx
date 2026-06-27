"use client";

// Settings — Indexers / Download Clients. Both are integration configs with the
// same shape (host/port/api-key/ssl/enabled, a Test button that surfaces an
// AlertBanner result, and Save). SRCL-only: Card, Input, Select, Checkbox,
// AlertBanner, Button.
//
// Reads GET /api/v1/<kind> (the native snake_case list). Test + Save go through
// the Radarr/Sonarr-compatible /api/v3 shim, because the native /api/v1 surface
// has NO create-test routes and no customformat/test/indexer-test endpoint:
//   * /api/v1/indexers/test, /api/v1/downloadclients/test DO NOT EXIST (they
//     404-fall-through to the SPA index.html, which silently "succeeds");
//   * the working routes are POST /api/v3/{indexer,downloadclient}/test and
//     POST /api/v3/{indexer,downloadclient} (crates/cellarr-api/src/shim.rs).
// The v3 handlers expect a Radarr-shaped body (configContract + protocol +
// fields[]), so the flat form is mapped to that shape via `toV3Body` below
// (mirroring app/first-run WizardModal). Verified against the seeded daemon:
// test returns {isValid:true}, create returns the persisted resource (200).

import * as React from "react";

import Card from "@components/Card";
import Input from "@components/Input";
import Select from "@components/Select";
import Checkbox from "@components/Checkbox";
import Button from "@components/Button";
import ButtonGroup from "@components/ButtonGroup";
import Badge from "@components/Badge";
import Divider from "@components/Divider";
import Text from "@components/Text";

import { ApiError, CellarrClient, api as defaultApi } from "@lib/api/client";
import type {
  IndexerConfig,
  DownloadClientConfig,
  IndexerConfigV3,
  DownloadClientConfigV3,
  Tag,
} from "@lib/api/types";

import { useToast } from "@app/_lib/ToastProvider";
import { useAsync, toApiError } from "@app/settings/_components/useAsync";
import {
  Loading,
  ErrorBanner,
  EmptyState,
} from "@app/settings/_components/StatusBanners";
import ConfirmDialog from "@app/settings/_components/ConfirmDialog";
import ManagedBadge from "@app/settings/_components/ManagedBadge";
import TagInput from "@app/settings/_components/TagInput";

type IntegrationKind = "indexers" | "downloadclients";
type RawConfig = IndexerConfig | DownloadClientConfig;

interface IntegrationForm {
  id: string;
  name: string;
  implementation: string;
  host: string;
  port: string;
  apiKey: string;
  ssl: boolean;
  enabled: boolean;
  // Download-client extras (Transmission / Deluge / rTorrent take credential +
  // path fields beyond host/port; the *arr apps hard-deref `category`).
  username: string;
  password: string;
  urlBase: string;
  category: string;
  // Indexer release-selection criteria (mirrors the v3 shim's typed
  // minimumSeeders / seedCriteria.* / requiredFlags fields).
  priority: string;
  minimumSeeders: string;
  seedRatio: string;
  seedTime: string;
  /** The freeleech-only policy is `requiredFlags: ["freeleech"]`. */
  requireFreeleech: boolean;
  /** Tag ids that scope this integration (empty = applies to all content). */
  tags: number[];
}

// Which download-client implementations take credential / urlBase fields. A
// blackhole client is just a watch dir (host = path), so it shows none of these.
const CLIENT_FIELDS: Record<
  string,
  {
    host?: boolean;
    port?: boolean;
    username?: boolean;
    password?: boolean;
    urlBase?: boolean;
  }
> = {
  transmission: {
    host: true,
    port: true,
    username: true,
    password: true,
    urlBase: true,
  },
  deluge: { host: true, port: true, password: true, urlBase: true },
  rtorrent: {
    host: true,
    port: true,
    username: true,
    password: true,
    urlBase: true,
  },
  qbittorrent: { host: true, port: true, username: true, password: true },
  sabnzbd: { host: true, port: true, urlBase: true },
  nzbget: { host: true, port: true, username: true, password: true },
};

function clientFields(impl: string) {
  return (
    CLIENT_FIELDS[impl.toLowerCase()] ?? {
      host: true,
      port: true,
      username: true,
      password: true,
    }
  );
}

interface TestResult {
  ok: boolean;
  message: string;
}

// The native /api/v1 list carries the adapter as `kind` ("deluge", "qbittorrent",
// "torznab", …), not the camel/Pascal `implementation` the form models. Match the
// kind against the schema's implementation list (case/separator-insensitive) so a
// saved client/indexer shows its real type instead of falling back to the first
// implementation in the list (which mislabeled e.g. Deluge as "qBittorrent").
function normImpl(s: unknown): string {
  return String(s ?? "")
    .toLowerCase()
    .replace(/[^a-z0-9]/g, "");
}
function implFromKind(
  kind: unknown,
  implementations: string[],
): string | undefined {
  const k = normImpl(kind);
  if (!k) return undefined;
  return implementations.find((impl) => normImpl(impl) === k);
}

function toForm(raw: RawConfig, implementations: string[]): IntegrationForm {
  const rec = raw as Record<string, unknown>;
  // Native list configs carry the typed extras either at the top level
  // (`category`, `priority`) or inside the `settings` / `criteria` blobs.
  const settings = (rec.settings as Record<string, unknown> | undefined) ?? {};
  const criteria = (rec.criteria as Record<string, unknown> | undefined) ?? {};
  const seedCriteria =
    (criteria.seedCriteria as Record<string, unknown> | undefined) ?? {};
  const flags = criteria.requiredFlags;
  const flagList = Array.isArray(flags)
    ? flags.map((f) => String(f).toLowerCase())
    : typeof flags === "string"
      ? flags.split(",").map((f) => f.trim().toLowerCase())
      : [];
  const str = (...vals: unknown[]) => {
    const v = vals.find((x) => x != null && x !== "");
    return v != null ? String(v) : "";
  };
  return {
    id: String(rec.id ?? rec.name ?? ""),
    name: String(rec.name ?? ""),
    implementation: String(
      rec.implementation ??
        implFromKind(rec.kind, implementations) ??
        implementations[0] ??
        "",
    ),
    host: str(rec.host, settings.host, settings.base_url),
    port: str(rec.port, settings.port),
    apiKey: str(rec.api_key, rec.apiKey, settings.apiKey),
    ssl: rec.ssl === true || rec.use_ssl === true,
    enabled: rec.enabled !== false,
    username: str(rec.username, settings.username),
    password: str(rec.password, settings.password),
    urlBase: str(rec.urlBase, settings.urlBase),
    category: str(rec.category),
    priority: str(rec.priority, criteria.priority),
    minimumSeeders: str(criteria.minimumSeeders),
    seedRatio: str(seedCriteria.seedRatio),
    seedTime: str(seedCriteria.seedTime),
    requireFreeleech: flagList.includes("freeleech"),
    tags: Array.isArray(rec.tags)
      ? rec.tags.filter((t): t is number => typeof t === "number")
      : [],
  };
}

function blankForm(implementations: string[]): IntegrationForm {
  return {
    id: "",
    name: "",
    implementation: implementations[0] ?? "",
    host: "",
    port: "",
    apiKey: "",
    ssl: false,
    enabled: true,
    username: "",
    password: "",
    urlBase: "",
    category: "",
    priority: "",
    minimumSeeders: "",
    seedRatio: "",
    seedTime: "",
    requireFreeleech: false,
    tags: [],
  };
}

export interface IntegrationSectionProps {
  kind: IntegrationKind;
  title: string;
  implementations: string[];
  client?: CellarrClient;
}

const IntegrationSection: React.FC<IntegrationSectionProps> = ({
  kind,
  title,
  implementations,
  client = defaultApi,
}) => {
  const load = React.useCallback(
    (signal: AbortSignal) =>
      kind === "indexers"
        ? client.listIndexers(signal)
        : client.listDownloadClients(signal),
    [client, kind],
  );
  const loadTags = React.useCallback(
    (signal: AbortSignal) => client.listTags(signal),
    [client],
  );
  // The native /api/v1 list the form is built from does NOT carry the additive
  // `managed` flag — only the Radarr-shaped /api/v3 list does. Fetch that in
  // parallel purely to learn which configs the config-as-code reconciler owns,
  // matched back to the native rows by name (the human identity shown per row).
  // Best-effort: a failure here just means nothing is badged as managed.
  const loadManaged = React.useCallback(
    (signal: AbortSignal) =>
      kind === "indexers"
        ? client.listIndexersV3(signal)
        : client.listDownloadClientsV3(signal),
    [client, kind],
  );
  const { data, loading, error, reload } = useAsync<RawConfig[]>(load);
  const { data: managedList } = useAsync<
    (IndexerConfigV3 | DownloadClientConfigV3)[]
  >(loadManaged);
  const { data: tagList, reload: reloadTags } = useAsync<Tag[]>(loadTags);
  const { success, error: toastError, info } = useToast();

  const tags = tagList ?? [];

  // The set of config-owned names (a config-managed indexer / download client is
  // locked read-only in the UI). Empty when the v3 list is unavailable.
  const managedNames = React.useMemo(() => {
    const set = new Set<string>();
    for (const m of managedList ?? []) {
      if (m.managed === true && typeof m.name === "string") set.add(m.name);
    }
    return set;
  }, [managedList]);

  // Mint a new tag inline (the TagInput "+ new" path) + refresh the catalogue.
  const createTag = React.useCallback(
    async (label: string): Promise<Tag> => {
      const tag = await client.createTag({ label });
      reloadTags();
      return tag;
    },
    [client, reloadTags],
  );

  const singular = title.replace(/s$/, "");

  const [form, setForm] = React.useState<IntegrationForm>(() =>
    blankForm(implementations),
  );
  const [testing, setTesting] = React.useState(false);
  const [testResult, setTestResult] = React.useState<TestResult | undefined>(
    undefined,
  );
  const [saving, setSaving] = React.useState(false);
  const [saveError, setSaveError] = React.useState<ApiError | undefined>(
    undefined,
  );
  const [pendingDelete, setPendingDelete] =
    React.useState<IntegrationForm | null>(null);
  const [deleting, setDeleting] = React.useState(false);

  const configs = data ?? [];

  const edit = (raw: RawConfig) => {
    setForm(toForm(raw, implementations));
    setTestResult(undefined);
    setSaveError(undefined);
  };

  const reset = () => {
    setForm(blankForm(implementations));
    setTestResult(undefined);
    setSaveError(undefined);
  };

  // Torrent vs usenet drives the protocol/configContract the v3 shim validates
  // against (mirrors app/first-run WizardModal's mapping).
  const protocolFor = (impl: string): "usenet" | "torrent" =>
    /newznab|usenet|sab|nzb/i.test(impl) ? "usenet" : "torrent";

  // Map the flat form to the Radarr/Sonarr-shaped body the /api/v3 test + create
  // handlers expect: configContract + protocol + a fields[] array. An indexer's
  // endpoint lives under `baseUrl`; a download client's under `host`/`port`.
  const toV3Body = (): Partial<IndexerConfigV3> &
    Partial<DownloadClientConfigV3> => {
    const port = form.port ? Number.parseInt(form.port, 10) : undefined;
    const num = (v: string) => {
      const trimmed = v.trim();
      if (!trimmed) return undefined;
      const n = Number(trimmed);
      return Number.isFinite(n) ? n : undefined;
    };
    const priority = num(form.priority);
    const minSeeders = num(form.minimumSeeders);
    const seedRatio = num(form.seedRatio);
    const seedTime = num(form.seedTime);

    const fields =
      kind === "indexers"
        ? [
            { name: "baseUrl", value: form.host },
            ...(form.apiKey ? [{ name: "apiKey", value: form.apiKey }] : []),
            // Typed release-selection criteria the v3 shim lifts into
            // IndexerConfig.criteria. Only emit a field when the user set it.
            ...(minSeeders !== undefined
              ? [{ name: "minimumSeeders", value: minSeeders }]
              : []),
            ...(seedRatio !== undefined
              ? [{ name: "seedCriteria.seedRatio", value: seedRatio }]
              : []),
            ...(seedTime !== undefined
              ? [{ name: "seedCriteria.seedTime", value: seedTime }]
              : []),
            ...(form.requireFreeleech
              ? [{ name: "requiredFlags", value: ["freeleech"] }]
              : []),
          ]
        : (() => {
            const spec = clientFields(form.implementation);
            return [
              ...(spec.host ? [{ name: "host", value: form.host }] : []),
              ...(spec.port && port !== undefined
                ? [{ name: "port", value: port }]
                : []),
              ...(spec.urlBase && form.urlBase
                ? [{ name: "urlBase", value: form.urlBase }]
                : []),
              ...(spec.username && form.username
                ? [{ name: "username", value: form.username }]
                : []),
              ...(spec.password && form.password
                ? [{ name: "password", value: form.password }]
                : []),
              { name: "useSsl", value: form.ssl },
              // The category the *arr ecosystem hard-derefs; always present so a
              // downstream consumer never null-derefs it.
              { name: "category", value: form.category },
            ];
          })();
    return {
      name: form.name,
      implementation: form.implementation,
      configContract: `${form.implementation}Settings`,
      protocol: protocolFor(form.implementation),
      ...(priority !== undefined ? { priority } : {}),
      ...(kind === "indexers"
        ? {
            enableRss: form.enabled,
            enableAutomaticSearch: form.enabled,
            enableInteractiveSearch: form.enabled,
          }
        : { enable: form.enabled }),
      fields,
      tags: form.tags,
    } as Partial<IndexerConfigV3> & Partial<DownloadClientConfigV3>;
  };

  const test = async () => {
    if (!form.name.trim()) {
      toastError("Give it a name before testing.");
      return;
    }
    setTesting(true);
    setTestResult(undefined);
    info("Testing connection…", { durationMs: 2000 });
    try {
      const body = toV3Body();
      if (kind === "indexers") await client.testIndexer(body);
      else await client.testDownloadClient(body);
      setTestResult({ ok: true, message: "Connection successful." });
      success("Connection successful.");
    } catch (err) {
      const e = toApiError(err);
      setTestResult({ ok: false, message: `${e.code}: ${e.message}` });
      toastError(`Test failed — ${e.message}`);
    } finally {
      setTesting(false);
    }
  };

  const save = async () => {
    if (!form.name.trim()) {
      toastError("Give it a name before saving.");
      return;
    }
    setSaving(true);
    setSaveError(undefined);
    try {
      const body = toV3Body();
      if (kind === "indexers") await client.createIndexer(body);
      else await client.createDownloadClient(body);
      success(`${singular} saved.`);
      reset();
      reload();
    } catch (err) {
      const e = toApiError(err);
      setSaveError(e);
      toastError(`Could not save — ${e.message}`);
    } finally {
      setSaving(false);
    }
  };

  const confirmDelete = async () => {
    if (!pendingDelete) return;
    const numericId = Number.parseInt(pendingDelete.id, 10);
    if (!Number.isFinite(numericId)) {
      // The native list keys configs by an opaque id; the v3 delete route is
      // addressed by a numeric id. Surface this rather than firing a request
      // that cannot resolve.
      toastError("This entry cannot be deleted from here (no numeric id).");
      setPendingDelete(null);
      return;
    }
    setDeleting(true);
    try {
      if (kind === "indexers") await client.deleteIndexer(numericId);
      else await client.deleteDownloadClient(numericId);
      success(`${singular} removed.`);
      if (form.id === pendingDelete.id) reset();
      setPendingDelete(null);
      reload();
    } catch (err) {
      const e = toApiError(err);
      toastError(`Could not remove — ${e.message}`);
    } finally {
      setDeleting(false);
    }
  };

  return (
    <Card title={title}>
      {loading ? (
        <Loading label={`Loading ${title.toLowerCase()}`} />
      ) : error ? (
        <ErrorBanner error={error} />
      ) : (
        <>
          {configs.length ? (
            <ul style={{ listStyle: "none", padding: 0, margin: "0 0 1ch 0" }}>
              {configs.map((raw) => {
                const f = toForm(raw, implementations);
                const managed = !!f.name && managedNames.has(f.name);
                return (
                  <li
                    key={f.id || f.name}
                    style={{
                      display: "flex",
                      alignItems: "center",
                      justifyContent: "space-between",
                      gap: "1ch",
                      padding: "0.5ch 0",
                    }}
                  >
                    <span>
                      <Badge>{f.enabled ? "enabled" : "disabled"}</Badge>{" "}
                      {f.name || "(unnamed)"}{" "}
                      <span style={{ opacity: 0.5 }}>{f.implementation}</span>{" "}
                      {managed ? (
                        <ManagedBadge entityLabel={`${singular} ${f.name || "(unnamed)"}`} />
                      ) : null}
                    </span>
                    <span style={{ display: "inline-flex", gap: "0.5ch" }}>
                      <Button
                        theme="SECONDARY"
                        aria-label={`Edit ${f.name || singular}`}
                        isDisabled={managed}
                        onClick={managed ? undefined : () => edit(raw)}
                      >
                        Edit
                      </Button>
                      <Button
                        theme="DANGER"
                        aria-label={`Remove ${f.name || singular}`}
                        isDisabled={managed}
                        onClick={managed ? undefined : () => setPendingDelete(f)}
                      >
                        Remove
                      </Button>
                    </span>
                  </li>
                );
              })}
            </ul>
          ) : (
            <EmptyState>No {title.toLowerCase()} configured yet.</EmptyState>
          )}

          <Divider type="GRADIENT" />

          <Text style={{ opacity: 0.6, margin: "1ch 0 0.5ch" }}>
            {form.id
              ? `Editing ${form.name || form.id}`
              : `New ${singular.toLowerCase()}`}
          </Text>

          <div style={{ margin: "0.5ch 0" }}>
            <Text style={{ opacity: 0.6 }}>Name</Text>
            <Input
              name={`${kind}-name`}
              aria-label="Name"
              value={form.name}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
            />
          </div>

          <div style={{ margin: "0.5ch 0" }}>
            <Text style={{ opacity: 0.6 }}>Implementation</Text>
            <Select
              name={`${kind}-impl`}
              options={implementations}
              defaultValue={form.implementation}
              onChange={(value) => setForm({ ...form, implementation: value })}
            />
          </div>

          {(() => {
            const spec =
              kind === "downloadclients"
                ? clientFields(form.implementation)
                : undefined;
            const showHost = kind === "indexers" || !spec || spec.host;
            const showPort = kind === "downloadclients" && (!spec || spec.port);
            return (
              <>
                {showHost ? (
                  <div style={{ display: "flex", gap: "1ch" }}>
                    <div style={{ flex: 2, margin: "0.5ch 0" }}>
                      <Text style={{ opacity: 0.6 }}>
                        {kind === "indexers" ? "Base URL" : "Host"}
                      </Text>
                      <Input
                        name={`${kind}-host`}
                        aria-label={kind === "indexers" ? "Base URL" : "Host"}
                        placeholder={
                          kind === "indexers"
                            ? "http://localhost:9117"
                            : "localhost"
                        }
                        value={form.host}
                        onChange={(e) =>
                          setForm({ ...form, host: e.target.value })
                        }
                      />
                    </div>
                    {showPort ? (
                      <div style={{ flex: 1, margin: "0.5ch 0" }}>
                        <Text style={{ opacity: 0.6 }}>Port</Text>
                        <Input
                          name={`${kind}-port`}
                          aria-label="Port"
                          type="number"
                          placeholder="9117"
                          value={form.port}
                          onChange={(e) =>
                            setForm({ ...form, port: e.target.value })
                          }
                        />
                      </div>
                    ) : null}
                  </div>
                ) : null}
              </>
            );
          })()}

          {kind === "indexers" ? (
            <div style={{ margin: "0.5ch 0" }}>
              <Text style={{ opacity: 0.6 }}>API key</Text>
              <Input
                name={`${kind}-apikey`}
                aria-label="API key"
                type="password"
                value={form.apiKey}
                onChange={(e) => setForm({ ...form, apiKey: e.target.value })}
              />
            </div>
          ) : null}

          {/* Download-client credential / path / category fields. Which appear is
              driven by the implementation (a Deluge WebUI takes only a password;
              rTorrent / Transmission take Basic creds + a urlBase mount path). */}
          {kind === "downloadclients"
            ? (() => {
                const spec = clientFields(form.implementation);
                return (
                  <>
                    {spec.urlBase ? (
                      <div style={{ margin: "0.5ch 0" }}>
                        <Text style={{ opacity: 0.6 }}>URL base</Text>
                        <Input
                          name={`${kind}-urlbase`}
                          aria-label="URL base"
                          placeholder="/transmission"
                          value={form.urlBase}
                          onChange={(e) =>
                            setForm({ ...form, urlBase: e.target.value })
                          }
                        />
                      </div>
                    ) : null}
                    {spec.username ? (
                      <div style={{ margin: "0.5ch 0" }}>
                        <Text style={{ opacity: 0.6 }}>Username</Text>
                        <Input
                          name={`${kind}-username`}
                          aria-label="Username"
                          value={form.username}
                          onChange={(e) =>
                            setForm({ ...form, username: e.target.value })
                          }
                        />
                      </div>
                    ) : null}
                    {spec.password ? (
                      <div style={{ margin: "0.5ch 0" }}>
                        <Text style={{ opacity: 0.6 }}>Password</Text>
                        <Input
                          name={`${kind}-password`}
                          aria-label="Password"
                          type="password"
                          value={form.password}
                          onChange={(e) =>
                            setForm({ ...form, password: e.target.value })
                          }
                        />
                      </div>
                    ) : null}
                    <div style={{ margin: "0.5ch 0" }}>
                      <Text style={{ opacity: 0.6 }}>Category</Text>
                      <Input
                        name={`${kind}-category`}
                        aria-label="Category"
                        placeholder="cellarr"
                        value={form.category}
                        onChange={(e) =>
                          setForm({ ...form, category: e.target.value })
                        }
                      />
                    </div>
                  </>
                );
              })()
            : null}

          {/* Indexer release-selection criteria. */}
          {kind === "indexers" ? (
            <>
              <div style={{ display: "flex", gap: "1ch" }}>
                <div style={{ flex: 1, margin: "0.5ch 0" }}>
                  <Text style={{ opacity: 0.6 }}>Priority</Text>
                  <Input
                    name={`${kind}-priority`}
                    aria-label="Priority"
                    type="number"
                    placeholder="25"
                    value={form.priority}
                    onChange={(e) =>
                      setForm({ ...form, priority: e.target.value })
                    }
                  />
                </div>
                <div style={{ flex: 1, margin: "0.5ch 0" }}>
                  <Text style={{ opacity: 0.6 }}>Minimum seeders</Text>
                  <Input
                    name={`${kind}-minseeders`}
                    aria-label="Minimum seeders"
                    type="number"
                    placeholder="1"
                    value={form.minimumSeeders}
                    onChange={(e) =>
                      setForm({ ...form, minimumSeeders: e.target.value })
                    }
                  />
                </div>
              </div>
              <div style={{ display: "flex", gap: "1ch" }}>
                <div style={{ flex: 1, margin: "0.5ch 0" }}>
                  <Text style={{ opacity: 0.6 }}>Seed ratio</Text>
                  <Input
                    name={`${kind}-seedratio`}
                    aria-label="Seed ratio"
                    type="number"
                    placeholder="1.0"
                    value={form.seedRatio}
                    onChange={(e) =>
                      setForm({ ...form, seedRatio: e.target.value })
                    }
                  />
                </div>
                <div style={{ flex: 1, margin: "0.5ch 0" }}>
                  <Text style={{ opacity: 0.6 }}>Seed time (minutes)</Text>
                  <Input
                    name={`${kind}-seedtime`}
                    aria-label="Seed time"
                    type="number"
                    placeholder="60"
                    value={form.seedTime}
                    onChange={(e) =>
                      setForm({ ...form, seedTime: e.target.value })
                    }
                  />
                </div>
              </div>
            </>
          ) : null}

          <div style={{ display: "flex", gap: "2ch", margin: "0.5ch 0" }}>
            <Checkbox
              name={`${kind}-ssl`}
              defaultChecked={form.ssl}
              onChange={(e) => setForm({ ...form, ssl: e.target.checked })}
            >
              Use SSL
            </Checkbox>
            <Checkbox
              name={`${kind}-enabled`}
              defaultChecked={form.enabled}
              onChange={(e) => setForm({ ...form, enabled: e.target.checked })}
            >
              Enabled
            </Checkbox>
            {kind === "indexers" ? (
              <Checkbox
                name={`${kind}-freeleech`}
                defaultChecked={form.requireFreeleech}
                onChange={(e) =>
                  setForm({ ...form, requireFreeleech: e.target.checked })
                }
              >
                Require freeleech
              </Checkbox>
            ) : null}
          </div>

          <Text style={{ opacity: 0.6, margin: "1ch 0 0.5ch" }}>Tags</Text>
          <TagInput
            available={tags}
            value={form.tags}
            onChange={(next) => setForm({ ...form, tags: next })}
            onCreate={createTag}
            label={`${singular} tags`}
          />

          {/* Inline test result stays near the form as a persistent indicator;
              the same outcome is also announced via toast. */}
          {testResult ? (
            <div role="status" style={{ margin: "0.5ch 0" }}>
              <Badge>{testResult.ok ? "✓ ok" : "✗ failed"}</Badge>{" "}
              {testResult.message}
            </div>
          ) : null}
          {saveError ? <ErrorBanner error={saveError} /> : null}

          <div style={{ marginTop: "1ch" }}>
            <ButtonGroup
              items={[
                {
                  body: testing ? "Testing…" : "Test",
                  onClick: testing ? undefined : test,
                },
                {
                  body: saving ? "Saving…" : "Save",
                  onClick: saving ? undefined : save,
                },
                ...(form.id ? [{ body: "New", onClick: reset }] : []),
              ]}
            />
          </div>

          {pendingDelete ? (
            <ConfirmDialog
              title={`Remove ${singular.toLowerCase()}`}
              confirmLabel={`Remove ${singular.toLowerCase()}`}
              pendingLabel="Removing…"
              pending={deleting}
              onConfirm={confirmDelete}
              onCancel={() => (deleting ? undefined : setPendingDelete(null))}
            >
              <Text>
                Remove{" "}
                <strong>{pendingDelete.name || singular.toLowerCase()}</strong>?
                cellarr will stop using this {singular.toLowerCase()}.
              </Text>
            </ConfirmDialog>
          ) : null}
        </>
      )}
    </Card>
  );
};

export default IntegrationSection;
