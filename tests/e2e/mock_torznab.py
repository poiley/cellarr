#!/usr/bin/env python3
"""A tiny, real Torznab endpoint + .torrent server for the cellarr live e2e.

It speaks just enough of the Torznab protocol to exercise cellarr's indexer
adapter for real:

  GET /api?t=caps              -> a <caps> document advertising the search/movie
                                  modes and their params.
  GET /api?t=search&q=...      -> an RSS feed with ONE <item> for our release,
  GET /api?t=movie&...            whose <enclosure url=...> points at the
                                  download URL (a magnet OR the .torrent).
  GET /file.torrent            -> the .torrent bytes (so qBittorrent can fetch
                                  full metadata offline and recheck pre-staged
                                  data to Completed in seconds).

Every received request line is appended to a log file so the harness can assert
"the mock RECEIVED the search". No third-party deps; pure stdlib.

The .torrent is created in pure Python (bencode + SHA1 of the info dict) so we
control the infohash exactly, and the magnet we advertise carries that same
btih — which is what cellarr derives the qBittorrent download id from.
"""

import hashlib
import os
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs


# ---- bencode (just enough to build a single-file .torrent) -----------------

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
        for k in sorted(value.keys()):
            out += bencode(k) + bencode(value[k])
        return out + b"e"
    raise TypeError(f"cannot bencode {type(value)}")


def build_torrent(payload_path, name, piece_length=262144):
    with open(payload_path, "rb") as f:
        data = f.read()
    pieces = b""
    for i in range(0, len(data), piece_length):
        pieces += hashlib.sha1(data[i:i + piece_length]).digest()
    info = {
        "name": name,
        "piece length": piece_length,
        "length": len(data),
        "pieces": pieces,
        "private": 1,
    }
    info_encoded = bencode(info)
    infohash = hashlib.sha1(info_encoded).hexdigest()
    torrent = {
        # No real announce; this is a private, offline, pre-staged torrent.
        "announce": "http://127.0.0.1:1/announce",
        "info": info,
    }
    return bencode(torrent), infohash, len(data)


def main():
    bind_host = os.environ["MOCK_BIND"]          # e.g. 127.0.0.1
    bind_port = int(os.environ["MOCK_PORT"])     # 0 -> OS picks
    advertise_host = os.environ["MOCK_ADVERTISE_HOST"]  # host qbit/cellarr reach
    payload_path = os.environ["PAYLOAD_PATH"]
    release_title = os.environ["RELEASE_TITLE"]
    payload_name = os.environ["PAYLOAD_NAME"]
    log_path = os.environ["MOCK_LOG"]
    port_file = os.environ["MOCK_PORT_FILE"]
    # Which enclosure to advertise: "torrent" (http .torrent) or "magnet".
    enclosure_mode = os.environ.get("ENCLOSURE_MODE", "torrent")

    torrent_bytes, infohash, size = build_torrent(payload_path, payload_name)
    # Persist the infohash so the harness can assert against qBittorrent.
    with open(os.environ["INFOHASH_FILE"], "w") as f:
        f.write(infohash)

    log_lock = threading.Lock()

    def record(line):
        with log_lock:
            with open(log_path, "a") as lf:
                lf.write(line + "\n")

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass  # silence default stderr logging

        def _advertise_base(self):
            return f"http://{advertise_host}:{self.server.server_address[1]}"

        def do_GET(self):
            record(self.path)
            parsed = urlparse(self.path)
            qs = parse_qs(parsed.query)

            if parsed.path == "/file.torrent":
                self.send_response(200)
                self.send_header("Content-Type", "application/x-bittorrent")
                self.send_header("Content-Length", str(len(torrent_bytes)))
                self.end_headers()
                self.wfile.write(torrent_bytes)
                return

            t = (qs.get("t", [""])[0]).lower()
            if t == "caps":
                self._xml(CAPS)
                return
            if t in ("search", "movie", "tvsearch"):
                base = self._advertise_base()
                if enclosure_mode == "magnet":
                    dl = f"magnet:?xt=urn:btih:{infohash}&dn={payload_name}"
                else:
                    dl = f"{base}/file.torrent"
                feed = FEED_TEMPLATE.format(
                    title=release_title,
                    guid=f"{base}/g/{infohash}",
                    enclosure=dl,
                    size=size,
                    infohash=infohash,
                )
                self._xml(feed)
                return

            self.send_error(404, "unknown torznab mode")

        def _xml(self, body):
            data = body.encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/xml; charset=utf-8")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

    httpd = ThreadingHTTPServer((bind_host, bind_port), Handler)
    actual_port = httpd.server_address[1]
    with open(port_file, "w") as f:
        f.write(str(actual_port))
    sys.stderr.write(
        f"mock-torznab up on {bind_host}:{actual_port} "
        f"(advertise {advertise_host}) infohash={infohash} mode={enclosure_mode}\n"
    )
    sys.stderr.flush()
    httpd.serve_forever()


CAPS = """<?xml version="1.0" encoding="UTF-8"?>
<caps>
  <server title="cellarr-e2e-mock"/>
  <limits max="100" default="50"/>
  <searching>
    <search available="yes" supportedParams="q"/>
    <movie-search available="yes" supportedParams="q,imdbid,tmdbid"/>
    <tv-search available="yes" supportedParams="q,season,ep,tvdbid"/>
  </searching>
  <categories>
    <category id="2000" name="Movies"/>
    <category id="5000" name="TV"/>
  </categories>
</caps>"""


FEED_TEMPLATE = """<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed">
  <channel>
    <title>cellarr-e2e-mock</title>
    <item>
      <title>{title}</title>
      <guid>{guid}</guid>
      <enclosure url="{enclosure}" length="{size}" type="application/x-bittorrent"/>
      <torznab:attr name="size" value="{size}"/>
      <torznab:attr name="seeders" value="5"/>
      <torznab:attr name="infohash" value="{infohash}"/>
      <torznab:attr name="downloadvolumefactor" value="0"/>
    </item>
  </channel>
</rss>"""


if __name__ == "__main__":
    main()
