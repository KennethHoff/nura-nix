---
status: accepted
date: 2026-11-21
---

# Install Nura on a Fairphone (Gen. 6) by writing only what Nura's installer writes

A Fairphone (Gen. 6) that runs /e/OS is to try Nura (the renamed postmarketOS). The design is
meant for any phone with an unlocked bootloader that Nura builds for; the Fairphone (Gen. 6) is
the only one supported and tested. A phone that can no longer reach fastboot can only be repaired
by Fairphone (the emergency download loader isn't public), so the tool is scoped to this one
install and avoids every write that could cause that.

## Decision

`nura-install` (Rust, `pkgs/nura-install/`, Linux only; it takes no arguments, and the action is
inferred from the phone's state and its record, below) is the only way this repo writes to the phone.
It writes the same few partitions of one slot as Nura's web flasher:

| Install Nura (the web flasher's FP6 steps) |
|---|
| erase `dtbo`, erase `vendor_boot`, flash `userdata` (Nura's rootfs), flash `boot` |

Firmware, `super`, `vbmeta`, `misc`, `metadata`, the partition tables' layout and the active slot
are never written, so the phone keeps its firmware and Android system partitions throughout.
There is no way back in the tool: going back to Android is the manufacturer's or the OS's own
installer, run by hand. The write allowlist holds only what the install and the unlock use.

- **The tool may unlock the bootloader, never lock it.** A locked phone gets an *Unlock the
  bootloader* section after the backup (an unlock wipes the phone, so the backup comes first):
  the owner turns on OEM unlocking (read over `adb` from `sys.oem_unlock_allowed`, which AOSP's
  `PersistentDataBlockService` sets to `1` or `0`; where the phone does not report it the owner
  confirms in a menu), goes into fastboot, and approves `fastboot flashing unlock`, then after
  the wipe approves `fastboot flashing unlock_critical`, each at a gate that names the erase. The
  phone asks for its own confirmation on its buttons, and the install goes on from fastboot without
  booting the wiped Android again. The allowlist gains those two commands and nothing else;
  `flashing lock`, `flashing lock_critical` and `oem lock` stay refused. The commands are the
  FP6's facts, kept beside its install recipe in `group.rs`. The record says `unlock-in-progress`
  from before the first command, with the backup path, and `unlocked` once both ran; a rerun from
  fastboot reads `unlocked` and resumes at the step left. The bootloader reports no variable for
  the critical unlock, so that step is known only from the command's success.
- **Nura's builds come from Nura's image index** (`https://images.nura.eco/bpo/index.json`), fetched
  fresh on every run, and nothing is pinned in the repo. The phone's fastboot `product` (`FP6`) maps
  to Nura's device name (`fairphone-fp6`) in a small table in the code; the interfaces offered are
  those the index lists for that device, and the build is the newest of the chosen interface across
  the releases that list it (the newest timestamp, in the release that holds it). The download must
  match the SHA256 the index gives (https, the host allowlist, no redirects); the unpacked images are
  hashed by the tool itself, and those hashes go into the phone's record and are checked again right
  before the write. The owner trusts Nura's server, which is also where the images come from, and
  wants the tool to work for any phone Nura builds for; the tool never picks anything the index
  does not list. After the Preflight the run needs no internet.
- **Firmware is not pinned or checked.** Nura declares no required version and ships its own runtime
  firmware (from FP6.QREL.15.91.0); the phone is installed on the firmware it has.
- **Installing Nura starts only from Android** and is one run in four sections, each step either the
  owner's or the tool's: *Preflight* (read-only: this computer, the phone over `adb`, then a first trip to fastboot
  where `fastboot getvar all` is read once and every answer the install needs is checked, with all
  failures reported together and before anything is downloaded or backed up; Nura's index, read for
  the phone's product, and the interface choice; then back to Android, the same phone confirmed over
  `adb`, downloads and their verification), *Prepare* (a confirmation, the backup choice and the
  backup, fastboot and the same checks again, with every image fitting its partition),
  *Install* (the approval, then the writes), *Finish* (restart, the choice to copy the per-phone
  partitions and the copy, summary). Over `adb` it pins the serial, records the slot, and checks no
  account is signed in and the battery is at least 50 %. The screen lock can't be read over `adb`, so
  removing it is a checklist item acknowledged by a menu choice, next to the list of what is lost for
  good (app data, messengers, authenticators, passkeys, eSIM profiles). The backup is the last
  thing before fastboot, so it can't go stale: storage verified against hashes taken on the
  phone, and contacts, SMS, call log and calendar with their row counts shown, into
  `backups/<serial>/`. The backup is a choice made before anything is read: back up, or go on
  without one, which is a second menu naming that the data is gone for good. Nothing reads keys
  while the backup runs but Ctrl-C. Going on without a backup leaves no backup directory, the
  record's backup path reads `no backup taken` and every summary says so. Only on a
  real terminal.
- **The partitions no download can replace are copied once Nura runs**: `persist`, `modemst1`,
  `modemst2`, `fsg`, `fsc` (the phone's IMEI and calibration). Fastboot can't read
  them. After `sudo systemctl start sshd` on Nura, the tool reads them over the USB cable's
  network link only (it stops if the desktop could route that subnet elsewhere), with Nura's host
  key pinned per phone on first contact and the password given through OpenSSH's askpass hook,
  verifies each against a hash taken on the phone (and that the label names one device, the copy
  is as long as the partition; a blank partition is kept and named, since the FP6's `modemst1` is
  blank), fsyncs the files and directories, and stops `sshd`
  again, also on Ctrl-C, hang-up and terminate; if that stop cannot run, the exact manual command is
  printed. A skipped or failed copy doesn't undo the install; running `nura-install` on an installed phone whose copy is missing then does only the copy.
- **A record per serial** says where the phone is: none, `nura in progress`, `nura installed`. It
  is written atomically before a group's first write and after its last. The action is inferred
  from the phone and the record: Android over adb with no record installs Nura; an installed Nura
  that was never copied, and is not in fastboot, does only the copy; `nura in progress` in
  fastboot finishes the install; `nura installed` in fastboot reinstalls. Each of those asks only
  for the approval of its writes, since no other action is left to choose. Anything else exits 3
  with "nothing to do" and the next command. The record holds the slot, the backup path, the Nura
  build and the hashes of the images written, and where the partition copy is.
- **In fastboot** (entered with Volume Down + Power, since `adb reboot bootloader` writes `misc`),
  both check: same serial, bootloader fastboot, unlocked, the recorded slot of two, the
  partitions it will write laid out as expected (`partition-size:<name>_a` and `_b` above zero for
  `boot`, `vendor_boot` and `dtbo`; `partition-size:userdata` above zero, and `userdata_a`
  answered "Variable Not found": the FP6's bootloader answers
  `has-slot` for a few partitions only, not `vendor_boot`), every image about to be written no bigger
  than its partition, and a charged battery. Every answer must come exactly once; a
  failed, missing or contradictory one stops the run. fastboot exits 0 after a remote `FAILED` for
  `getvar` (AOSP `DisplayVarOrError`), so its text decides, and a `FAILED` line in the output of a
  write fails that write whatever the exit status. Its raw answers are in the transcript. The checks run again after the gate, except at the
  reboot gate, which comes after erases and flashes and so asks only who the phone is: the same
  serial, in bootloader mode (`is-userspace` no), unlocked, on the recorded slot. The battery may legitimately differ by then and must not strand the run.
- **Stop on doubt.** Any check that answers no, can't decide or can't get its data stops the run.
- **One gate per group of writes that must finish together** (the install's writes, then the
  reboot). It lists the writes and shows a menu whose option names the group and its cost
  ("Install Nura (erases Android and all its data)"); "Stop the run" ends the run with a report of the
  phone's state. Ctrl-C and hang-up are held off inside a group (`libc`). A group is full overwrites,
  so finishing an interrupted one repeats it. Gates only read a terminal, after discarding anything
  typed ahead. A group that wipes a working Nura says so.
- **Choices and approvals are arrow-key menus, not typed phrases.** Every choice (the interface,
  back up or skip, copy or skip)
  and every approval (the screen lock is off, an empty export, the install, reinstall and
  reboot, going on without a backup, trusting the phone's
  host key) is one. A menu starts with nothing highlighted: Enter does nothing until an arrow has
  put the cursor on an option, and "Stop the run" is always the last entry (also Esc, `q` and
  Ctrl-C). Enter has a *security delay*: it is ignored (and dropped, never queued) for 3 s after the
  menu appears and for 1 s after each move of the cursor, and the hint dims "enter select" until it
  works; styling falls back to words where `NO_COLOR` or a dumb terminal says so. This is the
  precedent of browsers' dialogs (Firefox's `security.dialog_enable_delay`, Chromium's input
  protection): the delay and the empty cursor replace the protection a typed phrase gave against a
  reflexive confirmation. Typed-ahead input is discarded when a menu is drawn, nothing reads keys
  during a running step but Ctrl-C, and the transcript logs `choice: <option>` with the time the
  menu was open. The delays are test-hook overrides only; the shipped binary has fixed values.
- **Nothing downloaded is executed.** The tool hands image paths to `fastboot` and nothing else.
  After the downloads the run needs no internet.
- **What it keeps** lives in one cache directory the consumer sets, private (`0700`/`0600`, owner checked); it
  refuses to run as root, logs counts rather than contents, and keeps a transcript of every run. The shipped binary has no overrides for the index address,
  tools, hosts, delays or checks; the test hooks exist only in the build the flake check uses.

## Considered options

- **Nura's web flasher.** The same install writes, but no checks and no backup.
- **A guided way back to Android in the tool.** It restored the phone's own /e/OS images (found
  from Murena's build list, proven against the phone's `vbmeta` digest with `avbtool`, kept
  per phone) and fingerprinted the firmware partitions so the way back could prove nothing had
  changed underneath. Dropped by the owner: it was never tested on a phone and the owner does not
  intend to test it, and untested code that erases a phone is worse than none. Going back is the
  manufacturer's or the OS's own installer, by hand. The design is in this repository's history
  and on the `rollback-to-android` branch.
- **Pinning hashes in the repo (`pins.toml`, one edge build per interface; the previous design).**
  A committed hash was the only check that did not rest on Nura's server, but it tied the tool to
  one build per phone and interface, went stale with every Nura build, and needed a commit and a
  rebuild to install anything newer or for any other phone. Rejected because the owner trusts
  Nura's server (the images come from it either way) and wants any phone Nura builds for to be
  installable; the index's SHA256 is still checked on every download, and the unpacked images are
  hashed by the tool.
- **A required `--os nura|android` flag.** The action is a fact of the phone and its record, not a
  choice to get wrong: the flag let a run start from a state it did not fit and needed extra
  "nothing to do" rules. Inferred instead, one action per state.
- **Typed approval phrases (`install nura`, `reboot`, `no backup`, `Confirm`,
  `trust this phone`), and typing `skip` while a step runs.**
  Dropped because the owner prefers selecting; the security delay, the empty cursor and Stop as an
  explicit entry replace the protection a typed phrase gave against reflexive confirmation, and
  choosing before a step runs removes the keystrokes read during one.
- **NixOS on the phone (`hosts/fairphone/`).** The goal, but no FP6 NixOS port has booted in a
  maintained form yet; revisit when one does.

## Consequences

- The tool is meant to move to a public repository of its own, so it holds nothing specific to one
  phone or one computer: no serials or other per-device values (test fixtures use placeholders),
  no host paths in the code or its docs (the consumer sets the cache directory), and nothing
  personal in its texts.
- The tool has no way back to Android. A phone that runs Nura goes back with the manufacturer's or
  the OS's own installer, by hand, and loses what Nura holds; only the backup and the copy of the
  per-phone partitions on this computer survive.
- The phone keeps the firmware it had. Firmware only goes up and not back, so Nura is tried on
  the current firmware first.
- The copy of the per-phone partitions, like every phone answer, can only be proven on the phone.
- If Nura fails to boot about seven times in a row, the bootloader falls back to the other slot,
  which after an Android update no longer has a system. Getting back would then take a slot switch,
  which this tool never does. So when Nura doesn't come up on the first boot or two, go straight to
  fastboot and run `nura-install`, which reinstalls or finishes the install. On the phone this was developed on, `getvar all` reports the other
  slot as `slot-unbootable:<slot>:yes`, so the bootloader would stay in fastboot instead of falling
  back; that is an observation of one phone, not a guarantee, and the advice stands.
- Flashing `boot` makes the bootloader reset that slot's boot flags, and Nura's `qbootctl -m`
  marks the slot successful once; both write attribute bits only. `qbootctl -s` (slot switching)
  has bricked an FP6 and must never be run on Nura.
- Nura's kernel updates `dd` the running slot's `boot`; an interrupted update is recovered by running `nura-install` again
  from fastboot, which reinstalls.
- The backup is a copy for reading back, not a restore image. Snapshots or off-host backups of the
  cache directory copy it too.
- The unlock is untested on a real locked phone: the owner's phone was already unlocked. The
  commands come from Fairphone's guide, the OEM property from AOSP source, and the buttons
  and wording of the phone's own confirmation are unverified. If the phone is interrupted between
  an unlock command succeeding and its record being written, the rerun repeats that command.
- The flake check runs the tool against a fake `adb` and `fastboot`, with answers captured from
  the real phone once available; how the real phone behaves can only be checked on the phone.
