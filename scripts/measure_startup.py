#!/usr/bin/env python3
"""Measure repeated M1 code-entry-to-bind and parent-observed bind times.

This does not measure Minecraft readiness or guarantee a cold OS page cache.
"""

import argparse
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import time


def request_cache_drop(path: Path) -> bool:
    if not hasattr(os, "posix_fadvise") or not hasattr(os, "POSIX_FADV_DONTNEED"):
        return False
    with path.open("rb") as binary:
        os.posix_fadvise(binary.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
    return True


def measure(binary: Path, config: Path) -> tuple[float, float]:
    started = time.perf_counter_ns()
    process = subprocess.Popen(
        [str(binary), "--run", str(config)],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        bufsize=0,
    )
    try:
        buffer = b""
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            remaining = deadline - time.monotonic()
            ready, _, _ = select.select([process.stdout], [], [], max(0, remaining))
            if not ready:
                break
            chunk = os.read(process.stdout.fileno(), 4096)
            if not chunk:
                break
            buffer += chunk
            while b"\n" in buffer:
                raw_line, buffer = buffer.split(b"\n", 1)
                line = raw_line.decode("utf-8", "replace")
                if "event=listener_bound " in line:
                    observed_ms = (time.perf_counter_ns() - started) / 1_000_000
                    fields = dict(field.split("=", 1) for field in line.split() if "=" in field)
                    process.send_signal(signal.SIGTERM)
                    stdout, stderr = process.communicate(timeout=5)
                    if process.returncode != 0 or b"event=stopped " not in stdout:
                        raise RuntimeError(f"unclean shutdown: code={process.returncode}, stderr={stderr.decode(errors='replace')}")
                    return int(fields["elapsed_us"]) / 1000, observed_ms
        raise RuntimeError("listener_bound event not observed within five seconds")
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/rustmc-server"))
    parser.add_argument("--config", type=Path, default=Path("config/rustmc.example.toml"))
    parser.add_argument("--pairs", type=int, default=5)
    args = parser.parse_args()
    if args.pairs < 1:
        parser.error("--pairs must be positive")
    binary = args.binary.resolve()
    config = args.config.resolve()
    if not binary.is_file() or not config.is_file():
        parser.error("build the binary and provide an existing configuration file first")
    print("cycle,mode,cache_drop_requested,code_entry_to_bound_ms,parent_spawn_to_bound_ms")
    for cycle in range(1, args.pairs + 1):
        cache_hint = request_cache_drop(binary)
        for mode in ["cold_hint", "warm_repeat"]:
            internal_ms, observed_ms = measure(binary, config)
            print(f"{cycle},{mode},{str(cache_hint if mode == 'cold_hint' else False).lower()},{internal_ms:.3f},{observed_ms:.3f}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
