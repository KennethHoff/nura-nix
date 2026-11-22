# End-to-end test of nura-install against a fake adb, fastboot, ssh, ip and local image servers.
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
export UNLOCKED_FLAG=$root/fb-unlocked CRITICAL_FLAG=$root/fb-critical
export HCOUNT=$root/psize-count SSH_LOG=$root/ssh.log SSH_ENV_LOG=$root/ssh-env.log ADB_COUNT=$root/adb-count PARTS=$root/phoneparts NC_PID=$root/nc.pid SSHD_STOPPED=$root/sshd-stopped WROTE=$root/wrote ADB_BACK=$root/adb-back
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

# The phone's per-phone partitions as Nura sees them.
mkdir -p "$PARTS"
for n in persist modemst1 modemst2 fsg fsc; do { echo "partition $n"; seq 1 300; } > "$PARTS/$n"; done
echo 0 > "$root/ip_forward"

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
  "getprop sys.oem_unlock_allowed") if [ "${FAKE_OEM:-1}" = none ]; then echo; else echo "${FAKE_OEM:-1}" | crlf; fi ;;
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
#   FAKE_FB_AFTER=n  `fastboot devices` lists the phone only from its (n+1)th call; FAKE_FB_SERIAL its serial
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
#   FAKE_LOCKED=1  the bootloader says `unlocked:no` until `flashing unlock` ran (the flag file outlives a run: phone_locked resets it)
#   FAKE_NOCONFIRM=<unlock|unlock_critical>:<never|declined>  the owner never confirms on the phone (the command hangs) or declines (FAILED)
#   FAKE_INTERRUPT_AFTER=<unlock|unlock_critical>  that command succeeds, then nura-install is sent SIGINT
#   (FAKE_FAIL, FAKE_FAIL0 and FAKE_HANG also take `unlock` and `unlock_critical` as the partition)
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
  if [ "${FAKE_LOCKED:-}" = 1 ]; then if [ -e "$UNLOCKED_FLAG" ]; then unl=yes; else unl=no; fi; fi
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
    [ "$n" -gt "${FAKE_FB_AFTER:-0}" ] && printf '%s\t%s\n' "$S" "${FAKE_FB_STATE:-fastboot}"
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
  flashing)
    case "$2" in
      unlock | unlock_critical) ;;
      *) echo "fake fastboot: refusing flashing $2" >&2; exit 1 ;;
    esac
    case "${FAKE_NOCONFIRM:-}" in
      "$2:never") touch "$WROTE"; exec sleep 60 ;;
      "$2:declined") echo "FAILED (remote: 'the owner declined on the phone')" >&2; exit 1 ;;
    esac
    write flashing "$2"
    if [ "$2" = unlock ]; then touch "$UNLOCKED_FLAG"; else touch "$CRITICAL_FLAG"; fi
    [ "${FAKE_INTERRUPT_AFTER:-}" != "$2" ] || { kill -INT "$PPID"; sleep "${FAKE_SIG_SLEEP:-0}"; } ;;
  flash) write flash "$2"; cp "$3" "$FLASHED/$2" ;;
  erase) write erase "$2" ;;
  reboot) write reboot "" ;;
esac
EOS
# Fake ssh knobs (the phone's partitions are the files in $PARTS):
#   FAKE_SSH_HOSTKEY=<key>  the host key the phone presents (default AAAAKEY1)
#   FAKE_SSH_SERIAL=<serial>  Nura's androidboot.serialno
#   FAKE_SSH_BADHASH=<name>  the phone reports a wrong sha256 for that partition
#   FAKE_SSH_CORRUPT=<name>  the streamed bytes of that partition differ from what the phone hashed
#   FAKE_SSH_DDFAIL=<name>  the stream of that partition breaks off with an error
#   FAKE_SSH_HANG=<text>  a remote command containing that text never returns
#   FAKE_SSH_STOPFAIL=1  `systemctl stop sshd` fails
#   FAKE_SSH_DUPLABEL=<name>  two devices carry that partition label
#   FAKE_SSH_NOHOLDER=<name>  no device carries that partition label
#   FAKE_SSH_SIZE_OFF=<name>  the phone reports that partition one byte too big
cat > "$root/bin/ssh" <<'EOS'
[ "$1" = -V ] && { echo "OpenSSH_fake" >&2; exit 0; }
echo "$*" >> "$SSH_LOG"
env | sort >> "$SSH_ENV_LOG"
known=
while [ $# -gt 0 ]; do
  case $1 in
    -F | -b | -p) shift 2 ;;
    -o) case $2 in UserKnownHostsFile=*) known=${2#UserKnownHostsFile=} ;; esac; shift 2 ;;
    -*) shift ;;
    *) break ;;
  esac
done
target=$1; shift; remote=$*
[ -s "$known" ] || { echo "Host key verification failed." >&2; exit 255; }
[ "$(awk '{print $3}' "$known")" = "${FAKE_SSH_HOSTKEY:-AAAAKEY1}" ] || { echo "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!" >&2; exit 255; }
pw=$("$SSH_ASKPASS" "$target's password: ") || { echo "Permission denied (no password)" >&2; exit 255; }
[ "$pw" = 147147 ] || { echo "Permission denied, please try again." >&2; exit 255; }
[ -z "${FAKE_SSH_HANG:-}" ] || case $remote in *"$FAKE_SSH_HANG"*) exec sleep 30 ;; esac
sudo_pw() { read -r p; [ "$p" = 147147 ] || { echo "sudo: incorrect password" >&2; exit 1; }; }
case $remote in
  "ls -1 /dev/disk/by-partlabel") ls -1 "$PARTS" ;;
  "cat /proc/cmdline") echo "quiet androidboot.serialno=${FAKE_SSH_SERIAL:-ABC123} root=/dev/x" ;;
  "grep -lx PARTNAME="*" /sys/class/block/*/uevent || true")
    name=${remote#*PARTNAME=}; name=${name%% *}
    [ "${FAKE_SSH_NOHOLDER:-}" != "$name" ] || exit 0
    echo "/sys/class/block/dev-$name/uevent"
    [ "${FAKE_SSH_DUPLABEL:-}" != "$name" ] || echo "/sys/class/block/other-$name/uevent" ;;
  "sudo -S -p '' blockdev --getsize64 /dev/disk/by-partlabel/"*)
    sudo_pw; name=${remote##*/}
    n=$(stat -c %s "$PARTS/$name")
    [ "${FAKE_SSH_SIZE_OFF:-}" != "$name" ] || n=$((n + 1))
    echo "$n" ;;
  "sudo -S -p '' sha256sum /dev/disk/by-partlabel/"*)
    sudo_pw; name=${remote##*/}
    h=$(sha256sum "$PARTS/$name" | cut -d' ' -f1)
    [ "${FAKE_SSH_BADHASH:-}" != "$name" ] || h=$(printf 0 | sha256sum | cut -d' ' -f1)
    echo "$h  /dev/disk/by-partlabel/$name" ;;
  "sudo -S -p '' dd if=/dev/disk/by-partlabel/"*" bs=4M")
    sudo_pw; name=${remote#*partlabel/}; name=${name%% *}
    if [ "${FAKE_SSH_DDFAIL:-}" = "$name" ]; then head -c 3 "$PARTS/$name"; echo "dd: error reading" >&2; exit 1; fi
    cat "$PARTS/$name"
    [ "${FAKE_SSH_CORRUPT:-}" != "$name" ] || printf x ;;
  "sudo -S -p '' systemctl stop sshd")
    sudo_pw
    [ -z "${FAKE_SSH_STOPFAIL:-}" ] || { echo "Failed to stop sshd.service" >&2; exit 1; }
    touch "$SSHD_STOPPED"; kill "$(cat "$NC_PID")" 2> /dev/null || true ;;
  *) echo "fake ssh: unexpected remote command: $remote" >&2; exit 1 ;;
esac
EOS

cat > "$root/bin/ssh-keyscan" <<'EOS'
echo "# ${@: -1}:22 SSH-2.0-fake" >&2
echo "[${@: -1}]:${@: -2:1} ssh-ed25519 ${FAKE_SSH_HOSTKEY:-AAAAKEY1}"
EOS
cat > "$root/bin/ssh-keygen" <<'EOS'
read -r line
echo "256 SHA256:FAKEFINGERPRINT-$(echo "$line" | awk '{print $3}') no comment (ED25519)"
EOS

# Fake ip: the phone's USB network is usb0 with the computer's address; knobs:
#   FAKE_IP_NOADDR=1  no interface has the address;  FAKE_IP_EXTRA_ROUTE=1  a second route to the subnet, on tailscale0
#   FAKE_IP_GET_DEV=<dev>  the interface a route to the phone goes over
cat > "$root/bin/ip" <<'EOS'
[ "$1" = -V ] && { echo "ip utility, iproute2-fake"; exit 0; }
[ "$1" = -j ] || exit 1
shift
case "$*" in
  "addr show")
    if [ -n "${FAKE_IP_NOADDR:-}" ]; then
      echo '[{"ifname":"lo","addr_info":[{"local":"127.0.0.1"}]}]'
    else
      echo '[{"ifname":"lo","addr_info":[{"local":"127.0.0.1"}]},{"ifname":"usb0","addr_info":[{"local":"'"$NURA_INSTALL_LOCAL_ADDR"'"}]}]'
    fi ;;
  "route show")
    extra=
    [ -z "${FAKE_IP_EXTRA_ROUTE:-}" ] || extra=',{"dst":"127.0.0.0/24","dev":"tailscale0"}'
    echo '[{"dst":"default","dev":"wlan0"},{"dst":"127.0.0.0/24","dev":"usb0"}'"$extra"']' ;;
  "route get "*) echo '[{"dst":"'"${@: -1}"'","dev":"'"${FAKE_IP_GET_DEV:-usb0}"'"}]' ;;
  *) exit 1 ;;
