import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";

import { CellarrClient } from "@lib/api/client";
import IntegrationSection from "@app/settings/_components/IntegrationSection";

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

// The section loads, on mount, two side catalogues that must NOT consume the
// positional response queue these ordered tests rely on:
//   * the tag catalogue (GET /api/v3/tag) for its tag chip-input;
//   * the Radarr-shaped list (GET /api/v3/indexer | /downloadclient) used purely
//     to learn which configs the config-as-code reconciler owns (the `managed`
//     flag the native /api/v1 list does not carry).
// Intercept both and answer directly, delegating every other call (incl. the
// POST/DELETE/test that share those URLs) to the test's own mock. `managed` is
// the optional list of config-owned v3 configs the managed-badge derives from.
function isGet(opts?: RequestInit) {
  return !opts || opts.method === undefined || opts.method === "GET";
}

function withTags(
  inner: (url: string, opts?: RequestInit) => Promise<Response>,
  tags: unknown[] = [],
  managed: unknown[] = [],
) {
  return vi.fn().mockImplementation((url: string, opts?: RequestInit) => {
    const u = String(url);
    if (u.endsWith("/api/v3/tag") && isGet(opts)) {
      return Promise.resolve(jsonResponse(tags));
    }
    if (
      (u.endsWith("/api/v3/indexer") || u.endsWith("/api/v3/downloadclient")) &&
      isGet(opts)
    ) {
      return Promise.resolve(jsonResponse(managed));
    }
    return inner(url, opts);
  });
}

const INDEXERS = [
  { id: "prowl", name: "Prowlarr", implementation: "Prowlarr", enabled: true },
];

