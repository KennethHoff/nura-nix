# End-to-end test of nura-install: the checks of this computer.
# Usage: nura-install-test.sh <test-hooks binary> <shipped binary> <the shipped binary's cache dir>
set -euo pipefail
umask 077
# The wrapper puts coreutils first on PATH, which would hide the fake df below; the unwrapped binary
# takes this script's PATH.
tool=$(dirname "$1")/.nura-install-wrapped
shipped=$2
shipped_cache=$3
root=$(mktemp -d)
bash_bin=$(command -v bash)
real_df=$(command -v df)
export REAL_DF=$real_df
cache=$root/cache

fail() {
  echo "FAIL: $1" >&2
  echo "--- output" >&2
  cat "$root/out" >&2 || true
  exit 1
}

mkdir -p "$root/bin"

# Fake df: the real one, after FAKE_SLOW_DF seconds (not for its version probe); FAKE_DF=fail makes it exit 1.
cat > "$root/bin/df" <<'EOS'
[ "${FAKE_DF:-}" != fail ] || exit 1
[ "$1" = --version ] || sleep "${FAKE_SLOW_DF:-0}"
exec "$REAL_DF" "$@"
EOS
sed -i "1i #!$bash_bin" "$root/bin/df"
chmod +x "$root/bin/df"
export PATH=$root/bin:$PATH

export NURA_INSTALL_CACHE=$cache NURA_INSTALL_NEEDED_GIB=0

fresh_cache() { rm -rf "$cache"; }
run() { # stdin, args...
  local input=$1
  shift
  rc=0
  printf '%b' "$input" | "$tool" "$@" > "$root/out" 2>&1 || rc=$?
}
refused() { # name, expected message, expected rc (default 1)
  [ "$rc" = "${3:-1}" ] || fail "$1 exited $rc"
  grep -q -- "$2" "$root/out" || fail "$1 not reported as: $2"
}

# Terminal behaviour helpers.
plain_output() { # name, file: no carriage return or escape byte
  [ -z "$(tr -d -c '\r\033' < "$2")" ] || fail "$1: the output has a carriage return or escape byte"
}
every_wait_is_named() { # name, file: each result line has a naming line right before it and ends in a duration
  awk -v name="$1" '
    /^  \[( ok |FAIL)\] / {
      if (prev !~ /^  \.\.\. /) bad = bad "\n  no naming line before: " $0
      if ($0 !~ /([0-9]+\.[0-9] s|[0-9]+ s|[0-9]+:[0-9][0-9]|[0-9]+:[0-9][0-9]:[0-9][0-9])$/) bad = bad "\n  no duration: " $0
      results++
    }
    { prev = $0 }
    END { if (bad != "" || results == 0) { print name ": " results " results" bad; exit 1 } }' "$2" || fail "$1: a wait was not named before it ran or has no duration"
}
step_blocks() { # name, file: every header has its block: a Takes line and a Does line, no Do line
  awk -v name="$1" '
    function check() {
      if (cur == "") return
      if (!hasdoes || hasdo) bad = bad "\n  step needs Does: and no Do:: " cur
      if (!takes) bad = bad "\n  step lacks Takes:: " cur
      steps++
    }
    /^== Step / { check(); cur = $0; hasdo = 0; hasdoes = 0; takes = 0; first = 1; next }
    cur != "" && first { first = 0; if ($0 !~ /^  (Takes|Does|Phone):/) bad = bad "\n  no block under: " cur }
    /^  Do:/ { hasdo = 1 }
    /^  Does:/ { hasdoes = 1 }
    /^  Takes:/ { takes = 1 }
    END { check(); if (bad != "" || steps == 0) { print name ": " steps " steps" bad; exit 1 } }' "$2" || fail "$1: a step header lacks its announcement block"
}
wait_for() { # file, text: up to 20 s
  local i
  for i in $(seq 200); do
    grep -qF -- "$2" "$1" 2> /dev/null && return 0
    sleep 0.1
  done
  echo "--- $1" >&2; tail -c 1500 "$1" >&2 || true
  fail "never saw: $2"
}
sections_of() { # file: the sections and numbered steps in the order they appear, one line
  grep -E '^(=== |== Step )' "$1" | sed -E 's/^=== (.*)$/[\1]/; s/^== Step ([0-9]+) of ([0-9]+) · TOOL: .*$/\1\/\2/' | tr '\n' ' ' | sed 's/ $//'
}

## Shipped binary: help, usage errors, and exported overrides that do nothing

