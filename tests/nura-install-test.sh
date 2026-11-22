# End-to-end test of nura-install against a fake adb, fastboot and local image servers.
# Usage: nura-install-test.sh <test-hooks binary> <shipped binary> <the shipped binary's cache dir>
set -euo pipefail
umask 077
tool=$1
shipped=$2
shipped_cache=$3
root=$(mktemp -d)
bash_bin=$(command -v bash)
export LOG=$root/fastboot.log ADB_LOG=$root/adb.log FLASHED=$root/flashed COUNT=$root/devices-count
export STATE_LOG=$root/state.log SIG_LOG=$root/sig.log CURL_LOG=$root/curl.log PCOUNT=$root/product-count
export HCOUNT=$root/psize-count ADB_COUNT=$root/adb-count WROTE=$root/wrote ADB_BACK=$root/adb-back
cache=$root/cache
export STATE_FILE=$cache/phones/ABC123/state.json

fail() {
  echo "FAIL: $1" >&2
  echo "--- output" >&2
  cat "$root/out" >&2 || true
  echo "--- fastboot log" >&2
  cat "$LOG" >&2 || true
  exit 1
}

mkdir -p "$root/srv" "$root/bin" "$root/sdcard/sub dir"
sum() { sha256sum "$1" | cut -d' ' -f1; }

# Fake phone storage.
printf 'hello' > "$root/sdcard/a.txt"
printf 'nested' > "$root/sdcard/sub dir/b c.txt"
printf '' > "$root/sdcard/empty"

# Nura's images: the newest build (what the index should lead to), an older one, and one from another release.
for k in nura old rel; do
  printf '%s-boot' "$k" > "$root/$k-boot.raw"
  printf '%s-rootfs' "$k" > "$root/$k-rootfs.raw"
  xz -c "$root/$k-boot.raw" > "$root/srv/$k-boot.img.xz"
  xz -c "$root/$k-rootfs.raw" > "$root/srv/$k-rootfs.img.xz"
done
cp "$root/nura-boot.raw" "$root/boot.raw"
cp "$root/nura-rootfs.raw" "$root/rootfs.raw"

other_sha=$(printf x | sha256sum | cut -d' ' -f1)

# Nura's index: the committed fixture (a trimmed copy of the real one) with the Fairphone (Gen. 6) images
# pointing at local files: the 2026-11-13 builds at nura-*, the 2026-11-06 ones at old-*; others are never fetched.
fixture_index() { # extra jq filter applied last
  jq --arg s "file://$root/srv" \
    --arg nb "$(sum "$root/srv/nura-boot.img.xz")" --arg nr "$(sum "$root/srv/nura-rootfs.img.xz")" \
    --arg ob "$(sum "$root/srv/old-boot.img.xz")" --arg or "$(sum "$root/srv/old-rootfs.img.xz")" '
    (.releases[].devices[] | select(.name == "fairphone-fp6") | .interfaces[].images[]) |=
      ((if (.timestamp | startswith("2026-11-13")) then "nura" else "old" end) as $k
       | (if (.name | endswith("-boot.img.xz")) then "boot" else "rootfs" end) as $f
       | .url = "\($s)/\($k)-\($f).img.xz"
       | .sha256 = (if $k == "nura" then (if $f == "boot" then $nb else $nr end) else (if $f == "boot" then $ob else $or end) end))
    | '"$1" "$NURA_FIXTURE"
}
make_index() { fixture_index "${1:-.}" > "$root/index.json"; }
make_index
fp6_images='(.releases[].devices[] | select(.name == "fairphone-fp6") | .interfaces[].images[])'

# FAKE_REDIRECT=<url fragment>  a HEAD request (-sI) for a URL containing it answers with a 302
cat > "$root/bin/curl" <<'EOS'
echo "$*" >> "$CURL_LOG"
case " $* " in
  *" -sI "*)
    case "$*" in
      *"${FAKE_REDIRECT:-@none@}"*) printf 'HTTP/1.1 302 Found\r\nLocation: https://evil.example/x\r\n\r\n'; exit 0 ;;
    esac ;;
esac
exec "$REAL_CURL" "$@"
EOS

# Fake adb knobs (environment variables of a test case):
#   FAKE_ADB=none|unauthorized|offline  what `adb devices` lists (default: the authorized phone)
#   FAKE_ADB_AFTER=n  `adb devices` lists the phone only from its (n+1)th call;  FAKE_ADB_EXTRA=1  a second phone, XYZ789
#   FAKE_ADB_SERIAL, FAKE_SUFFIX  getprop answers
#   FAKE_ACCOUNTS, FAKE_LEVEL, FAKE_DUMPSYS_BAD=1  dumpsys answers
#   FAKE_CRLF=1  end the getprop and dumpsys lines with CRLF as a real adb shell can
#   FAKE_UNREADABLE=1|quiet  the hashing run can't read a file: exit 1, or the message with exit 0
#   FAKE_BAD_MANIFEST=1  the phone's hash list disagrees with the tar
#   FAKE_CONTENT_DENY=stderr|stdout|exit  a content query is denied: on stderr, as stdout text, or by exit code
#   FAKE_CONTENT_EMPTY=<uri fragment>  that content query says "No result found."
#   FAKE_SLOW_TAR=<seconds>  the tar stream stalls that long after its first 512 bytes;  FAKE_SLOW_BATTERY=<seconds>  dumpsys battery is slow
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
sd=${FAKE_SDCARD}
if [ "$1" = exec-out ]; then
  [ "$*" = "exec-out tar -cf - -C /sdcard ." ] || exit 1
  if [ -n "${FAKE_SLOW_TAR:-}" ]; then
    tar -b 1 -cf - -C "$sd" . | { dd bs=512 count=1 iflag=fullblock 2> /dev/null; sleep "$FAKE_SLOW_TAR"; cat; }
    exit 0
  fi
  exec tar -b 1 -cf - -C "$sd" .
fi
[ "$1" = shell ] || exit 1
shift
case "$*" in
  "getprop ro.serialno") echo "${FAKE_ADB_SERIAL:-ABC123}" | crlf ;;
  "getprop ro.boot.slot_suffix") echo "${FAKE_SUFFIX:-_a}" | crlf ;;
  "dumpsys account")
    if [ "${FAKE_DUMPSYS_BAD:-}" = 1 ]; then echo "Accounts changed their format"; exit 0; fi
    printf 'User UserInfo{0:Owner:13}:\n  Accounts: %s\nUser UserInfo{10:Work:30}:\n  Accounts: 0\n' "${FAKE_ACCOUNTS:-0}" | crlf ;;
  "cd /sdcard && find . -printf '%M %s %p\n'") (cd "$sd" && find . -printf '%M %s %p\n') ;;
  "dumpsys battery") sleep "${FAKE_SLOW_BATTERY:-0}"; printf 'Current Battery Service state:\n  AC powered: false\n  level: %s\n' "${FAKE_LEVEL:-80}" | crlf ;;
  "cd /sdcard && find . -type f -exec sha256sum {} +")
    case "${FAKE_UNREADABLE:-}" in
      1) echo "sha256sum: ./secret: Permission denied" >&2; exit 1 ;;
      quiet) echo "sha256sum: ./secret: Permission denied" >&2 ;;
    esac
    out=$(cd "$sd" && find . -type f -exec sha256sum {} +)
    if [ "${FAKE_BAD_MANIFEST:-}" = 1 ]; then out=$(printf '%s\n' "$out" | sed '1s/^./0/;1s/^00/11/'); fi
    printf '%s\n' "$out" ;;
  "content query --uri "*)
    case "${FAKE_CONTENT_DENY:-}" in
      stderr) echo "Error while accessing provider:${4#content://}" >&2; echo "java.lang.SecurityException: Permission Denial" >&2; exit 0 ;;
      stdout) echo "Error while accessing provider:${4#content://}"; exit 0 ;;
      exit) exit 1 ;;
    esac
    case "$4" in *"${FAKE_CONTENT_EMPTY:-@none@}"*) echo "No result found."; exit 0 ;; esac
    printf 'Row: 0 _id=1, name=x\nRow: 1 _id=2, name=y\n' ;;
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
#   FAKE_PSIZE_PRESENT=<name>  answered with a size (userdata_a must be absent);  FAKE_PSIZE_GARBAGE=<name>  answered with another FAILED
#   FAKE_PSIZE_FLIP=n  partition-size:boot_a is not found from its nth check on
#   FAKE_FAIL0=<partition>  that write prints FAILED and exits 0
#   FAKE_DUP=<var>  that getvar is answered twice, the second time with a different value
#   FAKE_GETVAR_FAIL=<var>  that getvar prints its answer and exits 1
#   FAKE_TAMPER=<file>  appends to that file during the second product check, i.e. after the first gate
#   FAKE_FAIL=<partition>  that write fails;  FAKE_HANG=<partition>  that write never returns
#   FAKE_AFTER_SNAPSHOT, FAKE_AFTER_BATTERY, FAKE_AFTER_UNLOCKED  those getvar answers once any write ran
#   FAKE_SIG_ON=<partition>  that write records its signal mask and sends SIGINT to nura-install
cat > "$root/bin/fastboot" <<'EOS'
[ "$1" = --version ] && { echo "fastboot version fake"; exit 0; }
echo "$*" >> "$LOG"
S=${FAKE_FB_SERIAL:-ABC123}
if [ "$1" = -s ]; then
  [ "$2" = "$S" ] || { echo "unknown serial $2" >&2; exit 1; }
  shift 2
