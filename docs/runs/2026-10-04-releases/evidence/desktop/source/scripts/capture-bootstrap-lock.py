#!/usr/bin/env python3
"""Record the complete package closure and signed archive hashes, without secrets."""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("evidence", type=Path)
parser.add_argument("output", type=Path)
parser.add_argument("--archive-date", required=True)
args = parser.parse_args()

def read_hashes(filename):
    result = {}
    for line in (args.evidence / filename).read_text().splitlines():
        digest, name = line.split(maxsplit=1)
        result[Path(name).name] = digest
    return result

archives = read_hashes("package-archives.sha256")
packages = []
for line in (args.evidence / "packages.txt").read_text().splitlines():
    name, version = line.split(maxsplit=1)
    prefixes = (f"{name}-{version}-", f"{name}-{version.split(':', 1)[-1]}-")
    matches = [filename for filename in archives
               if filename.startswith(prefixes) and not filename.endswith(".sig")]
    if len(matches) != 1:
        parser.error(f"Expected one cached archive for {name} {version}: {matches}")
    filename = matches[0]
    if filename + ".sig" not in archives:
        parser.error(f"Missing signature archive for {name}")
    packages.append({
        "name": name, "version": version,
        "archive": filename, "sha256": archives[filename],
        "signature": filename + ".sig", "signature_sha256": archives[filename + ".sig"],
    })
artifacts = read_hashes("build-artifacts.sha256")
lock = {
    "schema_version": 1,
    "kind": "looom-bootstrap-package-inventory",
    "architecture": "x86_64",
    "archive_date": args.archive_date,
    "repository_url_template": f"https://archive.archlinux.org/repos/{args.archive_date}/$repo/os/$arch",
    "repositories": {name: {"database_sha256": artifacts[name + ".db"]}
                     for name in ("core", "extra")},
    "package_signatures": "Required; verified by pacman during installation",
    "packages": sorted(packages, key=lambda pkg: pkg["name"]),
}
args.output.parent.mkdir(parents=True, exist_ok=True)
args.output.write_text(json.dumps(lock, ensure_ascii=False, indent=2) + "\n")
print(f"Recorded {len(packages)} exact packages with archive and signature hashes")
