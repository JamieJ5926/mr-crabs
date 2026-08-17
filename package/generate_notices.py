#!/usr/bin/env python3
"""Generate deterministic third-party notices from Cargo metadata and package license files."""
from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest-path", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    raw = subprocess.check_output([
        "cargo", "metadata", "--format-version", "1", "--locked",
        "--manifest-path", str(args.manifest_path),
    ])
    metadata = json.loads(raw)
    workspace = set(metadata["workspace_members"])
    packages = sorted(
        (p for p in metadata["packages"] if p["id"] not in workspace),
        key=lambda p: (p["name"].lower(), p["version"], p["id"]),
    )
    out = [
        "MR CRABS THIRD-PARTY NOTICES",
        "",
        "Generated from Cargo.lock/Cargo metadata. The Mr Crabs and bundled",
        "Ghostty theme sources are distributed under the Ghostty repository's",
        "MIT license; see the bundled LICENSE.",
        "",
        "The bundled shell-integration scripts contain kitty-derived portions",
        "licensed under GPL-3.0. Their source headers identify the affected files;",
        "corresponding source is shipped under Resources/ghostty/shell-integration.",
        "",
        "Dependency license texts follow when distributed by the dependency package.",
        "",
    ]
    emitted_texts: set[str] = set()
    for package in packages:
        out.extend([
            "=" * 72,
            f"{package['name']} {package['version']}",
            f"License: {package.get('license') or 'NOT DECLARED'}",
            f"Repository: {package.get('repository') or package.get('homepage') or 'not declared'}",
            "",
        ])
        root = Path(package["manifest_path"]).parent
        files = sorted(
            p for p in root.iterdir()
            if p.is_file() and p.name.upper().startswith(("LICENSE", "COPYING", "NOTICE"))
        )
        for license_file in files:
            body = license_file.read_text(encoding="utf-8", errors="replace").strip()
            digest = hashlib.sha256(body.encode()).hexdigest()
            if not body or digest in emitted_texts:
                continue
            emitted_texts.add(digest)
            out.extend([f"--- {license_file.name} ---", body, ""])
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text("\n".join(out).rstrip() + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