rc=0; "$shipped" --help > "$root/out" 2>&1 || rc=$?
[ "$rc" = 0 ] || fail "shipped --help exited $rc"
grep -q 'Exit codes' "$root/out" || fail "--help lacks the exit codes"
for bad in "--os nura" "--os" "nura" "--rollback" "--help --os"; do
  rc=0; "$shipped" $bad > "$root/out" 2>&1 || rc=$?
  [ "$rc" = 2 ] || fail "shipped '$bad' exited $rc"
done
mkdir -p "$root/home"
export HOME=$root/home
# An exported NURA_INSTALL_* variable changes nothing in the shipped binary: it keeps its own cache and
# its own disk limit (12 GiB), which the machine running this check may or may not have free.
export NURA_INSTALL_CACHE=$root/shipped-fake/cache NURA_INSTALL_NEEDED_GIB=9999
rc=0; "$shipped" > "$root/out" 2>&1 < /dev/null || rc=$?
case $rc in
  0) grep -q '^done in [0-9.]* s: finished$' "$root/out" || fail "the shipped run did not finish: $(cat "$root/out")" ;;
  1) grep -q 'the run needs about 12 GiB' "$root/out" || fail "the shipped run stopped for another reason (an exported override was honoured?): $(cat "$root/out")" ;;
  *) fail "the shipped binary exited $rc" ;;
