#!/usr/bin/env python3
"""A scrape-style tracker that misbehaves on demand.

`torznab_mock.py` next door speaks Torznab and behaves. This one serves an HTML
site a Cardigann definition scrapes, and its whole purpose is to behave *badly*,
on request, in the specific ways real trackers do:

  --rate-limit N          429 once N requests have been served, like a tracker
                          whose limiter bites part-way through a search.
  --challenge             an interstitial until a clearance cookie comes back,
                          so "solve once, then reuse" can be told apart from
                          "re-solve every request".
  --sign-downloads        the magnet is not in the listing. It is fetched by
                          POSTing the row id with sha256(id|timestamp|pageToken),
                          where pageToken only exists on the details page.
  --categories named      /sub/movies/... instead of /sub/54/..., the mirror
                          variant whose category mappings silently resolve to
                          nothing and make category-filtered searches empty.
  --fail-when SUBSTR      502 for any query string containing SUBSTR — a site
                          that is up but dies on one parameter combination.
  --latency MS            slow every response, for pacing and timeout budgets.
  --die-after N           serve N requests, then fail permanently.

Every request line is appended to the log file so a harness can assert on what
was actually asked for, not merely on what came back. Pure stdlib, no deps.

Run standalone:

    python3 mock_tracker.py --port 8080 --log /tmp/reqs.log --sign-downloads

and point a Cardigann definition (see mock_tracker.yml) at http://127.0.0.1:8080.
"""

import argparse
import hashlib
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

# The token the signed-download flow mixes into its digest. Served in the details
# page exactly the way a real tracker embeds it in an inline <script>.
PAGE_TOKEN = "0123456789abcdef0123456789abcdef"
CSRF_TOKEN = "fedcba9876543210fedcba9876543210"

ROWS = [
    # (id, title, size cell, seeders, category-slug)
    (11, "Example Show S01E01 1080p WEB-DL x264-GROUP", "2.1 GB", 42, "tv"),
    (22, "Example Show S01E02 1080p WEB-DL x264-GROUP", "2.2 GB", 7, "tv"),
    (33, "Example Movie 2024 1080p BluRay x264-GROUP", "8.4 GB", 130, "movies"),
]

# Numeric ids are what the canonical sites use; the named variant is the mirror
# shape that quietly breaks category mapping.
NUMERIC_CAT = {"tv": "5", "movies": "54"}


class State:
    """Mutable server state — what makes this a tracker and not a fixture."""

    def __init__(self, args):
        self.args = args
        self.served = 0
        self.cleared = set()  # cookies that have passed the challenge
        self.lock = threading.Lock()

    def next_count(self):
        with self.lock:
            self.served += 1
            return self.served


def _rows_html(state, query):
    hidden = '<span class="hide-on-mob">Size</span> '
    seeds_label = '<span class="hide-on-mob">Seeds</span> '
    out = ['<html><body><table class="torrents"><tbody>']
    for rid, title, size, seeders, slug in ROWS:
        if query and query.lower() not in title.lower():
            continue
        if state.args.categories == "named":
            cat_href = f"/sub/{slug}/Dubs-Dual Audio/1/"
        else:
            cat_href = f"/sub/{NUMERIC_CAT[slug]}/1/"
        # With --sign-downloads there is deliberately no magnet in the row: only
        # a button carrying the id, exactly like the trackers that broke this.
        dl = (
            f'<a class="dl" href="javascript:void(0);" data-id="{rid}">magnet</a>'
            if state.args.sign_downloads
            else f'<a class="dl" href="magnet:?xt=urn:btih:{rid:040d}">magnet</a>'
        )
        out.append(
            f'<tr><td class="name">'
            f'<a class="cat" href="{cat_href}">cat</a>'
            f'<a class="title" href="/t/{rid}/">{title}</a>{dl}</td>'
            f'<td class="size">{hidden}{size}</td>'
            f'<td class="seeders">{seeds_label}{seeders}</td></tr>'
        )
    out.append("</tbody></table></body></html>")
    return "".join(out)


def _details_html(rid):
    # The tokens the signed download needs, inline the way a real page carries
    # them — a server-side caller can read them without executing JavaScript.
    return (
        "<html><head><script>"
        f"window.pageToken = '{PAGE_TOKEN}';"
        f"window.csrfToken = '{CSRF_TOKEN}';"
        "</script></head><body>"
        f'<a class="dl" href="javascript:void(0);" data-id="{rid}">Get magnet</a>'
        "</body></html>"
    )


CHALLENGE_HTML = (
    "<html><head><title>Just a moment...</title></head>"
    "<body>Checking your browser before accessing the site.</body></html>"
)


