#!/usr/bin/env python3
"""Generate docs/parity/matrix.md from docs/PARITY.md (the source of truth).

    scripts/parity-matrix.py            # regenerate the matrix
    scripts/parity-matrix.py --check    # exit 1 if the matrix is stale

PARITY.md owns the feature list and the rewrite status markers. This script adds:

* a stable ID per item (`docs/parity/ids.json`; IDs are frozen once assigned, new items get
  the next free number in their section, removed items keep their ID retired);
* the columns PARITY.md does not carry, from `docs/parity/annotations/*.json`
  (`{ "<ID>": {"behavior": "...", "states": "...", "screens": "...", "test": "...",
  "rust_evidence": "...", "unsure": "..."} }`).

Rust status comes only from the PARITY.md marker (`[ ]` missing, `[~]` partial, `[x]` done,
`[-]` unsupported), so the matrix can never claim more than PARITY.md does.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PARITY = ROOT / "docs" / "PARITY.md"
OUT_DIR = ROOT / "docs" / "parity"
IDS = OUT_DIR / "ids.json"
ANN_DIR = OUT_DIR / "annotations"
MATRIX = OUT_DIR / "matrix.md"

SECTION_CODES = {
    "Chat & composer": "CHAT",
    "Transcript rendering": "TRNS",
    "Tools / terminal rows": "TOOL",
    "Approvals / clarify": "APRV",
    "Sessions & sidebar": "SESS",
    "Projects / workspaces": "PROJ",
    "Models & providers": "MODL",
    "Profiles": "PROF",
    "Skills": "SKIL",
    "Tools / toolsets": "TSET",
    "MCP": "MCP",
    "Plugins / connectors": "PLUG",
    "Memory": "MEM",
    "Messaging gateways": "MSG",
    "Scheduled / cron": "CRON",
    "Artifacts / preview": "ART",
    "Right panel (files / review / terminal)": "PANE",
    "Voice": "VOIC",
    "Command palette & shortcuts": "KEYS",
    "Settings (shell)": "SET",
    "Settings — Model page": "SMOD",
    "Settings — Chat page": "SCHT",
    "Settings — Appearance page": "SAPP",
    "Settings — Workspace page": "SWRK",
    "Settings — Safety page": "SSAF",
    "Settings — Browser page": "SBRW",
    "Settings — Memory & Context page": "SMEM",
    "Settings — Voice page": "SVOI",
    "Settings — Advanced page": "SADV",
    "Settings — Gateway / Connections page": "SGTW",
    "Settings — Keybinds page": "SKEY",
    "Settings — Notifications page": "SNOT",
    "Settings — Sessions page": "SSES",
    "Settings — Vault page": "SVLT",
    "Settings — Providers & Keys page": "SPRV",
    "Settings — Plugins page": "SPLG",
    "Settings — Billing page": "SBIL",
    "Settings — About page": "SABT",
    "Appearance / themes": "THEM",
    "Notifications": "NOTI",
    "Updates": "UPD",
    "Window / tray / HUD / pet": "WIN",
    "Onboarding / auth / billing": "ONB",
    "Remote connections": "RMT",
    "Extras: webhooks, imports, subagents, learning, telemetry": "EXTR",
}

STATUS = {" ": "missing", "~": "partial", "x": "done", "-": "unsupported"}
ITEM = re.compile(r"^- \[([ x~-])\] (.+)$")


def parse() -> list[dict]:
    items, section = [], None
    for line in PARITY.read_text().splitlines():
        if line.startswith("## "):
            section = line[3:].strip()
            continue
        m = ITEM.match(line)
        if not m or section not in SECTION_CODES:
            continue
        body, _, note = m.group(2).partition(" ⟶ ")
        parts = [p.strip() for p in body.split(" — ")]
        feature = parts[0]
        surface = parts[1] if len(parts) > 1 else ""
        src = " — ".join(parts[2:]) if len(parts) > 2 else ""
        items.append(
            {
                "section": section,
                "feature": feature,
                "surface": surface,
                "source": src,
                "status": STATUS[m.group(1)],
                "note": note.strip(),
            }
        )
    return items


def assign_ids(items: list[dict]) -> dict:
    ids = json.loads(IDS.read_text()) if IDS.exists() else {}
    # key = section + feature text; value = ID
    used = set(ids.values())
    for it in items:
        key = f"{it['section']}::{it['feature']}"
        if key not in ids:
            code = SECTION_CODES[it["section"]]
            n = 1
            while f"{code}-{n:03d}" in used:
                n += 1
            ids[key] = f"{code}-{n:03d}"
            used.add(ids[key])
        it["id"] = ids[key]
    dupes = {v for v in ids.values() if list(ids.values()).count(v) > 1}
    if dupes:
        sys.exit(f"duplicate IDs: {sorted(dupes)}")
    return ids


def load_annotations() -> dict:
    ann: dict = {}
    for f in sorted(ANN_DIR.glob("*.json")) if ANN_DIR.exists() else []:
        for k, v in json.loads(f.read_text()).items():
            if k.startswith("_"):
                continue
            ann.setdefault(k, {}).update(v)
    return ann


def cell(s: str) -> str:
    return (s or "").replace("|", "\\|").replace("\n", " ").strip() or "—"


def render(items: list[dict], ann: dict) -> str:
    counts: dict[str, int] = {}
    for it in items:
        counts[it["status"]] = counts.get(it["status"], 0) + 1
    annotated = sum(1 for it in items if ann.get(it["id"], {}).get("behavior"))
    out = [
        "# Hermes Desktop parity matrix",
        "",
        "Generated by `scripts/parity-matrix.py` from [`docs/PARITY.md`](../PARITY.md) "
        "(source of truth for the feature list and status) plus `docs/parity/annotations/*.json`. "
        "Do not edit by hand — edit PARITY.md or an annotation file and regenerate.",
        "",
        f"**{len(items)} rows** · done {counts.get('done', 0)} · partial {counts.get('partial', 0)} · "
        f"missing {counts.get('missing', 0)} · unsupported {counts.get('unsupported', 0)} · "
        f"behavior annotated {annotated}",
        "",
        "Columns: **Behavior** is `trigger → result`, written from the Electron source. "
        "**States** lists the visual/interaction states the element has in the original. "
        "**Screens** are files under `docs/parity/screens/`. **Unsure** flags anything the "
        "analyst could not confirm from source or observation.",
        "",
    ]
    section = None
    for it in items:
        if it["section"] != section:
            section = it["section"]
            out += [
                "",
                f"## {section}",
                "",
                "| ID | Element / setting | Behavior (trigger → result) | Data source / IPC / API / config | States | Screens | Test | Rust status | Unsure |",
                "|---|---|---|---|---|---|---|---|---|",
            ]
        a = ann.get(it["id"], {})
        rust = it["status"]
        if it["note"]:
            rust += f": {it['note']}"
        elif a.get("rust_evidence"):
            rust += f" (code: {a['rust_evidence']})"
        src = it["surface"] + (f" · `{it['source']}`" if it["source"] else "")
        out.append(
            "| "
            + " | ".join(
                cell(x)
                for x in (
                    it["id"],
                    it["feature"],
                    a.get("behavior", ""),
                    src,
                    " · ".join(
                        x
                        for x in (
                            a.get("states", ""),
                            f"seen: {a['visual']}" if a.get("visual") else "",
                        )
                        if x
                    ),
                    a.get("screens", ""),
                    a.get("test", ""),
                    rust,
                    a.get("unsure", ""),
                )
            )
            + " |"
        )
    return "\n".join(out) + "\n"


def main() -> None:
    items = parse()
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    ids = assign_ids(items)
    text = render(items, load_annotations())
    if "--check" in sys.argv:
        if not MATRIX.exists() or MATRIX.read_text() != text:
            sys.exit("docs/parity/matrix.md is stale; run scripts/parity-matrix.py")
        return
    IDS.write_text(json.dumps(ids, indent=1, ensure_ascii=False) + "\n")
    MATRIX.write_text(text)
    # A flat list the annotators work from.
    (OUT_DIR / "items.json").write_text(
        json.dumps(items, indent=1, ensure_ascii=False) + "\n"
    )
    print(f"{len(items)} rows -> {MATRIX.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