esac
EOS
for t in adb fastboot curl ssh ssh-keyscan ssh-keygen ip; do
  sed -i "1i #!$bash_bin" "$root/bin/$t"
  chmod +x "$root/bin/$t"
done

export NURA_INSTALL_ADB=$root/bin/adb NURA_INSTALL_FASTBOOT=$root/bin/fastboot
export NURA_INSTALL_NURA_INDEX=file://$root/index.json NURA_INSTALL_CACHE=$cache
export NURA_INSTALL_ALLOW_FILE_URLS=1 NURA_INSTALL_WAIT_SECONDS=0 NURA_INSTALL_NEEDED_GIB=0
export NURA_INSTALL_ALLOW_PIPED_INPUT=1 NURA_INSTALL_WRITE_TIMEOUT_SECONDS=60
sshd_port=$(( 30000 + $$ % 20000 ))
export NURA_INSTALL_SSH=$root/bin/ssh NURA_INSTALL_SSH_KEYSCAN=$root/bin/ssh-keyscan NURA_INSTALL_SSH_KEYGEN=$root/bin/ssh-keygen NURA_INSTALL_IP=$root/bin/ip
export NURA_INSTALL_PHONE_ADDR=127.0.0.1:$sshd_port NURA_INSTALL_LOCAL_ADDR=127.0.0.2 NURA_INSTALL_IP_FORWARD_FILE=$root/ip_forward
export NURA_INSTALL_COPY_WAIT_SECONDS=0 NURA_INSTALL_SSH_TIMEOUT_SECONDS=20
export FAKE_SDCARD=$root/sdcard
REAL_CURL=$(command -v curl)
export REAL_CURL

# Typed input shared by the cases.
done_phrase='Confirm'
# Menus are answered by their word when the input is piped (the test build only); the pty cases press keys.
choice='phosh\n'
confirmations="${choice}$done_phrase\nbackup\n"
trust='trust this phone'
copy_in="copy\n$trust\n"
fresh_in="${confirmations}install nura\nreboot\n"
install_in="${choice}install nura\nreboot\n"
reinstall_in="${choice}reinstall nura\nreboot\n"
fresh_cache() { rm -rf "$cache"; }
set_record() { # state [slot]: a record
  mkdir -p -m 700 "$cache/phones/ABC123"
  jq -n --arg s "$1" --arg slot "${2:-a}" \
    --arg nb "$(sum "$root/boot.raw")" --arg nr "$(sum "$root/rootfs.raw")" \
    '{state: $s, slot: $slot, backup: "/b",
      nura_build: "fake", nura_boot_sha256: $nb, nura_rootfs_sha256: $nr}' > "$STATE_FILE"
}
sshd_was_stopped() { [ -e "$SSHD_STOPPED" ] || fail "$1: sshd was not stopped"; }
set_copied() { jq '. + {copy_path: "/p", copy_time: "t"}' "$STATE_FILE" > "$STATE_FILE.tmp" && mv "$STATE_FILE.tmp" "$STATE_FILE"; }
state() { jq -r .state "$STATE_FILE"; }
run() { # stdin, args...
  local input=$1
  shift
  rm -rf "$FLASHED" "$LOG" "$COUNT" "$PCOUNT" "$ADB_LOG" "$STATE_LOG" "$SIG_LOG" "$CURL_LOG" "$ADB_COUNT" "$HCOUNT" "$SSH_LOG" "$SSH_ENV_LOG" "$SSHD_STOPPED" "$WROTE" "$ADB_BACK"
  mkdir -p "$FLASHED"
  : > "$LOG"
  : > "$ADB_LOG"
  : > "$SSH_LOG"
  : > "$SSH_ENV_LOG"
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
    prev ~ /^YOU: Approve the reinstall$/ && $0 != "TOOL: Reinstall Nura" { bad = 1 }
    prev ~ /^YOU: Approve the reboot$/ && $0 != "TOOL: Restart the phone" { bad = 1 }
    prev ~ /^YOU: Approve the unlock$/ && $0 != "TOOL: Unlock the bootloader" { bad = 1 }
    prev ~ /^YOU: Approve the critical unlock$/ && $0 != "TOOL: Unlock the critical partitions" { bad = 1 }
    /^TOOL: (Install Nura|Reinstall Nura|Restart the phone|Unlock the bootloader|Unlock the critical partitions)$/ && prev !~ /^YOU: Approve the / { bad = 1 }
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

sshd_up() { # a listener on the phone's address, as sshd on Nura
  sshd_down
  nc -lk 127.0.0.1 "$sshd_port" > /dev/null 2>&1 &
  echo $! > "$NC_PID"
  local i
  for i in $(seq 50); do
    (exec 3<> "/dev/tcp/127.0.0.1/$sshd_port") 2> /dev/null && return 0
    sleep 0.1
  done
  fail "the fake sshd did not start"
}
sshd_down() {
  if [ -s "$NC_PID" ]; then kill "$(cat "$NC_PID")" 2> /dev/null || true; fi
  : > "$NC_PID"
  sleep 0.2
}
sshd_down
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
# The askpass variable only counts when it names an ancestor; set by hand it changes nothing.
rc=0; NURA_INSTALL_ASKPASS=999999 "$shipped" "Password: " > "$root/out" 2>&1 || rc=$?
[ "$rc" = 2 ] || fail "the shipped binary with a stray askpass variable exited $rc, not 2"
! grep -q 147147 "$root/out" || fail "a stray askpass variable made the shipped binary print the password"
export NURA_INSTALL_ADB=$root/bin/adb NURA_INSTALL_FASTBOOT=$root/bin/fastboot
export NURA_INSTALL_NURA_INDEX=file://$root/index.json NURA_INSTALL_CACHE=$cache NURA_INSTALL_NEEDED_GIB=0
export NURA_INSTALL_CURL=$root/bin/curl

## Fresh from Android

fresh_cache
sshd_up
FAKE_FB_AFTER=1 run "${fresh_in}$copy_in"
[ "$rc" = 0 ] || fail "fresh install exited $rc"
[ "$(cat "$LOG")" = "devices
devices
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
# The backup, and later the copy of the partitions, are on disk before the run goes on (fsync of files and directories).
synced=$(grep 'check synced to disk' "$cache/logs/"* | sed 's/.*synced to disk: //')
[ "$(printf '%s\n' "$synced" | grep -c "$cache/backups/ABC123/backup-")" = 1 ] || fail "the backup was not synced: $synced"
[ "$(printf '%s\n' "$synced" | grep -c "$cache/backups/ABC123/partitions-")" = 1 ] || fail "the partition copy was not synced: $synced"
[ "$(cat "$FLASHED/userdata")" = nura-rootfs ] || fail "userdata got the wrong image"
[ "$(cat "$FLASHED/boot")" = nura-boot ] || fail "boot got the wrong image"
cp "$root/out" "$root/out.install"
cp "$SSH_LOG" "$root/ssh.install"
cp "$SSH_ENV_LOG" "$root/ssh-env.install"
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
  '  Preflight passed: ' '^done in [0-9.]+ s: finished$' '^  Step 9 of 19 done  '; do
  grep -qE -- "$line" "$root/out.install" || fail "install output lacks: $line"
done
[ "$(grep -c '^== Step ' "$root/out.install")" = 19 ] || fail "the install is not nineteen steps"
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
[ "$(sections_of "$root/out.install")" = "[Preflight] 1/19 2/19 3/19 4/19 5/19 6/19 7/19 8/19 [Prepare] 9/19 10/19 11/19 12/19 [Install] 13/19 14/19 [Finish] 15/19 16/19 17/19 18/19 19/19" ] \
  || fail "sections out of order: $(sections_of "$root/out.install")"
order=$(grep -n -E '^== Step ' "$root/out" | sed -E 's/^[0-9]*:== Step [0-9]+ of [0-9]+ · (YOU|TOOL): //; s/ +POINT OF NO RETURN$//' | tr '\n' '|')
[ "$order" = "Check this computer|Check the phone over adb|Put the phone in fastboot|Check the bootloader|Find Nura's builds for the phone|Choose Nura's interface|Restart into Android|Find, download and verify the images|Confirm what is lost|Back up the phone|Put the phone in fastboot|Check the phone in fastboot|Approve the install|Install Nura|Approve the reboot|Restart the phone|Start sshd on Nura|Copy the per-phone partitions|Summarize|" ] \
  || fail "steps out of order: $order"
actors=$(grep -E '^== Step ' "$root/out" | sed -E 's/^.* · (YOU|TOOL): .*/\1/' | tr '\n' ' ')
[ "$actors" = "TOOL TOOL YOU TOOL TOOL YOU YOU TOOL YOU TOOL YOU TOOL YOU TOOL YOU TOOL YOU TOOL TOOL " ] || fail "actors out of order: $actors"
grep -q '^== Step 1 of 19 · TOOL: Check this computer$' "$root/out" || fail "steps are not numbered"
grep -q '^== Step 13 of 19 · YOU: Approve the install  *POINT OF NO RETURN$' "$root/out" || fail "the approval is not the marked point of no return"
grep -q '^== Step 19 of 19 · TOOL: Summarize$' "$root/out" || fail "the last step is not 19 of 19"
grep -q 'Hold Power, tap Power off' "$root/out" && grep -q 'Hold Volume Down + Power until the fastboot screen shows' "$root/out" || fail "no fastboot instruction"
grep -q '^  \.\.\. waiting for the phone in fastboot mode (up to 0 s)' "$root/out" || fail "no waiting line"
[ "$(grep -n 'never relock' "$root/out" | head -1 | cut -d: -f1)" -lt "$(grep -n 'Install Nura runs' "$root/out" | cut -d: -f1)" ] \
  || fail "the warnings come after the gate"
# The copy of the per-phone partitions.
part_dir=$(jq -r .copy_path "$STATE_FILE")
case $part_dir in "$cache"/backups/ABC123/partitions-*) ;; *) fail "unexpected copy path $part_dir" ;; esac
[ -n "$(jq -r .copy_time "$STATE_FILE")" ] || fail "the copy time was not recorded"
for n in persist modemst1 modemst2 fsg fsc; do
  cmp -s "$part_dir/$n.img" "$PARTS/$n" || fail "the copy of $n differs from the phone's"
  [ "$(stat -c %a "$part_dir/$n.img")" = 600 ] || fail "$n.img is not private"
  grep -q "^$(sum "$PARTS/$n")  $(stat -c %s "$PARTS/$n")  $n.img$" "$part_dir/manifest.txt" || fail "the manifest lacks $n"
