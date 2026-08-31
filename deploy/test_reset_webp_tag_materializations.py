#!/usr/bin/env python3
"""Deterministic tests for WebP-cover tag rematerialization release."""

import hashlib
import sqlite3
import subprocess
import tempfile
from pathlib import Path
import unittest


SCRIPT = Path(__file__).with_name("reset-webp-tag-materializations.py")


SCHEMA = """
PRAGMA user_version=20;
CREATE TABLE recordings(id INTEGER PRIMARY KEY, preferred_artifact_id INTEGER);
CREATE TABLE artifacts(id INTEGER PRIMARY KEY, recording_id INTEGER, path TEXT,
 sha256 TEXT, health TEXT);
CREATE TABLE artwork_blobs(id INTEGER PRIMARY KEY, mime_type TEXT);
CREATE TABLE recording_provider_artwork(recording_id INTEGER PRIMARY KEY, blob_id INTEGER);
CREATE TABLE acquisition_commits(final_path TEXT, status TEXT);
CREATE TABLE repair_commits(final_path TEXT, committed_at TEXT);
CREATE TABLE metadata_materializations(
 recording_id INTEGER PRIMARY KEY, source_artifact_id INTEGER, result_artifact_id INTEGER,
 source_path TEXT, source_sha256 TEXT, history_path TEXT, staged_path TEXT,
 final_path TEXT, result_sha256 TEXT, result_bytes INTEGER, codec TEXT,
 duration_ms INTEGER, sample_rate_hz INTEGER, channels INTEGER,
 canonical_snapshot_json TEXT, state TEXT, message TEXT, prepared_at TEXT,
 committed_at TEXT, updated_at TEXT);
CREATE TABLE events(id INTEGER PRIMARY KEY, level TEXT, component TEXT, event TEXT,
 message TEXT, context_json TEXT);
"""


class ResetWebpTagMaterializationsTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.library = root / "music"
        self.library.mkdir()
        self.database = root / "state.sqlite3"
        self.audio = self.library / "track.opus"
        self.audio.write_bytes(b"owned tagged audio")
        sha256 = hashlib.sha256(self.audio.read_bytes()).hexdigest()
        connection = sqlite3.connect(self.database)
        connection.executescript(SCHEMA)
        connection.execute("INSERT INTO recordings VALUES (1,10)")
        connection.execute(
            "INSERT INTO artifacts VALUES (10,1,?1,?2,'healthy')",
            (str(self.audio), sha256),
        )
        connection.execute("INSERT INTO artwork_blobs VALUES (2,'image/webp')")
        connection.execute("INSERT INTO recording_provider_artwork VALUES (1,2)")
        connection.execute(
            "INSERT INTO acquisition_commits VALUES (?1,'committed')", (str(self.audio),)
        )
        connection.execute(
            """INSERT INTO metadata_materializations VALUES
            (1,9,10,'/history/source',?1,'/history/source','/staged/output',?2,?1,
             18,'opus',1000,48000,2,'{"title":"Track"}','committed','done',
             'now','now','now')""",
            (sha256, str(self.audio)),
        )
        connection.commit()
        connection.close()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def run_script(self, *extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                "python3",
                str(SCRIPT),
                "--database",
                str(self.database),
                "--library-root",
                str(self.library),
                *extra,
            ],
            check=True,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )

    def test_dry_run_is_read_only(self) -> None:
        before = self.database.read_bytes()
        result = self.run_script()
        self.assertIn("eligible WebP tag materializations: 1", result.stdout)
        self.assertEqual(self.database.read_bytes(), before)
        self.assertEqual(self.audio.read_bytes(), b"owned tagged audio")

    def test_apply_releases_only_state_and_keeps_audit(self) -> None:
        result = self.run_script("--apply")
        self.assertIn("released materializations: 1", result.stdout)
        connection = sqlite3.connect(self.database)
        self.assertEqual(
            connection.execute("SELECT COUNT(*) FROM metadata_materializations").fetchone()[0],
            0,
        )
        event = connection.execute(
            "SELECT event,context_json FROM events"
        ).fetchone()
        self.assertEqual(event[0], "webp_tags_released")
        self.assertIn('"result_artifact_id": 10', event[1])
        connection.close()
        self.assertEqual(self.audio.read_bytes(), b"owned tagged audio")

        # A later committed row represents the compatibility rematerialization. The
        # audit event makes the maintenance release strictly one-shot.
        connection = sqlite3.connect(self.database)
        sha256 = hashlib.sha256(self.audio.read_bytes()).hexdigest()
        connection.execute(
            """INSERT INTO metadata_materializations VALUES
            (1,10,10,'/history/previous',?1,'/history/previous','/staged/next',?2,?1,
             18,'opus',1000,48000,2,'{"title":"Track"}','committed','done',
             'later','later','later')""",
            (sha256, str(self.audio)),
        )
        connection.commit()
        connection.close()
        repeat = self.run_script()
        self.assertIn("eligible WebP tag materializations: 0", repeat.stdout)

    def test_changed_audio_is_rejected_without_database_mutation(self) -> None:
        self.audio.write_bytes(b"changed")
        result = subprocess.run(
            [
                "python3",
                str(SCRIPT),
                "--database",
                str(self.database),
                "--library-root",
                str(self.library),
                "--apply",
            ],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.assertNotEqual(result.returncode, 0)
        connection = sqlite3.connect(self.database)
        self.assertEqual(
            connection.execute("SELECT COUNT(*) FROM metadata_materializations").fetchone()[0],
            1,
        )
        self.assertEqual(connection.execute("SELECT COUNT(*) FROM events").fetchone()[0], 0)
        connection.close()


if __name__ == "__main__":
    unittest.main()