describe("IntegrationSection (indexers / clients)", () => {
  beforeEach(() => {
    window.matchMedia = vi.fn().mockReturnValue({
      matches: false,
      addEventListener: () => {},
      removeEventListener: () => {},
    }) as never;
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("lists existing configs", async () => {
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse(INDEXERS));
    const client = new CellarrClient({ fetchImpl: withTags(fetchImpl) });
    render(
      <IntegrationSection
        kind="indexers"
        title="Indexers"
        implementations={["Prowlarr"]}
        client={client}
      />,
    );
    await waitFor(() =>
      expect(screen.getAllByText("Prowlarr").length).toBeGreaterThan(0),
    );
  });

  it("labels a saved client by its real kind, not the first implementation", async () => {
    // The native /api/v1 list carries `kind` (e.g. "deluge"), not `implementation`.
    // The row must show "Deluge" — not fall back to implementations[0] ("qBittorrent").
    const clients = [
      { id: "dl1", name: "My Deluge", kind: "deluge", enabled: true },
    ];
    const fetchImpl = vi.fn().mockResolvedValue(jsonResponse(clients));
    const client = new CellarrClient({ fetchImpl: withTags(fetchImpl) });
    render(
      <IntegrationSection
        kind="downloadclients"
        title="Download Clients"
        implementations={["qBittorrent", "Deluge", "RTorrent"]}
        client={client}
      />,
    );
    // Pre-fix the saved row mislabeled to implementations[0] ("qBittorrent") and
    // "Deluge" appeared nowhere (the add-form dropdown is closed). Post-fix the row
    // shows its real kind, so "Deluge" is rendered.
    await waitFor(() =>
      expect(screen.getAllByText("Deluge").length).toBeGreaterThan(0),
    );
    expect(screen.getByText("My Deluge")).toBeTruthy();
  });

  it("shows a success indicator when the test passes", async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(INDEXERS)) // load
      .mockResolvedValueOnce(jsonResponse({ ok: true })); // test
    const client = new CellarrClient({ fetchImpl: withTags(fetchImpl) });
    render(
      <IntegrationSection
        kind="indexers"
        title="Indexers"
        implementations={["Prowlarr"]}
        client={client}
      />,
    );
    await waitFor(() =>
      expect(screen.getAllByText("Prowlarr").length).toBeGreaterThan(0),
    );

    fireEvent.change(screen.getByLabelText("Name"), {
      target: { value: "My indexer" },
    });
    fireEvent.click(screen.getByText("Test"));
    await waitFor(() =>
      expect(screen.getByText(/connection successful/i)).toBeTruthy(),
    );
    // Test goes through the working v3 route (the v1 /indexers/test route does
    // not exist on the daemon — it 404-falls-through to the SPA).
    const testCall = fetchImpl.mock.calls.find(([url]) =>
      String(url).endsWith("/api/v3/indexer/test"),
    );
    expect(testCall).toBeTruthy();
  });

  it("shows a failure indicator when the test fails", async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(INDEXERS))
      .mockResolvedValueOnce(
        jsonResponse({ code: "connection_refused", message: "host down" }, 502),
      );
    const client = new CellarrClient({ fetchImpl: withTags(fetchImpl) });
    render(
      <IntegrationSection
        kind="indexers"
        title="Indexers"
        implementations={["Prowlarr"]}
        client={client}
      />,
    );
    await waitFor(() =>
      expect(screen.getAllByText("Prowlarr").length).toBeGreaterThan(0),
    );

    fireEvent.change(screen.getByLabelText("Name"), {
      target: { value: "My indexer" },
    });
    fireEvent.click(screen.getByText("Test"));
    // The inline indicator carries the failure (the same outcome is also toasted,
    // but the toast lives in the ToastProvider which is not mounted in this unit).
    await waitFor(() => expect(screen.getByText(/host down/)).toBeTruthy());
  });

  it("confirms before deleting a config and then DELETEs it", async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(
        jsonResponse([
          {
            id: "7",
            name: "Prowlarr",
            implementation: "Prowlarr",
            enabled: true,
          },
        ]),
      )
      .mockResolvedValueOnce(new Response(null, { status: 204 })) // delete
      .mockResolvedValueOnce(jsonResponse([])); // reload
    const client = new CellarrClient({ fetchImpl: withTags(fetchImpl) });
    render(
      <IntegrationSection
        kind="indexers"
        title="Indexers"
        implementations={["Prowlarr"]}
        client={client}
      />,
    );
    await waitFor(() =>
      expect(screen.getByLabelText("Remove Prowlarr")).toBeTruthy(),
    );

    // Clicking Remove opens a confirm dialog — no DELETE yet.
    fireEvent.click(screen.getByLabelText("Remove Prowlarr"));
    expect(
      fetchImpl.mock.calls.find(([, opts]) => opts?.method === "DELETE"),
    ).toBeFalsy();
    await waitFor(() => expect(screen.getByRole("alertdialog")).toBeTruthy());

    // Confirm in the dialog fires the v3 numeric DELETE.
    fireEvent.click(screen.getByRole("button", { name: "Remove indexer" }));
    await waitFor(() => {
      const del = fetchImpl.mock.calls.find(
        ([url, opts]) =>
          String(url).endsWith("/api/v3/indexer/7") &&
          opts?.method === "DELETE",
      );
      expect(del).toBeTruthy();
    });
  });

  it("POSTs a new config on save", async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse([])) // load (empty)
      .mockResolvedValueOnce(jsonResponse({ id: "new" })) // save
      .mockResolvedValueOnce(jsonResponse([])); // reload
    const client = new CellarrClient({ fetchImpl: withTags(fetchImpl) });
    render(
      <IntegrationSection
        kind="downloadclients"
        title="Download Clients"
        implementations={["qBittorrent"]}
        client={client}
      />,
    );
    await waitFor(() =>
      expect(screen.getByText(/no download clients/i)).toBeTruthy(),
    );

    fireEvent.change(screen.getByLabelText("Name"), {
      target: { value: "qbit" },
    });
    fireEvent.change(screen.getByLabelText("Host"), {
      target: { value: "localhost" },
    });
    fireEvent.click(screen.getByText("Save"));

    await waitFor(() => {
      // Save goes through the working v3 create route with a Radarr-shaped body
      // (host lives inside fields[], not as a flat property). The v1
      // /downloadclients POST the screen used to send had no working test/shape.
      const postCall = fetchImpl.mock.calls.find(
        ([url, opts]) =>
          String(url).endsWith("/api/v3/downloadclient") &&
          opts?.method === "POST",
      );
      expect(postCall).toBeTruthy();
      const body = JSON.parse((postCall![1] as RequestInit).body as string);
      expect(body.name).toBe("qbit");
      const hostField = (
        body.fields as Array<{ name: string; value: unknown }>
      ).find((f) => f.name === "host");
      expect(hostField?.value).toBe("localhost");
    });
  });

  // Helper: drive a download-client save and return the parsed POST body.
  async function saveClientAndGetBody(
    impl: string,
    fill: (byLabel: (label: string) => HTMLElement) => void,
  ) {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse([])) // load (empty)
      .mockResolvedValueOnce(jsonResponse({ id: "new" })) // save
      .mockResolvedValueOnce(jsonResponse([])); // reload
    const client = new CellarrClient({ fetchImpl: withTags(fetchImpl) });
    render(
      <IntegrationSection
        kind="downloadclients"
        title="Download Clients"
        implementations={[impl]}
        client={client}
      />,
    );
    await waitFor(() =>
      expect(screen.getByText(/no download clients/i)).toBeTruthy(),
    );

    fill((label) => screen.getByLabelText(label));
    fireEvent.click(screen.getByText("Save"));

    let postCall: unknown[] | undefined;
    await waitFor(() => {
      postCall = fetchImpl.mock.calls.find(
        ([url, opts]) =>
          String(url).endsWith("/api/v3/downloadclient") &&
          opts?.method === "POST",
      );
      expect(postCall).toBeTruthy();
    });
    return JSON.parse((postCall![1] as RequestInit).body as string) as {
      name: string;
      implementation: string;
      fields: Array<{ name: string; value: unknown }>;
    };
  }

  it("builds a Deluge download-client body (host/port/password/urlBase/category)", async () => {
    const body = await saveClientAndGetBody("Deluge", (byLabel) => {
      fireEvent.change(byLabel("Name"), { target: { value: "deluge" } });
      fireEvent.change(byLabel("Host"), { target: { value: "deluge.local" } });
      fireEvent.change(byLabel("Port"), { target: { value: "8112" } });
      fireEvent.change(byLabel("URL base"), { target: { value: "/deluge" } });
      fireEvent.change(byLabel("Password"), { target: { value: "secret" } });
      fireEvent.change(byLabel("Category"), { target: { value: "cellarr" } });
    });
    expect(body.implementation).toBe("Deluge");
    const field = (name: string) =>
      body.fields.find((f) => f.name === name)?.value;
    expect(field("host")).toBe("deluge.local");
    expect(field("port")).toBe(8112);
    expect(field("urlBase")).toBe("/deluge");
    expect(field("password")).toBe("secret");
    expect(field("category")).toBe("cellarr");
    // Deluge's JSON-RPC login takes only a WebUI password — no username field.
    expect(body.fields.find((f) => f.name === "username")).toBeUndefined();
  });

  it("builds an rTorrent download-client body (username + urlBase + category)", async () => {
    const body = await saveClientAndGetBody("RTorrent", (byLabel) => {
      fireEvent.change(byLabel("Name"), { target: { value: "rtorrent" } });
      fireEvent.change(byLabel("Host"), { target: { value: "seedbox" } });
      fireEvent.change(byLabel("Port"), { target: { value: "443" } });
      fireEvent.change(byLabel("URL base"), { target: { value: "/RPC2" } });
      fireEvent.change(byLabel("Username"), { target: { value: "user" } });
      fireEvent.change(byLabel("Password"), { target: { value: "pw" } });
      fireEvent.change(byLabel("Category"), { target: { value: "tv" } });
    });
    expect(body.implementation).toBe("RTorrent");
    const field = (name: string) =>
      body.fields.find((f) => f.name === name)?.value;
    expect(field("host")).toBe("seedbox");
    expect(field("port")).toBe(443);
    expect(field("urlBase")).toBe("/RPC2");
    expect(field("username")).toBe("user");
    expect(field("password")).toBe("pw");
    expect(field("category")).toBe("tv");
  });

  it("posts the selected tag ids in the v3 body", async () => {
    const inner = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse([])) // load (empty)
      .mockResolvedValueOnce(jsonResponse({ id: "new" })) // save
      .mockResolvedValueOnce(jsonResponse([])); // reload
    // Seed the tag catalogue so a one-click suggestion chip is offered.
    const fetchImpl = withTags(inner, [
      { id: 1, label: "hd" },
      { id: 2, label: "kids" },
    ]);
    const client = new CellarrClient({ fetchImpl });
    render(
      <IntegrationSection
        kind="indexers"
        title="Indexers"
        implementations={["Torznab"]}
        client={client}
      />,
    );
    await waitFor(() => expect(screen.getByText(/no indexers/i)).toBeTruthy());

    fireEvent.change(screen.getByLabelText("Name"), {
      target: { value: "torznab" },
    });
    fireEvent.change(screen.getByLabelText("Base URL"), {
      target: { value: "http://localhost:9117" },
    });
    // Pick an existing tag from the suggestion chips.
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Add tag hd" })).toBeTruthy(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Add tag hd" }));
    fireEvent.click(screen.getByText("Save"));

    await waitFor(() => {
      const postCall = inner.mock.calls.find(
        ([url, opts]) =>
          String(url).endsWith("/api/v3/indexer") && opts?.method === "POST",
      );
      expect(postCall).toBeTruthy();
      const body = JSON.parse((postCall![1] as RequestInit).body as string) as {
        tags: number[];
      };
      expect(body.tags).toEqual([1]);
    });
  });

  it("posts the new indexer criteria fields (priority + seeders + seed criteria + freeleech)", async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse([])) // load (empty)
      .mockResolvedValueOnce(jsonResponse({ id: "new" })) // save
      .mockResolvedValueOnce(jsonResponse([])); // reload
    const client = new CellarrClient({ fetchImpl: withTags(fetchImpl) });
    render(
      <IntegrationSection
        kind="indexers"
        title="Indexers"
        implementations={["Torznab"]}
        client={client}
      />,
    );
    await waitFor(() => expect(screen.getByText(/no indexers/i)).toBeTruthy());

    fireEvent.change(screen.getByLabelText("Name"), {
      target: { value: "torznab" },
    });
    fireEvent.change(screen.getByLabelText("Base URL"), {
      target: { value: "http://localhost:9117" },
    });
    fireEvent.change(screen.getByLabelText("Priority"), {
      target: { value: "40" },
    });
    fireEvent.change(screen.getByLabelText("Minimum seeders"), {
      target: { value: "5" },
    });
    fireEvent.change(screen.getByLabelText("Seed ratio"), {
      target: { value: "1.5" },
    });
    fireEvent.change(screen.getByLabelText("Seed time"), {
      target: { value: "120" },
    });
    // The SRCL Checkbox keeps its caption in a sibling div (not an aria label),
    // so reach the input via its `name` and toggle it directly.
    const freeleech = document.querySelector(
      'input[name="indexers-freeleech"]',
    ) as HTMLInputElement;
    fireEvent.click(freeleech);
    fireEvent.click(screen.getByText("Save"));

    await waitFor(() => {
      const postCall = fetchImpl.mock.calls.find(
        ([url, opts]) =>
          String(url).endsWith("/api/v3/indexer") && opts?.method === "POST",
      );
      expect(postCall).toBeTruthy();
      const body = JSON.parse((postCall![1] as RequestInit).body as string) as {
        priority: number;
        fields: Array<{ name: string; value: unknown }>;
      };
      // Priority is lifted to the top-level v3 property.
      expect(body.priority).toBe(40);
      const field = (name: string) =>
        body.fields.find((f) => f.name === name)?.value;
      expect(field("baseUrl")).toBe("http://localhost:9117");
      expect(field("minimumSeeders")).toBe(5);
      expect(field("seedCriteria.seedRatio")).toBe(1.5);
      expect(field("seedCriteria.seedTime")).toBe(120);
      expect(field("requiredFlags")).toEqual(["freeleech"]);
    });
  });
});
