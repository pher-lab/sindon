#!/usr/bin/env bash
# Does a sindon binary survive being run on macOS?
#
# The criterion is the one the Linux first-run used: the process is still alive
# after a fixed wait, and it wrote nothing to stderr. Neither half is redundant
# — a window that never appears can still keep a process alive, and a process
# that dies at second 12 looks healthy to anyone who only checks that it
# started. macOS has no coreutils `timeout`, so the wait is done by hand.
#
# Read the result together with the control that runs before it: this script
# cannot tell "sindon is broken" from "this runner has no window server", and
# the control is what separates them.

set -uo pipefail

bin=${1:?usage: macos-run-smoke.sh <binary> [seconds]}
seconds=${2:-15}

if [[ ! -x "$bin" ]]; then
    echo "smoke: $bin is not executable — did the build step run?" >&2
    exit 1
fi

out=$(mktemp)
err=$(mktemp)

"$bin" >"$out" 2>"$err" &
pid=$!
sleep "$seconds"

verdict=alive
if kill -0 "$pid" 2>/dev/null; then
    kill "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null
else
    wait "$pid"
    verdict="exited early (status $?)"
fi

echo "smoke: $bin — $verdict after ${seconds}s"
echo "--- stdout ---"
cat "$out"
echo "--- stderr ---"
cat "$err"

rc=0
reason=""
if [[ "$verdict" != alive ]]; then
    rc=1
    reason="$verdict"
fi
if [[ -s "$err" ]]; then
    echo "smoke: stderr was not empty"
    rc=1
    reason="${reason:+$reason, }wrote to stderr"
fi

# The step that runs this is `continue-on-error`, and under that setting a
# failed step is still reported with `conclusion: success` by the API -- only
# the separate `outcome` field says otherwise. Anyone reading the job's step
# list, this script's author included, would see green. So say it out loud in
# the run summary, the way the residue job does for an absorbed flake, and
# still exit non-zero so the step itself is marked in the UI.
if [[ $rc -ne 0 ]]; then
    echo "::warning title=macOS run smoke failed::$bin: $reason. Read the control step first — if the control also failed, this says nothing about sindon."
fi
exit "$rc"
