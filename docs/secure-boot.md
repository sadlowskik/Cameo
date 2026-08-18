# Secure Boot — F3

With Secure Boot **on**, firmware runs only signed bootloaders. archiso's
`systemd-boot`/`syslinux` are unsigned, so an SB-on machine silently rejects the
USB and falls through to the next boot device — the "won't boot" surprise. This is
the plan to boot unmodified under SB, plus the fallback that works today.

> Status: **documented + fallback works now; the signing pipeline is
> hardware-gated.** Secure Boot cannot be validated without an SB machine and a
> Linux `mkarchiso` build, so the signing steps below are the design the build
> will implement, not yet a verified path. The container avoids this entirely
> (the host owns boot) — another reason container-first.

## The fallback that works today

Disable Secure Boot in firmware (or add the USB as a trusted boot option). This is
a documented, one-time BIOS setting and is how you boot Cameo now.

## The shim chain (the real fix)

The established pattern for a self-built distro is **shim + a signed second stage**:

1. **shim** is signed by Microsoft's UEFI CA, so firmware trusts it unmodified.
   Cameo ships the `shim-signed` EFI binary as the first-stage bootloader in the
   ESP (`EFI/BOOT/BOOTX64.EFI`).
2. shim then verifies the **second-stage bootloader** (`systemd-boot`) and the
   **kernel** against either Microsoft's db *or* a key you enroll. Cameo signs
   both with its own key (`sbsign`), and ships that key's certificate on the ESP.
3. On first SB boot, shim's **MOK Manager** prompts the user once to enroll
   Cameo's certificate (`mokutil` / the blue MOK screen). After that, every boot
   is unattended.

### What the build does (design)

`build-iso.sh` grows an **opt-in** signing step, guarded so default builds are
unchanged:

- Inputs: `CAMEO_SB_KEY` / `CAMEO_SB_CERT` (a key pair you generate once with
  `openssl`), and the `shim-signed` package.
- Steps: `sbsign --key … --cert …` the staged `systemd-bootx64.efi` and the
  kernel; place `shim` as `BOOTX64.EFI` chaining to the signed
  `grubx64.efi`/`systemd-bootx64.efi`; copy the `.cer` onto the ESP for MOK
  enrollment.
- Reproducibility (F4) is unaffected: signing is deterministic given the same key.

Keys are the operator's: Cameo does not ship a private signing key, and it never
signs with one baked into the repo.

## Verifying (needs the hardware)

1. Build with `CAMEO_SB_KEY`/`CAMEO_SB_CERT` set on an Arch host.
2. Boot an SB-on machine; enroll the certificate at the MOK prompt.
3. Confirm it reaches the Cameo console without disabling SB.

Until that run happens on real SB hardware, treat SB support as **planned with a
documented fallback**, not verified.
