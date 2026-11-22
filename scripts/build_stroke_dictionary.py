#!/usr/bin/env python3
"""Build the phrase-capable stroke dictionary used by the local Android build.

The normal MSIME dictionary release keeps a compact, character-ranked stroke
table.  This optional builder packages the community Rime stroke table from a
pinned ximeiorg/rime-stroke commit instead.  That table contains both
single-character rows and phrase rows whose key is a space-separated list of
per-character stroke codes, which is the format understood by the engine's
phrase query.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import sqlite3
import sys
import tempfile
import urllib.request
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUT = ROOT / "target/language-dictionaries"
DEFAULT_SOURCE_DIR = ROOT / "target/stroke-phrase-source"
SOURCE_COMMIT = "914d41d9ab7e7a44ec7c437c2eaab679cd435b53"
SOURCE_URL = (
    "https://raw.githubusercontent.com/ximeiorg/rime-stroke/"
    f"{SOURCE_COMMIT}/stroke.dict.yaml"
)
LICENSE_URL = (
    "https://raw.githubusercontent.com/ximeiorg/rime-stroke/"
    f"{SOURCE_COMMIT}/LICENSE"
)
SOURCE_SIZE = 9_894_840
SOURCE_SHA256 = "5747b81eb9a44c3b52e886e277f68f60fed6be536f872f64bd3d1fac88a689ae"
LICENSE_SIZE = 35_149
LICENSE_SHA256 = "3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986"
DATABASE_NAME = "msime-stroke.db"
LICENSE_NAME = "msime-rime_stroke_LICENSE.txt"
CHUNK_SIZE = 1 << 20
STROKES = frozenset("hspnz")

SCHEMA = """
CREATE TABLE metadata(name TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE syllables(syllable TEXT PRIMARY KEY) WITHOUT ROWID;
CREATE TABLE entries(key TEXT NOT NULL, text TEXT NOT NULL, weight INTEGER NOT NULL, PRIMARY KEY(key, text)) WITHOUT ROWID;
CREATE INDEX entries_by_key_weight ON entries(key, weight DESC);
"""


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(CHUNK_SIZE), b""):
            digest.update(block)
    return digest.hexdigest()


def verify(path: Path, expected_size: int, expected_sha256: str, label: str) -> None:
    actual_size = path.stat().st_size
    actual_sha256 = sha256(path)
    if actual_size != expected_size or actual_sha256 != expected_sha256:
        raise SystemExit(
            f"{label}: expected {expected_sha256} at {expected_size} bytes, "
            f"got {actual_sha256} at {actual_size} bytes"
        )


def download(url: str, destination: Path, expected_size: int, expected_sha256: str, label: str) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    staged = tempfile.NamedTemporaryFile(
        dir=destination.parent,
        prefix=f".{destination.name}.",
        delete=False,
    )
    staged_path = Path(staged.name)
    try:
        request = urllib.request.Request(
            url,
            headers={"User-Agent": "moli-ime-stroke-builder/1.0"},
        )
        with staged:
            with urllib.request.urlopen(request, timeout=300) as response:
                content_length = response.headers.get("Content-Length")
                if content_length is not None and int(content_length) > expected_size:
                    raise SystemExit(f"{label}: response is larger than the pinned source")
                size = 0
                while block := response.read(CHUNK_SIZE):
                    size += len(block)
                    if size > expected_size:
                        raise SystemExit(f"{label}: response is larger than the pinned source")
                    staged.write(block)
        verify(staged_path, expected_size, expected_sha256, label)
        os.replace(staged_path, destination)
    except BaseException:
        staged_path.unlink(missing_ok=True)
        raise


def ensure_source(path: Path, url: str, size: int, digest: str, label: str) -> None:
    if path.is_file():
        verify(path, size, digest, label)
        return
    download(url, path, size, digest, label)


def rows_from_rime(path: Path):
    """Yield (key, text, weight) from the tabular part of a Rime YAML file."""

    in_table = False
    for line_number, raw_line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not in_table:
            if raw_line == "...":
                in_table = True
            continue
        if not raw_line or raw_line.startswith("#"):
            continue
        fields = raw_line.split("\t")
        if len(fields) != 3:
            raise SystemExit(
                f"{path}:{line_number}: expected text, stroke key and weight separated by tabs"
            )
        text, key, weight_text = fields
        if not text or not key or key.startswith(" ") or key.endswith(" ") or "  " in key:
            raise SystemExit(f"{path}:{line_number}: invalid empty or repeated stroke segment")
        segments = key.split(" ")
        if any(not segment or any(letter not in STROKES for letter in segment) for segment in segments):
            raise SystemExit(f"{path}:{line_number}: unsupported stroke key {key!r}")
        if len(text) != len(segments):
            raise SystemExit(
                f"{path}:{line_number}: {text!r} has {len(text)} characters but {key!r} has {len(segments)} segments"
            )
        try:
            weight = int(weight_text)
        except ValueError as error:
            raise SystemExit(f"{path}:{line_number}: invalid weight {weight_text!r}") from error
        if weight < 0:
            raise SystemExit(f"{path}:{line_number}: negative weight {weight}")
        yield key, text, weight
    if not in_table:
        raise SystemExit(f"{path}: missing the Rime YAML table terminator (...)")


def build_database(source: Path, destination: Path) -> tuple[int, int]:
    rows = list(rows_from_rime(source))
    if not rows:
        raise SystemExit(f"{source}: no dictionary rows")
    if len({(key, text) for key, text, _ in rows}) != len(rows):
        raise SystemExit(f"{source}: duplicate (stroke key, text) row")

    destination.parent.mkdir(parents=True, exist_ok=True)
    file_descriptor, temporary_name = tempfile.mkstemp(
        dir=destination.parent,
        prefix=f".{destination.name}.",
        suffix=".tmp",
    )
    os.close(file_descriptor)
    temporary = Path(temporary_name)
    try:
        connection = sqlite3.connect(temporary)
        try:
            connection.executescript(SCHEMA)
            connection.executemany(
                "INSERT INTO metadata(name, value) VALUES (?, ?)",
                [
                    ("format_version", "1"),
                    ("source_commit", SOURCE_COMMIT),
                    ("license", "GPL-3.0-or-later"),
                ],
            )
            connection.executemany(
                "INSERT INTO syllables(syllable) VALUES (?)",
                [(stroke,) for stroke in "hspnz"],
            )
            connection.executemany(
                "INSERT INTO entries(key, text, weight) VALUES (?, ?, ?)",
                rows,
            )
            connection.commit()
            connection.execute("PRAGMA integrity_check").fetchone()
        finally:
            connection.close()
        os.replace(temporary, destination)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise
    phrase_count = sum(" " in key for key, _, _ in rows)
    return len(rows), phrase_count


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Build the pinned phrase-capable Rime stroke dictionary"
    )
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--source", type=Path, default=DEFAULT_SOURCE_DIR / "stroke.dict.yaml")
    parser.add_argument("--license-source", type=Path, default=DEFAULT_SOURCE_DIR / "LICENSE")
    arguments = parser.parse_args()

    ensure_source(
        arguments.source,
        SOURCE_URL,
        SOURCE_SIZE,
        SOURCE_SHA256,
        "stroke.dict.yaml",
    )
    ensure_source(
        arguments.license_source,
        LICENSE_URL,
        LICENSE_SIZE,
        LICENSE_SHA256,
        "rime-stroke LICENSE",
    )
    database = arguments.out / DATABASE_NAME
    license_path = arguments.out / LICENSE_NAME
    rows, phrases = build_database(arguments.source, database)
    license_path.parent.mkdir(parents=True, exist_ok=True)
    staged_license = license_path.with_name(f".{license_path.name}.tmp")
    staged_license.write_bytes(arguments.license_source.read_bytes())
    os.replace(staged_license, license_path)
    print(
        f"built {database} from ximeiorg/rime-stroke@{SOURCE_COMMIT}: "
        f"{rows} rows ({phrases} phrases); license {license_path}",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
