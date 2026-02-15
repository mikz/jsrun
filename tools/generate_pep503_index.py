#!/usr/bin/env python3

from __future__ import annotations

import html
import json
import os
import re
import sys
from pathlib import Path
from typing import Any
from urllib.error import HTTPError
from urllib.request import Request, urlopen


def _gh_get_json(api_path: str) -> Any:
    url = f"https://api.github.com/{api_path.lstrip('/')}"
    headers = {
        "Accept": "application/vnd.github+json",
        "User-Agent": "jsrun-pep503-index-generator",
    }
    token = os.environ.get("GITHUB_TOKEN")
    if token:
        headers["Authorization"] = f"Bearer {token}"
    req = Request(url, headers=headers)
    with urlopen(req, timeout=30) as resp:
        return json.load(resp)


def _write_html_index(path: Path, links: list[tuple[str, str]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    body = "".join(
        f'<a href="{html.escape(url)}">{html.escape(name)}</a>\n' for name, url in links
    )
    path.write_text(f"<!doctype html>\n<html><body>\n{body}</body></html>\n", encoding="utf-8")


def _collect_assets(release: dict[str, Any]) -> list[tuple[str, str]]:
    out: list[tuple[str, str]] = []
    for asset in release.get("assets", []):
        name = asset.get("name")
        url = asset.get("browser_download_url")
        if not isinstance(name, str) or not isinstance(url, str):
            continue
        if name.endswith(".whl") or name.endswith(".tar.gz"):
            out.append((name, url))
    out.sort(key=lambda x: x[0])
    return out


def main() -> int:
    repo = os.environ.get("GITHUB_REPOSITORY")
    output_dir = os.environ.get("OUTPUT_DIR")
    if not repo or not output_dir:
        print("ERROR: require GITHUB_REPOSITORY and OUTPUT_DIR", file=sys.stderr)
        return 2

    root = Path(output_dir)

    # Stable: all non-draft, non-prerelease releases with v* tags.
    releases = _gh_get_json(f"repos/{repo}/releases?per_page=100")
    stable_assets: list[tuple[str, str]] = []
    tag_re = re.compile(r"^v[0-9]")
    if isinstance(releases, list):
        for r in releases:
            if not isinstance(r, dict):
                continue
            if r.get("draft") or r.get("prerelease"):
                continue
            tag = r.get("tag_name")
            if not isinstance(tag, str) or not tag_re.match(tag):
                continue
            stable_assets.extend(_collect_assets(r))

    # Nightly: optional release at tag "nightly".
    nightly_assets: list[tuple[str, str]] = []
    try:
        nightly = _gh_get_json(f"repos/{repo}/releases/tags/nightly")
        if isinstance(nightly, dict) and not nightly.get("draft"):
            nightly_assets = _collect_assets(nightly)
    except HTTPError as e:
        if e.code != 404:
            raise

    package_name = "jsrun"

    _write_html_index(root / "simple" / "index.html", [(package_name, f"{package_name}/")])
    _write_html_index(root / "simple" / package_name / "index.html", stable_assets)

    _write_html_index(
        root / "nightly" / "simple" / "index.html",
        [(package_name, f"{package_name}/")],
    )
    _write_html_index(root / "nightly" / "simple" / package_name / "index.html", nightly_assets)

    print(f"Wrote PEP 503 indexes into {root}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

