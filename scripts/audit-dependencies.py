"""固定依存をOSVと公開日時で検査する。依存のインストールは行わない。"""

import argparse
import datetime as dt
import json
import hashlib
import re
import sys
import tomllib
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path


def request_json(url, data=None):
    body = None if data is None else json.dumps(data).encode()
    request = urllib.request.Request(
        url, data=body,
        headers={"Content-Type": "application/json", "User-Agent": "TypedSolid-dependency-audit/0.1"},
    )
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.load(response)


def packages():
    result = []
    for name, version in re.findall(r"^([\w-]+)==([^\s\\]+)", Path("requirements-dev.lock").read_text(), re.M):
        result.append({"package": {"name": name, "ecosystem": "PyPI"}, "version": version})
    if Path("Cargo.lock").exists():
        cargo = tomllib.loads(Path("Cargo.lock").read_text())
        for package in cargo["package"]:
            if package.get("source", "").startswith("registry+"):
                result.append({"package": {"name": package["name"], "ecosystem": "crates.io"}, "version": package["version"]})
    return result


def release_info(query):
    name, ecosystem = query["package"]["name"], query["package"]["ecosystem"]
    version = query["version"]
    print(f"Checking release: {ecosystem}/{name}@{version}", file=sys.stderr, flush=True)
    if ecosystem == "PyPI":
        data = request_json(f"https://pypi.org/pypi/{name}/{version}/json")
        dates = [item["upload_time_iso_8601"] for item in data["urls"] if not item["yanked"]]
        if not dates:
            raise ValueError(f"No usable distribution: {name} {version}")
        return min(dates)
    data = request_json(f"https://crates.io/api/v1/crates/{name}/{version}")["version"]
    if data["yanked"]:
        raise ValueError(f"Yanked: {name} {version}")
    return data["created_at"]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cutoff", default="2026-08-29")
    args = parser.parse_args()
    cutoff = dt.datetime.fromisoformat(args.cutoff).replace(tzinfo=dt.timezone.utc)
    queries = packages()
    response = request_json("https://api.osv.dev/v1/querybatch", {"queries": queries})
    if len(response["results"]) != len(queries):
        raise ValueError("Incomplete OSV response")
    if any(result.get("next_page_token") for result in response["results"]):
        raise ValueError("OSV response requires pagination; review before accepting")
    with ThreadPoolExecutor(max_workers=4) as pool:
        dates = list(pool.map(release_info, queries))
    records = []
    for query, result, date in zip(queries, response["results"], dates, strict=True):
        records.append({**query, "released": date, "cooldown_ok": dt.datetime.fromisoformat(date.replace("Z", "+00:00")) <= cutoff, "vulnerabilities": result.get("vulns", [])})
    hashes = {name: hashlib.sha256(Path(name).read_bytes()).hexdigest() for name in ("Cargo.lock", "requirements-dev.lock") if Path(name).exists()}
    print(json.dumps({"cutoff": args.cutoff, "lock_sha256": hashes, "packages": records}, indent=2))
    return int(any(not r["cooldown_ok"] or r["vulnerabilities"] for r in records))


if __name__ == "__main__":
    raise SystemExit(main())