fi
write() { # verb partition
  touch "$WROTE"
  [ ! -f "$STATE_FILE" ] || echo "$1 $2 under $(jq -r .state "$STATE_FILE")" >> "$STATE_LOG"
  [ -z "${FAKE_HANG:-}" ] || [ "$FAKE_HANG" != "$2" ] || exec sleep 60
  [ -n "${FAKE_FAIL:-}" ] && [ "$FAKE_FAIL" = "$2" ] && { echo "FAILED (remote: 'boom')" >&2; exit 1; }
  [ -n "${FAKE_FAIL0:-}" ] && [ "$FAKE_FAIL0" = "$2" ] && { echo "FAILED (remote: 'boom')" >&2; return 0; }
  if [ -n "${FAKE_SIG_ON:-}" ] && [ "$FAKE_SIG_ON" = "$2" ]; then grep SigBlk /proc/self/status >> "$SIG_LOG"; kill -INT "$PPID"; sleep "${FAKE_SIG_SLEEP:-0}"; fi
  return 0
}
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
  local after=
  [ ! -e "$WROTE" ] || after=1
  local unl=${FAKE_UNLOCKED:-yes} bat=${FAKE_FB_BATTERY:-yes} snap=${FAKE_SNAPSHOT:-none}
  if [ -n "$after" ]; then unl=${FAKE_AFTER_UNLOCKED:-$unl}; bat=${FAKE_AFTER_BATTERY:-$bat}; snap=${FAKE_AFTER_SNAPSHOT:-$snap}; fi
  local n=${FAKE_PSIZE_FLIP_NAME:-boot_a} drop=@none@
  [ -z "${FAKE_PSIZE_FLIP:-}" ] || [ "$(cat "$HCOUNT" 2> /dev/null || echo 0)" -lt "$FAKE_PSIZE_FLIP" ] || drop=$n
  {
    sed -e "$(setvar serialno "$S")" -e "$(setvar product "${FAKE_PRODUCT:-FP6}")" \
      -e "$(setvar is-userspace "${FAKE_USERSPACE:-no}")" -e "$(setvar unlocked "$unl")" \
      -e "$(setvar current-slot "${FAKE_SLOT:-a}")" -e "$(setvar slot-count "${FAKE_SLOTS:-2}")" \
      -e "$(setvar battery-soc-ok "$bat")" -e "$(setvar snapshot-update-status "$snap")" \
      -e "/^(bootloader) partition-size:${FAKE_PSIZE_MISSING:-@none@}:/d" \
      -e "/^(bootloader) partition-size:$drop:/d" "$FP6_GETVAR_ALL" \
      | { if [ -n "${FAKE_PSIZE:-}" ]; then sed "s/^(bootloader) partition-size:${FAKE_PSIZE%%=*}: .*/(bootloader) partition-size:${FAKE_PSIZE%%=*}: ${FAKE_PSIZE#*=}/"; else cat; fi; }
    [ -z "${FAKE_PSIZE_PRESENT:-}" ] || echo "(bootloader) partition-size:$FAKE_PSIZE_PRESENT: 0x1000"
    [ -z "${FAKE_DUP:-}" ] || echo "(bootloader) $FAKE_DUP:bogus"
  }
}
answer() { # one getvar answer as `fastboot getvar <name>` prints it, or the "not found" failure (exit status 0)
  local q=$1
  case $q in
    product) local p=$(( $(cat "$PCOUNT" 2> /dev/null || echo 0) + 1 )); echo "$p" > "$PCOUNT"
      [ -z "${FAKE_TAMPER:-}" ] || [ "$p" -lt 2 ] || echo x >> "$FAKE_TAMPER" ;;
    partition-size:boot_a) local h=$(( $(cat "$HCOUNT" 2> /dev/null || echo 0) + 1 )); echo "$h" > "$HCOUNT" ;;
  esac
  if [ "$q" = "${FAKE_PSIZE_GARBAGE:-@none@}" ] || [ "partition-size:${FAKE_PSIZE_GARBAGE:-@none@}" = "$q" ]; then
    echo "getvar:$q                  FAILED (remote: 'oops')" >&2
    return
  fi
  local out
  out=$(effective | awk -v q="$q" '{
    l = $0; sub(/^\(bootloader\) /, "", l)
    i = index(l, ": ")
    if (i > 0) { n = substr(l, 1, i - 1); v = substr(l, i + 2) }
    else { j = 0; for (k = length(l); k > 0; k--) if (substr(l, k, 1) == ":") { j = k; break }
           if (j == 0) next; n = substr(l, 1, j - 1); v = substr(l, j + 1) }
    if (n == q) print n ": " v }')
  if [ -z "$out" ]; then
    echo "getvar:$q                  FAILED (remote: 'GetVar Variable Not found')" >&2
  else
    printf '%s\n' "$out" >&2
  fi
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
    if [ "$2" = all ]; then
      p=$(( $(cat "$PCOUNT" 2> /dev/null || echo 0) + 1 )); echo "$p" > "$PCOUNT"
      [ -z "${FAKE_TAMPER:-}" ] || [ "$p" -lt 2 ] || echo x >> "$FAKE_TAMPER"
      h=$(( $(cat "$HCOUNT" 2> /dev/null || echo 0) + 1 )); echo "$h" > "$HCOUNT"
      effective >&2
      [ -z "${FAKE_GETVAR_FAIL:-}" ] || exit 1
    else
      answer "$2"
      echo "Finished. Total time: 0.001s" >&2
      [ "${FAKE_GETVAR_FAIL:-}" != "$2" ] || exit 1
    fi ;;
  flash) write flash "$2"; cp "$3" "$FLASHED/$2" ;;
  erase) write erase "$2" ;;
  reboot) write reboot "" ;;
esac
EOS
for t in adb fastboot curl; do
  sed -i "1i #!$bash_bin" "$root/bin/$t"
  chmod +x "$root/bin/$t"
done

export NURA_INSTALL_ADB=$root/bin/adb NURA_INSTALL_FASTBOOT=$root/bin/fastboot
export NURA_INSTALL_NURA_INDEX=file://$root/index.json NURA_INSTALL_CACHE=$cache
export NURA_INSTALL_ALLOW_FILE_URLS=1 NURA_INSTALL_WAIT_SECONDS=0 NURA_INSTALL_NEEDED_GIB=0
export NURA_INSTALL_ALLOW_PIPED_INPUT=1 NURA_INSTALL_WRITE_TIMEOUT_SECONDS=60
export FAKE_SDCARD=$root/sdcard
REAL_CURL=$(command -v curl)
export REAL_CURL

