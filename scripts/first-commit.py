#!/usr/bin/env python3
"""Renderer-independent first-frame measurement.

Runs the app with WAYLAND_DEBUG=client and reports process start → the first
`wl_surface.commit` that follows a buffer `attach` (i.e. the first frame handed to the
compositor). Works for every Slint renderer, including the software renderer that has no
rendering notifier. Run inside a headless display:

  scripts/headless.sh scripts/first-commit.py <binary> [runs] [VAR=value ...]
"""

import os
import re
import subprocess
import sys
import threading
import time


def one(binary, extra_env):
    env = dict(os.environ, WAYLAND_DEBUG="client", RUST_LOG="warn", NO_COLOR="1", **extra_env)
    # libwayland stamps lines with the UTC wall clock as [HH:MM:SS.micros].
    start = time.time()
    day0 = start - (start % 86400)
    p = subprocess.Popen([binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    watchdog = threading.Timer(15, p.kill)  # never hang on a build that doesn't present
    watchdog.start()
    attached = False
    first = None
    assert p.stderr is not None
    for line in p.stderr:
        m = re.match(r"\[(\d+):(\d+):(\d+)\.(\d+)\](?:\s+\{[^}]*\})?\s+->\s+wl_surface#\d+\.(attach|commit)\((.*)\)", line)
        if m:
            h, mi, s, us = (int(x) for x in m.groups()[:4])
            t = day0 + h * 3600 + mi * 60 + s + us / 1e6
            if m.group(5) == "attach" and not m.group(6).startswith("nil"):
                attached = True
            elif m.group(5) == "commit" and attached:
                first = t
                break
    watchdog.cancel()
    p.kill()
    p.wait()
    if first is None:
        return None
    return (first - start) * 1000.0


def main():
    binary = sys.argv[1]
    runs = int(sys.argv[2]) if len(sys.argv) > 2 else 5
    extra = dict(kv.split("=", 1) for kv in sys.argv[3:])
    vals = []
    for _ in range(runs):
        try:
            v = one(binary, extra)
        except Exception as e:  # noqa: BLE001
            print("run failed:", e, file=sys.stderr)
            v = None
        vals.append(v)
        time.sleep(0.5)
    ok = sorted(v for v in vals if v is not None)
    print("first buffer commit (ms):", " ".join(f"{v:.1f}" if v is not None else "n/a" for v in vals))
    if ok:
        med = ok[len(ok) // 2] if len(ok) % 2 else (ok[len(ok) // 2 - 1] + ok[len(ok) // 2]) / 2
        print(f"median {med:.1f}  min {ok[0]:.1f}  max {ok[-1]:.1f}  (n={len(ok)})")


if __name__ == "__main__":
    main()
