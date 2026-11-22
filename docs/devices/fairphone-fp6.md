# Fairphone (Gen. 6)

Tested on one phone, installing from /e/OS.

## Unlocking the bootloader

This follows [Fairphone's guide](https://support.fairphone.com/hc/en-us/articles/10492476238865-How-to-unlock-or-lock-your-Fairphone-s-bootloader), written out here in case that page moves.

Unlocking erases everything on the phone, twice. The tool does these steps for you: when the bootloader is locked, it backs up the phone first, then asks you to turn on OEM unlocking, and runs both unlock commands after you approve each. It has not been tried on a locked phone.

By hand:

On the phone:

1. Update to the latest system release.
2. Go to Settings > About phone and tap Build number seven times. This turns on Developer options.
3. Go to Settings > System > Developer options and turn on USB debugging.
4. In the same menu, turn on OEM unlocking. The phone needs an internet connection for this, and may ask for your PIN.

On the computer, with the phone plugged in:

1. Open a shell with the Android tools: `nix shell nixpkgs#android-tools`.
2. Run `adb devices`, and tap Allow on the phone.
3. Run `adb reboot bootloader`. The phone restarts into fastboot.
4. Run `fastboot flashing unlock`, and confirm on the phone with the volume and power buttons. The phone wipes itself and restarts.
5. Go back into fastboot. Turn the phone off, then hold Volume Down and Power until the fastboot screen shows.
6. Run `fastboot flashing unlock_critical`, and confirm on the phone again. It wipes itself a second time.

Don't lock the bootloader again while Nura is installed. Locking it with an operating system Fairphone didn't sign can stop the phone from booting.

## Reaching fastboot

Turn the phone off, then hold Volume Down and Power until the fastboot screen shows. The tool asks for this when it needs it.

## What the install writes

It erases `dtbo` and `vendor_boot`, writes Nura's root filesystem to `userdata` and its kernel to `boot`, all on the active slot. That's the same as Nura's web flasher does for this phone.

## What it copies

Once Nura runs, the tool copies `persist`, `modemst1`, `modemst2`, `fsg` and `fsc`.

They hold the IMEI and calibration, which are unique to your phone, not just to the model. If they're ever lost, no download can bring them back, so the tool keeps a copy.

## Quirks

- Phosh may hang on its first boot, but a second reboot should bring it up.
- `modemst1` is blank on the tested phone. The tool reports that and keeps the copy as is.
- The bootloader answers `has-slot` for only a few partitions, so the tool checks `partition-size` instead.
- The bootloader leaves `version-bootloader` and `version-baseband` empty.

[`tests/fixtures/fp6-getvar-all.txt`](../../tests/fixtures/fp6-getvar-all.txt) is this bootloader's real `fastboot getvar all`, with the serial replaced.
