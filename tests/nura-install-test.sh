# End-to-end test of nura-install against a fake adb and fastboot.
# Usage: nura-install-test.sh <test-hooks binary> <shipped binary> <the shipped binary's cache dir>
set -euo pipefail
umask 077
tool=$1
shipped=$2
shipped_cache=$3
root=$(mktemp -d)
bash_bin=$(command -v bash)
export LOG=$root/fastboot.log ADB_LOG=$root/adb.log COUNT=$root/devices-count
export ADB_COUNT=$root/adb-count ADB_BACK=$root/adb-back
cache=$root/cache

fail() {
  echo "FAIL: $1" >&2
  echo "--- output" >&2
  cat "$root/out" >&2 || true
  echo "--- fastboot log" >&2
  cat "$LOG" >&2 || true
  exit 1
}

mkdir -p "$root/bin"

# Fake adb knobs (environment variables of a test case):
#   FAKE_ADB=none|unauthorized|offline  what `adb devices` lists (default: the authorized phone)
#   FAKE_ADB_AFTER=n  `adb devices` lists the phone only from its (n+1)th call;  FAKE_ADB_EXTRA=1  a second phone, XYZ789
#   FAKE_ADB_SERIAL, FAKE_SUFFIX  getprop answers
#   FAKE_ACCOUNTS, FAKE_LEVEL, FAKE_DUMPSYS_BAD=1  dumpsys answers
#   FAKE_CRLF=1  end the getprop and dumpsys lines with CRLF as a real adb shell can
#   FAKE_SLOW_BATTERY=<seconds>  dumpsys battery is slow
cat > "$root/bin/adb" <<'EOS'
[ "$1" = version ] && { echo "Android Debug Bridge version fake"; exit 0; }
echo "$*" >> "$ADB_LOG"
crlf() { if [ "${FAKE_CRLF:-}" = 1 ]; then sed 's/$/\r/'; else cat; fi; }
if [ "$1" = devices ] && [ -n "${FAKE_ADB_BACK_AFTER:-}" ] && grep -q 'getvar all' "$LOG" 2> /dev/null; then
  b=$(( $(cat "$ADB_BACK" 2> /dev/null || echo 0) + 1 )); echo "$b" > "$ADB_BACK"
  if [ "$b" -le "$FAKE_ADB_BACK_AFTER" ]; then echo "List of devices attached"; exit 0; fi
fi
if [ "$1" = devices ]; then
  n=$(( $(cat "$ADB_COUNT" 2> /dev/null || echo 0) + 1 )); echo "$n" > "$ADB_COUNT"
  echo "List of devices attached"
  if [ "$n" -gt "${FAKE_ADB_AFTER:-0}" ]; then
    [ "${FAKE_ADB:-device}" = none ] || printf 'ABC123\t%s\n' "${FAKE_ADB:-device}"
    [ -z "${FAKE_ADB_EXTRA:-}" ] || printf 'XYZ789\tdevice\n'
  fi
  exit 0
fi
[ "$1" = -s ] && [ "$2" = ABC123 ] || { echo "error: device not found" >&2; exit 1; }
shift 2
[ "$1" = shell ] || exit 1
shift
case "$*" in
  "getprop ro.serialno") echo "${FAKE_ADB_SERIAL:-ABC123}" | crlf ;;
  "getprop ro.boot.slot_suffix") echo "${FAKE_SUFFIX:-_a}" | crlf ;;
  "dumpsys account")
    if [ "${FAKE_DUMPSYS_BAD:-}" = 1 ]; then echo "Accounts changed their format"; exit 0; fi
    printf 'User UserInfo{0:Owner:13}:\n  Accounts: %s\nUser UserInfo{10:Work:30}:\n  Accounts: 0\n' "${FAKE_ACCOUNTS:-0}" | crlf ;;
  "dumpsys battery") sleep "${FAKE_SLOW_BATTERY:-0}"; printf 'Current Battery Service state:\n  AC powered: false\n  level: %s\n' "${FAKE_LEVEL:-80}" | crlf ;;
  *) echo "fake adb: unexpected shell command: $*" >&2; exit 1 ;;
