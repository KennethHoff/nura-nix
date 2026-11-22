# nura-nix

Install [Nura](https://nura.eco) on any phone with an unlocked bootloader, without risking a brick.

This does the same install as Nura's web flasher, but walks you through each step and checks the phone as it goes. It also backs up what you can't download again.

## Devices

A phone is supported once its install recipe, bootloader checks and partition copy are in the tool, and it has been tried on a real phone.

| Phone | Android | Firmware | Nura | Last tested |
|---|---|---|---|---|
| [Fairphone (Gen. 6)](docs/devices/fairphone-fp6.md) | /e/OS 3.6 (Android 15) | FP6.QREL.15.176.0 | edge 2026-11-13 | 2026-11-22 |

Each phone's page says how to unlock its bootloader and lists its quirks.

Nura builds for many more phones. They're listed in its [image index](https://images.nura.eco/bpo/index.json), and any of them can be added. Contributing says how.

## Usage

You need:

- Linux with Nix installed (macOS support planned)
- the phone's bootloader unlocked, or let the tool unlock it; its page under Devices says how
- a USB cable, plugged straight into the computer, without a hub
- about 12 GiB free on the computer

Then:

1. On the phone, turn on USB debugging and plug it in.
2. Run `nix run github:KennethHoff/nura-nix` in a terminal.
3. Follow the steps it shows. It tells you when to press buttons on the phone, and asks before it writes anything.
4. When Nura has started, the tool asks you to start SSH on the phone, and shows how.

A run takes about 15–30 minutes. The backup and the first download take most of it, so a phone full of photos or a slow connection takes longer.

A locked bootloader adds an unlock after the backup. It erases the phone, twice, and each erase waits for your approval and for a confirmation on the phone. This part has not been tried on a locked phone.

If the run stops before you approve the unlock or the install, nothing on the phone has changed. Run the same command again to start over from wherever the phone is.

If it stops during the unlock, run the same command again with the phone in fastboot. It finishes the unlock and goes on.

## Design

It reads the phone over `adb` and `fastboot` and works out what to do from the phone's state. You then pick a Nura interface from the builds in Nura's image index.

It writes only the partitions Nura's own web flasher writes, on the active slot. It never touches bootloader firmware, partition tables or the active slot. It can unlock the bootloader when you approve, and it never locks it.

Before downloading anything, it checks every bootloader answer the install depends on. It checks them again right after you approve the writes. A check that fails or can't decide stops the run.

Before the writes, it backs up the phone's storage, contacts, messages, call log and calendar.

Once Nura runs, it copies the partitions that hold the IMEI and calibration. Nothing can download those again.

There is no way back built in. To return to Android, use the phone maker's or the OS's own installer.

## Contributing

To add a phone:

1. Check that Nura builds for the phone. It must be listed in `https://images.nura.eco/bpo/index.json`.
2. Capture the bootloader's `fastboot getvar all`, replace the serial, and add it to `tests/fixtures/`.
3. Map the phone's fastboot `product` to Nura's device name in `pkgs/nura-install/src/nura.rs`.
4. Write down the install from Nura's web flasher recipe. [The Fairphone's ADR](docs/adr/0001-guarded-fairphone-6-flashing.md) shows the shape.
5. List the partitions that can't be downloaded again, such as the IMEI and calibration, so the tool copies them.
6. Run `nix flake check`, then try it on the real phone.
7. Open a pull request with the run's transcript from `~/.local/share/nura-install/logs/`, with the serial removed.

## License

[MIT](LICENSE)
