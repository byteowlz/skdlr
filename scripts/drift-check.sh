#!/usr/bin/env bash
#
# Drift check (tmpl-8adx): verify that the facts this repo documents
# (commands, crate/version references) match the machine-checked configuration
# (the actual Cargo.toml manifests and the build). Anything here is "source of
# truth" in AGENTS.md — if it fails, either the code or the docs drifted.
#
# Run after touching any manifest or versioned claim.

set -u
FAILED=0

echo "==> Required commands on PATH"
for cmd in cargo just trx python3 jq; do
  if command -v "$cmd" >/dev/null 2>&1; then
    echo "  ok: $cmd -> $(command -v "$cmd")"
  else
    echo "  FAIL: required command not found on PATH: $cmd"
    FAILED=1
  fi
done

echo "==> No YAML dependency (JSON/TOML application formats only)"
if command -v rg >/dev/null 2>&1 && rg -l 'serde_yaml|serde-yaml' --glob 'Cargo.toml' . >/dev/null 2>&1; then
  echo "  FAIL: found 'serde_yaml' in a Cargo.toml"
  rg -n 'serde_yaml|serde-yaml' --glob 'Cargo.toml' . || true
  FAILED=1
else
  echo "  ok: no serde_yaml dependency in any manifest"
fi

echo "==> Config crate feature set"
# skdlr intentionally keeps the `config` crate at default features (yaml is
# benign here and the user does not want it tightened); so this is informational
# rather than a hard gate. It still surfaces disabled-feature drift.
python3 - <<'PY'
import re
from pathlib import Path
text = Path('Cargo.toml').read_text()
m = re.search(r'^config\s*=\s*\{([^}]*)\}', text, re.M)
print("  manifest line: config = { %s }" % (m.group(1) if m else "?") if m else "  io: no explicit config dep in workspace manifest")
PY

echo "==> rmcp version references consistent with Cargo.toml"
python3 - <<'PY'
import re, sys
from pathlib import Path

root = Path('.')
ws = root / 'Cargo.toml'
text = ws.read_text()
m = re.search(r'^rmcp\s*=\s*\{[^}]*version\s*=\s*"([^"]+)"', text, re.M)
if m:
    actual = m.group(1)
    major = actual.split('.')[0]
    print(f"  actual rmcp version in Cargo.toml: {actual}")
    bad = []
    for md in list(root.rglob('*.md')):
        for line in md.read_text().splitlines():
            for mm in re.finditer(r'rmcp\s+(\d+)\.\d+', line, re.I):
                if mm.group(1) != major:
                    bad.append(f"{md}: {line.strip()}")
    if bad:
        print("  FAIL: documented rmcp version contradicts Cargo.toml:")
        for b in bad:
            print("    " + b)
        sys.exit(1)
    print("  ok: no stale rmcp version claims in docs")
    sys.exit(0)
else:
    print("  io: no explicit rmcp dep found (nothing to check)")
    sys.exit(0)
PY
RC=$?
FAILED=$((FAILED == 0 ? RC : FAILED))

echo "==> Workspace resolves (cargo metadata)"
if cargo metadata --no-deps --format-version 1 >/dev/null 2>&1; then
  echo "  ok: cargo metadata resolves"
else
  echo "  FAIL: cargo metadata --no-deps does not resolve"
  FAILED=1
fi

echo "==> Generated config examples are up to date"
if cargo test -q -p skdlr-core validate_examples_are_up_to_date >/dev/null 2>&1; then
  echo "  ok: examples/config.toml + config.schema.json are current"
else
  echo "  FAIL: examples are stale. Run 'just generate-config'."
  FAILED=1
fi

echo
if [[ "$FAILED" -eq 0 ]]; then
  echo "drift-check: PASS"
else
  echo "drift-check: FAIL"
  exit 1
fi