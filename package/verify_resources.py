#!/usr/bin/env python3
"""Verify the frozen release resource tree against the S11 manifest."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


def tree_digest(root: Path) -> tuple[int, int, str]:
    digest = hashlib.sha256()
    count = 0
    total = 0
    for path in sorted(p for p in root.rglob("*") if p.is_file()):
        if path.name == "THIRD_PARTY_NOTICES.txt":
            continue
        body = path.read_bytes()
        relative = path.relative_to(root).as_posix()
        mode = path.stat().st_mode & 0o777
        record = (
            f"{relative}\0{mode:o}\0{len(body)}\0"
            f"{hashlib.sha256(body).hexdigest()}\n"
        ).encode()
        digest.update(record)
        count += 1
        total += len(body)
    return count, total, digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--resources", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    args = parser.parse_args()
    expected = json.loads(args.manifest.read_text(encoding="utf-8"))["frozen_tree"]
    count, total, digest = tree_digest(args.resources)
    actual = {"files": count, "bytes": total, "sha256": digest}
    if actual != expected:
        raise SystemExit(f"resource tree mismatch: expected {expected}, got {actual}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
