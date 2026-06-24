#!/usr/bin/env python3
"""A tiny, dependency-free Torznab server + .torrent builder for the cellarr
live e2e (tests/e2e/run.sh).

Two responsibilities, selected by argv[1]:

  make-torrent <payload-file> <out.torrent> <save-dir-name>
        Build a single-file BitTorrent v1 metainfo from <payload-file> whose
        piece hashes match the file's bytes, so a download client that already
        has the data on disk rechecks straight to Completed (no peers needed).
        Prints the lowercase infohash (the btih) to stdout.

  serve <bind-host> <port> <torrent-file> <release-title> <infohash> <public-base>
        Serve a real Torznab endpoint:
          * t=caps           -> advertises search + tvsearch modes + categories
          * t=search/tvsearch -> ONE <item> whose <enclosure url> points at the
            .torrent over HTTP, with &xt=urn:btih:<infohash> appended so the
            qBittorrent adapter (which derives the id from the URL) gets the
            matching infohash while qBittorrent itself downloads the .torrent.
          * GET /file.torrent -> the raw .torrent bytes.
        Every received request line is appended to <torrent-file>.requests.log
        so the harness can prove the mock was actually hit.

The protocol shapes match what cellarr-indexers/{caps,feed,torznab}.rs parse.
"""

import hashlib
import http.server
import socketserver
import sys
import urllib.parse
from pathlib import Path


# --- bencode (minimal v1 encoder) ------------------------------------------

def bencode(value):
    if isinstance(value, int):
        return b"i" + str(value).encode() + b"e"
    if isinstance(value, bytes):
        return str(len(value)).encode() + b":" + value
    if isinstance(value, str):
        return bencode(value.encode())
    if isinstance(value, list):
        return b"l" + b"".join(bencode(v) for v in value) + b"e"
    if isinstance(value, dict):
        out = b"d"
        for k in sorted(value):  # bencode dict keys must be sorted
            kk = k.encode() if isinstance(k, str) else k
            out += bencode(kk) + bencode(value[k])
        return out + b"e"
    raise TypeError(f"cannot bencode {type(value)}")


def make_torrent(payload_path: Path, out_path: Path) -> str:
    data = payload_path.read_bytes()
    piece_len = 16384  # 16 KiB pieces; payload is a few KiB so this is fine
    pieces = b""
    for off in range(0, len(data), piece_len):
        pieces += hashlib.sha1(data[off:off + piece_len]).digest()
    info = {
        "name": payload_path.name,
        "piece length": piece_len,
        "length": len(data),
        "pieces": pieces,
    }
    metainfo = {
        # No announce URL: a private, offline, peerless torrent. qBittorrent
        # still adds it and (with the data pre-staged) rechecks to Completed.
        "announce": "http://localhost:1/announce",
        "info": info,
    }
    out_path.write_bytes(bencode(metainfo))
    infohash = hashlib.sha1(bencode(info)).hexdigest()
    return infohash


# --- Torznab server ---------------------------------------------------------

def build_handler(torrent_file: Path, title: str, infohash: str, public_base: str):
    reqlog = torrent_file.with_suffix(torrent_file.suffix + ".requests.log")
    # Torrent download URL with the infohash appended as a query param so the
    # cellarr qBittorrent adapter can extract btih from the URL, while
    # qBittorrent downloads the real .torrent (ignoring the extra param).
    enclosure = f"{public_base}/file.torrent?xt=urn:btih:{infohash}"

    caps_xml = (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        "<caps>\n"
        '  <server title="cellarr-e2e-mock"/>\n'
        '  <limits max="100" default="50"/>\n'
        "  <searching>\n"
        '    <search available="yes" supportedParams="q"/>\n'
        '    <tv-search available="yes" supportedParams="q,season,ep,tvdbid"/>\n'
        '    <movie-search available="yes" supportedParams="q,imdbid,tmdbid"/>\n'
        "  </searching>\n"
        "  <categories>\n"
        '    <category id="5000" name="TV">\n'
        '      <subcat id="5040" name="TV/HD"/>\n'
        "    </category>\n"
        '    <category id="2000" name="Movies">\n'
        '      <subcat id="2040" name="Movies/HD"/>\n'
        "    </category>\n"
        "  </categories>\n"
        "</caps>\n"
    )

    def search_xml() -> str:
        from xml.sax.saxutils import escape, quoteattr
        return (
            '<?xml version="1.0" encoding="UTF-8"?>\n'
            '<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed">\n'
            "  <channel>\n"
            "    <title>cellarr-e2e-mock</title>\n"
            "    <item>\n"
            f"      <title>{escape(title)}</title>\n"
            f"      <guid>{escape(infohash)}</guid>\n"
            f"      <link>{escape(enclosure)}</link>\n"
            f"      <enclosure url={quoteattr(enclosure)} "
            'type="application/x-bittorrent"/>\n'
            f'      <torznab:attr name="infohash" value="{escape(infohash)}"/>\n'
            '      <torznab:attr name="seeders" value="5"/>\n'
            '      <torznab:attr name="peers" value="5"/>\n'
            '      <torznab:attr name="size" value="100000"/>\n'
            '      <torznab:attr name="downloadvolumefactor" value="1"/>\n'
            "    </item>\n"
            "  </channel>\n"
            "</rss>\n"
        )

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass  # silence default stderr logging

        def _record(self):
            with reqlog.open("a") as fh:
                fh.write(self.requestline + "\n")

        def do_GET(self):
            self._record()
            parsed = urllib.parse.urlparse(self.path)
            qs = urllib.parse.parse_qs(parsed.query)
            path = parsed.path.rstrip("/")

            if path.endswith("/file.torrent") or path == "/file.torrent":
                body = torrent_file.read_bytes()
                self.send_response(200)
                self.send_header("Content-Type", "application/x-bittorrent")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return

            t = (qs.get("t") or [""])[0]
            if t == "caps":
                body = caps_xml.encode()
            elif t in ("search", "tvsearch", "movie"):
                body = search_xml().encode()
            else:
                self.send_response(400)
                self.end_headers()
                self.wfile.write(b"unknown t mode")
                return
            self.send_response(200)
            self.send_header("Content-Type", "application/xml")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    return Handler


class QuietTCPServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def serve(bind_host, port, torrent_file, title, infohash, public_base):
    handler = build_handler(Path(torrent_file), title, infohash, public_base)
    with QuietTCPServer((bind_host, int(port)), handler) as httpd:
        httpd.serve_forever()


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    if cmd == "make-torrent":
        payload, out = Path(sys.argv[2]), Path(sys.argv[3])
        print(make_torrent(payload, out))
    elif cmd == "serve":
        serve(sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5],
              sys.argv[6], sys.argv[7])
    else:
        print(__doc__)
        sys.exit(2)


if __name__ == "__main__":
    main()