class Handler(BaseHTTPRequestHandler):
    state: State = None  # set on the server instance

    # --- plumbing ---------------------------------------------------------
    def log_message(self, fmt, *args):  # noqa: A003 - stdlib hook
        pass

    def _record(self, method):
        line = f"{method} {self.path}"
        if self.state.args.log:
            with open(self.state.args.log, "a", encoding="utf-8") as fh:
                fh.write(line + "\n")

    def _send(self, code, body, ctype="text/html; charset=utf-8", cookie=None):
        raw = body.encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(raw)))
        if cookie:
            self.send_header("Set-Cookie", f"clearance={cookie}; Path=/")
        self.end_headers()
        self.wfile.write(raw)

    def _faults(self):
        """Return a (code, body) to serve instead of content, or None."""
        a = self.state.args
        n = self.state.next_count()

        if a.latency:
            time.sleep(a.latency / 1000.0)
        if a.die_after and n > a.die_after:
            return 500, "error code: 1006"
        if a.rate_limit and n > a.rate_limit:
            return 429, "<error code=\"429\" description=\"Too many requests\" />"
        if a.fail_when and a.fail_when in self.path:
            # A site that is up but whose origin dies for this one combination.
            return 502, "<html><title>502: origin timed out</title></html>"
        if a.challenge:
            cookie = self.headers.get("Cookie", "")
            if "clearance=" not in cookie:
                # Hand out clearance WITH the interstitial: a caller that keeps
                # its cookies sails past next time, one that rebuilds its session
                # every request never does.
                self._send(503, CHALLENGE_HTML, cookie="ok")
                return "sent", None
        return None

    # --- routes -----------------------------------------------------------
    def do_GET(self):  # noqa: N802 - stdlib hook
        self._record("GET")
        fault = self._faults()
        if fault == ("sent", None):
            return
        if fault:
            return self._send(fault[0], fault[1])

        parsed = urlparse(self.path)
        if parsed.path.startswith("/t/"):
            rid = parsed.path.strip("/").split("/")[1]
            return self._send(200, _details_html(rid))
        if parsed.path.rstrip("/") in ("/browse", ""):
            q = (parse_qs(parsed.query).get("q") or [""])[0]
            return self._send(200, _rows_html(self.state, q))
        return self._send(404, "<html>not found</html>")

    def do_POST(self):  # noqa: N802 - stdlib hook
        self._record("POST")
        fault = self._faults()
        if fault == ("sent", None):
            return
        if fault:
            return self._send(fault[0], fault[1])

        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length).decode("utf-8") if length else ""
        form = parse_qs(body)
        if urlparse(self.path).path != "/ajax/getMagnet.php":
            return self._send(404, "<html>not found</html>")

        rid = (form.get("torrent_id") or [""])[0]
        ts = (form.get("timestamp") or [""])[0]
        sig = (form.get("hmac") or [""])[0]
        sess = (form.get("sessid") or [""])[0]

        expected = hashlib.sha256(f"{rid}|{ts}|{PAGE_TOKEN}".encode()).hexdigest()
        if sess != CSRF_TOKEN or sig != expected:
            # What a caller that guessed, reused another page's token, or skipped
            # the details fetch gets.
            return self._send(
                200,
                json.dumps({"success": False, "error": "Invalid request"}),
                ctype="application/json",
            )
        magnet = f"magnet:?xt=urn:btih:{int(rid):040d}&dn=example"
        return self._send(
            200,
            json.dumps({"success": True, "magnet": magnet}),
            ctype="application/json",
        )


def build_parser():
    p = argparse.ArgumentParser(description="A tracker that misbehaves on demand.")
    p.add_argument("--port", type=int, default=0)
    p.add_argument("--log", help="append every request line here")
    p.add_argument("--rate-limit", type=int, default=0, help="429 after N requests")
    p.add_argument("--challenge", action="store_true", help="interstitial until cleared")
    p.add_argument("--sign-downloads", action="store_true", help="magnet only via signed POST")
    p.add_argument("--categories", choices=["numeric", "named"], default="numeric")
    p.add_argument("--fail-when", help="502 for any path containing this")
    p.add_argument("--latency", type=int, default=0, help="delay every response, ms")
    p.add_argument("--die-after", type=int, default=0, help="fail permanently after N")
    return p


def serve(args):
    """Start the server and return (httpd, port). Caller owns shutdown."""
    state = State(args)
    handler = type("BoundHandler", (Handler,), {"state": state})
    httpd = ThreadingHTTPServer(("127.0.0.1", args.port), handler)
    port = httpd.server_address[1]
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd, port




# --- selftest ---------------------------------------------------------------