# Typed input shared by the cases.
done_phrase='Confirm'
# Menus are answered by their word when the input is piped (the test build only); the pty cases press keys.
choice='phosh\n'
confirmations="${choice}$done_phrase\nbackup\n"
fresh_in="${confirmations}install nura\nreboot\n"
fresh_cache() { rm -rf "$cache"; }
again() { run "$fresh_in"; } # a full run that keeps the cache
go() { fresh_cache; again; }
state() { jq -r .state "$STATE_FILE"; }
run() { # stdin, args...
  local input=$1
  shift
  rm -rf "$FLASHED" "$LOG" "$COUNT" "$PCOUNT" "$ADB_LOG" "$STATE_LOG" "$SIG_LOG" "$CURL_LOG" "$ADB_COUNT" "$HCOUNT" "$WROTE" "$ADB_BACK"
  mkdir -p "$FLASHED"
  : > "$LOG"
  : > "$ADB_LOG"
    rc=0
  printf '%b' "$input" | "$tool" "$@" > "$root/out" 2>&1 || rc=$?
}
writes() { grep -E '(erase|flash|reboot)' "$LOG" || true; }
no_writes() { [ -z "$(writes)" ] || fail "$1: the phone was written to"; }
refused() { # name, expected message, expected rc (default 1)
  [ "$rc" = "${3:-1}" ] || fail "$1 exited $rc"
  grep -q -- "$2" "$root/out" || fail "$1 not reported as: $2"
  no_writes "$1"
}
no_backup() { # nothing was copied off the phone
  ! grep -q 'exec-out' "$ADB_LOG" || fail "$1: the phone was read for the backup"
  [ ! -e "$cache/backups" ] || fail "$1: a backup directory exists"
}
untouched_fresh() { # nothing recorded or written
  no_writes "$1"
  [ ! -e "$cache/phones" ] || fail "$1: wrote a record"
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
gate_precedes_its_write() { # name, file
  grep -E '^== Step ' "$2" | sed -E 's/^== Step [0-9]+ of [0-9]+ · //; s/ +POINT OF NO RETURN$//' | awk -v name="$1" '
    prev ~ /^YOU: Approve the install$/ && $0 != "TOOL: Install Nura" { bad = 1 }
    prev ~ /^YOU: Approve the reboot$/ && $0 != "TOOL: Restart the phone" { bad = 1 }
    /^TOOL: (Install Nura|Restart the phone)$/ && prev !~ /^YOU: Approve the / { bad = 1 }
    { prev = $0 }
    END { exit bad }' || fail "$1: a gate's YOU step does not directly precede the TOOL step that writes"
}
wait_pty() { # the run on the terminal ends within 30 s; sets rc
  local i
  for i in $(seq 300); do
    kill -0 "$spid" 2> /dev/null || break
    sleep 0.1
  done
  if kill -0 "$spid" 2> /dev/null; then
    { echo "--- $(date +%s) rc-state; spid=$spid"; tail -c 9000 "$root/pty" | cat -v; } >&2
    fail "the run on the terminal did not end"
  fi
  rc=0
  wait "$spid" || rc=$?
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
fit_install="-s ABC123 getvar partition-size:userdata
-s ABC123 getvar partition-size:boot_a"
fb_identity="devices
-s ABC123 getvar is-userspace
-s ABC123 getvar unlocked
-s ABC123 getvar current-slot"
nura_raw=$cache/downloads

## Shipped binary: help, usage errors, nothing to do, and exported overrides that do nothing

rc=0; "$shipped" --help > "$root/out" 2>&1 || rc=$?
[ "$rc" = 0 ] || fail "shipped --help exited $rc"
grep -q 'Exit codes' "$root/out" || fail "--help lacks the exit codes"
for bad in "--os nura" "--os" "nura" "--rollback" "--help --os"; do
  rc=0; "$shipped" $bad > "$root/out" 2>&1 || rc=$?
  [ "$rc" = 2 ] || fail "shipped '$bad' exited $rc"
done
mkdir -p "$root/shipped-fake" "$root/home"
export HOME=$root/home
export NURA_INSTALL_NURA_INDEX=file://$root/missing.json NURA_INSTALL_CACHE=$root/shipped-fake/cache
export NURA_INSTALL_ADB=/bin/false NURA_INSTALL_FASTBOOT=/bin/false NURA_INSTALL_CURL=/bin/false
export NURA_INSTALL_ALLOW_FILE_URLS=1 NURA_INSTALL_NEEDED_GIB=9999 NURA_INSTALL_ALLOW_PIPED_INPUT=1
rc=0; "$shipped" > "$root/out" 2>&1 < /dev/null || rc=$?
[ "$rc" = 1 ] || fail "shipped with no phone and no terminal exited $rc, not 1 (an exported override was honoured?)"
grep -q 'interactive terminal' "$root/out" || fail "shipped with no terminal did not say so: $(cat "$root/out")"
[ ! -e "$root/shipped-fake/cache" ] || fail "the shipped binary used an exported NURA_INSTALL_CACHE"
ls "$shipped_cache/logs"/*-run.log > /dev/null || fail "the shipped run left no transcript in its own cache"
rm -rf "$shipped_cache"
export NURA_INSTALL_ADB=$root/bin/adb NURA_INSTALL_FASTBOOT=$root/bin/fastboot
export NURA_INSTALL_NURA_INDEX=file://$root/index.json NURA_INSTALL_CACHE=$cache NURA_INSTALL_NEEDED_GIB=0
export NURA_INSTALL_CURL=$root/bin/curl

## Fresh from Android

fresh_cache
run "$fresh_in"
[ "$rc" = 0 ] || fail "fresh install exited $rc"
[ "$(cat "$LOG")" = "devices
$fb_all
devices
$fb_checks
$fit_install
$fb_checks
$fit_install
-s ABC123 erase dtbo
-s ABC123 erase vendor_boot
-s ABC123 flash userdata $nura_raw/nura-rootfs.img
-s ABC123 flash boot $nura_raw/nura-boot.img
$fb_identity
-s ABC123 reboot" ] || fail "fresh install ran: $(cat "$LOG")"
# The backup is on disk before the run goes on (fsync of files and directories).
synced=$(grep 'check synced to disk' "$cache/logs/"* | sed 's/.*synced to disk: //')
[ "$(printf '%s\n' "$synced" | grep -c "$cache/backups/ABC123/backup-")" = 1 ] || fail "the backup was not synced: $synced"
[ "$(cat "$FLASHED/userdata")" = nura-rootfs ] || fail "userdata got the wrong image"
[ "$(cat "$FLASHED/boot")" = nura-boot ] || fail "boot got the wrong image"
cp "$root/out" "$root/out.install"
plain_output "install" "$root/out.install"
! LC_ALL=C grep -aq -P '\x1b\]2;' "$root/out.install" || fail "a title escape reached a pipe"
every_wait_is_named "install" "$root/out.install"
step_blocks "install" "$root/out.install"
gate_precedes_its_write "install" "$root/out.install"
for line in '  \.\.\. streaming the phone.s storage to [^ ]*sdcard\.tar$' '  \.\.\. hashing 3 files on the phone' \
  '  \[ ok \] hash /sdcard on the phone: 3 hashes' '  \[ ok \] flash userdata: .* written' '  \[ ok \] fastboot product: FP6' \
  '  \[ ok \] fastboot partition-size:userdata_a \(absent\): absent' '  \[ ok \] sizing /sdcard: 3 files, 4\.5 KiB' \
  '  \[ ok \] exactly one phone: ABC123' '  \[ ok \] accounts signed in: none signed in' \
  '  Nura build: edge phosh 2026-11-13T23:32:00' \
  '  Preflight passed: ' '^done in [0-9.]+ s: finished$' '^  Step 9 of 17 done  '; do
  grep -qE -- "$line" "$root/out.install" || fail "install output lacks: $line"
done
[ "$(grep -c '^== Step ' "$root/out.install")" = 17 ] || fail "the install is not seventeen steps"
[ "$(grep -c '^=== ' "$root/out.install")" = 4 ] || fail "the install is not four sections"
[ "$(state)" = nura-installed ] || fail "state after install is $(state)"
# The planned length of the /sdcard tar equals what the phone streamed, byte for byte.
planned() { grep -h 'check sdcard tar bytes' "$1"/logs/* | sed 's/.*: //'; }
[ "$(planned "$cache")" = "$(stat -c %s "$cache"/backups/ABC123/backup-*/sdcard.tar)" ] || fail "the planned tar length $(planned "$cache") differs from the stream $(stat -c %s "$cache"/backups/ABC123/backup-*/sdcard.tar)"
[ "$(jq -r .slot "$STATE_FILE")" = a ] || fail "slot not recorded"
[ "$(jq -r '[.nura_build, .nura_boot_sha256, .nura_rootfs_sha256] | join(" ")' "$STATE_FILE")" = "edge phosh 2026-11-13T23:32:00 $(sum "$root/boot.raw") $(sum "$root/rootfs.raw")" ] \
  || fail "the Nura build and image hashes were not recorded"
[ "$(grep -c under "$STATE_LOG")" = 5 ] || fail "writes not all recorded"
! grep -v 'under nura-in-progress' "$STATE_LOG" | grep -v 'reboot' > /dev/null || fail "a write ran outside nura-in-progress: $(cat "$STATE_LOG")"
backup=$(jq -r .backup "$STATE_FILE")
case $backup in "$cache"/backups/ABC123/backup-*) ;; *) fail "unexpected backup path $backup" ;; esac
for f in sdcard.tar manifest.sha256 contacts.txt sms.txt call-log.txt calendar.txt; do
  [ -s "$backup/$f" ] || [ "$f" = sdcard.tar ] || fail "backup lacks $f"
