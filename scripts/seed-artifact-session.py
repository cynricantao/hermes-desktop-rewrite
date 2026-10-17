#!/usr/bin/env python3
"""Seed one artifact-rich conversation into a PRIVATE live-test backend's session store.

    HERMES_HOME=$HOME/.cache/hermes-desktop-rs/live-home scripts/seed-artifact-session.py [profile]

Writes a synthetic session with the shapes the Artifacts page indexes: assistant prose with
`MEDIA:` tags, markdown images/links, bare URLs and absolute paths, plus tool results carrying
artifact keys (`terminal` output text, an image tool's `artifact_path`, a writer tool's
`path`). Real PNG/JPEG/SVG files are created under `$HERMES_LIVE_SCRATCH/artifact-fixtures`
so the image grid has thumbnails to show.

No model call is made; the rows go straight into the (isolated) state.db. Refuses to run
against the real ~/.hermes. Prints the new session id.
"""
import json
import os
import sqlite3
import sys
import time
from pathlib import Path

home = os.environ.get("HERMES_HOME", "")
real = Path.home() / ".hermes"
if not home or Path(home).resolve() == real.resolve():
    sys.exit("refusing: set HERMES_HOME to the private live-test home (not ~/.hermes)")
home_path = Path(home)
if not (home_path / "config.yaml").exists():
    sys.exit(f"refusing: {home} has no config.yaml; run scripts/live-test-backend.sh first")

profile = sys.argv[1] if len(sys.argv) > 1 else "default"
scratch = Path(os.environ.get("HERMES_LIVE_SCRATCH", str(Path(os.environ.get("TMPDIR", "/tmp")) / "hermes-live-test")))
fixtures = scratch / "artifact-fixtures" / profile
fixtures.mkdir(parents=True, exist_ok=True)

profile_home = home_path if profile == "default" else home_path / "profiles" / profile
if not profile_home.is_dir():
    sys.exit(f"refusing: no profile home at {profile_home}")

# --- fixture files -----------------------------------------------------------------------
def png(path: Path, w: int = 320, h: int = 200, rgb=(64, 128, 200)) -> None:
    import struct
    import zlib

    rows = b"".join(b"\x00" + bytes(rgb) * w for _ in range(h))
    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows, 6))
        + chunk(b"IEND", b"")
    )


def jpeg(path: Path, w: int = 320, h: int = 200, rgb=(200, 96, 64)) -> None:
    from PIL import Image

    Image.new("RGB", (w, h), rgb).save(path, "JPEG", quality=80)


def svg(path: Path) -> None:
    path.write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" width="240" height="120">'
        '<rect width="240" height="120" fill="#1f6feb"/>'
        '<text x="12" y="66" fill="#ffffff" font-size="20">fixture</text></svg>\n'
    )


render_a1 = fixtures / "final_A1.jpg"
render_a2 = fixtures / "final_A2.png"
diagram = fixtures / "diagram.svg"
term_shot = fixtures / "term-shot.png"
generated = fixtures / "generated.png"
notes = fixtures / "notes.md"
report = fixtures / "report.pdf"
jpeg(render_a1)
png(render_a2, rgb=(40, 160, 120))
svg(diagram)
png(term_shot, rgb=(180, 60, 60))
png(generated, rgb=(90, 90, 200))
notes.write_text("# Notes\n\nFixture notes for the Artifacts page.\n")
report.write_bytes(b"%PDF-1.4\n% fixture\n")

# --- session rows ------------------------------------------------------------------------
db_path = profile_home / "state.db"
if not db_path.exists():
    # A profile store is created lazily by its own backend, which a fresh live home never
    # starts. Clone the schema from the default store instead (same install, same version).
    source_db = home_path / "state.db"
    if not source_db.exists():
        sys.exit(f"refusing: {source_db} does not exist yet; seed the default profile first")
    src = sqlite3.connect(source_db)
    dst = sqlite3.connect(db_path)
    for (name, sql) in src.execute("select name, sql from sqlite_master where sql is not null and name not like 'sqlite_%'"):
        # FTS5 shadow tables are recreated by the virtual table itself.
        if any(part in name for part in ("_fts_data", "_fts_idx", "_fts_docsize", "_fts_config", "_fts_content")):
            continue
        try:
            dst.execute(sql)
        except sqlite3.OperationalError as e:
            print(f"skip schema object {name}: {e}", file=sys.stderr)
    dst.commit()
    dst.close()

sid = time.strftime("%Y%m%d_%H%M%S") + f"_{profile[:4]:<4}_{int(time.time() * 1000) % 1000:03d}"
now = time.time()
conn = sqlite3.connect(db_path)
conn.execute(
    "insert into sessions (id, source, created_source, model, started_at, message_count, title, title_source,"
    " last_activity_at, last_activity_description, profile_name, archived, hidden, pinned)"
    " values (?,?,?,?,?,?,?,?,?,?,?,0,0,0)",
    (sid, "desktop", "desktop", "test-model", now, 0, f"Artifact fixture ({profile})", "seed", now, "", profile),
)

order = 0


def add(role: str, content: object, tool_name: str | None = None) -> None:
    global order
    order += 1
    conn.execute(
        "insert into messages (session_id, role, content, tool_name, timestamp, active, compacted,"
        " _compressed_summary, observed, display_order, message_uid)"
        " values (?,?,?,?,?,1,0,0,0,?,?)",
        (sid, role, content if isinstance(content, str) else json.dumps(content), tool_name,
         now + order * 0.5, order, f"{sid}-{order}"),
    )


add("user", "Show me the renders and where they live.")
add(
    "assistant",
    "Here are the renders.\n\n"
    f"![final render]({render_a1})\n\n"
    f"MEDIA:{render_a2}\n\n"
    f"The diagram lives at {diagram} and the write-up at [notes]({notes}).\n\n"
    f"Reference: https://example.com/renders/final.html\n"
    f"Also grabbed a package: https://files.pythonhosted.org/packages/ab/cd/pkg-1.0-py3-none-any.whl.metadata\n",
)
add(
    "tool",
    {"output": f"wrote {generated}\nlink https://example.com/terminal/report.pdf\n", "exit_code": 0, "error": None},
    "terminal",
)
add("tool", {"artifact_path": str(term_shot), "bytes": 1024}, "image_generate")
add("tool", {"path": str(report), "created": True}, "files_write")
conn.execute("update sessions set message_count=? where id=?", (order, sid))
conn.commit()
conn.close()

print(sid)