esac
[ ! -e "$root/shipped-fake/cache" ] || fail "the shipped binary used an exported NURA_INSTALL_CACHE"
ls "$shipped_cache/logs"/*-run.log > /dev/null || fail "the shipped run left no transcript in its own cache"
rm -rf "$shipped_cache"
export NURA_INSTALL_CACHE=$cache NURA_INSTALL_NEEDED_GIB=0

## Check this computer

fresh_cache
run ''
[ "$rc" = 0 ] || fail "the run exited $rc"
cp "$root/out" "$root/out.run"
plain_output "run" "$root/out.run"
! LC_ALL=C grep -aq -P '\x1b\]2;' "$root/out.run" || fail "a title escape reached a pipe"
every_wait_is_named "run" "$root/out.run"
step_blocks "run" "$root/out.run"
for line in '  \[ ok \] tools: all 2 are there' '  \[ ok \] disk space: ' '  Preflight passed: 2 checks' \
  '^done in [0-9.]+ s: finished$' '^  Step 1 of 1 done  '; do
  grep -qE -- "$line" "$root/out.run" || fail "the output lacks: $line"
done
[ "$(sections_of "$root/out.run")" = "[Preflight] 1/1" ] || fail "sections out of order: $(sections_of "$root/out.run")"
grep -q '^== Step 1 of 1 · TOOL: Check this computer$' "$root/out.run" || fail "the step is not numbered"
[ -z "$(find "$cache" -perm /077 -not -type l)" ] || fail "something in the cache is not private: $(find "$cache" -perm /077)"
# The transcript.
[ "$(ls "$cache/logs" | wc -l)" = 1 ] || fail "expected one transcript"
transcript=$cache/logs/$(ls "$cache/logs")
case $transcript in *-run.log) ;; *) fail "transcript not named for the run: $transcript" ;; esac
[ "$(stat -c %a "$transcript")" = 600 ] || fail "transcript is not private"
grep -q 'run: df -B1' "$transcript" || fail "transcript lacks a command"
grep -q 'outcome: finished' "$transcript" || fail "transcript lacks the outcome"
# A second run over the same cache passes too.
run ''
[ "$rc" = 0 ] || fail "the second run exited $rc"

## Arguments

for args in "--os nura" "--os" "nura" "-x"; do
  run '' $args
  [ "$rc" = 2 ] || fail "nura-install $args exited $rc, not 2"
  grep -q 'unexpected argument' "$root/out" || fail "nura-install $args gave no usage error"
done
run '' --help
[ "$rc" = 0 ] || fail "--help exited $rc"

## Host prerequisites: every failing check is reported together, then the run stops

fresh_cache
FAKE_DF=fail run ''
refused "a prerequisite that fails" 'prerequisite df does not run'
grep -q '^Not ready: 2 checks failed' "$root/out" || fail "wrong count of failures: $(grep '^Not ready' "$root/out")"
grep -q 'Preflight not ready' "$root/out" || fail "the run did not stop at the Preflight"
[ "$(sed -n '/^What each one means/,$p' "$root/out" | grep -c '^  - ')" = 2 ] || fail "not every failure is explained"
NURA_INSTALL_NEEDED_GIB=999999 run ''
refused "too little disk space" 'GiB free'
grep -q '^Not ready: 1 check failed' "$root/out" || fail "wrong count of failures: $(grep '^Not ready' "$root/out")"
chmod 755 "$cache"
run ''
refused "cache not private" 'private'
chmod 700 "$cache"
rc=0
flock "$cache/lock" "$tool" > "$root/out" 2>&1 < /dev/null || rc=$?
refused "concurrent run" 'another nura-install'

## Terminal behaviour: spinner on a terminal only, progress in the transcript, the terminal title, Ctrl-C

fresh_cache
NURA_INSTALL_PROGRESS_SECONDS=1 FAKE_SLOW_DF=3 run ''
[ "$rc" = 0 ] || fail "slow disk check exited $rc"
plain_output "slow disk check" "$root/out"
transcript=$cache/logs/$(ls "$cache/logs")
[ "$(grep -c -E '^  \.\.\. disk space: still running, ' "$transcript")" -ge 2 ] \
  || fail "the transcript has no periodic progress lines for the slow check: $(grep '\.\.\.' "$transcript")"
! grep -q -E '^  \.\.\. disk space: still running' "$root/out" || fail "progress lines went to a pipe"
! LC_ALL=C grep -q -P '\xE2[\xA0-\xA3][\x80-\xBF]|\x1b' "$transcript" || fail "a spinner frame is in the transcript"

# On a terminal (a pty) the spinner shows on screen, the title shows the step and its clocks, and neither reaches the transcript.
pty_run() { # env assignments for the run
  fresh_cache
  : > "$root/pty"
  env TERM=xterm "$@" script -qefc "$tool" /dev/null < /dev/null > "$root/pty" 2>&1 || rc=$?
}
rc=0
pty_run FAKE_SLOW_DF=2
[ "$rc" = 0 ] || fail "the run on a terminal exited $rc"
LC_ALL=C grep -q -P '\r\x1b\[K  \xE2[\xA0-\xA3][\x80-\xBF] disk space' "$root/pty" || fail "no spinner on the terminal"
grep -q "$(printf '\033\[K')" "$root/pty" || fail "the spinner was not erased in place"
transcript=$cache/logs/$(ls "$cache/logs")
[ -z "$(tr -d -c '\r\033' < "$transcript")" ] || fail "escape bytes in the transcript of a terminal run"
! LC_ALL=C grep -q -P '\xE2[\xA0-\xA3][\x80-\xBF]' "$transcript" || fail "a spinner frame is in the transcript of a terminal run"
grep -qE '^  \[ ok \] disk space: ' "$transcript" || fail "the finished line is missing from the transcript"
LC_ALL=C grep -aq -P '\x1b\]2;[^\x07]* nura 1/1 Check this computer[^\x07]* · [0-9]+:[0-9]{2} \([0-9]+:[0-9]{2}\)\x07' "$root/pty" || fail "no working title on the terminal: $(LC_ALL=C grep -a -o -P '\x1b\]2;[^\x07]*' "$root/pty" | head -3 | cat -v)"
LC_ALL=C grep -aq -P '\x1b\]2;✓ nura done · [0-9]+:[0-9]{2}\x07' "$root/pty" || fail "no closing title on the terminal"
! LC_ALL=C grep -aq -P '\x1b\]2;' "$transcript" || fail "a title escape is in the transcript"
rc=0
NURA_INSTALL_NEEDED_GIB=999999 pty_run
[ "$rc" = 1 ] || fail "a stopped run on a terminal exited $rc"
LC_ALL=C grep -aq -P '\x1b\]2;✗ nura stopped at 1/1 · [0-9]+:[0-9]{2}\x07' "$root/pty" || fail "no stopped title on the terminal"

# Ctrl-C ends the run at once and exits 130.
fresh_cache
FAKE_SLOW_DF=8 "$tool" > "$root/out" 2>&1 &
pid=$!
wait_for "$root/out" '... checking disk space'
kill -INT "$pid"
rc=0
wait "$pid" || rc=$?
[ "$rc" = 130 ] || fail "Ctrl-C exited $rc, not 130"
grep -q '^interrupted$' "$root/out" || fail "no state line: $(tail -5 "$root/out")"
grep -q '^done in [0-9.]* s: interrupted$' "$root/out" || fail "no closing line"
grep -q 'outcome: interrupted' "$cache/logs/"* || fail "the transcript lacks the outcome"

echo "PASS: nura-install checks this computer and stops on every doubt"
