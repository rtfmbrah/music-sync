#!/usr/bin/env python3
"""Release committed owned tags for one safe PNG-cover rematerialization pass."""

import argparse
import hashlib
import json
from pathlib import Path
import sqlite3
import sys


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            value.update(chunk)
    return value.hexdigest()


def candidates(connection: sqlite3.Connection) -> list[sqlite3.Row]:
    return connection.execute(
        """
        SELECT materialization.*, artifacts.path AS current_path,
               artifacts.sha256 AS current_sha256,
               artwork_blobs.mime_type AS artwork_mime_type
        FROM metadata_materializations AS materialization
        JOIN recordings ON recordings.id = materialization.recording_id
        JOIN artifacts ON artifacts.id = recordings.preferred_artifact_id
        JOIN recording_provider_artwork
          ON recording_provider_artwork.recording_id = recordings.id
        JOIN artwork_blobs ON artwork_blobs.id = recording_provider_artwork.blob_id
        WHERE materialization.state = 'committed'
          AND materialization.result_artifact_id = artifacts.id
          AND materialization.final_path = artifacts.path
          AND materialization.result_sha256 = artifacts.sha256
          AND artifacts.health = 'healthy'
          AND artwork_blobs.mime_type = 'image/webp'
          AND NOT EXISTS (
            SELECT 1 FROM events
            WHERE events.event = 'webp_tags_released'
              AND json_valid(events.context_json)
              AND json_extract(events.context_json, '$.recording_id') =
                  materialization.recording_id
          )
          AND (
            EXISTS (
              SELECT 1 FROM acquisition_commits
              WHERE acquisition_commits.final_path = artifacts.path
                AND acquisition_commits.status = 'committed'
            )
            OR EXISTS (
              SELECT 1 FROM repair_commits
              WHERE repair_commits.final_path = artifacts.path
                AND repair_commits.committed_at IS NOT NULL
            )
          )
        ORDER BY materialization.recording_id
        """
    ).fetchall()


def safe_current_path(row: sqlite3.Row, library_root: Path) -> Path:
    path = Path(row["current_path"])
    if not path.is_absolute() or path.is_symlink() or not path.is_file():
        raise RuntimeError(f"unsafe current artifact path: {path}")
    try:
        path.relative_to(library_root)
    except ValueError as error:
        raise RuntimeError(f"artifact is outside the library root: {path}") from error
    actual = digest(path)
    if actual != row["current_sha256"]:
        raise RuntimeError(
            f"current artifact hash changed for recording {row['recording_id']}: {path}"
        )
    return path


def release_one(
    connection: sqlite3.Connection, row: sqlite3.Row, library_root: Path
) -> None:
    path = safe_current_path(row, library_root)
    audit = {key: row[key] for key in row.keys() if key not in {"current_path", "current_sha256"}}
    audit["current_path"] = str(path)
    connection.execute("BEGIN IMMEDIATE")
    try:
        current = connection.execute(
            """
            SELECT recordings.preferred_artifact_id, artifacts.path, artifacts.sha256,
                   metadata_materializations.state,
                   metadata_materializations.result_artifact_id
            FROM recordings
            JOIN artifacts ON artifacts.id = recordings.preferred_artifact_id
            JOIN metadata_materializations
              ON metadata_materializations.recording_id = recordings.id
            WHERE recordings.id = ?1
            """,
            (row["recording_id"],),
        ).fetchone()
        expected = (
            row["result_artifact_id"],
            row["final_path"],
            row["result_sha256"],
            "committed",
            row["result_artifact_id"],
        )
        if current is None or tuple(current) != expected:
            raise RuntimeError(
                f"materialization changed for recording {row['recording_id']}"
            )
        connection.execute(
            """
            INSERT INTO events(level,component,event,message,context_json)
            VALUES ('info','maintenance','webp_tags_released',?1,?2)
            """,
            (
                f"released recording {row['recording_id']} for PNG-cover tag rematerialization",
                json.dumps(audit, sort_keys=True),
            ),
        )
        changed = connection.execute(
            """
            DELETE FROM metadata_materializations
            WHERE recording_id = ?1 AND state = 'committed'
              AND result_artifact_id = ?2 AND result_sha256 = ?3
            """,
            (
                row["recording_id"],
                row["result_artifact_id"],
                row["result_sha256"],
            ),
        ).rowcount
        if changed != 1:
            raise RuntimeError(
                f"failed to release recording {row['recording_id']} exactly once"
            )
        connection.commit()
    except Exception:
        connection.rollback()
        raise
    print(f"released recording {row['recording_id']}: {path}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--database", type=Path, required=True)
    parser.add_argument("--library-root", type=Path, required=True)
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    args.library_root = args.library_root.resolve()
    connection = sqlite3.connect(args.database)
    connection.row_factory = sqlite3.Row
    schema = connection.execute("PRAGMA user_version").fetchone()[0]
    if schema != 20:
        raise RuntimeError(f"expected schema 20, found {schema}")
    rows = candidates(connection)
    print(f"eligible WebP tag materializations: {len(rows)}")
    if not args.apply:
        for row in rows:
            safe_current_path(row, args.library_root)
            print(f"would release recording {row['recording_id']}: {row['current_path']}")
        return 0
    for row in rows:
        release_one(connection, row, args.library_root)
    print(f"released materializations: {len(rows)}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError, sqlite3.Error, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