done
grep -q '^serial: ABC123$' "$part_dir/manifest.txt" || fail "the manifest lacks the serial"
[ -e "$SSHD_STOPPED" ] || fail "sshd was not stopped"
grep -q 'sshd is stopped again' "$root/out" || fail "the run did not say sshd was stopped"
grep -qF "copied to $part_dir" "$root/out" || fail "the summary lacks the copy"
known=$cache/phones/ABC123/known_hosts
[ "$(stat -c %a "$known")" = 600 ] || fail "known_hosts is not private"
grep -q 'ssh-ed25519 AAAAKEY1' "$known" || fail "the host key was not pinned"
grep -q 'SHA256:FAKEFINGERPRINT-AAAAKEY1' "$root/out" || fail "the host key fingerprint was not shown"
# The password is never an argument, and the environment only carries the askpass contract.
! grep -q 147147 "$root/ssh.install" || fail "the password is in ssh's arguments"
! grep -q 147147 "$root/ssh-env.install" || fail "the password is in ssh's environment"
grep -q '^NURA_INSTALL_ASKPASS=[0-9]*$' "$root/ssh-env.install" || fail "ssh was not given the askpass contract"
grep -q '^SSH_ASKPASS_REQUIRE=force$' "$root/ssh-env.install" || fail "ssh was not made to use the askpass hook"
for opt in 'StrictHostKeyChecking=yes' "UserKnownHostsFile=$known" 'PreferredAuthentications=password' 'PubkeyAuthentication=no' 'NumberOfPasswordPrompts=1' '-F /dev/null' '-b 127.0.0.2'; do
  grep -qF -- "$opt" "$root/ssh.install" || fail "ssh was run without: $opt"
done
# The transcript.
[ "$(ls "$cache/logs" | wc -l)" = 1 ] || fail "expected one transcript"
transcript=$cache/logs/$(ls "$cache/logs")
case $transcript in *-run.log) ;; *) fail "transcript not named for the run: $transcript" ;; esac
[ "$(stat -c %a "$transcript")" = 600 ] || fail "transcript is not private"
grep -q 'run: .*fastboot.* flash boot' "$transcript" || fail "transcript lacks the write command"
grep -q 'check serial: ABC123' "$transcript" || fail "transcript lacks the serial check"
grep -q 'choice: install nura' "$transcript" || fail "transcript lacks the gate"
grep -q 'choice: trust this phone' "$transcript" || fail "transcript lacks the host key confirmation"
grep -q 'outcome: finished' "$transcript" || fail "transcript lacks the outcome"
! grep -E '^(run|exit|output|check):' "$transcript" | grep -q 147147 || fail "the password is in the transcript's commands"
[ "$(grep -c 147147 "$transcript")" = "$(grep 147147 "$transcript" | grep -ci 'log in as\|user / 147147\|Log in: ')" ] || fail "the password appears in the transcript outside the login instructions"
sshd_down

# Cached files are re-hashed, never trusted.
set_record nura-installed
run "$reinstall_in"
[ "$rc" = 0 ] || fail "cached rerun exited $rc"
grep -q 'cached nura-boot.img.xz: hash matches' "$root/out" || fail "cache not reused"
[ ! -e "$cache/phones/ABC123/known_hosts" ] || fail "a new Nura install kept the old host key"
printf 'x' >> "$cache/downloads/nura-boot.img.xz"
printf 'damaged' > "$cache/downloads/nura-rootfs.img"
run "$reinstall_in"
[ "$rc" = 0 ] || fail "rerun over damaged cache exited $rc"
grep -q 'download nura-boot.img.xz: ' "$root/out" || fail "a damaged download was not fetched again"
[ "$(sum "$cache/downloads/nura-boot.img.xz")" = "$(sum "$root/srv/nura-boot.img.xz")" ] || fail "the cached download is still damaged"
[ "$(cat "$FLASHED/userdata")" = nura-rootfs ] || fail "a damaged unpacked image was flashed"

# The slot written to the record is the phone's.
fresh_cache
FAKE_SUFFIX=_b FAKE_SLOT=b FAKE_FB_AFTER=1 run "$fresh_in"
[ "$rc" = 0 ] || fail "slot b install exited $rc"
[ "$(jq -r .slot "$STATE_FILE")" = b ] || fail "the record does not hold the phone's slot b"

# CRLF line endings from adb shell are handled.
fresh_cache
FAKE_CRLF=1 FAKE_FB_AFTER=1 run "$fresh_in"
[ "$rc" = 0 ] || fail "CRLF adb output exited $rc"

# ... also with a path over 100 bytes, which the tar writes a long-name entry for.
longdir=$root/sdcard/$(printf 'd%.0s' $(seq 60))/$(printf 'e%.0s' $(seq 60))
mkdir -p "$longdir" && echo hi > "$longdir/f"
NURA_INSTALL_CACHE=$root/cache2 FAKE_FB_AFTER=1 run "${fresh_in}skip\n"
[ "$rc" = 0 ] || fail "the install with a long path exited $rc"
[ "$(planned "$root/cache2")" = "$(stat -c %s "$root"/cache2/backups/ABC123/backup-*/sdcard.tar)" ] || fail "the planned tar length $(planned "$root/cache2") differs from the stream with a long path"
rm -rf "$root/sdcard/ddd"* "$root/cache2"

## What the phone's state and its record decide

fresh_cache
FAKE_FB_AFTER=0 run "$choice"
refused "a phone in fastboot with no record" 'has no record' 3
[ ! -e "$cache/phones" ] || fail "nothing-to-do wrote a record"
set_record nura-installed
set_copied
FAKE_FB_AFTER=9 FAKE_ADB=none run ''
refused "an installed phone, copied, not in fastboot" 'hold Volume Down + Power' 3
! grep -q 'Choose' "$root/out" || fail "a menu was offered where only one action is left"
set_record nura-in-progress
FAKE_FB_AFTER=9 FAKE_ADB=none run ''
refused "a half installed phone, not in fastboot" 'is `nura-in-progress`' 3
set_record nura-installed
set_copied
mkdir -p -m 700 "$cache/phones/OTHER1" && cp "$STATE_FILE" "$cache/phones/OTHER1/state.json"
FAKE_FB_AFTER=9 FAKE_ADB=none run ''
refused "two recorded phones and none in fastboot" 'phones have a record and none is in fastboot'
rm -rf "$cache/phones/OTHER1"
set_record nura-installed
run "$reinstall_in"
[ "$rc" = 0 ] || fail "reinstalling exited $rc"
grep -q 'Nura is installed: this run reinstalls it' "$root/out" || fail "the reinstall was not announced"
set_record nura-installed
FAKE_FB_AFTER=9 run "$choice"
refused "a recorded phone on adb" 'from fastboot' 3
fresh_cache
FAKE_FB_AFTER=9 FAKE_ADB=none run "$choice"
refused "no phone anywhere" 'no phone is visible over adb'
for args in "--os nura" "--os" "nura" "-x"; do
  run '' $args
  [ "$rc" = 2 ] || fail "nura-install $args exited $rc, not 2"
  grep -q 'unexpected argument' "$root/out" || fail "nura-install $args gave no usage error"
done
run '' --help
[ "$rc" = 0 ] || fail "--help exited $rc"

## Finishing nura-in-progress from fastboot

set_record nura-in-progress
run "$install_in"
[ "$rc" = 0 ] || fail "finishing exited $rc"
[ "$(state)" = nura-installed ] || fail "finishing left $(state)"
[ "$(writes | wc -l)" = 5 ] || fail "finishing ran $(writes)"
grep -q 'erases everything on Nura' "$root/out" && fail "finishing an install claims to erase Nura"

## Android phase stops

