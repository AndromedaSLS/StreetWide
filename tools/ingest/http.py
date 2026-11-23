"""Downloads with a generic User-Agent, size limits and checksum checks.

Downloaded files are untrusted input: they are written to `<dest>.part`
first and only renamed into place once complete and, where the source
publishes one, once their SHA-256 matches.
"""

from __future__ import annotations

import hashlib
import os
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

from tools.ingest import USER_AGENT

TIMEOUT_S = 120
CHUNK = 1 << 20


class DownloadError(RuntimeError):
    """A download failed or did not match its checksum."""


def _request(url: str, data: bytes | None = None) -> urllib.request.Request:
    if urllib.parse.urlsplit(url).scheme != "https":
        raise DownloadError(f"refusing non-HTTPS URL {url}")
    headers = {"User-Agent": USER_AGENT}
    if data is not None:
        headers["Content-Type"] = "application/x-www-form-urlencoded"
    return urllib.request.Request(url, data=data, headers=headers)


def fetch_bytes(url: str, *, data: bytes | None = None, max_bytes: int, retries: int = 2) -> bytes:
    """GET (or POST `data`) and return the body, at most `max_bytes`."""
    for attempt in range(retries + 1):
        try:
            with urllib.request.urlopen(_request(url, data), timeout=TIMEOUT_S) as r:
                body = r.read(max_bytes + 1)
            if len(body) > max_bytes:
                raise DownloadError(f"{url}: response larger than {max_bytes} bytes")
            return body
        except (urllib.error.URLError, TimeoutError) as e:
            if attempt == retries:
                raise DownloadError(f"{url}: {e}") from e
            time.sleep(5 * (attempt + 1))
    raise AssertionError("unreachable")


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        while chunk := f.read(CHUNK):
            h.update(chunk)
    return h.hexdigest()


def fetch_file(url: str, dest: Path, *, sha256: str | None, max_bytes: int, offline: bool) -> Path:
    """Downloads `url` to `dest` unless a verified copy is already cached."""
    if dest.exists():
        if sha256 is None or sha256_file(dest) == sha256.lower():
            return dest
        raise DownloadError(f"cached {dest} does not match its checksum; delete it and retry")
    if offline:
        raise DownloadError(f"{dest} is not cached and --offline is set")
    dest.parent.mkdir(parents=True, exist_ok=True)
    part = dest.with_name(dest.name + ".part")
    print(f"  downloading {url}", flush=True)
    h = hashlib.sha256()
    size = 0
    try:
        with urllib.request.urlopen(_request(url), timeout=TIMEOUT_S) as r, part.open("wb") as out:
            while chunk := r.read(CHUNK):
                size += len(chunk)
                if size > max_bytes:
                    raise DownloadError(f"{url}: larger than {max_bytes} bytes")
                h.update(chunk)
                out.write(chunk)
    except (urllib.error.URLError, TimeoutError) as e:
        part.unlink(missing_ok=True)
        raise DownloadError(f"{url}: {e}") from e
    except BaseException:
        part.unlink(missing_ok=True)
        raise
    if sha256 is not None and h.hexdigest() != sha256.lower():
        part.unlink(missing_ok=True)
        raise DownloadError(f"{url}: SHA-256 mismatch")
    os.replace(part, dest)
    print(f"  saved {dest} ({size / 1e6:.1f} MB)", flush=True)
    return dest