esac
EOS

# Fake fastboot knobs:
#   FAKE_FB_SERIAL  the phone's serial in fastboot
#   FAKE_FB_EXTRA=1  a second phone, XYZ789;  FAKE_FB_STATE=<state>  the state column;  FAKE_FB_NOPERM=1  a "no permissions" entry
#   FAKE_DEVICES_FAIL=1  `fastboot devices` exits 1;  FAKE_SWAP_FROM=n  the serial changes from the nth `devices` call
#   FAKE_PRODUCT, FAKE_USERSPACE, FAKE_UNLOCKED, FAKE_SLOT, FAKE_SLOTS, FAKE_FB_BATTERY, FAKE_SNAPSHOT,
#   (the getvar answers are the real capture in nura-install-fixtures/, changed by these knobs)
#   FAKE_PSIZE=<name>=<hex>  that partition-size answer (names: boot_a, userdata, ...);  FAKE_PSIZE_MISSING=<name>  answered "Variable Not found"
#   FAKE_PSIZE_PRESENT=<name>  answered with a size (userdata_a must be absent)
#   FAKE_DUP=<var>  that getvar is answered twice, the second time with a different value
#   FAKE_GETVAR_FAIL=1  `getvar all` prints its answer and exits 1
cat > "$root/bin/fastboot" <<'EOS'
[ "$1" = --version ] && { echo "fastboot version fake"; exit 0; }
echo "$*" >> "$LOG"
S=${FAKE_FB_SERIAL:-ABC123}
if [ "$1" = -s ]; then
  [ "$2" = "$S" ] || { echo "unknown serial $2" >&2; exit 1; }
  shift 2
fi
# The fake's bootloader answers come from a real capture of `fastboot getvar all` (the serial is a
# placeholder), changed by the knobs of the case.
setvar() { # name value: replace that answer, or drop it when the value is MISSING
  if [ "$2" = MISSING ]; then
    echo "/^(bootloader) $1:/d"
  else
    echo "s/^(bootloader) $1:.*/(bootloader) $1:$2/"
  fi
}
effective() {
  local unl=${FAKE_UNLOCKED:-yes} bat=${FAKE_FB_BATTERY:-yes} snap=${FAKE_SNAPSHOT:-none}
  {
    sed -e "$(setvar serialno "$S")" -e "$(setvar product "${FAKE_PRODUCT:-FP6}")" \
      -e "$(setvar is-userspace "${FAKE_USERSPACE:-no}")" -e "$(setvar unlocked "$unl")" \
      -e "$(setvar current-slot "${FAKE_SLOT:-a}")" -e "$(setvar slot-count "${FAKE_SLOTS:-2}")" \
      -e "$(setvar battery-soc-ok "$bat")" -e "$(setvar snapshot-update-status "$snap")" \
      -e "/^(bootloader) partition-size:${FAKE_PSIZE_MISSING:-@none@}:/d" "$FP6_GETVAR_ALL" \
      | { if [ -n "${FAKE_PSIZE:-}" ]; then sed "s/^(bootloader) partition-size:${FAKE_PSIZE%%=*}: .*/(bootloader) partition-size:${FAKE_PSIZE%%=*}: ${FAKE_PSIZE#*=}/"; else cat; fi; }
    [ -z "${FAKE_PSIZE_PRESENT:-}" ] || echo "(bootloader) partition-size:$FAKE_PSIZE_PRESENT: 0x1000"
    [ -z "${FAKE_DUP:-}" ] || echo "(bootloader) $FAKE_DUP:bogus"
  }
}
case "$1" in
  devices)
    [ -z "${FAKE_DEVICES_FAIL:-}" ] || exit 1
    n=$(( $(cat "$COUNT" 2>/dev/null || echo 0) + 1 ))
    echo "$n" > "$COUNT"
    [ -n "${FAKE_SWAP_FROM:-}" ] && [ "$n" -ge "$FAKE_SWAP_FROM" ] && S=XYZ789
    printf '%s\t%s\n' "$S" "${FAKE_FB_STATE:-fastboot}"
    [ -z "${FAKE_FB_EXTRA:-}" ] || printf 'XYZ789\tfastboot\n'
    [ -z "${FAKE_FB_NOPERM:-}" ] || printf '????????????\tno permissions (user)\n'
    exit 0 ;;
  getvar)
    [ "$2" = all ] || { echo "fake fastboot: unexpected getvar $2" >&2; exit 1; }
    effective >&2
    [ -z "${FAKE_GETVAR_FAIL:-}" ] || exit 1 ;;
