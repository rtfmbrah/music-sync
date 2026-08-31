#!/usr/bin/env python3
"""Losslessly publish committed managed WebM/Opus artifacts as Ogg/Opus."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys


def digest(path: Path) -> tuple[str, int]:
    value = hashlib.sha256()
    size = 0
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            value.update(chunk)
            size += len(chunk)
    return value.hexdigest(), size


def probe(executable: str, path: Path) -> dict:
    result = subprocess.run(
        [executable, "-v", "error", "-show_entries", "format=duration:stream=codec_name,codec_type,sample_rate,channels", "-of", "json", str(path)],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        timeout=60,
    )
    payload = json.loads(result.stdout)
    audio = [stream for stream in payload.get("streams", []) if stream.get("codec_type") == "audio"]
    if len(audio) != 1:
        raise RuntimeError(f"expected exactly one audio stream in {path}, found {len(audio)}")
    duration = payload.get("format", {}).get("duration")
    return {
        "codec": audio[0].get("codec_name"),
        "sample_rate": int(audio[0]["sample_rate"]) if audio[0].get("sample_rate") else None,
        "channels": audio[0].get("channels"),
        "duration_ms": round(float(duration) * 1000) if duration is not None else None,
    }


def fsync_path(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def candidates(connection: sqlite3.Connection, root: Path) -> list[sqlite3.Row]:
    prefix = f"{root}/%"
    return connection.execute(
        """
        SELECT artifacts.id AS artifact_id, artifacts.recording_id, artifacts.path,
               artifacts.sha256 AS source_sha256, artifacts.duration_ms,
               acquisition_commits.job_id
        FROM artifacts
        JOIN recordings ON recordings.preferred_artifact_id = artifacts.id
        JOIN acquisition_commits ON acquisition_commits.final_path = artifacts.path
                                AND acquisition_commits.status = 'committed'
        WHERE artifacts.health = 'healthy'
          AND lower(artifacts.path) LIKE '%.webm'
          AND artifacts.path LIKE ?1
        ORDER BY artifacts.id
        """,
        (prefix,),
    ).fetchall()


def migrate_one(args: argparse.Namespace, connection: sqlite3.Connection, row: sqlite3.Row) -> None:
    source = Path(row["path"])
    if source.is_symlink() or not source.is_file() or source.parent != args.library_root:
        raise RuntimeError(f"unsafe managed source path: {source}")
    destination = source.with_suffix(".opus")
    staging = args.state_directory / "webm-remux-staging" / f"artifact-{row['artifact_id']}.opus"
    staging.parent.mkdir(parents=True, exist_ok=True)

    source_hash, _ = digest(source)
    if row["source_sha256"] and source_hash != row["source_sha256"]:
        raise RuntimeError(f"source hash changed for artifact {row['artifact_id']}: {source}")
    source_probe = probe(args.ffprobe, source)
    if source_probe["codec"] != "opus":
        raise RuntimeError(f"artifact {row['artifact_id']} is {source_probe['codec']}, not Opus")

    if not staging.exists():
        subprocess.run(
            [args.ffmpeg, "-nostdin", "-v", "error", "-n", "-i", str(source), "-map", "0:a:0", "-vn", "-c:a", "copy", "-f", "opus", str(staging)],
            check=True,
            timeout=180,
        )
    result_probe = probe(args.ffprobe, staging)
    if result_probe["codec"] != source_probe["codec"]:
        raise RuntimeError(f"codec changed while remuxing artifact {row['artifact_id']}")
    if source_probe["duration_ms"] is not None and result_probe["duration_ms"] is not None:
        if abs(source_probe["duration_ms"] - result_probe["duration_ms"]) > 1000:
            raise RuntimeError(f"duration changed while remuxing artifact {row['artifact_id']}")
    result_hash, result_size = digest(staging)

    if destination.exists():
        existing_hash, existing_size = digest(destination)
        if (existing_hash, existing_size) != (result_hash, result_size):
            raise RuntimeError(f"different destination already exists: {destination}")
    else:
        os.link(staging, destination)
    os.chmod(destination, 0o644)
    fsync_path(destination)
    fsync_path(destination.parent)

    connection.execute("BEGIN IMMEDIATE")
    try:
        current = connection.execute(
            "SELECT preferred_artifact_id FROM recordings WHERE id=?1", (row["recording_id"],)
        ).fetchone()
        if current is None or current[0] != row["artifact_id"]:
            raise RuntimeError(f"preferred artifact changed for recording {row['recording_id']}")
        connection.execute(
            """INSERT INTO artifacts(recording_id,path,sha256,duration_ms,codec,sample_rate_hz,channels,health)
               VALUES (?1,?2,?3,?4,?5,?6,?7,'healthy')""",
            (row["recording_id"], str(destination), result_hash, result_probe["duration_ms"], result_probe["codec"], result_probe["sample_rate"], result_probe["channels"]),
        )
        result_artifact_id = connection.execute("SELECT last_insert_rowid()").fetchone()[0]
        connection.execute(
            """INSERT INTO artifact_fingerprints(artifact_id,algorithm,max_seconds,duration_ms,fingerprint_json,value_count,updated_at)
               SELECT ?1,algorithm,max_seconds,duration_ms,fingerprint_json,value_count,updated_at
               FROM artifact_fingerprints WHERE artifact_id=?2""",
            (result_artifact_id, row["artifact_id"]),
        )
        connection.execute(
            "UPDATE recordings SET preferred_artifact_id=?2 WHERE id=?1",
            (row["recording_id"], result_artifact_id),
        )
        changed = connection.execute(
            """UPDATE acquisition_commits SET final_path=?2,sha256=?3,bytes=?4,codec=?5,
                      duration_ms=?6,sample_rate_hz=?7,channels=?8
               WHERE job_id=?1 AND final_path=?9 AND status='committed'""",
            (row["job_id"], str(destination), result_hash, result_size, result_probe["codec"], result_probe["duration_ms"], result_probe["sample_rate"], result_probe["channels"], str(source)),
        ).rowcount
        if changed != 1:
            raise RuntimeError(f"acquisition ownership changed for artifact {row['artifact_id']}")
        connection.execute(
            "INSERT INTO events(level,component,event,message,context_json) VALUES ('info','maintenance','managed_webm_remux',?1,?2)",
            (f"losslessly remuxed managed artifact {row['artifact_id']} for Navidrome", json.dumps({"source_artifact_id": row["artifact_id"], "result_artifact_id": result_artifact_id, "source_path": str(source), "result_path": str(destination), "source_sha256": source_hash, "result_sha256": result_hash}, sort_keys=True)),
        )
        connection.commit()
    except Exception:
        connection.rollback()
        raise
    print(f"migrated artifact {row['artifact_id']}: {source.name} -> {destination.name}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--database", type=Path, required=True)
    parser.add_argument("--library-root", type=Path, required=True)
    parser.add_argument("--state-directory", type=Path, required=True)
    parser.add_argument("--ffmpeg", default="/usr/bin/ffmpeg")
    parser.add_argument("--ffprobe", default="/usr/bin/ffprobe")
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    args.library_root = args.library_root.resolve()
    args.state_directory = args.state_directory.resolve()
    connection = sqlite3.connect(args.database)
    connection.row_factory = sqlite3.Row
    rows = candidates(connection, args.library_root)
    print(f"eligible managed WebM artifacts: {len(rows)}")
    if not args.apply:
        for row in rows:
            print(f"would migrate artifact {row['artifact_id']}: {row['path']}")
        return 0
    for row in rows:
        migrate_one(args, connection, row)
    print(f"migration complete: {len(rows)} artifact(s)")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError, sqlite3.Error, subprocess.SubprocessError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