stop_fresh() { # name, message: a stop before any write or record
  fresh_cache
  FAKE_FB_AFTER=1 run "$fresh_in"
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
  FAKE_FB_AFTER=1 run "$2"
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
FAKE_CONTENT_EMPTY=content://sms FAKE_FB_AFTER=1 run "$fresh_in"
[ "$rc" = 1 ] || fail "empty SMS export without acknowledgement exited $rc"
no_writes "empty SMS"
grep -q 'sms.txt: 0 rows' "$root/out" || fail "an empty export was not counted"
fresh_cache
FAKE_CONTENT_EMPTY=content://sms FAKE_FB_AFTER=1 run "${confirmations}Confirm\ninstall nura\nreboot\n"
[ "$rc" = 0 ] || fail "acknowledged empty SMS export exited $rc"
fresh_cache
FAKE_CONTENT_EMPTY=content://sms FAKE_FB_AFTER=1 run "${confirmations}confirm\n"
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
  [ "$(cat "$LOG")" = devices ] || [ "$(cat "$LOG")" = "devices
devices
$fb_all" ] || fail "$1: fastboot was asked more than the early read: $(cat "$LOG")"
  [ ! -e "$cache/phones" ] || fail "$1: wrote a record"
  [ ! -e "$cache/backups" ] || fail "$1: a backup directory exists"
  no_backup "$1"
}
pre_fail() { # name, message(s) that must be reported
  local name=$1
  shift
  fresh_cache
  FAKE_FB_AFTER=1 run "$fresh_in"
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
grep -q 'Step 8 of 19 .*Find, download and verify the images' "$root/out" || fail "the Preflight did not reach its last step"
make_index
# The bootloader is read first, before any download or backup: every wrong answer is reported at once, then the run stops.
fresh_cache
FAKE_PSIZE_MISSING=boot_a FAKE_FB_BATTERY=no FAKE_FB_AFTER=1 run "$fresh_in"
[ "$rc" = 1 ] || fail "a wrong bootloader exited $rc"
for m in 'no value for `getvar partition-size:boot_a`' 'battery-soc-ok: no'; do
  grep -qF -- "$m" "$root/out" || fail "the early bootloader check did not report: $m"
done
grep -q '^Not ready: 2 checks failed' "$root/out" || fail "wrong count after the bootloader check: $(grep '^Not ready' "$root/out")"
! grep -q 'Restart into Android' "$root/out" || fail "the owner was sent back to Android after a failed bootloader check"
[ ! -e "$cache/downloads" ] || fail "something was downloaded before the bootloader was checked"
no_backup "a failed bootloader check"
no_writes "a failed bootloader check"
[ ! -e "$cache/phones" ] || fail "a failed bootloader check wrote a record"
# A phone that is not on Android needs the owner first: the run waits for it in a step of its own.
fresh_cache
FAKE_ADB_AFTER=1 NURA_INSTALL_WAIT_SECONDS=10 FAKE_FB_AFTER=1 run "$fresh_in"
[ "$rc" = 0 ] || fail "the run that waited for the phone exited $rc"
[ "$(sections_of "$root/out")" = "[Preflight] 1/20 2/20 3/20 4/20 5/20 6/20 7/20 8/20 9/20 [Prepare] 10/20 11/20 12/20 13/20 [Install] 14/20 15/20 [Finish] 16/20 17/20 18/20 19/20 20/20" ] || fail "sections with the wait: $(sections_of "$root/out")"
grep -q '^== Step 2 of 20 · YOU: Connect the phone with USB debugging$' "$root/out" || fail "no step to connect the phone"
grep -q '^  \.\.\. waiting for the phone over adb (up to 10 s)' "$root/out" || fail "the wait for adb was not named"
# A YOU step ends when the phone shows its result: the restart step stays open until the phone is back over adb.
fresh_cache
FAKE_ADB_BACK_AFTER=2 NURA_INSTALL_WAIT_SECONDS=20 FAKE_FB_AFTER=1 run "$fresh_in"
[ "$rc" = 0 ] || fail "the run that waited for the phone to come back exited $rc"
awk '
  /^== Step 7 of 19 · YOU: Restart into/ { in5 = 1; next }
  /^== Step 8 of 19/ { in5 = 0 }
  in5 && /waiting for the phone over adb/ { waited = 1 }
  in5 && /^  Step 7 of 19 done/ { done5 = $0 }
  END { if (!waited || done5 ~ / 0\.[0-9] s$/) { print "waited=" waited " done=" done5; exit 1 } }' "$root/out" || fail "the restart step ended before the phone was back over adb"
fresh_cache
FAKE_FB_AFTER=1 run "$choice"
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

fb_fail() { # name, message
  set_record nura-installed
  run "$reinstall_in"
  refused "$1" "$2"
  [ "$(state)" = nura-installed ] || fail "$1 changed the record"
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
FAKE_FB_SERIAL=XYZ789 FAKE_FB_AFTER=1 run "$fresh_in"
refused "wrong serial after the adb phase" 'is XYZ789, not ABC123'
[ ! -e "$cache/phones" ] || fail "wrong serial wrote a record"

## Unlocking a locked bootloader: after the backup, each command behind its own approval

phone_locked() { rm -f "$UNLOCKED_FLAG" "$CRITICAL_FLAG"; }
unlock_in="${confirmations}unlock bootloader\nunlock critical\ninstall nura\nreboot\n"
unlocks() { grep -E 'flashing' "$LOG" || true; }
# A locked phone: the Preflight takes it, the unlock runs after the backup, the install goes on from fastboot.
fresh_cache
phone_locked
FAKE_LOCKED=1 FAKE_FB_AFTER=1 run "$unlock_in"
[ "$rc" = 0 ] || fail "a locked phone's run exited $rc"
[ "$(unlocks)" = "-s ABC123 flashing unlock
-s ABC123 flashing unlock_critical" ] || fail "the unlock ran: $(unlocks)"
[ "$(writes | sed 's/^-s ABC123 //' | tr '\n' '|')" = "flashing unlock|flashing unlock_critical|erase dtbo|erase vendor_boot|flash userdata $nura_raw/nura-rootfs.img|flash boot $nura_raw/nura-boot.img|reboot|" ] \
  || fail "the writes were out of order: $(writes)"
[ "$(tr '\n' '|' < "$STATE_LOG" | sed 's/|$//')" = "flashing unlock under unlock-in-progress|flashing unlock_critical under unlock-in-progress|erase dtbo under nura-in-progress|erase vendor_boot under nura-in-progress|flash userdata under nura-in-progress|flash boot under nura-in-progress|reboot  under nura-installed" ] \
  || fail "the unlock ran outside its record: $(cat "$STATE_LOG")"
[ "$(state)" = nura-installed ] || fail "state after the unlock and install is $(state)"
case $(jq -r .backup "$STATE_FILE") in "$cache"/backups/ABC123/backup-*) ;; *) fail "the backup path was lost: $(jq -r .backup "$STATE_FILE")" ;; esac
grep -q 'getprop sys.oem_unlock_allowed' "$ADB_LOG" || fail "OEM unlocking was not read over adb"
grep -q 'OEM unlocking: on' "$root/out" || fail "the OEM unlocking answer is not shown"
cp "$root/out" "$root/out.unlock"
step_blocks "unlock" "$root/out.unlock"
gate_precedes_its_write "unlock" "$root/out.unlock"
plain_output "unlock" "$root/out.unlock"
order=$(grep -E '^== Step ' "$root/out.unlock" | sed -E 's/^== Step [0-9]+ of [0-9]+ · (YOU|TOOL): //; s/ +POINT OF NO RETURN$//' | tr '\n' '|')
[ "$order" = "Check this computer|Check the phone over adb|Put the phone in fastboot|Check the bootloader|Find Nura's builds for the phone|Choose Nura's interface|Restart into Android|Find, download and verify the images|Confirm what is lost|Back up the phone|Turn on OEM unlocking|Put the phone in fastboot|Check the bootloader is locked|Approve the unlock|Unlock the bootloader|Put the phone in fastboot|Check the bootloader is unlocked|Approve the critical unlock|Unlock the critical partitions|Put the phone in fastboot|Check the phone in fastboot|Approve the install|Install Nura|Approve the reboot|Restart the phone|Start sshd on Nura|Copy the per-phone partitions|Summarize|" ] \
  || fail "the locked run's steps are out of order: $order"
[ "$(grep -c '^== Step ' "$root/out.unlock")" = 28 ] || fail "a locked run is not 28 steps"
[ "$(sections_of "$root/out.unlock" | grep -o '\[[^]]*\]' | tr '\n' ' ')" = "[Preflight] [Prepare] [Unlock the bootloader] [Install] [Finish] " ] || fail "sections of a locked run: $(sections_of "$root/out.unlock")"
grep -q '^== Step 14 of 28 · YOU: Approve the unlock  *POINT OF NO RETURN$' "$root/out.unlock" || fail "the unlock approval is not the marked point of no return"
grep -q 'press a Volume button to move to `Unlock the bootloader`, then press Power' "$root/out.unlock" || fail "the buttons were not named"
grep -q 'choice: unlock bootloader' "$cache/logs/"* && grep -q 'choice: unlock critical' "$cache/logs/"* || fail "the transcript lacks the unlock approvals"
# No Android after the wipe: the adb phone is read only before the first unlock.
[ "$(grep -c 'Check the phone over adb' "$root/out.unlock")" = 1 ] || fail "adb was used again after the unlock"
! grep -q 'reboot' "$ADB_LOG" || fail "adb rebooted the phone"

# OEM unlocking off: stop after the backup, before fastboot is used for the unlock.
fresh_cache
phone_locked
FAKE_OEM=0 FAKE_LOCKED=1 FAKE_FB_AFTER=1 run "$unlock_in"
refused "OEM unlocking off" 'says OEM unlocking is off'
[ -z "$(unlocks)" ] || fail "an unlock ran with OEM unlocking off"
[ -d "$cache"/backups/ABC123/backup-* ] || fail "the backup was not taken before the OEM check"
[ ! -e "$cache/phones" ] || fail "OEM unlocking off wrote a record"
# ... a phone that does not report it asks the owner instead, and Stop there writes nothing.
fresh_cache
phone_locked
FAKE_OEM=none FAKE_LOCKED=1 FAKE_FB_AFTER=1 run "${confirmations}oem unlocking\nunlock bootloader\nunlock critical\ninstall nura\nreboot\n"
[ "$rc" = 0 ] || fail "an unreported OEM property with a confirmation exited $rc"
grep -q 'does not report OEM unlocking' "$root/out" || fail "the unreported property was not said"
fresh_cache
phone_locked
FAKE_OEM=none FAKE_LOCKED=1 FAKE_FB_AFTER=1 run "${confirmations}stop\n"
refused "OEM unlocking unconfirmed" 'stopped'
[ ! -e "$cache/phones" ] || fail "an unconfirmed OEM unlocking wrote a record"

# Stop at the first approval: nothing is written, no record.
fresh_cache
phone_locked
FAKE_LOCKED=1 FAKE_FB_AFTER=1 run "${confirmations}stop\n"
refused "Stop at the unlock approval" 'stopped at the `unlock bootloader` gate'
[ ! -e "$cache/phones" ] || fail "Stop at the unlock approval wrote a record"
[ ! -e "$UNLOCKED_FLAG" ] || fail "Stop at the unlock approval unlocked the phone"
# ... and at the second: the first unlock stays, nothing more is written, the record says where it is.
fresh_cache
phone_locked
FAKE_LOCKED=1 FAKE_FB_AFTER=1 run "${confirmations}unlock bootloader\nstop\n"
[ "$rc" = 1 ] || fail "Stop at the critical approval exited $rc"
grep -q 'stopped at the `unlock critical` gate' "$root/out" || fail "Stop at the critical approval not reported"
[ "$(unlocks)" = "-s ABC123 flashing unlock" ] || fail "Stop at the critical approval ran: $(unlocks)"
! grep -Eq 'erase|flash (userdata|boot)|reboot' "$LOG" || fail "Stop at the critical approval wrote the install"
[ "$(state)" = unlock-in-progress ] || fail "Stop at the critical approval left $(state)"
[ "$(jq -r .backup "$STATE_FILE")" != "no backup taken" ] || fail "the record lost the backup path"
# ... a rerun from fastboot resumes at the critical unlock, with no second unlock command.
FAKE_LOCKED=1 run "${choice}unlock critical\ninstall nura\nreboot\n"
[ "$rc" = 0 ] || fail "the resumed run exited $rc"
[ "$(unlocks)" = "-s ABC123 flashing unlock_critical" ] || fail "the resumed run unlocked again: $(unlocks)"
grep -q 'The bootloader unlock is half done' "$root/out" || fail "the resumed run did not say where it was"
[ "$(state)" = nura-installed ] || fail "the resumed run left $(state)"
case $(jq -r .backup "$STATE_FILE") in "$cache"/backups/ABC123/backup-*) ;; *) fail "the resume lost the backup path" ;; esac