esac
EOS
for t in adb fastboot; do
  sed -i "1i #!$bash_bin" "$root/bin/$t"
  chmod +x "$root/bin/$t"
done

export NURA_INSTALL_ADB=$root/bin/adb NURA_INSTALL_FASTBOOT=$root/bin/fastboot NURA_INSTALL_CACHE=$cache
export NURA_INSTALL_WAIT_SECONDS=0 NURA_INSTALL_NEEDED_GIB=0

fresh_cache() { rm -rf "$cache"; }
again() { run ""; } # a run that keeps the cache
go() { fresh_cache; again; }
run() { # stdin, args...
  local input=$1
  shift
  rm -rf "$LOG" "$COUNT" "$ADB_LOG" "$ADB_COUNT" "$ADB_BACK"
  : > "$LOG"
  : > "$ADB_LOG"
    rc=0
  printf '%b' "$input" | "$tool" "$@" > "$root/out" 2>&1 || rc=$?
}
no_writes() { ! grep -qE '(erase|flash|reboot)' "$LOG" || fail "$1: the phone was written to"; }
refused() { # name, expected message, expected rc (default 1)
  [ "$rc" = "${3:-1}" ] || fail "$1 exited $rc"
  grep -q -- "$2" "$root/out" || fail "$1 not reported as: $2"
  no_writes "$1"
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
step_blocks() { # name, file: every header has its block, one actor per step; an approval's only action is its prompt
  awk -v name="$1" '
    function check() {
      if (cur == "") return
      if (actor == "YOU" && hasdoes) bad = bad "\n  YOU step has Does:: " cur
      if (actor == "YOU" && !hasdo && cur !~ / · YOU: Approve the /) bad = bad "\n  YOU step needs Do:: " cur
      if (actor == "TOOL" && (!hasdoes || hasdo)) bad = bad "\n  TOOL step needs Does: and no Do:: " cur
      if (actor == "TOOL" && !takes) bad = bad "\n  TOOL step lacks Takes:: " cur
      steps++
    }
    /^== Step / { check(); cur = $0; actor = ($0 ~ / · YOU: /) ? "YOU" : "TOOL"; hasdo = 0; hasdoes = 0; takes = 0; first = 1; next }
    cur != "" && first { first = 0; if ($0 !~ /^  (Takes|Do|Does|Phone|After this):/) bad = bad "\n  no block under: " cur }
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
  grep -E '^(=== |== Step )' "$1" | sed -E 's/^=== (.*)$/[\1]/; s/^== Step ([0-9]+) of ([0-9]+) · (YOU|TOOL): .*$/\1\/\2/' | tr '\n' ' ' | sed 's/ $//'
}

fb_checks="devices
-s ABC123 getvar product
-s ABC123 getvar is-userspace
-s ABC123 getvar unlocked
-s ABC123 getvar current-slot
-s ABC123 getvar slot-count
-s ABC123 getvar battery-soc-ok
-s ABC123 getvar partition-size:boot_a
-s ABC123 getvar partition-size:boot_b
-s ABC123 getvar partition-size:vendor_boot_a
-s ABC123 getvar partition-size:vendor_boot_b
-s ABC123 getvar partition-size:dtbo_a
-s ABC123 getvar partition-size:dtbo_b
-s ABC123 getvar partition-size:userdata
-s ABC123 getvar partition-size:userdata_a"
fb_all="devices
-s ABC123 getvar all
devices"
nura_raw=$cache/downloads

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
# Exported overrides do nothing in the shipped binary: with no phone it waits until Ctrl-C, and its
# own cache holds the transcript.
export NURA_INSTALL_CACHE=$root/shipped-fake/cache NURA_INSTALL_ADB=/bin/false NURA_INSTALL_FASTBOOT=/bin/false
export NURA_INSTALL_NEEDED_GIB=9999 NURA_INSTALL_WAIT_SECONDS=0
rc=0; timeout --preserve-status -s INT 10 "$shipped" > "$root/out" 2>&1 < /dev/null || rc=$?
[ "$rc" = 130 ] || fail "the shipped binary with no phone exited $rc, not 130 (an exported override was honoured?): $(cat "$root/out")"
grep -q 'waiting for the phone over adb' "$root/out" || fail "the shipped binary did not wait for a phone: $(cat "$root/out")"
[ ! -e "$root/shipped-fake/cache" ] || fail "the shipped binary used an exported NURA_INSTALL_CACHE"
ls "$shipped_cache/logs"/*-run.log > /dev/null || fail "the shipped run left no transcript in its own cache"
rm -rf "$shipped_cache"
export NURA_INSTALL_ADB=$root/bin/adb NURA_INSTALL_FASTBOOT=$root/bin/fastboot NURA_INSTALL_CACHE=$cache NURA_INSTALL_NEEDED_GIB=0

## Fresh from Android

fresh_cache
run ""
[ "$rc" = 0 ] || fail "the run exited $rc"
[ "$(cat "$LOG")" = "devices
$fb_all" ] || fail "the run asked fastboot: $(cat "$LOG")"
no_writes "the run"
cp "$root/out" "$root/out.run"
plain_output "run" "$root/out.run"
! LC_ALL=C grep -aq -P '\x1b\]2;' "$root/out.run" || fail "a title escape reached a pipe"
every_wait_is_named "run" "$root/out.run"
step_blocks "run" "$root/out.run"
for line in '  \[ ok \] fastboot product: FP6' \
  '  \[ ok \] fastboot partition-size:userdata_a \(absent\): absent' \
  '  \[ ok \] exactly one phone: ABC123' '  \[ ok \] accounts signed in: none signed in' \
    '  Preflight passed: ' '^done in [0-9.]+ s: finished$' '^  Step 3 of 6 done  '; do
  grep -qE -- "$line" "$root/out.run" || fail "the output lacks: $line"
done
[ "$(grep -c '^== Step ' "$root/out.run")" = 6 ] || fail "the run is not six steps"
[ "$(grep -c '^=== ' "$root/out.run")" = 2 ] || fail "the run is not two sections"
# The sections and their steps, in order.
[ "$(sections_of "$root/out.run")" = "[Preflight] 1/6 2/6 3/6 4/6 5/6 [Finish] 6/6" ] \
  || fail "sections out of order: $(sections_of "$root/out.run")"
order=$(grep -n -E '^== Step ' "$root/out" | sed -E 's/^[0-9]*:== Step [0-9]+ of [0-9]+ · (YOU|TOOL): //' | tr '\n' '|')
[ "$order" = "Check this computer|Check the phone over adb|Put the phone in fastboot|Check the bootloader|Restart into Android|Summarize|" ] \
  || fail "steps out of order: $order"
actors=$(grep -E '^== Step ' "$root/out" | sed -E 's/^.* · (YOU|TOOL): .*/\1/' | tr '\n' ' ')
[ "$actors" = "TOOL TOOL YOU TOOL YOU TOOL " ] || fail "actors out of order: $actors"
grep -q '^== Step 1 of 6 · TOOL: Check this computer$' "$root/out" || fail "steps are not numbered"
grep -q '^== Step 6 of 6 · TOOL: Summarize$' "$root/out" || fail "the last step is not 6 of 6"
grep -q 'Hold Power, tap Power off' "$root/out" && grep -q 'Hold Volume Down + Power until the fastboot screen shows' "$root/out" || fail "no fastboot instruction"
grep -q '^  \.\.\. waiting for the phone in fastboot mode (up to 0 s)' "$root/out" || fail "no waiting line"
# The transcript.
[ "$(ls "$cache/logs" | wc -l)" = 1 ] || fail "expected one transcript"
transcript=$cache/logs/$(ls "$cache/logs")
case $transcript in *-run.log) ;; *) fail "transcript not named for the run: $transcript" ;; esac
[ "$(stat -c %a "$transcript")" = 600 ] || fail "transcript is not private"
grep -q 'check serial: ABC123' "$transcript" || fail "transcript lacks the serial check"
grep -q 'outcome: finished' "$transcript" || fail "transcript lacks the outcome"