def _get(url, cookie=None):
    import urllib.request
    import urllib.error

    req = urllib.request.Request(url)
    if cookie:
        req.add_header("Cookie", cookie)
    try:
        with urllib.request.urlopen(req, timeout=10) as r:
            return r.status, r.read().decode("utf-8"), r.headers.get("Set-Cookie")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8"), e.headers.get("Set-Cookie")


def _post(url, form):
    import urllib.parse
    import urllib.request
    import urllib.error

    data = urllib.parse.urlencode(form).encode()
    try:
        with urllib.request.urlopen(url, data=data, timeout=10) as r:
            return r.status, r.read().decode("utf-8")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8")


def _case(name, args_list, fn):
    args = build_parser().parse_args(args_list)
    httpd, port = serve(args)
    try:
        fn(f"http://127.0.0.1:{port}")
        print(f"  ok   {name}")
        return True
    except AssertionError as e:
        print(f"  FAIL {name}: {e}")
        return False
    finally:
        httpd.shutdown()


def selftest():
    """Prove each fault mode does what it claims. A harness that lies is worse
    than no harness, so this runs the server against itself."""
    results = []

    def rows_ok(base):
        code, body, _ = _get(f"{base}/browse/?q=Example")
        assert code == 200, code
        assert body.count("<tr>") == 3, body.count("<tr>")
        assert "Size</span> 2.1 GB" in body, "hidden label must be present"

    results.append(_case("serves rows with labelled cells", [], rows_ok))

    def rate_limit(base):
        assert _get(f"{base}/browse/")[0] == 200
        assert _get(f"{base}/browse/")[0] == 200
        assert _get(f"{base}/browse/")[0] == 429, "third request must be refused"

    results.append(_case("--rate-limit refuses after N", ["--rate-limit", "2"], rate_limit))

    def challenge(base):
        code, body, cookie = _get(f"{base}/browse/")
        assert code == 503 and "Just a moment" in body, (code, body[:40])
        assert cookie and "clearance=" in cookie, cookie
        code2, body2, _ = _get(f"{base}/browse/", cookie="clearance=ok")
        assert code2 == 200 and "<tr>" in body2, (code2, body2[:60])

    results.append(_case("--challenge clears once and stays cleared", ["--challenge"], challenge))

    def fail_when(base):
        assert _get(f"{base}/browse/?q=fine")[0] == 200
        assert _get(f"{base}/browse/?q=cursed")[0] == 502, "one combination must break"

    results.append(_case("--fail-when breaks one query only", ["--fail-when", "cursed"], fail_when))

    def named_cats(base):
        _, body, _ = _get(f"{base}/browse/")
        assert "/sub/tv/" in body and "/sub/5/1/" not in body, body[:200]

    results.append(_case("--categories named uses name paths", ["--categories", "named"], named_cats))

    def signed(base):
        _, listing, _ = _get(f"{base}/browse/")
        assert "magnet:?xt=" not in listing, "the row must NOT carry a magnet"
        _, details, _ = _get(f"{base}/t/11/")
        assert PAGE_TOKEN in details and CSRF_TOKEN in details

        ts = "1700000000"
        good = hashlib.sha256(f"11|{ts}|{PAGE_TOKEN}".encode()).hexdigest()
        code, body = _post(
            f"{base}/ajax/getMagnet.php",
            {"torrent_id": "11", "timestamp": ts, "hmac": good, "sessid": CSRF_TOKEN},
        )
        assert code == 200 and json.loads(body)["success"] is True, body
        assert "magnet:?xt=urn:btih:" in json.loads(body)["magnet"]

        _, bad = _post(
            f"{base}/ajax/getMagnet.php",
            {"torrent_id": "11", "timestamp": ts, "hmac": "wrong", "sessid": CSRF_TOKEN},
        )
        assert json.loads(bad)["success"] is False, "an unsigned request must be refused"

    results.append(_case("--sign-downloads needs the page token", ["--sign-downloads"], signed))

    def die_after(base):
        assert _get(f"{base}/browse/")[0] == 200
        assert _get(f"{base}/browse/")[0] == 500, "must fail permanently after N"
        assert _get(f"{base}/browse/")[0] == 500

    results.append(_case("--die-after fails permanently", ["--die-after", "1"], die_after))

    passed = sum(1 for r in results if r)
    print(f"{passed}/{len(results)} fault modes behave as advertised")
    return 0 if passed == len(results) else 1


def main():
    if "--selftest" in sys.argv:
        return selftest()
    args = build_parser().parse_args()
    httpd, port = serve(args)
    print(f"mock tracker on http://127.0.0.1:{port}", flush=True)
    try:
        threading.Event().wait()
    except KeyboardInterrupt:
        httpd.shutdown()
    return 0


if __name__ == "__main__":
    sys.exit(main())