# The owner never confirms on the phone: the command is stopped, the phone is still locked, the record says so.
fresh_cache
phone_locked
FAKE_NOCONFIRM=unlock:never NURA_INSTALL_WRITE_TIMEOUT_SECONDS=2 FAKE_LOCKED=1 FAKE_FB_AFTER=1 run "$unlock_in"
[ "$rc" = 1 ] || fail "an unconfirmed unlock exited $rc"
grep -q 'did not finish within 2 seconds and was killed' "$root/out" || fail "the unconfirmed unlock was not reported"
[ "$(state)" = unlock-in-progress ] || fail "an unconfirmed unlock left $(state)"
! grep -q 'unlock_critical' "$LOG" || fail "the critical unlock ran after an unconfirmed unlock"
# ... rerun: still locked, so it resumes at the first unlock.
FAKE_LOCKED=1 run "${choice}unlock bootloader\nunlock critical\ninstall nura\nreboot\n"
[ "$rc" = 0 ] || fail "the rerun after an unconfirmed unlock exited $rc"
[ "$(unlocks)" = "-s ABC123 flashing unlock
-s ABC123 flashing unlock_critical" ] || fail "the rerun unlocked: $(unlocks)"
[ "$(state)" = nura-installed ] || fail "the rerun left $(state)"
# The owner declines on the phone, or fastboot prints FAILED and exits 0: the step fails and the record stays.
for mode in "FAKE_NOCONFIRM=unlock:declined" "FAKE_FAIL0=unlock" "FAKE_FAIL=unlock"; do
  fresh_cache
  phone_locked
  export "$mode"
  FAKE_LOCKED=1 FAKE_FB_AFTER=1 run "$unlock_in"
  unset "${mode%%=*}"
  [ "$rc" = 1 ] || fail "$mode exited $rc"
  grep -q 'FAILED\|boom' "$root/out" || fail "$mode: no failure shown: $(tail -5 "$root/out")"
  [ "$(state)" = unlock-in-progress ] || fail "$mode left $(state)"
  [ ! -e "$UNLOCKED_FLAG" ] || [ "$mode" = FAKE_FAIL0=unlock ] || fail "$mode unlocked the phone"
  ! grep -q 'unlock_critical' "$LOG" || fail "$mode went on to the critical unlock"
  ! grep -Eq 'erase|flash (userdata|boot)|reboot' "$LOG" || fail "$mode wrote the install"
done
# A FAILED critical unlock leaves the record at unlock-in-progress, not unlocked.
fresh_cache
phone_locked
FAKE_FAIL0=unlock_critical FAKE_LOCKED=1 FAKE_FB_AFTER=1 run "$unlock_in"
[ "$rc" = 1 ] || fail "a FAILED critical unlock exited $rc"
[ "$(state)" = unlock-in-progress ] || fail "a FAILED critical unlock left $(state)"
! grep -Eq 'erase|flash (userdata|boot)|reboot' "$LOG" || fail "a FAILED critical unlock went on to the install"
# Interrupted between the two commands: the record keeps the place, the rerun does only what is left.
fresh_cache
phone_locked
FAKE_INTERRUPT_AFTER=unlock FAKE_SIG_SLEEP=1 FAKE_LOCKED=1 FAKE_FB_AFTER=1 run "$unlock_in"
[ "$rc" = 1 ] || fail "an interruption after the first unlock exited $rc"
grep -q 'interrupted while writing' "$root/out" || fail "no interruption report"
[ "$(state)" = unlock-in-progress ] || fail "an interruption left $(state)"
! grep -q 'unlock_critical' "$LOG" || fail "the critical unlock ran after the interruption"
backup_then=$(jq -r .backup "$STATE_FILE")
FAKE_LOCKED=1 run "${choice}unlock critical\ninstall nura\nreboot\n"
[ "$rc" = 0 ] || fail "the run after the interruption exited $rc"
[ "$(unlocks)" = "-s ABC123 flashing unlock_critical" ] || fail "the resume after the interruption ran: $(unlocks)"
[ "$(jq -r .backup "$STATE_FILE")" = "$backup_then" ] || fail "the backup path changed on the resume"
# A phone recorded as unlocked goes straight to the install.
set_record unlocked
FAKE_LOCKED=1 run "$install_in"
[ "$rc" = 0 ] || fail "an unlocked-state run exited $rc"
[ -z "$(unlocks)" ] || fail "an unlocked-state run unlocked again: $(unlocks)"
[ "$(state)" = nura-installed ] || fail "an unlocked-state run left $(state)"
# A locked phone is still refused for everything but the unlock: a record that says installed needs it unlocked,
# and the lock commands are refused whatever asks (see the allowlist's unit tests).
set_record nura-installed
phone_locked
FAKE_LOCKED=1 run "$reinstall_in"
refused "a locked phone being reinstalled" 'unlocked: no'
! grep -q 'lock' <(sed 's/unlock//g; s/unlocked//g; s/Unlock//g' "$LOG") || fail "a lock command reached fastboot"
phone_locked

## Gates

set_record nura-in-progress
run "${choice}"'install\nInstall Nura\nINSTALL NURA\nuninstall nura\ninstall nura\nreboot\n'
[ "$rc" = 0 ] || fail "typos then the phrase exited $rc"
[ "$(grep -c 'That is not one of install nura' "$root/out")" = 4 ] || fail "typos not all re-asked"
set_record nura-installed
run "${choice}install nura\n"
refused "install phrase at a reinstall gate" 'stopped at the `reinstall nura` gate'
grep -q 'erases everything on Nura' "$root/out" || fail "the reinstall gate does not say it erases Nura"
run "${choice}stop\n"
refused "stop at the reinstall gate" 'stopped at the `reinstall nura` gate'
[ "$(state)" = nura-installed ] || fail "stop changed the record"
run "$choice"
refused "end of input at the gate" 'stopped at the `reinstall nura` gate'
run "${choice}reinstall nura\nstop\n"
[ "$rc" = 1 ] || fail "stop at the reboot gate exited $rc"
grep -q 'stopped at the `reboot` gate' "$root/out" || fail "no reboot gate report"
[ "$(state)" = nura-installed ] || fail "reboot gate changed the record"
! grep -q reboot "$LOG" || fail "rebooted after stop"
fresh_cache
FAKE_FB_AFTER=1 run "${confirmations}stop\n"
refused "stop at the first gate of a fresh run" 'unchanged Android'
[ ! -e "$cache/phones" ] || fail "a stopped fresh run left a record"

# Gates and confirmations read only a terminal; without the piped-input allowance nothing starts.
fresh_cache
NURA_INSTALL_ALLOW_PIPED_INPUT= FAKE_FB_AFTER=1 run "$fresh_in"
refused "piped input on a fresh install" 'interactive terminal'
! grep -q 'getprop\|exec-out' "$ADB_LOG" || fail "a run without a terminal went on to check the phone"
set_record nura-installed
NURA_INSTALL_ALLOW_PIPED_INPUT= run "$reinstall_in"
refused "piped input at a gate" 'interactive terminal'
! grep -q getvar "$LOG" || fail "a run without a terminal went on to check fastboot"
fresh_cache
NURA_INSTALL_ALLOW_PIPED_INPUT= FAKE_FB_AFTER=0 run ''
[ "$rc" = 3 ] || fail "a run that cannot reach a gate was refused for lacking a terminal"

## Failed write, interrupted group, a phone that vanishes

set_record nura-installed
set_record nura-installed
FAKE_FAIL0=dtbo run "$reinstall_in"
[ "$rc" = 1 ] || fail "a write that printed FAILED and exited 0 let the run go on (exit $rc)"
grep -q 'fastboot exited 0 but printed `.*FAILED' "$root/out" || fail "the FAILED line was not named: $(tail -5 "$root/out")"
! grep -q ' flash ' "$LOG" || fail "writes went on after a FAILED line"
grep -q 'output: .*FAILED (remote' "$cache/logs/"* || fail "fastboot's output is not in the transcript"
grep -q 'fastboot getvar partition-size:userdata_a: .*Variable Not found' "$cache/logs/"* || fail "a getvar answer is not in the transcript"
set_record nura-installed
FAKE_FAIL=userdata run "$reinstall_in"
[ "$rc" = 1 ] || fail "failed write exited $rc"
grep -q 'Writes that ran: erase dtbo; erase vendor_boot' "$root/out" || fail "no report of the writes that ran"
grep -q 'The record for phone ABC123 says `nura-in-progress`' "$root/out" || fail "no state in the failure report"
[ "$(state)" = nura-in-progress ] || fail "failed write left $(state)"
! grep -Eq 'flash boot|reboot' "$LOG" || fail "kept going after a failed write"
run "$install_in"
[ "$rc" = 0 ] || fail "finishing after a failed write exited $rc"
[ "$(state)" = nura-installed ] || fail "finishing after a failed write left $(state)"

set_record nura-installed
FAKE_HANG=userdata NURA_INSTALL_WRITE_TIMEOUT_SECONDS=1 run "$reinstall_in"
[ "$rc" = 1 ] || fail "a hung write exited $rc"
grep -q 'did not finish within 1 seconds and was killed' "$root/out" || fail "a hung write was not reported"
[ "$(state)" = nura-in-progress ] || fail "a hung write left $(state)"
! grep -Eq 'flash boot|reboot' "$LOG" || fail "kept going after a hung write"

set_record nura-installed
FAKE_SIG_SLEEP=1 FAKE_SIG_ON=boot run "$reinstall_in"
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
set_record nura-installed
rm -rf "$cache/downloads"
cp "$root/srv/nura-rootfs.img.xz" "$root/good.xz"
printf 'x' >> "$root/srv/nura-rootfs.img.xz"
run "$reinstall_in"
refused "corrupt download" "does not match Nura's index"
[ ! -e "$cache/downloads/nura-rootfs.img.xz" ] || fail "a corrupt download was kept"
cp "$root/good.xz" "$root/srv/nura-rootfs.img.xz"
# A hash the index gets wrong stops the run before anything is written.
make_index "$fp6_images"' |= (if (.name | endswith("-boot.img.xz")) then .sha256 = "'"$other_sha"'" else . end)'
rm -rf "$cache/downloads"
run "$reinstall_in"
refused "a wrong sha256 in the index" "nura-boot.img.xz does not match Nura's index"
[ "$(state)" = nura-installed ] || fail "a refused download changed the record"
! grep -q 'Approve the reinstall' "$root/out" || fail "the gate was shown with a download that did not verify"
make_index
# The newest build of the chosen interface in the newest release is the one installed, and it is shown.
run "$reinstall_in"
[ "$rc" = 0 ] || fail "the newest build exited $rc"
[ "$(cat "$FLASHED/userdata")" = nura-rootfs ] || fail "an older build was installed"
grep -q 'Nura build: edge phosh 2026-11-13T23:32:00' "$root/out" || fail "the build was not shown"
grep -q 'edge phosh 2026-11-13T23:32:00' "$STATE_FILE" || fail "the build was not recorded"
grep -q 'index.json' "$CURL_LOG" || fail "the index was not fetched this run"
rel_img() { jq -n --arg n "$1" --arg u "file://$root/srv/rel-$2.img.xz" --arg s "$(sum "$root/srv/rel-$2.img.xz")" '{name: $n, timestamp: "2026-12-01T10:00:00", url: $u, sha256: $s}'; }
newer_release='.releases += [{name: "v27.01", pretty_name: "v27.01", devices: [{name: "fairphone-fp6", pretty_name: "x", interfaces: [{name: "phosh", pretty_name: "Phosh", images: ['"$(rel_img 20261201-1000-nura-v27.01-phosh-fairphone-fp6-boot.img.xz boot)"', '"$(rel_img 20261201-1000-nura-v27.01-phosh-fairphone-fp6.img.xz rootfs)"']}]}]}]'
make_index "$newer_release"
set_record nura-installed
run "$reinstall_in"
[ "$rc" = 0 ] || fail "a newer release exited $rc"
[ "$(cat "$FLASHED/userdata")" = rel-rootfs ] || fail "the newer release's build was not installed"
grep -q 'Nura build: v27.01 phosh 2026-12-01T10:00:00' "$root/out" || fail "the newer release was not shown"
# Only builds that exist count: without the newer ones the older build is installed.
make_index "del($fp6_images | select(.timestamp | startswith(\"2026-11-13\")))"
set_record nura-installed
run "$reinstall_in"
[ "$rc" = 0 ] || fail "the older build exited $rc"
[ "$(cat "$FLASHED/userdata")" = old-rootfs ] || fail "the older build was not installed when it is the newest listed"
# The interfaces offered are the ones the index lists for the phone; unknown ones are listed by name.
make_index '.releases |= map(.devices |= map(if .name == "fairphone-fp6" then .interfaces += [(.interfaces[] | select(.name == "phosh") | .name = "lomiri")] | .interfaces |= map(select(.name != "sxmo-de-sway")) else . end))'
set_record nura-installed
run "sxmo\nlomiri\nreinstall nura\nreboot\n"
[ "$rc" = 0 ] || fail "an interface the index adds exited $rc"
grep -q '^  lomiri ' "$root/out" || fail "an unknown interface was not listed by name"
! grep -q '^  sxmo ' "$root/out" || fail "an interface the index lacks was offered"
[ "$(grep -c 'That is not one of phosh, plasma, gnome, lomiri' "$root/out")" = 1 ] || fail "a missing interface was not asked again"
grep -q 'Nura build: edge lomiri ' "$root/out" || fail "the unknown interface's build was not shown"
make_index 'del(.releases[].devices[] | select(.name == "fairphone-fp6"))'
set_record nura-installed
run "$reinstall_in"
refused "an index with no builds for the phone" 'lists no images for fairphone-fp6'
make_index "$fp6_images"'.url |= sub("^file://.*"; "https://evil.example/x.img.xz")'
run "$reinstall_in"
refused "foreign host" 'downloads may only come from'
make_index
NURA_INSTALL_NURA_INDEX=https://evil.example/index.json run "$reinstall_in"
refused "an index from a foreign host" 'downloads may only come from'
NURA_INSTALL_NURA_INDEX=file://$root/missing.json run "$reinstall_in"
refused "an index that cannot be fetched" 'missing.json'
echo 'not json' > "$root/index.json"
run "$reinstall_in"
refused "an index that is not JSON" "reading Nura's index"
make_index
NURA_INSTALL_ALLOW_FILE_URLS= run "$reinstall_in"
refused "file urls by default" 'only https downloads are allowed'
for empty in url sha256; do
  make_index "$fp6_images"'.'"$empty"' = ""'
  run "$reinstall_in"
  refused "empty $empty in the index" 'no URL or no valid sha256'
done
make_index

# Images are hashed again right before the first write, after the gate.
set_record nura-installed
rm -rf "$cache/downloads"
FAKE_TAMPER=$cache/downloads/nura-boot.img run "$reinstall_in"
refused "an image changed after the gate" 'nura-boot.img does not match the hash taken when it was unpacked'
[ "$(state)" = nura-installed ] || fail "a refused write changed the record"

## The reboot gate asks only who the phone is: erasing may change the snapshot status or the battery answer

set_record nura-installed
FAKE_AFTER_SNAPSHOT=merging FAKE_AFTER_BATTERY=no run "$reinstall_in"
[ "$rc" = 0 ] || fail "answers that changed after the writes stranded the run (exit $rc)"
grep -q ' reboot$' "$LOG" || fail "the phone was not rebooted after harmless changes"
set_record nura-installed
FAKE_AFTER_UNLOCKED=no run "$reinstall_in"
[ "$rc" = 1 ] || fail "a phone that relocked after the writes exited $rc"
grep -q 'the phone reports `unlocked: no`' "$root/out" || fail "the relock was not named"
! grep -q ' reboot$' "$LOG" || fail "rebooted a phone that is no longer unlocked"

## A screen that stopped taking output (terminal closed) must not stop a group half way

set_record nura-installed
rm -rf "$FLASHED" "$LOG" "$COUNT" "$PCOUNT" "$STATE_LOG" "$WROTE"
mkdir -p "$FLASHED"
: > "$LOG"
printf '%b' "$reinstall_in" | "$tool" 2> "$root/err" | head -c 0 || true
! grep -q panicked "$root/err" || fail "a closed screen panicked: $(cat "$root/err")"
grep -q ' flash boot ' "$LOG" && grep -q ' reboot$' "$LOG" || fail "a closed screen cut the writes short: $(cat "$LOG")"
[ "$(state)" = nura-installed ] || fail "a closed screen left the record at $(state)"

## Host prerequisites

fresh_cache
NURA_INSTALL_ADB=$root/missing FAKE_FB_AFTER=1 run "$choice"
refused "a missing prerequisite" 'prerequisite'
NURA_INSTALL_FASTBOOT=/bin/false FAKE_FB_AFTER=1 run "$choice"
refused "a prerequisite that fails" 'prerequisite /bin/false does not run'
NURA_INSTALL_NEEDED_GIB=999999 FAKE_FB_AFTER=1 run "$choice"
refused "too little disk space" 'GiB free'
chmod 755 "$cache"
FAKE_FB_AFTER=1 run "$choice"
refused "cache not private" 'private'
chmod 700 "$cache"
rc=0
flock "$cache/lock" "$tool" > "$root/out" 2>&1 < /dev/null || rc=$?
refused "concurrent run" 'another nura-install'

## Copy of the per-phone partitions (a recorded, installed phone; none in fastboot or over adb; sshd up)

known=$cache/phones/ABC123/known_hosts
copy_only() { # stdin
  sshd_up
  rm -rf "$cache"/backups/ABC123/partitions-*
  set_record nura-installed
  FAKE_FB_AFTER=9 FAKE_ADB=none run "$1"
}
no_ssh() { # name: ssh was never run
  [ ! -s "$SSH_LOG" ] || fail "$1: ssh was run: $(cat "$SSH_LOG")"
}
sshd_was_stopped() { [ -e "$SSHD_STOPPED" ] || fail "$1: sshd was not stopped"; }
no_copy_recorded() { [ -z "$(jq -r '.copy_path // empty' "$STATE_FILE")" ] || fail "$1: a copy was recorded"; }

rm -f "$known"
copy_only "$copy_in"
[ "$rc" = 0 ] || fail "copy-only run exited $rc"
cp "$root/out" "$root/out.copy"
plain_output "copy-only" "$root/out.copy"
step_blocks "copy-only" "$root/out.copy"
[ "$(sections_of "$root/out.copy")" = "[Preflight] 1/4 [Finish] 2/4 3/4 4/4" ] || fail "copy-only sections: $(sections_of "$root/out.copy")"
[ "$(cat "$LOG")" = devices ] || fail "the copy-only run asked fastboot for more than where the phone is: $(cat "$LOG")"
no_writes "copy-only"
! grep -q 'getprop\|exec-out' "$ADB_LOG" || fail "the copy-only run read the phone over adb"
[ "$(state)" = nura-installed ] || fail "copy-only left $(state)"
part_dir=$(jq -r .copy_path "$STATE_FILE")
for n in persist modemst1 modemst2 fsg fsc; do cmp -s "$part_dir/$n.img" "$PARTS/$n" || fail "copy-only: $n differs"; done
[ -e "$SSHD_STOPPED" ] && grep -q 'sshd is stopped again' "$root/out" || fail "copy-only did not stop sshd"
! grep -q 147147 "$SSH_LOG" || fail "the password is in ssh's arguments"
! grep -q 147147 "$SSH_ENV_LOG" || fail "the password is in ssh's environment"
[ -s "$known" ] || fail "the host key was not pinned"
# An installed phone that was copied is reinstalled from fastboot, without the copy steps, and forgets its host key.
run "$reinstall_in"
[ "$rc" = 0 ] || fail "reinstall of a copied phone exited $rc"
[ "$(sections_of "$root/out")" = "[Preflight] 1/10 2/10 3/10 4/10 5/10 [Install] 6/10 7/10 [Finish] 8/10 9/10 10/10" ] || fail "reinstall sections: $(sections_of "$root/out")"
! grep -q 'Start sshd on Nura' "$root/out" || fail "a reinstall of a copied phone asked for the copy again"
grep -q 'copied earlier' "$root/out" || fail "the summary does not say the copy exists"
[ ! -e "$known" ] || fail "a reinstall kept the old host key"
[ "$(jq -r .copy_path "$STATE_FILE")" = "$part_dir" ] || fail "the reinstall lost the record of the copy"
no_ssh "reinstall of a copied phone"

# Refusals before anything is read from the phone: nothing is run over ssh, and the owner is told sshd is still running.
echo 1 > "$root/ip_forward"
rm -f "$known"
copy_only "$copy_in"
[ "$rc" = 1 ] || fail "ip_forward=1 exited $rc"
grep -q 'net.ipv4.ip_forward is 1' "$root/out" || fail "ip_forward=1 was not named"
grep -q 'sshd may still be running on the phone' "$root/out" || fail "no word on sshd after the refusal"
no_ssh "ip_forward=1"; no_copy_recorded "ip_forward=1"; [ ! -e "$known" ] || fail "ip_forward=1 still pinned a key"
echo 0 > "$root/ip_forward"
FAKE_IP_EXTRA_ROUTE=1 copy_only "$copy_in"
[ "$rc" = 1 ] || fail "a second route exited $rc"
grep -q 'another interface has a route to the phone.s subnet (127.0.0.0/24 on tailscale0)' "$root/out" || fail "the second route was not named"
no_ssh "a second route"; no_copy_recorded "a second route"
FAKE_IP_GET_DEV=tailscale0 copy_only "$copy_in"
[ "$rc" = 1 ] || fail "a route over another interface exited $rc"
grep -q 'over `tailscale0`, not over usb0' "$root/out" || fail "the route over another interface was not named"
no_ssh "a route over another interface"
FAKE_IP_NOADDR=1 copy_only "$copy_in"
[ "$rc" = 1 ] || fail "no USB interface exited $rc"
grep -q 'no interface has 127.0.0.2' "$root/out" || fail "the missing USB interface was not named"
no_ssh "no USB interface"
# The host key is pinned only by the menu choice (Stop is the last entry).
rm -f "$known"
copy_only 'copy\nstop\n'
[ "$rc" = 1 ] || fail "an untrusted host key exited $rc"
grep -q 'host key was not trusted' "$root/out" || fail "the untrusted key was not named"
[ ! -e "$known" ] || fail "an untrusted key was pinned"
no_ssh "an untrusted host key"; no_copy_recorded "an untrusted host key"
copy_only 'copy\ntrust\nTrust this phone\n'"$trust"'\n'
[ "$rc" = 0 ] || fail "typos at the trust phrase exited $rc"
[ "$(grep -c 'That is not one of trust this phone' "$root/out")" = 2 ] || fail "the trust typos were not asked again"
# A host key that changed on a later run is refused by ssh; nothing is trusted again.
cp "$known" "$root/known.before"
FAKE_SSH_HOSTKEY=AAAAOTHER copy_only "$copy_in"
[ "$rc" = 1 ] || fail "a changed host key exited $rc"
grep -q 'REMOTE HOST IDENTIFICATION HAS CHANGED' "$root/out" || fail "ssh's refusal was not shown"
cmp -s "$known" "$root/known.before" || fail "a changed host key was trusted"
no_copy_recorded "a changed host key"
grep -q 'sshd may still be running on the phone' "$root/out" || fail "no word on sshd after a changed host key"
# What the phone has and says.
mv "$PARTS/fsg" "$root/fsg.away"
copy_only "$copy_in"
[ "$rc" = 1 ] || fail "a missing partition label exited $rc"
grep -q 'no partition labelled fsg; its labels are: fsc, modemst1, modemst2, persist' "$root/out" || fail "the missing label was not named with what is there"
sshd_was_stopped "missing partlabel"; no_copy_recorded "missing partlabel"
mv "$root/fsg.away" "$PARTS/fsg"
FAKE_SSH_SERIAL=OTHER9 copy_only "$copy_in"
[ "$rc" = 1 ] || fail "a serial mismatch exited $rc"
grep -q 'Nura reports serial OTHER9, but this phone is ABC123' "$root/out" || fail "the serial mismatch was not named"
sshd_was_stopped "serial mismatch"; no_copy_recorded "serial mismatch"
FAKE_SSH_CORRUPT=modemst1 copy_only "$copy_in"
[ "$rc" = 1 ] || fail "a corrupted stream exited $rc"
grep -q "modemst1 on this computer hashes to .*, the phone's hash is " "$root/out" || fail "the hash mismatch was not named"
sshd_was_stopped "hash mismatch"; no_copy_recorded "hash mismatch"
last=$(ls -d "$cache"/backups/ABC123/partitions-* | tail -1)
[ ! -e "$last/modemst1.img" ] || fail "a copy that did not hash right was kept"
[ -e "$last/persist.img" ] && [ ! -e "$last/manifest.txt" ] || fail "an incomplete copy has a manifest or lost a good file"
FAKE_SSH_BADHASH=fsc copy_only "$copy_in"
[ "$rc" = 1 ] || fail "a wrong hash from the phone exited $rc"
grep -q "fsc on this computer hashes to .*, the phone's hash is " "$root/out" || fail "the wrong hash from the phone was not named"
sshd_was_stopped "wrong hash from the phone"
FAKE_SSH_DDFAIL=modemst2 copy_only "$copy_in"
[ "$rc" = 1 ] || fail "a failed stream exited $rc"
grep -q 'dd: error reading' "$root/out" || fail "the failure of the stream was not shown"
sshd_was_stopped "a mid-copy failure"; no_copy_recorded "a mid-copy failure"
grep -q 'sshd is stopped again' "$root/out" || fail "a mid-copy failure did not say sshd was stopped"
FAKE_SSH_DDFAIL=modemst2 FAKE_SSH_STOPFAIL=1 copy_only "$copy_in"
[ "$rc" = 1 ] || fail "a failed stream and a failed stop exited $rc"
[ ! -e "$SSHD_STOPPED" ] || fail "the failing stop stopped sshd"
grep -q 'sshd may still be running on the phone; stop it there with `sudo systemctl stop sshd`' "$root/out" || fail "a failed stop did not say how to stop sshd"
# Ctrl-C, hang-up and terminate in the middle of the copy still stop sshd, or say how to.
interrupt_copy() { # name, signal, expected exit code, env assignments...
  local name=$1 sig=$2 want=$3
  shift 3
  sshd_up
  rm -rf "$cache"/backups/ABC123/partitions-* "$SSHD_STOPPED"
  set_record nura-installed
  : > "$SSH_LOG"
  printf '%b' "$copy_in" | env FAKE_FB_AFTER=9 FAKE_ADB=none FAKE_SSH_HANG='dd if=' "$@" "$tool" > "$root/out" 2>&1 &
  local pid=$!
  wait_for "$SSH_LOG" 'dd if='
  kill -s "$sig" "$pid"
  rc=0
  wait "$pid" || rc=$?
  [ "$rc" = "$want" ] || fail "$name: exited $rc, not $want"
  grep -qF 'interrupted: ' "$root/out" || fail "$name: no state line"
  no_copy_recorded "$name"
}
for case in INT:130 TERM:143 HUP:129; do
  interrupt_copy "$case in the copy" "${case%%:*}" "${case##*:}"
  sshd_was_stopped "${case%%:*} in the copy"
  grep -q 'sshd is stopped again' "$root/out" || fail "${case%%:*} in the copy did not say sshd was stopped"
done
interrupt_copy "Ctrl-C in the copy with a failing stop" INT 130 FAKE_SSH_STOPFAIL=1
[ ! -e "$SSHD_STOPPED" ] || fail "the failing stop stopped sshd"
[ "$(grep -c 'stop it there with `sudo systemctl stop sshd`' "$root/out")" = 1 ] || fail "a failed stop after Ctrl-C did not give the manual command once"

# A partition the phone cannot account for is never kept.
FAKE_SSH_DUPLABEL=fsg copy_only "$copy_in"
[ "$rc" = 1 ] || fail "an ambiguous partition label exited $rc"
grep -q '2 devices on the phone carry the partition label fsg' "$root/out" || fail "the ambiguous label was not named"
sshd_was_stopped "an ambiguous label"; no_copy_recorded "an ambiguous label"
FAKE_SSH_NOHOLDER=fsg copy_only "$copy_in"
[ "$rc" = 1 ] || fail "a label with no holder exited $rc"
grep -q 'no device on the phone carries the partition label fsg' "$root/out" || fail "the label with no holder was not named"
FAKE_SSH_SIZE_OFF=persist copy_only "$copy_in"
[ "$rc" = 1 ] || fail "a size mismatch exited $rc"
grep -q 'persist on this computer is [0-9]* bytes, the partition on the phone is [0-9]*; the copy was dropped' "$root/out" || fail "the size mismatch was not named"
last=$(ls -d "$cache"/backups/ABC123/partitions-* | tail -1)
[ ! -e "$last/persist.img" ] || fail "a copy of the wrong size was kept"
no_copy_recorded "a size mismatch"
cp "$PARTS/fsc" "$root/fsc.real"
head -c 4096 /dev/zero > "$PARTS/fsc"
copy_only "$copy_in"
cp "$root/fsc.real" "$PARTS/fsc"
[ "$rc" = 0 ] || fail "a blank partition whose hash equals the phone's exited $rc"
grep -q 'copy fsc: .*its hash equals the phone.s, all zeros on the phone too' "$root/out" || fail "the blank partition was not kept and named"
last=$(ls -d "$cache"/backups/ABC123/partitions-* | tail -1)
[ -e "$last/fsc.img" ] || fail "the blank partition's copy was dropped"

# Skipping, and a phone that never answers.
sshd_down
set_record nura-installed
FAKE_FB_AFTER=9 FAKE_ADB=none run 'skip\n'
[ "$rc" = 3 ] || fail "skip exited $rc"
grep -q 'skipped at your request' "$root/out" || fail "skip was not acknowledged"
grep -q 'sudo systemctl start sshd' "$root/out" || fail "skip did not say how to take the copy later"
no_ssh "skip"; no_copy_recorded "skip"
rc=0
{ printf 'copy\n'; sleep 3; } | FAKE_FB_AFTER=9 FAKE_ADB=none NURA_INSTALL_COPY_WAIT_SECONDS=1 "$tool" > "$root/out" 2>&1 || rc=$?
[ "$rc" = 1 ] || fail "a phone that never answers exited $rc"
grep -q 'did not answer within 1 s' "$root/out" || fail "the wait limit was not named"
no_ssh "a phone that never answers"
# During an install a skipped or failed copy never fails the install and says clearly it is missing.
fresh_cache
FAKE_FB_AFTER=1 run "${fresh_in}skip\n"
[ "$rc" = 0 ] || fail "an install with a skipped copy exited $rc"
[ "$(state)" = nura-installed ] || fail "a skipped copy left $(state)"
no_copy_recorded "an install with a skipped copy"
grep -q 'The per-phone partitions are NOT copied: skipped at your request' "$root/out" || fail "the summary does not say the copy is missing"
grep -q 'nura-install. (does only the copy)' "$root/out" || fail "the summary does not say how to take the copy"
# The copy-only run that follows takes it.
sshd_up
FAKE_FB_AFTER=9 FAKE_ADB=none run "$copy_in"
[ "$rc" = 0 ] || fail "the copy after an install without one exited $rc"
[ -n "$(jq -r .copy_path "$STATE_FILE")" ] || fail "the late copy was not recorded"
fresh_cache
sshd_up
FAKE_FB_AFTER=1 FAKE_SSH_HANG='cat /proc/cmdline' NURA_INSTALL_SSH_TIMEOUT_SECONDS=1 run "${fresh_in}$copy_in"
[ "$rc" = 0 ] || fail "an install with a copy that timed out exited $rc"
[ "$(state)" = nura-installed ] || fail "a copy that timed out left $(state)"
grep -q 'did not finish within 1 seconds and was killed' "$root/out" || fail "the ssh timeout was not named"
sshd_was_stopped "an ssh timeout"
grep -q 'The per-phone partitions are NOT copied' "$root/out" || fail "the summary does not say the copy is missing after a timeout"
no_copy_recorded "an ssh timeout"
sshd_down
# Two installed phones without a copy: the run does not guess which one is meant.
set_record nura-installed
mkdir -p -m 700 "$cache/phones/OTHER1" && cp "$STATE_FILE" "$cache/phones/OTHER1/state.json"
FAKE_FB_AFTER=9 FAKE_ADB=none run "$choice"
refused "two installed phones without a copy" 'installed phones have no copy'
rm -rf "$cache/phones/OTHER1"
# A phone over adb is on Android, whatever its record says: no copy-only run.
set_record nura-installed
FAKE_FB_AFTER=9 run "$choice"
refused "a phone over adb with a record" 'from fastboot' 3

## Terminal behaviour: spinner on a terminal only, progress in the transcript, Ctrl-C, typed-ahead input

fresh_cache
NURA_INSTALL_PROGRESS_SECONDS=1 FAKE_SLOW_TAR=3 FAKE_FB_AFTER=1 run "$confirmations"
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
  env TERM=xterm NURA_INSTALL_SECURITY_DELAY_MS=$security_delay_ms NURA_INSTALL_SECURITY_DELAY_AFTER_MOVE_MS=$move_delay_ms FAKE_FB_AFTER=1 "$@" script -qefc "$tool" /dev/null < "$root/tty-in" > "$root/pty" 2>&1 &
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
LC_ALL=C grep -aq -P '\x1b\]2;[^\x07]* nura 10/19 Back up the phone[^\x07]* · [0-9]+:[0-9]{2} \([0-9]+:[0-9]{2}\)\x07' "$root/pty" || fail "no working title on the terminal: $(LC_ALL=C grep -a -o -P '\x1b\]2;[^\x07]*' "$root/pty" | head -3 | cat -v)"
LC_ALL=C grep -aq -P '\x1b\]2;⏸ nura 13/19 choose an option · ' "$root/pty" || fail "no waiting title at the gate"
LC_ALL=C grep -aq -P '\x1b\]2;✗ nura stopped at 13/19 · [0-9]+:[0-9]{2}\x07' "$root/pty" || fail "no closing title on the terminal"
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
menu_at 'Skip the copy'
pick_with "$DOWN$DOWN"
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
  printf '%b' "$2" | FAKE_SLOW_BATTERY=$battery FAKE_SLOW_TAR=8 FAKE_FB_AFTER=1 "$tool" > "$root/out" 2>&1 &
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
