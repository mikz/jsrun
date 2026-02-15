#!/usr/bin/env bash
set -euo pipefail

manifest="${MANIFEST_PATH:-/project/Cargo.toml}"

if [[ ! -f "${manifest}" ]]; then
  echo "ERROR: Cargo.toml not found at ${manifest}" >&2
  exit 1
fi

if grep -Eq '^\[patch\.crates-io\]\s*$' "${manifest}"; then
  if grep -Eq '^v8\s*=\s*\{\s*git\s*=\s*"https://github.com/denoland/rusty_v8"' "${manifest}"; then
    echo "v8 patch already present in ${manifest}"
    exit 0
  fi
  echo "ERROR: ${manifest} already contains a [patch.crates-io] section (not managed by this script)." >&2
  exit 1
fi

cat >>"${manifest}" <<'EOF'

# CI-only: The crates.io `v8` tarball is missing files needed for `V8_FROM_SOURCE=1`
# builds on Linux (required for manylinux wheels). Patch to upstream git tag.
[patch.crates-io]
v8 = { git = "https://github.com/denoland/rusty_v8", tag = "v142.1.0" }
EOF

echo "Patched v8 to git source in ${manifest}"

