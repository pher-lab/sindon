#!/usr/bin/env bash
#
# Gate: does the cosmic-text fork actually reach a downstream consumer?
#
# sindon's zeroize-first promise rests on a forked cosmic-text whose BufferLine
# wipes its plaintext on drop. That fork used to be applied with
# `[patch.crates-io]`, and Cargo only reads `[patch]` from the root workspace it
# is building. Every in-workspace build therefore resolved through the patch and
# stayed green, while anyone depending on sindon from crates.io silently got
# upstream cosmic-text — no zeroize, no trailing-line fix, no warning.
#
# The lesson is what this script encodes: whether a guarantee survives the trip
# downstream can only be measured from where downstream stands. `ci/downstream-gate`
# is an independent workspace root for exactly that reason, and this script asks
# the only question the in-workspace tests structurally cannot answer.
#
# Division of labour, so neither half is mistaken for the other:
#
#   * crates/sindon_text/tests/cosmic_residue.rs  — "is the fork correct?"
#     (runs in-workspace, scans for plaintext residue)
#   * this script                                 — "does the fork arrive?"
#     (runs from outside, inspects the resolved graph)
#
# Both are needed. A correct fork nobody receives is what we already shipped
# against once.
#
# Usage: ci/check-fork-propagation.sh   (from the repository root; needs bash +
# cargo, no jq — runs as-is in CI and in Git Bash on Windows.)

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
gate_manifest="$repo_root/ci/downstream-gate/Cargo.toml"

echo "Resolving the dependency graph as a downstream consumer sees it..."
echo "  manifest: $gate_manifest"

# `cargo metadata` resolves without compiling. Output is compact JSON, so a
# substring match on the package-name field is exact enough and keeps this
# dependency-free (no jq).
metadata="$(cargo metadata --format-version 1 --manifest-path "$gate_manifest")"

fork_present=0
upstream_present=0
grep -q '"name":"sindon-cosmic-text"' <<<"$metadata" && fork_present=1
# The leading quote matters: it is what keeps "sindon-cosmic-text" from
# matching here.
grep -q '"name":"cosmic-text"' <<<"$metadata" && upstream_present=1

failed=0

if [ "$fork_present" -eq 0 ]; then
  failed=1
  cat >&2 <<'EOF'

FAIL: the forked cosmic-text does not reach downstream.

  Package `sindon-cosmic-text` is absent from the graph that a downstream
  consumer of sindon resolves.

  This is the exact defect this gate exists for. Downstream builds will use
  upstream cosmic-text, whose BufferLine does NOT zeroize its plaintext on
  drop, so sindon's central promise — that secret text leaves no plaintext
  residue on the heap — silently stops holding for every consumer.

  It will not show up anywhere else: in-workspace tests, including the residue
  gate, resolve through the workspace manifest and stay green.

  Likely causes:
    * the dependency was reverted to a plain `cosmic-text = "0.18"`, or
    * the fork was put back behind `[patch.crates-io]` (which never propagates).

  Fix: depend on the fork directly, from the root Cargo.toml's
  [workspace.dependencies]:

    cosmic-text = { package = "sindon-cosmic-text", version = "0.1", path = "third_party/cosmic-text" }
EOF
fi

if [ "$upstream_present" -eq 1 ]; then
  failed=1
  cat >&2 <<'EOF'

FAIL: upstream cosmic-text is present in the downstream graph.

  Some crate depends on the unforked `cosmic-text`. Even if the fork is also
  present, this means two copies of the library are linked: any secret text
  shaped through the upstream copy leaves un-zeroed plaintext on the heap, and
  the two copies' types (FontSystem, Attrs, ...) are mutually incompatible.

  Find it with:
    cargo metadata --format-version 1 --manifest-path ci/downstream-gate/Cargo.toml \
      | tr ',' '\n' | grep -n 'cosmic-text'
EOF
fi

if [ "$failed" -eq 1 ]; then
  echo >&2
  exit 1
fi

echo "PASS: downstream resolves sindon-cosmic-text, and upstream cosmic-text is absent."
