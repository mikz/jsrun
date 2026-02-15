#!/usr/bin/env bash
set -euo pipefail

V8_VERSION="${V8_VERSION:-142.1.0}"
SOURCE_FILE="/project/.github/v8/known-target-triples.txt"

if [[ ! -f "${SOURCE_FILE}" ]]; then
  echo "ERROR: Missing ${SOURCE_FILE}" >&2
  exit 1
fi

# Ensure the v8 crate is unpacked into the cargo registry before patching.
cargo fetch -p v8 >/dev/null 2>&1 || cargo fetch >/dev/null 2>&1

cargo_home="${CARGO_HOME:-${HOME}/.cargo}"
v8_dir="$(ls -d "${cargo_home}/registry/src/"*/v8-"${V8_VERSION}" 2>/dev/null | head -n 1 || true)"
if [[ -z "${v8_dir}" ]]; then
  echo "ERROR: Could not locate v8-${V8_VERSION} in ${cargo_home}/registry/src" >&2
  ls -la "${cargo_home}/registry/src" || true
  exit 1
fi

target_dir="${v8_dir}/build/rust"
mkdir -p "${target_dir}"

dest="${target_dir}/known-target-triples.txt"
if [[ ! -f "${dest}" ]]; then
  cp "${SOURCE_FILE}" "${dest}"
fi
