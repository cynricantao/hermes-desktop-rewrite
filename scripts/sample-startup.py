#!/usr/bin/env python3
"""Poor-man's sampling profiler for the startup window (no perf/root needed).

Launches the app with PR_SET_PTRACER_ANY (so Yama ptrace_scope=1 lets a sibling attach),
then repeatedly snapshots the main thread with `eu-stack` until the process exits.
Aggregates frames across several runs and prints the hottest symbols per time bucket.

Must run inside a headless display (scripts/headless.sh). Use a symbolised build:
  cargo build --profile profiling -p hermes-app
  scripts/headless.sh scripts/sample-startup.py target/profiling/hermes-desktop-rs 6
"""

import collections
import ctypes
import os
import re
import subprocess
import sys
import time

PR_SET_PTRACER = 0x59616D61
PR_SET_PTRACER_ANY = -1 & 0xFFFFFFFFFFFFFFFF


def preexec():
    libc = ctypes.CDLL(None, use_errno=True)
    libc.prctl(PR_SET_PTRACER, ctypes.c_ulong(PR_SET_PTRACER_ANY), 0, 0, 0)


def sample(pid):
    r = subprocess.run(
        ["eu-stack", "-p", str(pid), "-1", "-d", "-m"],  # main thread, DWARF names, module
        capture_output=True,
        text=True,
        timeout=5,
    )
    frames = []
    for line in r.stdout.splitlines():
        m = re.match(r"#\d+\s+0x[0-9a-f]+\s+(?:- 1 )?(.*)", line.strip())
        if m:
            frames.append(m.group(1).split(" - ")[0].strip())
    return frames


def interesting(frames):
    """Pick the innermost frames from our stack that say *what* is happening."""
    keys = []
    for f in frames:
        short = re.sub(r"<.*?>", "", f)
        short = re.sub(r"\s+/.*$", "", short)
        keys.append(short[:110])
    return keys


def main():
    binary = sys.argv[1]
    runs = int(sys.argv[2]) if len(sys.argv) > 2 else 5
    env = dict(os.environ, HERMES_DESKTOP_EXIT_AFTER_FIRST_FRAME="1", RUST_LOG="startup=info,warn", NO_COLOR="1")
    buckets = collections.defaultdict(collections.Counter)  # 50ms bucket -> counter(symbol)
    leaf = collections.Counter()
    total = 0
    for _ in range(runs):
        t0 = time.monotonic()
        p = subprocess.Popen([binary], env=env, preexec_fn=preexec, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        while p.poll() is None:
            t = (time.monotonic() - t0) * 1000
            try:
                fr = sample(p.pid)
            except subprocess.TimeoutExpired:
                continue
            if not fr:
                continue
            total += 1
            keys = interesting(fr)
            b = int(t // 50) * 50
            # Count each distinct symbol once per sample (inclusive time).
            for k in set(keys[:60]):
                buckets[b][k] += 1
            leaf[keys[0]] += 1
        _, err_b = p.communicate()
        err = err_b.decode(errors="replace")
        ff = re.findall(r'phase="first frame" elapsed_ms=([0-9.]+)', err)
        print(f"run done; first frame {ff} ms (slowed by sampling)", file=sys.stderr)
    print(f"samples: {total}")
    print("\n== hottest leaf frames ==")
    for k, n in leaf.most_common(25):
        print(f"{n:5d}  {k}")
    print("\n== per 50ms bucket: top inclusive frames (filtered) ==")
    skip = re.compile(r"^(main|__libc_start|_start|std::|core::|<|call_once|__rust|clone|start_thread)")
    for b in sorted(buckets):
        c = buckets[b]
        n = max(c.values())
        tops = [(k, v) for k, v in c.most_common(400) if not skip.match(k)][:14]
        print(f"\n-- {b:4d}-{b + 50:4d} ms ({n} samples)")
        for k, v in tops:
            print(f"   {v:4d}  {k}")


if __name__ == "__main__":
    main()
