#!/usr/bin/env python3
"""Check the README's reduction table against a real run of the binary.

Every figure in that table is a byte count, and a byte count is easy to write
down as a character count by accident: the views contain multi-byte characters,
so `len(text)` and `len(bytes)` disagree by a few. This rebuilds each fixture,
runs the compressor, and compares the numbers the README claims. No model calls.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile


def fixtures() -> dict[str, bytes]:
    """The inputs behind the README rows, keyed by that row's leading label."""
    noise = "".join(f"progress {i:04d}: " + "unchanged " * 12 + "\n" for i in range(1000))
    rows = [
        {
            "id": i,
            "status": "failed" if i == 643 else "passed",
            "message": "error: audit-7139 amount=47" if i == 643 else "stable " * 12,
            "exact": 900719925474099312345,
            "nullable": None,
        }
        for i in range(1000)
    ]
    dumps = lambda value: json.dumps(value, separators=(",", ":"))

    jest = ["> app@1.4.2 test", "> jest --runInBand", ""]
    jest += [f"PASS  src/modules/module{i}.test.js" for i in range(240)]
    jest += [
        "FAIL  src/auth/session.test.js",
        "  ● session › refreshes an expiring token",
        "",
        "    expect(received).toBe(expected)",
        "",
        "    Expected: 1735689600",
        "    Received: 1735689599",
        "",
        "      at Object.<anonymous> (src/auth/session.test.js:42:31)",
    ]
    jest += [f"PASS  src/modules/module{i}.test.js" for i in range(240, 480)]
    jest += [
        "",
        "Test Suites: 1 failed, 480 passed, 481 total",
        "Tests:       1 failed, 1327 passed, 1328 total",
        "Time:        84.113 s",
    ]

    return {
        "480 passing tests, one failure": ("\n".join(jest) + "\n").encode(),
        "A retry loop hiding one fatal error": (
            "start\n" + "error: retry failed\nworking\n" * 1000 + "fatal error: rare root cause\nend\n"
        ).encode(),
        "1000 progress lines, then two diagnostics": (
            noise + "error: audit-7139 amount=47\nwarning: do NOT disable validation\n"
        ).encode(),
        "The same log with CRLF endings": (
            noise.replace("\n", "\r\n") + "error: original CRLF survives\r\n"
        ).encode(),
        "A receipt buried in the middle of a log": (
            noise[: len(noise) // 2]
            + "receipt audit-7139 amount=47 EUR"
            + " padding" * 20
            + "\n"
            + noise[len(noise) // 2 :]
        ).encode(),
        "A 1000-row JSON array": dumps(rows).encode(),
        "A 1000-row JSONL stream": "\n".join(dumps(row) for row in rows).encode(),
        "One 12 KB line with no newline": ("é" * 6000).encode(),
        "A 32-byte command output": b"No error: operation completed.\r\n",
    }


def claimed(readme: str) -> dict[str, tuple[int, int, str]]:
    """Parse the README rows as (input bytes, output bytes, kept)."""
    claims = {}
    for line in readme.splitlines():
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) != 4 or cells[0].startswith(("-", "What")):
            continue
        label, size_in, size_out, kept = cells
        digits = lambda text: text.replace(",", "")
        if not digits(size_in).isdigit() or not digits(size_out).isdigit():
            continue
        claims[label] = (int(digits(size_in)), int(digits(size_out)), kept.strip("*"))
    return claims


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--readme", type=Path, default=Path("README.md"))
    options = parser.parse_args()

    claims = claimed(options.readme.read_text())
    inputs = fixtures()
    missing = set(inputs) - set(claims)
    if missing:
        print(f"README is missing rows for: {sorted(missing)}", file=sys.stderr)
        return 1

    failures = []
    with tempfile.TemporaryDirectory(prefix="scopelet-readme-") as cache:
        for label, raw in inputs.items():
            result = subprocess.run(
                [str(options.binary.resolve()), "--cache-dir", cache, "compress"],
                input=raw,
                capture_output=True,
                timeout=120,
            )
            if result.returncode != 0:
                failures.append(f"{label}: exit {result.returncode}")
                continue
            # Byte counts, never character counts: views hold multi-byte text.
            actual_in, actual_out = len(raw), len(result.stdout)
            percent = 100 * (1 - actual_out / actual_in)
            want_in, want_out, want_kept = claims[label]
            kept = "untouched" if actual_out == actual_in else f"{percent:.1f}%"
            if (actual_in, actual_out) != (want_in, want_out) or kept != want_kept:
                failures.append(
                    f"{label}: README says {want_in} -> {want_out} ({want_kept}), "
                    f"measured {actual_in} -> {actual_out} ({kept})"
                )
            else:
                print(f"  ok  {label}: {actual_in} -> {actual_out} ({kept})")

    for failure in failures:
        print(f"  BAD {failure}", file=sys.stderr)
    if failures:
        print(f"\n{len(failures)} README figure(s) do not match the binary", file=sys.stderr)
        return 1
    print(f"\nREADME reduction table matches the binary ({len(inputs)} rows)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