# A phone on slot b is read as it is.
fresh_cache
FAKE_SUFFIX=_b FAKE_SLOT=b run ""
[ "$rc" = 0 ] || fail "slot b run exited $rc"

# CRLF line endings from adb shell are handled.
fresh_cache
FAKE_CRLF=1 run ""
[ "$rc" = 0 ] || fail "CRLF adb output exited $rc"

## Arguments

for args in "--os nura" "--os" "nura" "-x"; do
  run '' $args
  [ "$rc" = 2 ] || fail "nura-install $args exited $rc, not 2"
  grep -q 'unexpected argument' "$root/out" || fail "nura-install $args gave no usage error"
done
run '' --help
[ "$rc" = 0 ] || fail "--help exited $rc"

## Android phase stops

stop_fresh() { # name, message: a stop before the phone is written to
  fresh_cache
  run ""
  refused "$1" "$2"
  no_writes "$1"
}
stop_early() { # a failed Preflight check: the run stops before it summarizes
  stop_fresh "$@"
  grep -q 'Not ready: ' "$root/out" || fail "$1: no verdict"
  ! grep -q 'Summarize' "$root/out" || fail "$1: went on to the next section"
}
FAKE_ACCOUNTS=1 stop_early "signed-in account" 'accounts are signed in'
FAKE_LEVEL=49 stop_early "low battery" 'battery is at 49%'
# A stop after adb was used has no USB debugging reminder.
! grep -q 'USB debugging off' "$root/out" || fail "a USB debugging reminder is back after a stop"
FAKE_DUMPSYS_BAD=1 stop_early "unparsable dumpsys" 'no `Accounts:` line'
FAKE_SUFFIX=_c stop_early "odd slot suffix" 'unexpected slot suffix'
FAKE_ADB_SERIAL=OTHER stop_early "serial mismatch" 'reports serial OTHER'
FAKE_ADB=unauthorized stop_early "unauthorized phone" 'RSA'
FAKE_ADB=offline stop_early "offline phone" 'adb lists the phone as offline'
FAKE_ADB_EXTRA=1 stop_early "two phones over adb" '2 adb devices are connected'