done
[ "$(tar -xOf "$backup/sdcard.tar" './sub dir/b c.txt')" = nested ] || fail "tar lacks a file"
[ "$(stat -c %a "$backup/sdcard.tar")" = 600 ] || fail "backup is not private"
for n in contacts.txt sms.txt call-log.txt calendar.txt; do grep -q "$n: 2 rows" "$root/out" || fail "no row count for $n"; done
! grep -q 'rehearsal' "$root/out" || fail "the run still rehearses the backup"
! grep -q 'holding Power restarts' "$root/out" || fail "the fastboot step still offers the restart by holding Power"
! grep -q 'USB debugging off' "$root/out" || fail "a USB debugging reminder is back"
grep -q 'never relock' "$root/out" || fail "no relock warning"
[ -z "$(find "$cache" -perm /077 -not -type l)" ] || fail "something in the cache is not private: $(find "$cache" -perm /077)"
# The sections and their steps, in order.
[ "$(sections_of "$root/out.install")" = "[Preflight] 1/17 2/17 3/17 4/17 5/17 6/17 7/17 8/17 [Prepare] 9/17 10/17 11/17 12/17 [Install] 13/17 14/17 [Finish] 15/17 16/17 17/17" ] \
  || fail "sections out of order: $(sections_of "$root/out.install")"
order=$(grep -n -E '^== Step ' "$root/out" | sed -E 's/^[0-9]*:== Step [0-9]+ of [0-9]+ · (YOU|TOOL): //; s/ +POINT OF NO RETURN$//' | tr '\n' '|')
[ "$order" = "Check this computer|Check the phone over adb|Put the phone in fastboot|Check the bootloader|Find Nura's builds for the phone|Choose Nura's interface|Restart into Android|Find, download and verify the images|Confirm what is lost|Back up the phone|Put the phone in fastboot|Check the phone in fastboot|Approve the install|Install Nura|Approve the reboot|Restart the phone|Summarize|" ] \
  || fail "steps out of order: $order"
actors=$(grep -E '^== Step ' "$root/out" | sed -E 's/^.* · (YOU|TOOL): .*/\1/' | tr '\n' ' ')
[ "$actors" = "TOOL TOOL YOU TOOL TOOL YOU YOU TOOL YOU TOOL YOU TOOL YOU TOOL YOU TOOL TOOL " ] || fail "actors out of order: $actors"
grep -q '^== Step 1 of 17 · TOOL: Check this computer$' "$root/out" || fail "steps are not numbered"
grep -q '^== Step 13 of 17 · YOU: Approve the install  *POINT OF NO RETURN$' "$root/out" || fail "the approval is not the marked point of no return"
grep -q '^== Step 17 of 17 · TOOL: Summarize$' "$root/out" || fail "the last step is not 17 of 17"
grep -q 'Hold Power, tap Power off' "$root/out" && grep -q 'Hold Volume Down + Power until the fastboot screen shows' "$root/out" || fail "no fastboot instruction"
grep -q '^  \.\.\. waiting for the phone in fastboot mode (up to 0 s)' "$root/out" || fail "no waiting line"
[ "$(grep -n 'never relock' "$root/out" | head -1 | cut -d: -f1)" -lt "$(grep -n 'Install Nura runs' "$root/out" | cut -d: -f1)" ] \
  || fail "the warnings come after the gate"
# The transcript.
[ "$(ls "$cache/logs" | wc -l)" = 1 ] || fail "expected one transcript"
transcript=$cache/logs/$(ls "$cache/logs")
case $transcript in *-run.log) ;; *) fail "transcript not named for the run: $transcript" ;; esac
[ "$(stat -c %a "$transcript")" = 600 ] || fail "transcript is not private"
grep -q 'run: .*fastboot.* flash boot' "$transcript" || fail "transcript lacks the write command"
grep -q 'check serial: ABC123' "$transcript" || fail "transcript lacks the serial check"
grep -q 'choice: install nura' "$transcript" || fail "transcript lacks the gate"
grep -q 'outcome: finished' "$transcript" || fail "transcript lacks the outcome"
! grep -E '^(run|exit|output|check):' "$transcript" | grep -q 147147 || fail "the password is in the transcript's commands"
[ "$(grep -c 147147 "$transcript")" = "$(grep 147147 "$transcript" | grep -ci 'log in as\|user / 147147\|Log in: ')" ] || fail "the password appears in the transcript outside the login instructions"

# Cached files are re-hashed, never trusted.
again
[ "$rc" = 0 ] || fail "cached rerun exited $rc"
grep -q 'cached nura-boot.img.xz: hash matches' "$root/out" || fail "cache not reused"
printf 'x' >> "$cache/downloads/nura-boot.img.xz"
printf 'damaged' > "$cache/downloads/nura-rootfs.img"
again
[ "$rc" = 0 ] || fail "rerun over damaged cache exited $rc"
grep -q 'download nura-boot.img.xz: ' "$root/out" || fail "a damaged download was not fetched again"
[ "$(sum "$cache/downloads/nura-boot.img.xz")" = "$(sum "$root/srv/nura-boot.img.xz")" ] || fail "the cached download is still damaged"
[ "$(cat "$FLASHED/userdata")" = nura-rootfs ] || fail "a damaged unpacked image was flashed"

# The slot written to the record is the phone's.
fresh_cache
FAKE_SUFFIX=_b FAKE_SLOT=b run "$fresh_in"
[ "$rc" = 0 ] || fail "slot b install exited $rc"
[ "$(jq -r .slot "$STATE_FILE")" = b ] || fail "the record does not hold the phone's slot b"

# CRLF line endings from adb shell are handled.
fresh_cache
FAKE_CRLF=1 run "$fresh_in"
[ "$rc" = 0 ] || fail "CRLF adb output exited $rc"

# ... also with a path over 100 bytes, which the tar writes a long-name entry for.
longdir=$root/sdcard/$(printf 'd%.0s' $(seq 60))/$(printf 'e%.0s' $(seq 60))
mkdir -p "$longdir" && echo hi > "$longdir/f"
NURA_INSTALL_CACHE=$root/cache2 run "$fresh_in"
[ "$rc" = 0 ] || fail "the install with a long path exited $rc"
[ "$(planned "$root/cache2")" = "$(stat -c %s "$root"/cache2/backups/ABC123/backup-*/sdcard.tar)" ] || fail "the planned tar length $(planned "$root/cache2") differs from the stream with a long path"
rm -rf "$root/sdcard/ddd"* "$root/cache2"

## Arguments

for args in "--os nura" "--os" "nura" "-x"; do
  run '' $args
  [ "$rc" = 2 ] || fail "nura-install $args exited $rc, not 2"
  grep -q 'unexpected argument' "$root/out" || fail "nura-install $args gave no usage error"
done
run '' --help
[ "$rc" = 0 ] || fail "--help exited $rc"

## Android phase stops

stop_fresh() { # name, message: a stop before any write or record
  fresh_cache
  run "$fresh_in"
  refused "$1" "$2"
  untouched_fresh "$1"
}
stop_early() { # a failed Preflight check: no confirmation was asked, nothing was backed up
  stop_fresh "$@"
  grep -q 'Not ready: ' "$root/out" || fail "$1: no verdict"
  no_backup "$1"
  ! grep -q 'Confirm what is lost' "$root/out" || fail "$1: went on to the next section"
}
FAKE_ACCOUNTS=1 stop_early "signed-in account" 'accounts are signed in'
FAKE_LEVEL=49 stop_early "low battery" 'battery is at 49%'
# A stop after adb was used ends with the USB debugging reminder and the record's state.
! grep -q 'USB debugging off' "$root/out" || fail "a USB debugging reminder is back after a stop"
grep -q 'no record' "$root/out" || fail "no word on the record after a stop"
FAKE_DUMPSYS_BAD=1 stop_early "unparsable dumpsys" 'no `Accounts:` line'
FAKE_SUFFIX=_c stop_early "odd slot suffix" 'unexpected slot suffix'
FAKE_ADB_SERIAL=OTHER stop_early "serial mismatch" 'reports serial OTHER'
FAKE_ADB=unauthorized stop_early "unauthorized phone" 'RSA'
FAKE_ADB=offline stop_early "offline phone" 'adb lists the phone as offline'
FAKE_ADB_EXTRA=1 stop_early "two phones over adb" '2 adb devices are connected'

