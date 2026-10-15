# gdb Python: wall-clock duration of startup-relevant native calls (entry → return).
# Used for docs/PERF.md. Headless display only:
#   scripts/headless.sh gdb -q -batch -x scripts/gdb-startup-trace.py \
#       --args target/profiling/hermes-desktop-rs
# Each breakpoint costs a ptrace round-trip, so absolute times run a few ms slow.
import time

import gdb

T0 = time.monotonic()
FUNCS = [
    # fonts
    "FcInitLoadConfigAndFonts",
    "FcInitLoadConfig",
    "FcFontSort",
    "<fontique::collection::Collection>::register_fonts",
    "i_slint_common::sharedfontique::create_collection",
    # GL / EGL
    "eglGetPlatformDisplay",
    "eglInitialize",
    "eglChooseConfig",
    "eglCreateContext",
    "eglCreatePlatformWindowSurface",
    "eglMakeCurrent",
    "eglSwapBuffers",
]
MAX_HITS = 4
hits = {}


def now():
    return (time.monotonic() - T0) * 1000.0


class Ret(gdb.FinishBreakpoint):
    def __init__(self, frame, name, start):
        super().__init__(frame, internal=True)
        self.name, self.start = name, start

    def stop(self):
        t = now()
        print(f"TRACE {self.start:8.1f} → {t:8.1f} ms  ({t - self.start:6.1f})  {self.name}", flush=True)
        return False

    def out_of_scope(self):
        pass


class Entry(gdb.Breakpoint):
    def stop(self):
        name = self.location
        hits[name] = hits.get(name, 0) + 1
        if hits[name] >= MAX_HITS:
            self.enabled = False
        try:
            Ret(gdb.newest_frame(), name, now())
        except (ValueError, gdb.error):
            print(f"TRACE {now():8.1f} ms  (entry only) {name}", flush=True)
        return False


gdb.execute("set pagination off")
gdb.execute("set breakpoint pending on")
gdb.execute("set print thread-events off")
gdb.execute("set startup-with-shell off")
for f in FUNCS:
    try:
        Entry(f, internal=True)
    except gdb.error as e:
        print(f"(no symbol {f}: {e})")
T0 = time.monotonic()
gdb.execute("run")
