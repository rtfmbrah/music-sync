#!/usr/bin/env python3
"""Deterministic black-box acceptance for migrate-managed-webm.py."""

import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile


def write_executable(path: Path, body: str) -> None:
    path.write_text(f"#!/bin/sh\nset -eu\n{body}\n", encoding="utf-8")
    path.chmod(0o755)


def main() -> int:
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        library = root / "library" / "youtube"
        state = root / "state"
        library.mkdir(parents=True)
        state.mkdir()
        source = library / "fixture.webm"
        source.write_bytes(b"unchanged opus packets")
        database = state / "music-sync.sqlite3"
        connection = sqlite3.connect(database)
        connection.executescript(
            """
            CREATE TABLE recordings(id INTEGER PRIMARY KEY, preferred_artifact_id INTEGER);
            CREATE TABLE artifacts(id INTEGER PRIMARY KEY,recording_id INTEGER,path TEXT UNIQUE,sha256 TEXT,duration_ms INTEGER,codec TEXT,sample_rate_hz INTEGER,channels INTEGER,health TEXT);
            CREATE TABLE acquisition_commits(job_id INTEGER PRIMARY KEY,final_path TEXT UNIQUE,sha256 TEXT,bytes INTEGER,codec TEXT,duration_ms INTEGER,sample_rate_hz INTEGER,channels INTEGER,status TEXT);
            CREATE TABLE artifact_fingerprints(artifact_id INTEGER PRIMARY KEY,algorithm INTEGER,max_seconds INTEGER,duration_ms INTEGER,fingerprint_json TEXT,value_count INTEGER,updated_at TEXT);
            CREATE TABLE events(id INTEGER PRIMARY KEY,level TEXT,component TEXT,event TEXT,message TEXT,context_json TEXT,created_at TEXT DEFAULT CURRENT_TIMESTAMP);
            INSERT INTO recordings VALUES(1,1);
            INSERT INTO artifacts VALUES(1,1,'PLACEHOLDER',NULL,10000,'opus',48000,2,'healthy');
            INSERT INTO acquisition_commits VALUES(7,'PLACEHOLDER',NULL,21,'opus',10000,48000,2,'committed');
            INSERT INTO artifact_fingerprints VALUES(1,2,120,10000,'[1,2,3]',3,CURRENT_TIMESTAMP);
            """.replace("PLACEHOLDER", str(source))
        )
        connection.commit()
        connection.close()
        ffprobe = root / "ffprobe"
        ffmpeg = root / "ffmpeg"
        write_executable(ffprobe, "printf '%s\\n' '{\"streams\":[{\"codec_name\":\"opus\",\"codec_type\":\"audio\",\"sample_rate\":\"48000\",\"channels\":2}],\"format\":{\"duration\":\"10.000\"}}'")
        write_executable(ffmpeg, 'source=""; previous=""; for argument in "$@"; do if test "$previous" = -i; then source=$argument; fi; previous=$argument; destination=$argument; done; cp "$source" "$destination"')
        migrator = Path(__file__).with_name("migrate-managed-webm.py")
        subprocess.run([str(migrator), "--database", str(database), "--library-root", str(library), "--state-directory", str(state), "--ffmpeg", str(ffmpeg), "--ffprobe", str(ffprobe), "--apply"], check=True)
        destination = library / "fixture.opus"
        assert source.read_bytes() == b"unchanged opus packets"
        assert destination.read_bytes() == source.read_bytes()
        assert os.stat(destination).st_mode & 0o777 == 0o644
        connection = sqlite3.connect(database)
        assert connection.execute("SELECT preferred_artifact_id FROM recordings").fetchone()[0] == 2
        assert connection.execute("SELECT final_path FROM acquisition_commits").fetchone()[0] == str(destination)
        assert connection.execute("SELECT COUNT(*) FROM artifact_fingerprints WHERE artifact_id=2").fetchone()[0] == 1
        assert connection.execute("SELECT COUNT(*) FROM events WHERE event='managed_webm_remux'").fetchone()[0] == 1
    print("managed WebM migration acceptance passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