## Typed confirmations: each is required, in order, before any backup or write

confirmation_stop() { # name, input
  fresh_cache
  run "$2"
  [ "$rc" = 1 ] || fail "$1 exited $rc"
  no_backup "$1"
  no_writes "$1"
  [ ! -e "$cache/phones" ] || fail "$1: wrote a record"
}
confirmation_stop "end of input at Confirm" ''
confirmation_stop "stop at Confirm" "${choice}stop\n"
confirmation_stop "near-miss Confirm" "${choice}confirm\nCONFIRM\n"
[ "$(grep -c 'That is not' "$root/out")" = 2 ] || fail "typo not re-asked"
confirmation_stop "stop at the interface choice" 'stop\n'
confirmation_stop "an interface that is not offered" 'kde\nPhosh\n'
[ "$(grep -c 'That is not one of phosh, plasma, gnome, sxmo' "$root/out")" = 2 ] || fail "a wrong interface was not asked again"

## Empty backups need a typed acknowledgement

fresh_cache
FAKE_CONTENT_EMPTY=content://sms run "$fresh_in"
[ "$rc" = 1 ] || fail "empty SMS export without acknowledgement exited $rc"
no_writes "empty SMS"
grep -q 'sms.txt: 0 rows' "$root/out" || fail "an empty export was not counted"
fresh_cache
FAKE_CONTENT_EMPTY=content://sms run "${confirmations}Confirm\ninstall nura\nreboot\n"
[ "$rc" = 0 ] || fail "acknowledged empty SMS export exited $rc"
fresh_cache
FAKE_CONTENT_EMPTY=content://sms run "${confirmations}confirm\n"
[ "$rc" = 1 ] || fail "a near-miss Confirm was accepted"
no_writes "near-miss Confirm"

## The phone's data stops

FAKE_CONTENT_DENY=stderr stop_fresh "denied content query" 'failed or was denied'
FAKE_CONTENT_DENY=stdout stop_fresh "content error as stdout" 'neither rows nor an explicit empty result'
FAKE_CONTENT_DENY=exit stop_fresh "content query exit code" 'failed or was denied'
FAKE_BAD_MANIFEST=1 stop_fresh "backup hash mismatch" 'does not match the phone'
FAKE_UNREADABLE=1 stop_fresh "unreadable files" 'could not read 1 files'
FAKE_UNREADABLE=quiet stop_fresh "unreadable files with exit 0" 'could not read 1 files'
grep -q Permission "$root/out" && fail "unreadable file names leaked into the output"

## Preflight: read-only, runs every check it can, reports all failures together, then stops