## Preflight: read-only, runs every check it can, reports all failures together, then stops

fb_only='^(devices|-s [A-Z0-9]+ getvar [a-z:_-]+)$'
preflight_clean() { # name: nothing written to the phone
  no_writes "$1"
  # only where the phone is, or (a failure after the bootloader check) the one `getvar all` read as well
  [ ! -s "$LOG" ] || [ "$(cat "$LOG")" = "devices
$fb_all" ] || fail "$1: fastboot was asked more than the early read: $(cat "$LOG")"
}
pre_fail() { # name, message(s) that must be reported
  local name=$1
  shift
  fresh_cache
  run ""
  [ "$rc" = 1 ] || fail "$name exited $rc, not 1"
  for m in "$@"; do grep -qF -- "$m" "$root/out" || fail "$name not reported as: $m"; done
  grep -q '^Not ready: ' "$root/out" || fail "$name gave no verdict"
  grep -q 'Preflight not ready' "$root/out" || fail "$name did not stop at the Preflight"
  ! grep -q 'Summarize' "$root/out" || fail "$name went on past the Preflight"
  preflight_clean "$name"
}

FAKE_ADB=unauthorized pre_fail "unauthorized phone" 'unauthorized' '[skip] every other adb check'
# Several failures at once are all reported, nothing stops early, nothing is written.
FAKE_ADB_SERIAL=OTHER1 FAKE_ACCOUNTS=1 FAKE_LEVEL=10 \
  pre_fail "several failures over adb" 'ro.serialno: the phone reports serial OTHER1' 'accounts are signed in' 'battery is at 10%'
grep -q '^Not ready: 3 checks failed, 0 skipped' "$root/out" || fail "wrong count of failures over adb: $(grep '^Not ready' "$root/out")"
! grep -q 'Put the phone in fastboot' "$root/out" || fail "the owner was sent to fastboot with the adb checks failed"
# The bootloader is read first: every wrong answer is reported at once, then the run stops.
fresh_cache
FAKE_PSIZE_MISSING=boot_a FAKE_UNLOCKED=no FAKE_FB_BATTERY=no run ""
[ "$rc" = 1 ] || fail "a wrong bootloader exited $rc"
for m in 'no value for `getvar partition-size:boot_a`' 'unlocked: no' 'battery-soc-ok: no'; do
  grep -qF -- "$m" "$root/out" || fail "the early bootloader check did not report: $m"
done
grep -q '^Not ready: 3 checks failed' "$root/out" || fail "wrong count after the bootloader check: $(grep '^Not ready' "$root/out")"
! grep -q 'Restart into Android' "$root/out" || fail "the owner was sent back to Android after a failed bootloader check"
no_writes "a failed bootloader check"
# A phone that is not on Android needs the owner first: the run waits for it in a step of its own.
fresh_cache
FAKE_ADB_AFTER=1 NURA_INSTALL_WAIT_SECONDS=10 run ""
[ "$rc" = 0 ] || fail "the run that waited for the phone exited $rc"
[ "$(sections_of "$root/out")" = "[Preflight] 1/7 2/7 3/7 4/7 5/7 6/7 [Finish] 7/7" ] || fail "sections with the wait: $(sections_of "$root/out")"
grep -q '^== Step 2 of 7 · YOU: Connect the phone with USB debugging$' "$root/out" || fail "no step to connect the phone"
grep -q '^  \.\.\. waiting for the phone over adb (up to 10 s)' "$root/out" || fail "the wait for adb was not named"
# A YOU step ends when the phone shows its result: the restart step stays open until the phone is back over adb.
fresh_cache
FAKE_ADB_BACK_AFTER=2 NURA_INSTALL_WAIT_SECONDS=20 run ""
[ "$rc" = 0 ] || fail "the run that waited for the phone to come back exited $rc"
awk '
  /^== Step 5 of 6 · YOU: Restart into/ { in5 = 1; next }
  /^== Step 6 of 6/ { in5 = 0 }
  in5 && /waiting for the phone over adb/ { waited = 1 }
  in5 && /^  Step 5 of 6 done/ { done5 = $0 }
  END { if (!waited || done5 ~ / 0\.[0-9] s$/) { print "waited=" waited " done=" done5; exit 1 } }' "$root/out" || fail "the restart step ended before the phone was back over adb"
fresh_cache
run ""
grep -q '  Preflight passed: ' "$root/out" || fail "the Preflight did not pass: $(grep -E 'FAIL|Not ready' "$root/out")"
cp "$root/out" "$root/out.preflight"
plain_output "preflight" "$root/out.preflight"
step_blocks "preflight" "$root/out.preflight"

## Fastboot checks

fb_fail() { # name, message: a wrong answer stops the run before anything is written or recorded
  go
  refused "$1" "$2"
}
FAKE_PRODUCT=FP5 fb_fail "wrong product" 'product: FP5'
FAKE_USERSPACE=yes fb_fail "fastbootd" 'is-userspace: yes'
FAKE_USERSPACE=MISSING fb_fail "unanswered userspace" 'no value for `getvar is-userspace`'
FAKE_UNLOCKED=no fb_fail "locked" 'unlocked: no'
FAKE_SLOT=b fb_fail "slot fell back" 'Do NOT switch slots'
FAKE_SLOTS=1 fb_fail "slot count" 'slot-count: 1'
FAKE_FB_BATTERY=no fb_fail "low battery" 'battery-soc-ok: no'
FAKE_FB_BATTERY=MISSING fb_fail "unanswered battery" 'no value for `getvar battery-soc-ok`'
FAKE_PSIZE_MISSING=vendor_boot_b fb_fail "a slotted partition without a size" 'no value for `getvar partition-size:vendor_boot_b`'
FAKE_PSIZE=dtbo_a=0x0 fb_fail "a zero size" 'partition-size:dtbo_a: 0x0`, not a size above zero'
FAKE_PSIZE=boot_b=big fb_fail "a size that is not hex" 'partition-size:boot_b: big`'
FAKE_PSIZE_MISSING=userdata fb_fail "userdata without a size" 'no value for `getvar partition-size:userdata`'
FAKE_PSIZE_PRESENT=userdata_a fb_fail "userdata has a slot" 'partition-size:userdata_a: 0x1000`, but userdata has no slot'
FAKE_PSIZE_PRESENT=userdata_a fb_fail "userdata has a slot" 'partition-size:userdata_a: 0x1000`, but userdata has no slot'
FAKE_DUP=unlocked fb_fail "contradictory answers" 'unlocked` more than once'
FAKE_GETVAR_FAIL=unlocked fb_fail "getvar exit status" 'getvar all` failed'
FAKE_SWAP_FROM=3 fb_fail "swap before writing" 'XYZ789'
FAKE_FB_EXTRA=1 fb_fail "several phones" '2 fastboot devices are connected'
FAKE_FB_STATE=adb fb_fail "a device that is not in fastboot" 'not fastboot'
FAKE_FB_NOPERM=1 fb_fail "a device without permission" 'no permission'
FAKE_DEVICES_FAIL=1 fb_fail "failed fastboot devices" 'fastboot devices` failed'
fresh_cache
FAKE_FB_SERIAL=XYZ789 run ""
refused "wrong serial after the adb phase" 'is XYZ789, not ABC123'

## Host prerequisites

fresh_cache
NURA_INSTALL_ADB=$root/missing run ""
refused "a missing prerequisite" 'prerequisite'
NURA_INSTALL_FASTBOOT=/bin/false run ""
refused "a prerequisite that fails" 'prerequisite /bin/false does not run'
NURA_INSTALL_NEEDED_GIB=999999 run ""
refused "too little disk space" 'GiB free'
chmod 755 "$cache"
run ""
refused "cache not private" 'private'
chmod 700 "$cache"
rc=0
flock "$cache/lock" "$tool" > "$root/out" 2>&1 < /dev/null || rc=$?
refused "concurrent run" 'another nura-install'

## Terminal behaviour: spinner on a terminal only, progress in the transcript, the terminal title, Ctrl-C

fresh_cache
NURA_INSTALL_PROGRESS_SECONDS=1 FAKE_SLOW_BATTERY=3 run ""
[ "$rc" = 0 ] || fail "slow battery run exited $rc"
plain_output "slow battery" "$root/out"
transcript=$cache/logs/$(ls "$cache/logs")
[ "$(grep -c -E '^  \.\.\. battery: still running, ' "$transcript")" -ge 2 ] \
  || fail "the transcript has no periodic progress lines for the slow check: $(grep '\.\.\.' "$transcript")"
! grep -q -E '^  \.\.\. battery: still running' "$root/out" || fail "progress lines went to a pipe"
! LC_ALL=C grep -q -P '\xE2[\xA0-\xA3][\x80-\xBF]|\x1b' "$transcript" || fail "a spinner frame is in the transcript"

# On a terminal (a pty) the spinner shows on screen, the title shows the step and its clocks, and neither reaches the transcript.
pty_run() { # env assignments for the run
  fresh_cache
  rm -rf "$LOG" "$COUNT" "$ADB_LOG" "$ADB_COUNT" "$ADB_BACK"
  : > "$LOG"
  : > "$ADB_LOG"
  : > "$root/pty"
  env TERM=xterm "$@" script -qefc "$tool" /dev/null < /dev/null > "$root/pty" 2>&1 || rc=$?
}
rc=0
pty_run FAKE_SLOW_BATTERY=2 FAKE_ADB_BACK_AFTER=1 NURA_INSTALL_WAIT_SECONDS=20
[ "$rc" = 0 ] || fail "the run on a terminal exited $rc"
LC_ALL=C grep -q -P '\r\x1b\[K  \xE2[\xA0-\xA3][\x80-\xBF] battery' "$root/pty" || fail "no spinner on the terminal"
grep -q "$(printf '\033\[K')" "$root/pty" || fail "the spinner was not erased in place"
transcript=$cache/logs/$(ls "$cache/logs")
[ -z "$(tr -d -c '\r\033' < "$transcript")" ] || fail "escape bytes in the transcript of a terminal run"
! LC_ALL=C grep -q -P '\xE2[\xA0-\xA3][\x80-\xBF]' "$transcript" || fail "a spinner frame is in the transcript of a terminal run"
grep -qE '^  \[ ok \] battery: ' "$transcript" || fail "the finished line is missing from the transcript"
LC_ALL=C grep -aq -P '\x1b\]2;[^\x07]* nura 2/6 Check the phone over adb[^\x07]* · [0-9]+:[0-9]{2} \([0-9]+:[0-9]{2}\)\x07' "$root/pty" || fail "no working title on the terminal: $(LC_ALL=C grep -a -o -P '\x1b\]2;[^\x07]*' "$root/pty" | head -3 | cat -v)"
LC_ALL=C grep -aq -P '\x1b\]2;⏸ nura 5/6 restart into Android · ' "$root/pty" || fail "no waiting title while the owner restarts the phone"
LC_ALL=C grep -aq -P '\x1b\]2;✓ nura done · [0-9]+:[0-9]{2}\x07' "$root/pty" || fail "no closing title on the terminal"
! LC_ALL=C grep -aq -P '\x1b\]2;' "$transcript" || fail "a title escape is in the transcript"
rc=0
FAKE_ACCOUNTS=1 pty_run
[ "$rc" = 1 ] || fail "a stopped run on a terminal exited $rc"
LC_ALL=C grep -aq -P '\x1b\]2;✗ nura stopped at 2/6 · [0-9]+:[0-9]{2}\x07' "$root/pty" || fail "no stopped title on the terminal"

# Ctrl-C says nothing was written and exits 130.
interrupt_during() { # name, stdin, expected text, text to wait for; the slow check is running
  fresh_cache
  rm -f "$root/out" "$COUNT"
  : > "$root/out"
  printf '%b' "$2" | FAKE_SLOW_BATTERY=8 "$tool" > "$root/out" 2>&1 &
  local pid=$!
  wait_for "$root/out" "$4"
  kill -INT "$pid"
  rc=0
  wait "$pid" || rc=$?
  [ "$rc" = 130 ] || fail "$1: Ctrl-C exited $rc, not 130"
  grep -qF -- "interrupted: $3" "$root/out" || fail "$1: no state line: $(tail -5 "$root/out")"
  grep -q '^done in [0-9.]* s: interrupted$' "$root/out" || fail "$1: no closing line"
  no_writes "$1"
  grep -q 'outcome: interrupted' "$cache/logs/"* || fail "$1: the transcript lacks the outcome"
}
interrupt_during "Ctrl-C in the Preflight" '' 'Nothing was written to the phone.' '... checking battery'

echo "PASS: nura-install checks the phone over adb and fastboot, and stops on every doubt"
