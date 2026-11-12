#!/usr/bin/env python3
"""Validate actual Rust payloads against the installed CLI, without login/model calls.
Requires: python3 -m pip install jsonschema (optional development dependency).
Usage: python3 scripts/check_protocol.py [path/to/codex]
"""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import jsonschema

root = Path(__file__).resolve().parents[1]
codex = sys.argv[1] if len(sys.argv) > 1 else "codex"
subprocess.run([codex, "--version"], check=True)
with tempfile.TemporaryDirectory(prefix="coding-agent-schema-") as directory:
    subprocess.run([codex, "app-server", "generate-json-schema", "--out", directory], check=True)
    samples = json.loads(subprocess.check_output([
        "cargo", "run", "--quiet", "--locked", "--no-default-features", "--example", "protocol_samples"
    ], cwd=root))
    for sample in samples:
        schema = json.loads((Path(directory) / sample["schema"]).read_text())
        jsonschema.Draft7Validator(schema).validate(sample["value"])
    print(f"PASS: {len(samples)} actual Rust payloads match generated schemas")