fb_only='^(devices|-s [A-Z0-9]+ getvar [a-z:_-]+)$'
preflight_clean() { # name: nothing written, recorded or stored for the install
  no_writes "$1"
  # only where the phone is, or (a failure after the bootloader check) the one `getvar all` read as well
  [ ! -s "$LOG" ] || [ "$(cat "$LOG")" = "devices
$fb_all" ] || fail "$1: fastboot was asked more than the early read: $(cat "$LOG")"
  [ ! -e "$cache/phones" ] || fail "$1: wrote a record"
  [ ! -e "$cache/backups" ] || fail "$1: a backup directory exists"
  no_backup "$1"
}
pre_fail() { # name, message(s) that must be reported
  local name=$1
  shift
  fresh_cache
  run "$fresh_in"
  [ "$rc" = 1 ] || fail "$name exited $rc, not 1"
  for m in "$@"; do grep -qF -- "$m" "$root/out" || fail "$name not reported as: $m"; done
  grep -q '^Not ready: ' "$root/out" || fail "$name gave no verdict"
  grep -q 'Preflight not ready' "$root/out" || fail "$name did not stop at the Preflight"
  ! grep -q 'Confirm what is lost' "$root/out" || fail "$name went on past the Preflight"
  preflight_clean "$name"
}

FAKE_ADB=unauthorized pre_fail "unauthorized phone" 'unauthorized' '[skip] every other adb check'
# Several failures at once are all reported, nothing stops early, nothing is written.
FAKE_ADB_SERIAL=OTHER1 FAKE_ACCOUNTS=1 FAKE_LEVEL=10 \
  pre_fail "several failures over adb" 'ro.serialno: the phone reports serial OTHER1' 'accounts are signed in' 'battery is at 10%'
grep -q '^Not ready: 3 checks failed, 0 skipped' "$root/out" || fail "wrong count of failures over adb: $(grep '^Not ready' "$root/out")"
! grep -q 'Put the phone in fastboot' "$root/out" || fail "the owner was sent to fastboot with the adb checks failed"
make_index "$fp6_images"' |= (if (.name | endswith("-boot.img.xz") | not) then .sha256 = "'"$other_sha"'" else . end)'
pre_fail "a download that does not hash to Nura's index" "nura-rootfs.img.xz does not match Nura's index"
grep -q '^Not ready: 1 check failed, 0 skipped' "$root/out" || fail "wrong count of failures: $(grep '^Not ready' "$root/out")"
[ "$(sed -n '/^What each one means/,$p' "$root/out" | grep -c '^  - ')" = 1 ] || fail "not every failure is explained"
grep -q 'Step 8 of 17 .*Find, download and verify the images' "$root/out" || fail "the Preflight did not reach its last step"
make_index
# The bootloader is read first, before any download or backup: every wrong answer is reported at once, then the run stops.
fresh_cache
FAKE_PSIZE_MISSING=boot_a FAKE_UNLOCKED=no FAKE_FB_BATTERY=no run "$fresh_in"
[ "$rc" = 1 ] || fail "a wrong bootloader exited $rc"
for m in 'no value for `getvar partition-size:boot_a`' 'unlocked: no' 'battery-soc-ok: no'; do
  grep -qF -- "$m" "$root/out" || fail "the early bootloader check did not report: $m"
done
grep -q '^Not ready: 3 checks failed' "$root/out" || fail "wrong count after the bootloader check: $(grep '^Not ready' "$root/out")"
! grep -q 'Restart into Android' "$root/out" || fail "the owner was sent back to Android after a failed bootloader check"
[ ! -e "$cache/downloads" ] || fail "something was downloaded before the bootloader was checked"
no_backup "a failed bootloader check"
no_writes "a failed bootloader check"
[ ! -e "$cache/phones" ] || fail "a failed bootloader check wrote a record"
# A phone that is not on Android needs the owner first: the run waits for it in a step of its own.
fresh_cache
FAKE_ADB_AFTER=1 NURA_INSTALL_WAIT_SECONDS=10 run "$fresh_in"
[ "$rc" = 0 ] || fail "the run that waited for the phone exited $rc"
[ "$(sections_of "$root/out")" = "[Preflight] 1/18 2/18 3/18 4/18 5/18 6/18 7/18 8/18 9/18 [Prepare] 10/18 11/18 12/18 13/18 [Install] 14/18 15/18 [Finish] 16/18 17/18 18/18" ] || fail "sections with the wait: $(sections_of "$root/out")"
grep -q '^== Step 2 of 18 · YOU: Connect the phone with USB debugging$' "$root/out" || fail "no step to connect the phone"
grep -q '^  \.\.\. waiting for the phone over adb (up to 10 s)' "$root/out" || fail "the wait for adb was not named"
# A YOU step ends when the phone shows its result: the restart step stays open until the phone is back over adb.
fresh_cache
FAKE_ADB_BACK_AFTER=2 NURA_INSTALL_WAIT_SECONDS=20 run "$fresh_in"
[ "$rc" = 0 ] || fail "the run that waited for the phone to come back exited $rc"
awk '
  /^== Step 7 of 17 · YOU: Restart into/ { in5 = 1; next }
  /^== Step 8 of 17/ { in5 = 0 }
  in5 && /waiting for the phone over adb/ { waited = 1 }
  in5 && /^  Step 7 of 17 done/ { done5 = $0 }
  END { if (!waited || done5 ~ / 0\.[0-9] s$/) { print "waited=" waited " done=" done5; exit 1 } }' "$root/out" || fail "the restart step ended before the phone was back over adb"
fresh_cache
run "$choice"
grep -q '  Preflight passed: ' "$root/out" || fail "the Preflight did not pass: $(grep -E 'FAIL|Not ready' "$root/out")"
cp "$root/out" "$root/out.preflight"
plain_output "preflight" "$root/out.preflight"
step_blocks "preflight" "$root/out.preflight"
# curl is called with -q first, no redirects and https only (file only where the test allows it).
curls=$(grep -v '^--version$' "$CURL_LOG" || true)
[ -n "$curls" ] || fail "the fake curl was not used"
! printf '%s\n' "$curls" | grep -v '^-q ' || fail "curl was not given -q first: $curls"
[ "$(printf '%s\n' "$curls" | grep -c -- '--max-redirs 0 --proto-redir =https --proto =https,file')" = "$(printf '%s\n' "$curls" | wc -l)" ] || fail "curl lacks the redirect or protocol limits: $curls"

# ... and gives up on a stalled connection or transfer, with no total time limit for a transfer.
transfers=$(printf '%s\n' "$curls" | grep -v -- ' -sI ' || true)
[ -n "$transfers" ] || fail "no transfer was logged"
[ "$(printf '%s\n' "$transfers" | grep -c -- '--connect-timeout 30 --speed-limit 1024 --speed-time 60')" = "$(printf '%s\n' "$transfers" | wc -l)" ] || fail "a transfer lacks the stall limits: $transfers"
! printf '%s\n' "$transfers" | grep -q -- '--max-time' || fail "a transfer has a total time limit"
printf '%s\n' "$curls" | grep -- ' -sI ' | grep -qv -- '--connect-timeout 30 --max-time 60' && fail "a HEAD request lacks its time limits: $curls"

## Fastboot checks

fb_fail() { # name, message: a wrong answer stops the run before anything is written or recorded
  go
  refused "$1" "$2"
  [ ! -e "$cache/phones" ] || fail "$1: wrote a record"
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
FAKE_PSIZE_GARBAGE=userdata_a fb_fail "garbage where absence is expected" 'neither a size nor "not found"'
FAKE_PSIZE_FLIP=2 fb_fail "a size that disappears after the gate" 'no value for `getvar partition-size:boot_a`'
FAKE_PSIZE=boot_a=0x4 fb_fail "an image that does not fit its partition" 'the phone.s boot_a holds 4'
FAKE_DUP=unlocked fb_fail "contradictory answers" 'unlocked` more than once'
FAKE_GETVAR_FAIL=unlocked fb_fail "getvar exit status" 'getvar all` failed'
FAKE_SWAP_FROM=3 fb_fail "swap before writing" 'XYZ789'
FAKE_FB_EXTRA=1 fb_fail "several phones" '2 fastboot devices are connected'
FAKE_FB_STATE=adb fb_fail "a device that is not in fastboot" 'not fastboot'
FAKE_FB_NOPERM=1 fb_fail "a device without permission" 'no permission'
FAKE_DEVICES_FAIL=1 fb_fail "failed fastboot devices" 'fastboot devices` failed'
fresh_cache
FAKE_FB_SERIAL=XYZ789 run "$fresh_in"
refused "wrong serial after the adb phase" 'is XYZ789, not ABC123'
[ ! -e "$cache/phones" ] || fail "wrong serial wrote a record"

## Gates

fresh_cache
run "${confirmations}"'install\nInstall Nura\nINSTALL NURA\nuninstall nura\ninstall nura\nreboot\n'
[ "$rc" = 0 ] || fail "typos then the phrase exited $rc"
[ "$(grep -c 'That is not one of install nura' "$root/out")" = 4 ] || fail "typos not all re-asked"
fresh_cache
run "${confirmations}install nura\nstop\n"
[ "$rc" = 1 ] || fail "stop at the reboot gate exited $rc"
grep -q 'stopped at the `reboot` gate' "$root/out" || fail "no reboot gate report"
[ "$(state)" = nura-installed ] || fail "reboot gate changed the record"
! grep -q reboot "$LOG" || fail "rebooted after stop"
fresh_cache
run "${confirmations}stop\n"
refused "stop at the first gate of a fresh run" 'unchanged Android'
[ ! -e "$cache/phones" ] || fail "a stopped fresh run left a record"
fresh_cache
run "$confirmations"
refused "end of input at the gate" 'stopped at the `install nura` gate'

# Gates and confirmations read only a terminal; without the piped-input allowance nothing starts.
fresh_cache
NURA_INSTALL_ALLOW_PIPED_INPUT= run "$fresh_in"
refused "piped input on a fresh install" 'interactive terminal'
! grep -q 'getprop\|exec-out' "$ADB_LOG" || fail "a run without a terminal went on to check the phone"

## Failed write, interrupted group, a phone that vanishes

FAKE_FAIL0=dtbo go
[ "$rc" = 1 ] || fail "a write that printed FAILED and exited 0 let the run go on (exit $rc)"
grep -q 'fastboot exited 0 but printed `.*FAILED' "$root/out" || fail "the FAILED line was not named: $(tail -5 "$root/out")"
! grep -q ' flash ' "$LOG" || fail "writes went on after a FAILED line"
grep -q 'output: .*FAILED (remote' "$cache/logs/"* || fail "fastboot's output is not in the transcript"
grep -q 'fastboot getvar partition-size:userdata_a: .*Variable Not found' "$cache/logs/"* || fail "a getvar answer is not in the transcript"
FAKE_FAIL=userdata go
[ "$rc" = 1 ] || fail "failed write exited $rc"
grep -q 'Writes that ran: erase dtbo; erase vendor_boot' "$root/out" || fail "no report of the writes that ran"
grep -q 'The record for phone ABC123 says `nura-in-progress`' "$root/out" || fail "no state in the failure report"
[ "$(state)" = nura-in-progress ] || fail "failed write left $(state)"
! grep -Eq 'flash boot|reboot' "$LOG" || fail "kept going after a failed write"

FAKE_HANG=userdata NURA_INSTALL_WRITE_TIMEOUT_SECONDS=1 go
[ "$rc" = 1 ] || fail "a hung write exited $rc"
grep -q 'did not finish within 1 seconds and was killed' "$root/out" || fail "a hung write was not reported"
[ "$(state)" = nura-in-progress ] || fail "a hung write left $(state)"
! grep -Eq 'flash boot|reboot' "$LOG" || fail "kept going after a hung write"

FAKE_SIG_SLEEP=1 FAKE_SIG_ON=boot go
[ "$rc" = 1 ] || fail "interrupted install exited $rc"
grep -q 'interrupted while writing' "$root/out" || fail "no interruption report"
[ "$(grep -c 'interrupt noted; finishing this group first' "$root/out")" = 1 ] || fail "the held interrupt was not noted exactly once"
! grep -q reboot "$LOG" || fail "went on to the next gate after an interrupt"
[ "$(state)" = nura-installed ] || fail "interrupt left $(state)"
[ "$(writes | wc -l)" = 4 ] || fail "the group did not finish after the interrupt: $(writes)"
mask=$(sed -n 's/^SigBlk:\s*//p' "$SIG_LOG" | head -1)
[ $(( 0x$mask & 0x4003 )) = $(( 0x4003 )) ] || fail "fastboot did not inherit the blocked signals ($mask)"

## Nura's index

make_index
rm -rf "$cache/downloads"
cp "$root/srv/nura-rootfs.img.xz" "$root/good.xz"
printf 'x' >> "$root/srv/nura-rootfs.img.xz"
again
refused "corrupt download" "does not match Nura's index"
[ ! -e "$cache/downloads/nura-rootfs.img.xz" ] || fail "a corrupt download was kept"
cp "$root/good.xz" "$root/srv/nura-rootfs.img.xz"
# A hash the index gets wrong stops the run before anything is written.
make_index "$fp6_images"' |= (if (.name | endswith("-boot.img.xz")) then .sha256 = "'"$other_sha"'" else . end)'
rm -rf "$cache/downloads"
again
refused "a wrong sha256 in the index" "nura-boot.img.xz does not match Nura's index"
! grep -q 'Approve the install' "$root/out" || fail "the gate was shown with a download that did not verify"
make_index
# The newest build of the chosen interface in the newest release is the one installed, and it is shown.
again
[ "$rc" = 0 ] || fail "the newest build exited $rc"
[ "$(cat "$FLASHED/userdata")" = nura-rootfs ] || fail "an older build was installed"
grep -q 'Nura build: edge phosh 2026-11-13T23:32:00' "$root/out" || fail "the build was not shown"
grep -q 'edge phosh 2026-11-13T23:32:00' "$STATE_FILE" || fail "the build was not recorded"
grep -q 'index.json' "$CURL_LOG" || fail "the index was not fetched this run"
rel_img() { jq -n --arg n "$1" --arg u "file://$root/srv/rel-$2.img.xz" --arg s "$(sum "$root/srv/rel-$2.img.xz")" '{name: $n, timestamp: "2026-12-01T10:00:00", url: $u, sha256: $s}'; }
newer_release='.releases += [{name: "v27.01", pretty_name: "v27.01", devices: [{name: "fairphone-fp6", pretty_name: "x", interfaces: [{name: "phosh", pretty_name: "Phosh", images: ['"$(rel_img 20261201-1000-nura-v27.01-phosh-fairphone-fp6-boot.img.xz boot)"', '"$(rel_img 20261201-1000-nura-v27.01-phosh-fairphone-fp6.img.xz rootfs)"']}]}]}]'
make_index "$newer_release"
again
[ "$rc" = 0 ] || fail "a newer release exited $rc"
[ "$(cat "$FLASHED/userdata")" = rel-rootfs ] || fail "the newer release's build was not installed"
grep -q 'Nura build: v27.01 phosh 2026-12-01T10:00:00' "$root/out" || fail "the newer release was not shown"
# Only builds that exist count: without the newer ones the older build is installed.
make_index "del($fp6_images | select(.timestamp | startswith(\"2026-11-13\")))"
again
[ "$rc" = 0 ] || fail "the older build exited $rc"
[ "$(cat "$FLASHED/userdata")" = old-rootfs ] || fail "the older build was not installed when it is the newest listed"
# The interfaces offered are the ones the index lists for the phone; unknown ones are listed by name.
make_index '.releases |= map(.devices |= map(if .name == "fairphone-fp6" then .interfaces += [(.interfaces[] | select(.name == "phosh") | .name = "lomiri")] | .interfaces |= map(select(.name != "sxmo-de-sway")) else . end))'
run "sxmo\nlomiri\nConfirm\nbackup\ninstall nura\nreboot\n"
[ "$rc" = 0 ] || fail "an interface the index adds exited $rc"
grep -q '^  lomiri ' "$root/out" || fail "an unknown interface was not listed by name"
! grep -q '^  sxmo ' "$root/out" || fail "an interface the index lacks was offered"
[ "$(grep -c 'That is not one of phosh, plasma, gnome, lomiri' "$root/out")" = 1 ] || fail "a missing interface was not asked again"
grep -q 'Nura build: edge lomiri ' "$root/out" || fail "the unknown interface's build was not shown"
make_index 'del(.releases[].devices[] | select(.name == "fairphone-fp6"))'
again
refused "an index with no builds for the phone" 'lists no images for fairphone-fp6'
make_index "$fp6_images"'.url |= sub("^file://.*"; "https://evil.example/x.img.xz")'
again
refused "foreign host" 'downloads may only come from'
make_index
NURA_INSTALL_NURA_INDEX=https://evil.example/index.json again
refused "an index from a foreign host" 'downloads may only come from'
NURA_INSTALL_NURA_INDEX=file://$root/missing.json again
refused "an index that cannot be fetched" 'missing.json'
echo 'not json' > "$root/index.json"
again
refused "an index that is not JSON" "reading Nura's index"
make_index
NURA_INSTALL_ALLOW_FILE_URLS= again
refused "file urls by default" 'only https downloads are allowed'
for empty in url sha256; do
  make_index "$fp6_images"'.'"$empty"' = ""'
  again
  refused "empty $empty in the index" 'no URL or no valid sha256'
done
make_index

# Images are hashed again right before the first write, after the gate.
rm -rf "$cache/downloads"
FAKE_TAMPER=$cache/downloads/nura-boot.img again
refused "an image changed after the gate" 'nura-boot.img does not match the hash taken when it was unpacked'

## The reboot gate asks only who the phone is: erasing may change the snapshot status or the battery answer

FAKE_AFTER_SNAPSHOT=merging FAKE_AFTER_BATTERY=no again
[ "$rc" = 0 ] || fail "answers that changed after the writes stranded the run (exit $rc)"
grep -q ' reboot$' "$LOG" || fail "the phone was not rebooted after harmless changes"
FAKE_AFTER_UNLOCKED=no again
[ "$rc" = 1 ] || fail "a phone that relocked after the writes exited $rc"
grep -q 'the phone reports `unlocked: no`' "$root/out" || fail "the relock was not named"
! grep -q ' reboot$' "$LOG" || fail "rebooted a phone that is no longer unlocked"

## A screen that stopped taking output (terminal closed) must not stop a group half way

fresh_cache
rm -rf "$FLASHED" "$LOG" "$COUNT" "$PCOUNT" "$STATE_LOG" "$WROTE"
mkdir -p "$FLASHED"
: > "$LOG"
printf '%b' "$fresh_in" | "$tool" 2> "$root/err" | head -c 0 || true
! grep -q panicked "$root/err" || fail "a closed screen panicked: $(cat "$root/err")"
grep -q ' flash boot ' "$LOG" && grep -q ' reboot$' "$LOG" || fail "a closed screen cut the writes short: $(cat "$LOG")"
[ "$(state)" = nura-installed ] || fail "a closed screen left the record at $(state)"

## Host prerequisites

fresh_cache
NURA_INSTALL_ADB=$root/missing run "$choice"
refused "a missing prerequisite" 'prerequisite'
NURA_INSTALL_FASTBOOT=/bin/false run "$choice"
refused "a prerequisite that fails" 'prerequisite /bin/false does not run'
NURA_INSTALL_NEEDED_GIB=999999 run "$choice"
refused "too little disk space" 'GiB free'
chmod 755 "$cache"
run "$choice"
refused "cache not private" 'private'
chmod 700 "$cache"
rc=0
flock "$cache/lock" "$tool" > "$root/out" 2>&1 < /dev/null || rc=$?
refused "concurrent run" 'another nura-install'

## Terminal behaviour: spinner on a terminal only, progress in the transcript, Ctrl-C, typed-ahead input

fresh_cache
NURA_INSTALL_PROGRESS_SECONDS=1 FAKE_SLOW_TAR=3 run "$confirmations"
[ "$rc" = 1 ] || fail "slow backup exited $rc"
plain_output "slow backup" "$root/out"
transcript=$cache/logs/$(ls "$cache/logs")
[ "$(grep -c -E '^  \.\.\. copy /sdcard: .* of 4\.5 KiB \([0-9]+%\)' "$transcript")" -ge 2 ] \
  || fail "the transcript has no periodic progress lines for the slow stream: $(grep '\.\.\.' "$transcript")"
! grep -q -E '^  \.\.\. copy /sdcard: ' "$root/out" || fail "progress lines went to a pipe"
! LC_ALL=C grep -q -P '\xE2[\xA0-\xA3][\x80-\xBF]|\x1b' "$transcript" || fail "a spinner frame is in the transcript"

# Menus on a terminal: arrow keys and Enter on a pty. Enter has a security delay of 3 s after a menu is drawn and 1 s after each move
# of the cursor in the shipped build; test hooks shorten both.
DOWN=$'\x1b[B'
UP=$'\x1b[A'
ENTER=$'\r'
security_delay_ms=1500
move_delay_ms=700
pty_begin() { # env assignments for the run
  fresh_cache
  rm -f "$COUNT" "$PCOUNT" "$root/tty-in"
  : > "$LOG"
  mkfifo "$root/tty-in"
  : > "$root/pty"
  env TERM=xterm NURA_INSTALL_SECURITY_DELAY_MS=$security_delay_ms NURA_INSTALL_SECURITY_DELAY_AFTER_MOVE_MS=$move_delay_ms "$@" script -qefc "$tool" /dev/null < "$root/tty-in" > "$root/pty" 2>&1 &
  spid=$!
  exec 7> "$root/tty-in"
}
pick_with() { # the cursor moves (arrows), then Enter once both locks are over
  sleep 1.8
  printf '%s' "$1" >&7
  sleep 0.9
  printf '%s' "$ENTER" >&7
}
menu_at() { wait_for "$root/pty" "$1"; } # a menu's own text on screen
pty_end() { rc=0; wait_pty; exec 7>&-; }
chose() { grep -qF -- "  chosen: $1" "$root/pty" || fail "the menu did not choose: $1"; }

# The security delays: Enter is ignored while the countdown runs (and dropped, not queued), with no cursor, and for a moment after a move.
pty_begin FAKE_SLOW_TAR=2
menu_at 'choose'
printf '%s' "$ENTER" >&7
sleep 1.9
grep -q 'chosen:' "$root/pty" && fail "Enter during the lock chose something (or was queued)"
LC_ALL=C grep -aqF "$(printf '\033[2menter select\033[0m')" "$root/pty" || fail "enter was not dimmed while it is on its security delay"
grep -aqF 'choose · enter select' "$root/pty" || fail "enter was not made normal again after the security delay"
printf '%s' "$ENTER" >&7
sleep 0.5
grep -q 'chosen:' "$root/pty" && fail "Enter without a cursor chose something"
! LC_ALL=C grep -aq -P '\x1b\[1m  \xE2\x9D\xAF' "$root/pty" || fail "a menu started with a cursor on an option"
printf '%s' "$DOWN$ENTER" >&7
sleep 0.4
grep -q 'chosen:' "$root/pty" && fail "Enter right after a move of the cursor chose something"
sleep 0.9
printf '%s' "$ENTER" >&7
menu_at 'chosen: Phosh'
menu_at 'The screen lock is off (confirm)'
pick_with "$DOWN"
menu_at 'Back up the phone'
pick_with "$DOWN"
menu_at 'Install Nura (erases Android and all its data)'
pick_with "$UP"
pty_end
[ "$rc" = 1 ] || fail "backup then Stop at the install gate exited $rc"
chose 'Stop the run'
no_writes "Stop at the install gate"
# On a terminal the spinner shows on screen and never reaches the transcript.
LC_ALL=C grep -q -P '\r\x1b\[K  \xE2[\xA0-\xA3][\x80-\xBF] co[^ ]*  \xE2\x96[\x88\x91]' "$root/pty" || fail "no bar on the terminal"
grep -q "$(printf '\033\[K')" "$root/pty" || fail "the spinner was not erased in place"
transcript=$cache/logs/$(ls "$cache/logs")
[ -z "$(tr -d -c '\r\033' < "$transcript")" ] || fail "escape bytes in the transcript of a terminal run"
! LC_ALL=C grep -q -P '\xE2[\xA0-\xA3][\x80-\xBF]' "$transcript" || fail "a spinner frame is in the transcript of a terminal run"
grep -qE '^  \[ ok \] copy /sdcard: ' "$transcript" || fail "the finished line is missing from the transcript"
grep -qE '^choice: phosh \(menu open [0-9.]+ s\)$' "$transcript" || fail "the transcript lacks the choice and how long the menu was open"
grep -qE '^choice: stop \(menu open' "$transcript" || fail "the transcript lacks the stop"
# The terminal title (OSC 2) shows the step and its clocks on a terminal, ends with the outcome, and is never in the transcript.
LC_ALL=C grep -aq -P '\x1b\]2;[^\x07]* nura 10/17 Back up the phone[^\x07]* · [0-9]+:[0-9]{2} \([0-9]+:[0-9]{2}\)\x07' "$root/pty" || fail "no working title on the terminal: $(LC_ALL=C grep -a -o -P '\x1b\]2;[^\x07]*' "$root/pty" | head -3 | cat -v)"
LC_ALL=C grep -aq -P '\x1b\]2;⏸ nura 13/17 choose an option · ' "$root/pty" || fail "no waiting title at the gate"
LC_ALL=C grep -aq -P '\x1b\]2;✗ nura stopped at 13/17 · [0-9]+:[0-9]{2}\x07' "$root/pty" || fail "no closing title on the terminal"
! LC_ALL=C grep -aq -P '\x1b\]2;' "$transcript" || fail "a title escape is in the transcript"

# The first security delay outlasts the one after a move: Enter after the cursor moved long ago is still ignored.
pty_begin
menu_at 'choose'
printf '%s' "$DOWN" >&7
sleep 1.0
printf '%s' "$ENTER" >&7
sleep 0.2
grep -q 'chosen:' "$root/pty" && fail "Enter within the first security delay selected something"
sleep 0.8
printf '%s' "$ENTER" >&7
menu_at 'chosen: Phosh'
menu_at 'The screen lock is off (confirm)'
pick_with "$UP"
pty_end
[ "$rc" = 1 ] || fail "the first-security-delay case exited $rc"

# Up first puts the cursor on Stop; arrows move the cursor during the first lock; Ctrl-C works during it.
pty_begin
menu_at 'choose'
pick_with "$UP"
pty_end
[ "$rc" = 1 ] || fail "Up then Enter at the interface menu exited $rc"
chose 'Stop the run'
no_writes "Stop at the interface menu"
pty_begin
menu_at 'choose'
printf '%s' "$DOWN" >&7
sleep 0.3
LC_ALL=C grep -aq -P '\x1b\[1m  \xE2\x9D\xAF \(\xE2\x80\xA2\) Phosh' "$root/pty" || fail "an arrow during the lock did not move the cursor"
printf '\003' >&7
pty_end
[ "$rc" = 1 ] || fail "Ctrl-C during the lock exited $rc"
grep -q 'chosen: Stop the run' "$root/pty" || fail "Ctrl-C during the lock did not stop the menu"
no_writes "Ctrl-C during the lock"

# Skipping the backup: a menu before anything is read, then a second one that gives the data up; no partial files.
pty_begin
menu_at 'choose'
pick_with "$DOWN"
menu_at 'The screen lock is off (confirm)'
pick_with "$DOWN"
menu_at 'Skip the backup'
pick_with "$DOWN$DOWN"
menu_at 'Go on without a backup (the data is gone for good)'
pick_with "$DOWN"
menu_at 'Install Nura (erases Android and all its data)'
pick_with "$DOWN"
menu_at 'Reboot the phone (all writes are done)'
pick_with "$DOWN"
pty_end
[ "$rc" = 0 ] || fail "going on without a backup exited $rc"
! grep -q 'streaming the phone' "$root/pty" || fail "the phone was read although the backup was skipped"
[ ! -e "$cache/backups" ] || fail "a backup directory exists although no backup was taken"
grep -q 'NO BACKUP WAS TAKEN: Android and its data are gone for good' "$root/pty" || fail "the summary does not say no backup was taken"
grep -q 'no backup taken: skipped' "$cache/logs/"* || fail "the transcript does not say no backup was taken"
grep -q ' flash boot ' "$LOG" || fail "the install did not go on after the confirmation"
[ "$(jq -r .backup "$STATE_FILE")" = 'no backup taken' ] || fail "the record does not say no backup was taken"
# ... and Stop at the second menu ends the run before anything is written.
pty_begin
menu_at 'choose'
pick_with "$DOWN"
menu_at 'The screen lock is off (confirm)'
pick_with "$DOWN"
menu_at 'Skip the backup'
pick_with "$DOWN$DOWN"
menu_at 'Go on without a backup (the data is gone for good)'
pick_with "$UP"
pty_end
[ "$rc" = 1 ] || fail "skip then Stop exited $rc"
no_writes "skip then Stop"
[ ! -e "$cache/phones" ] || fail "skip then Stop wrote a record"

# Ctrl-C outside a write group says what state things are in and exits 130.
interrupt_during() { # name, stdin, expected text, text to wait for; the slow stage is running
  fresh_cache
  rm -f "$root/out" "$COUNT" "$PCOUNT"
  : > "$root/out"
  local battery=0
  [ "$4" != '... checking battery' ] || battery=8
  printf '%b' "$2" | FAKE_SLOW_BATTERY=$battery FAKE_SLOW_TAR=8 "$tool" > "$root/out" 2>&1 &
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
interrupt_during "Ctrl-C in the Preflight" '' 'Nothing was written to the phone. There is no record for phone ABC123.' '... checking battery'
interrupt_during "Ctrl-C during the backup" "$confirmations" 'Nothing was written to the phone. There is no record for phone ABC123.' 'streaming the phone'

echo "PASS: nura-install checks the phone, gates every group of writes, and stops on every doubt"
