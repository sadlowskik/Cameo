#!/usr/bin/env bash
# Build the Cameo ISO.
#
# LINUX + Arch only — needs `mkarchiso` (the `archiso` package) and root. This
# CANNOT run on Windows/macOS; build it on your Cameo/Arch box (or an Arch
# live-USB). It builds from a *copy* of the profile, so the source tree stays clean.
#
#   sudo ./scripts/build-iso.sh                        # full edition (ROCm included)
#   sudo CAMEO_EDITION=lite ./scripts/build-iso.sh     # Vulkan-only, small — ideal for
#                                                      # old / Tier-3 cards (e.g. an iGPU laptop)
#
# Run it with `sudo`, not as a root login: the cameo CLI is then compiled as the
# invoking user rather than as root (see "Building the CLI" below).
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="$REPO/archiso"
# The work dir must live OUTSIDE the profile. mkarchiso builds from a staged
# copy at $WORK/profile, so a work dir of archiso/work makes that copy
# `cp -r archiso archiso/work/profile` — a directory into itself, which cp
# refuses. Keep this outside archiso/ or the build cannot start.
WORK="${CAMEO_WORK:-$REPO/build}"
OUT="${CAMEO_OUT:-$REPO/archiso/out}"
EDITION="${CAMEO_EDITION:-full}"
RELENG="${CAMEO_RELENG:-/usr/share/archiso/configs/releng}"

log() { printf '\033[1;38;5;209m[cameo-iso]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[cameo-iso] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

command -v mkarchiso >/dev/null 2>&1 || die "mkarchiso not found — install the 'archiso' package (Arch only)."
[ "$(id -u)" -eq 0 ] || die "run as root — mkarchiso needs it: sudo $0"
[ -d "$RELENG" ] || die "baseline releng profile not found at $RELENG (install 'archiso'), or set CAMEO_RELENG."

# 0. A reproducible identity for the image.
#
# Deriving the version and volume label from `date` meant two builds of the same
# commit produced differently-named ISOs with different labels, so nothing could
# be reproduced or compared. Prefer SOURCE_DATE_EPOCH, then the commit date, and
# only fall back to now for a build from a tarball with no git history.
if [ -n "${SOURCE_DATE_EPOCH:-}" ]; then
  STAMP="$SOURCE_DATE_EPOCH"
elif STAMP="$(git -C "$REPO" log -1 --format=%ct 2>/dev/null)" && [ -n "$STAMP" ]; then
  :
else
  STAMP="$(date -u +%s)"
  log "No git history and no SOURCE_DATE_EPOCH: this build is not reproducible."
fi
export SOURCE_DATE_EPOCH="$STAMP"
COMMIT="$(git -C "$REPO" rev-parse --short=8 HEAD 2>/dev/null || echo unknown)"
CAMEO_ISO_VERSION="$(date -u -d "@$STAMP" +%Y.%m.%d).g$COMMIT"
CAMEO_ISO_LABEL="CAMEO_$(date -u -d "@$STAMP" +%Y%m)"
# The edition rides in the image name so both artifacts can sit in one release
# without colliding: `cameo-<ver>` (universal, ships ROCm) and `cameo-lite-<ver>`.
CAMEO_ISO_NAME="cameo"
[ "$EDITION" = "lite" ] && CAMEO_ISO_NAME="cameo-lite"
export CAMEO_ISO_VERSION CAMEO_ISO_LABEL CAMEO_ISO_NAME
log "Image identity: name $CAMEO_ISO_NAME, version $CAMEO_ISO_VERSION, label $CAMEO_ISO_LABEL"

BUILD="$WORK/profile"
log "Staging a build copy of the profile at $BUILD (edition: $EDITION)"
rm -rf "$BUILD"
mkdir -p "$WORK" "$OUT"
cp -r "$PROFILE" "$BUILD"
# Don't recursively copy our own work/out into the build copy.
rm -rf "$BUILD/work" "$BUILD/out"

# 1. Pull the baseline boot files the Cameo profile intentionally doesn't vendor.
[ -f "$BUILD/pacman.conf" ] || cp "$RELENG/pacman.conf" "$BUILD/"

# 1·pin. Reproducibility (F4): when CAMEO_ARCH_SNAPSHOT is a YYYY/MM/DD date, pin
# the *build* package source to the Arch Linux Archive snapshot for that day, so
# the same commit + snapshot produces the same packages. Arch is rolling, so this
# is the only way a build is truly reproducible. Unset (the default) keeps the
# live mirrors and the previous behaviour. The same snapshot value pins the
# container image (see containers/Containerfile), so ISO and container stay in
# lockstep.
if [ -n "${CAMEO_ARCH_SNAPSHOT:-}" ]; then
  case "$CAMEO_ARCH_SNAPSHOT" in
    [0-9][0-9][0-9][0-9]/[0-9][0-9]/[0-9][0-9]) : ;;
    *) die "CAMEO_ARCH_SNAPSHOT must be YYYY/MM/DD (got '$CAMEO_ARCH_SNAPSHOT')." ;;
  esac
  ARCHIVE="https://archive.archlinux.org/repos/${CAMEO_ARCH_SNAPSHOT}"
  # Single quotes are deliberate: $repo/$arch are pacman's own placeholders.
  # shellcheck disable=SC2016
  printf 'Server=%s/$repo/os/$arch\n' "$ARCHIVE" > "$BUILD/mirrorlist.pinned"
  # Point the build's pacman at the pinned mirrorlist instead of the rolling one.
  sed -i 's#^Include = /etc/pacman.d/mirrorlist#Include = '"$BUILD"'/mirrorlist.pinned#' \
    "$BUILD/pacman.conf"
  grep -qF "$BUILD/mirrorlist.pinned" "$BUILD/pacman.conf" \
    || die "failed to pin pacman.conf to $BUILD/mirrorlist.pinned (releng layout changed?)"
  log "Reproducible build: packages pinned to Arch archive ${CAMEO_ARCH_SNAPSHOT}"
fi
for d in efiboot syslinux grub; do
  if [ ! -e "$BUILD/$d" ]; then
    [ -d "$RELENG/$d" ] || die "baseline $d missing from $RELENG (install archiso)"
    cp -r "$RELENG/$d" "$BUILD/"
  fi
done
[ -e "$BUILD/efiboot" ] || [ -e "$BUILD/syslinux" ] \
  || die "no bootloader configs after staging (need efiboot or syslinux from releng)"

# 1a. Build airootfs as releng's live root with Cameo's files overlaid.
#
# Cherry-picking individual files from releng was the wrong model. Cameo's
# airootfs holds only branding and tuning, so everything a live system needs
# and Cameo doesn't happen to vendor -- the account database, mkinitcpio
# hooks, mirrorlist, network config -- was simply absent, and each omission
# surfaced only as a broken boot. Starting from releng's working root and
# letting Cameo win on conflict makes the live environment complete by
# construction rather than by enumeration.
#
# .wants is stripped from the base first: releng enables units for packages it
# ships and Cameo may not, which would leave systemd failing on units that do
# not exist. Cameo enables what it needs explicitly, below.
if [ -d "$RELENG/airootfs" ]; then
  MERGED="$WORK/airootfs.merged"
  rm -rf "$MERGED"
  cp -a "$RELENG/airootfs" "$MERGED"
  rm -rf "$MERGED"/etc/systemd/system/*.wants
  [ -d "$BUILD/airootfs" ] && cp -a "$BUILD/airootfs/." "$MERGED/"
  rm -rf "$BUILD/airootfs"
  mv "$MERGED" "$BUILD/airootfs"
  log "Merged airootfs: releng base, Cameo overlay"

  # Record the pinned snapshot into the image (F5): cameo-update on an installed
  # system reads /etc/cameo/snapshot to pin the same release, and cameo-install
  # copies /etc/cameo to the target. Rolling builds leave no file (rolling update).
  if [ -n "${CAMEO_ARCH_SNAPSHOT:-}" ]; then
    install -d "$BUILD/airootfs/etc/cameo"
    printf '%s\n' "$CAMEO_ARCH_SNAPSHOT" >"$BUILD/airootfs/etc/cameo/snapshot"
    log "Recorded snapshot ${CAMEO_ARCH_SNAPSHOT} to /etc/cameo/snapshot"
  fi
fi

# 1a½. Guarantee the console login path instead of inheriting it.
#
# "Locked out on boot" has two independent causes on an archiso-derived image,
# and both are cheaper to foreclose here than to trust what releng happened to
# ship and what the `.wants` strip above happened to leave intact:
#
#   * The boot target. If default.target resolves to graphical.target there is
#     no display manager on this image, so systemd reaches for a target it can
#     never complete and never foregrounds the autologin tty — a black screen
#     that looks exactly like a lockout. Pin it to multi-user.
#
#   * root's password field. `agetty --autologin` logs in via `login -f`, which
#     bypasses authentication, so autologin itself works regardless. But the
#     moment autologin does NOT fire — a serial console, a non-tty1 VT, any
#     future regression — the fallback is a manual `login:` prompt, and there a
#     locked or password-set root is a lockout with no recovery on a read-only
#     live image. Force root passwordless, which is the archiso norm for an
#     ephemeral live root and what makes the manual fallback always work.
ROOTFS="$BUILD/airootfs"
mkdir -p "$ROOTFS/etc/systemd/system"
ln -sf /usr/lib/systemd/system/multi-user.target "$ROOTFS/etc/systemd/system/default.target"
log "Pinned default.target to multi-user (text console + tty1 autologin)"

if [ -f "$ROOTFS/etc/shadow" ]; then
  # Empty only root's password field (the 2nd colon-separated field), leaving
  # every other account and root's own password-aging fields untouched.
  awk -F: 'BEGIN { OFS = ":" } $1 == "root" { $2 = "" } 1' \
    "$ROOTFS/etc/shadow" >"$ROOTFS/etc/shadow.new"
  mv "$ROOTFS/etc/shadow.new" "$ROOTFS/etc/shadow"
  log "Ensured root is passwordless (the console login can never lock out)"
else
  log "WARNING: no /etc/shadow in the merged airootfs — root state is releng's"
fi

# 1b. Drop the automation channel the merge inherits along with releng's root home.
#
# releng's /root/.zlogin runs /root/.automated_script.sh, which reads a `script=`
# kernel command-line parameter and fetches-and-executes whatever it points at,
# over the network, as root, at every boot. That is a deliberate feature for
# Arch's install medium and a remote-code-execution channel for a machine whose
# job is to sit there serving models. Cameo has its own first-boot unit and no
# unattended-install story, so nothing here wants it.
#
# Installation_guide goes with it: Cameo's installer is cameo-install, and a
# command that opens the Arch installation guide is a promise the image cannot
# keep.
for f in root/.zlogin root/.automated_script.sh usr/local/bin/Installation_guide; do
  if [ -e "$BUILD/airootfs/$f" ]; then
    rm -f "$BUILD/airootfs/$f"
    log "Removed inherited $f"
  fi
done

# 1c. Enable the units Cameo wants. systemd only honours [Install] at
# `systemctl enable` time, which never happens for a live image, so archiso
# profiles ship the .wants symlinks instead. Created here rather than committed
# because the tree is edited on Windows, where git symlinks are a trap.
WANTS="$BUILD/airootfs/etc/systemd/system/multi-user.target.wants"
mkdir -p "$WANTS"
ln -sf /etc/systemd/system/cameo-firstboot.service "$WANTS/cameo-firstboot.service"
# The control-plane console and its bootstrap. The binary is installed above and
# both units ship in the Cameo airootfs overlay, so these links cannot dangle.
# console-init generates a key + LAN bind each boot (ordered Before cameod); the
# operator can override either in /etc/cameo/cameod.env.
ln -sf /etc/systemd/system/cameo-console-init.service "$WANTS/cameo-console-init.service"
# Mounts a prepared CAMEO_DATA disk at the model-cache path before cameod starts,
# so persistence set up via cameo-persist-cache survives reboots (F2). A no-op
# when no such disk exists, so it is always safe to enable.
ln -sf /etc/systemd/system/cameo-storage-init.service "$WANTS/cameo-storage-init.service"
# Publish the baked-in starter GGUF into /var/lib/cameo/models before cameod.
ln -sf /etc/systemd/system/cameo-seed-models.service "$WANTS/cameo-seed-models.service"
# The guided disk installer. Gated by ConditionKernelCommandLine=cameo.install, so
# it activates only for the "Install Cameo to disk" boot entry and is inert on a
# normal live boot. Enabling it here is therefore always safe.
ln -sf /etc/systemd/system/cameo-installer.service "$WANTS/cameo-installer.service"
ln -sf /etc/systemd/system/cameod.service "$WANTS/cameod.service"
# networkd and resolved ship inside systemd itself, so these can never dangle.
ln -sf /usr/lib/systemd/system/systemd-networkd.service "$WANTS/systemd-networkd.service"
ln -sf /usr/lib/systemd/system/systemd-resolved.service "$WANTS/systemd-resolved.service"
# Initialises /etc/pacman.d/gnupg. Without it every pacman install on the live
# image fails on signature trust, which also kills the only workaround for a
# missing package.
ln -sf /etc/systemd/system/pacman-init.service "$WANTS/pacman-init.service"
# Wireless association. Package is in packages.x86_64, so this cannot dangle.
ln -sf /usr/lib/systemd/system/iwd.service "$WANTS/iwd.service"
log "Enabled cameo-firstboot, cameo-console-init, cameo-storage-init, cameo-seed-models, cameo-installer, cameod, networkd, resolved, pacman-init, iwd"

# Clock. A laptop with a flat CMOS battery boots with a wrong time, which
# breaks TLS on model downloads and pacman signature validity before anything
# else has a chance to fail.
SYSINIT="$BUILD/airootfs/etc/systemd/system/sysinit.target.wants"
mkdir -p "$SYSINIT"
ln -sf /usr/lib/systemd/system/systemd-timesyncd.service "$SYSINIT/systemd-timesyncd.service"
ln -sf /usr/lib/systemd/system/systemd-time-wait-sync.service "$SYSINIT/systemd-time-wait-sync.service"
log "Enabled time synchronisation"

# 1d. Drop boot-menu entries whose payload the image does not ship.
#
# releng offers Memtest86+, the hardware-detection tool and a speech-synthesis
# boot path. Their packages are not in Cameo's set, so each of those entries is
# a menu item that selects a kernel or module that is not on the medium and
# fails to load. Removing the entries is honest; adding the packages back is a
# size decision, not a correctness one.
log "Removing boot-menu entries for payloads the image does not ship"
dropped=0
for e in "$BUILD"/efiboot/loader/entries/*.conf; do
  [ -e "$e" ] || continue
  if grep -qiE 'speech|accessibility=on|memtest' "$e"; then
    rm -f "$e"
    dropped=$((dropped + 1))
  fi
done
for f in "$BUILD"/syslinux/*.cfg; do
  [ -e "$f" ] || continue
  # Drop a whole LABEL block if it is a memtest, hdt, or accessibility/speech
  # entry — matched anywhere in the block, not just by the LABEL's name. The old
  # exact `$2=="memtest"` missed the accessibility entry entirely: its label is
  # `arch64_accessibility`, its APPEND carries `accessibility=on` and its MENU
  # LABEL says "with speech". The verify step greps for exactly those strings, so
  # the strip must be at least as broad or it leaves a menu item pointing at a
  # speech-synthesis payload the image does not ship.
  awk '
    BEGIN { IGNORECASE = 1 }
    function flush() { if (block != "" && !bad) printf "%s", block; block = ""; bad = 0 }
    /^LABEL /                                          { flush() }
    /memtest|with speech|accessibility=on|^LABEL hdt/  { bad = 1 }
    { block = block $0 ORS }
    END { flush() }
  ' "$f" >"$f.new"
  mv "$f.new" "$f"
done
# grub has TWO config files (grub.cfg and loopback.cfg) and two droppable
# constructs, so both files get the same pass:
#   * Memtest86+ is wrapped in an `if ... -f '/boot/memtest86+/...' ; then
#     menuentry {...} fi` guard. Dropping only the inner menuentry leaves the
#     `if`/`fi` lines, and the guard path itself carries the string "memtest"
#     that the verify step greps for -- so the whole if...fi block is dropped.
#   * The accessibility entry is a plain menuentry, but releng titles it
#     "(x86_64, UEFI, Accessibility)" -- not "speech" -- and only its APPEND
#     carries `accessibility=on`. Matching the title on "speech" alone missed
#     it, so each menuentry block is buffered and dropped if any line in it
#     mentions accessibility, speech, or memtest.
for g in "$BUILD"/grub/grub.cfg "$BUILD"/grub/loopback.cfg; do
  [ -f "$g" ] || continue
  awk '
    BEGIN { IGNORECASE = 1 }
    /^[[:space:]]*if[[:space:]].*memtest/ { ifdrop = 1; next }
    ifdrop { if ($0 ~ /^[[:space:]]*fi[[:space:]]*$/) ifdrop = 0; next }
    /^[[:space:]]*menuentry/ { inme = 1; depth = 0; opened = 0; block = ""; bad = 0 }
    inme {
      block = block $0 ORS
      if ($0 ~ /accessibility|speech|memtest/) bad = 1
      n = gsub(/\{/, "{"); depth += n; if (n) opened = 1
      depth -= gsub(/\}/, "}")
      if (opened && depth <= 0) { if (!bad) printf "%s", block; inme = 0 }
      next
    }
    { print }
  ' "$g" >"$g.new"
  mv "$g.new" "$g"
done
log "Dropped $dropped systemd-boot entr(ies), plus syslinux/grub equivalents"

# 1e. Brand the boot menus. Those borrowed configs label every entry "Arch
# Linux install medium", so the boot screen would announce the wrong distro --
# and, worse, offer to install an operating system that has no installer.
# Retitle them in the staged copy — done with sed against whatever releng
# ships rather than by vendoring copies that would rot against upstream.
# Only human-readable titles change; kernel paths, archisobasedir and
# archisolabel are lowercase and untouched.
#
# Order matters. A single blanket "Arch Linux" -> "Cameo Linux" rule turns
# "Boot the Arch Linux install medium on BIOS" into "Boot the Cameo Linux on
# BIOS", so the longer, grammatical phrases are rewritten first and the blanket
# rule only mops up what is left.
log "Branding the boot menus"
branded=0
while IFS= read -r -d '' f; do
  sed -i \
    -e 's/Boot the Arch Linux install medium/Boot Cameo Linux live/g' \
    -e 's/Arch Linux install medium (x86_64, \([^)]*\))/Cameo Linux live (\1)/g' \
    -e 's/Arch Linux install medium/Cameo Linux live/g' \
    -e 's/Arch Linux/Cameo Linux/g' \
    -e 's/allows you to install Cameo Linux or perform system maintenance/runs and serves language models on the hardware it detects/g' \
    -e 's/[Ii]nstall Cameo Linux/run Cameo Linux/g' \
    "$f"
  branded=$((branded + 1))
done < <(find "$BUILD/syslinux" "$BUILD/efiboot" "$BUILD/grub" \
           -type f \( -name '*.cfg' -o -name '*.conf' \) -print0 2>/dev/null)
log "Rebranded $branded boot config file(s)"

# 1e-bis. Add an "Install Cameo to disk" boot entry to every bootloader AND make
# it the DEFAULT selection.
#
# Cameo is an installer image first and a live "try it" image second — the same
# shape every real OS install medium has (Ubuntu, Fedora): boot the ISO and it
# offers to install, with the live environment one keystroke away. Until now the
# default was the live RAM session and "install" was a buried last entry, which
# is exactly why the ISO felt like "just a live USB".
#
# The install entry boots the *same* kernel + initramfs as the live entry with
# `cameo.install` added to the kernel command line; cameo-installer.service keys
# off that word (ConditionKernelCommandLine) and launches the guided installer on
# tty1. A normal live boot lacks the word and is unaffected. Each format is built
# by CLONING the working live entry, so this is purely additive — it never edits
# the entry known to boot, and a format that is absent is simply skipped.
log "Adding the 'Install Cameo to disk' boot entry and making it the default"
installer_entries=0

# systemd-boot (primary UEFI path): clone the first entry that has an `options`
# line into 00-cameo-install.conf (sort-key sorts it to the TOP), retitle it, add
# the marker, then point loader.conf's `default` at it.
for e in "$BUILD"/efiboot/loader/entries/*.conf; do
  [ -e "$e" ] || continue
  grep -q '^options ' "$e" || continue
  new="$BUILD/efiboot/loader/entries/00-cameo-install.conf"
  [ -e "$new" ] && break
  sed -e 's/^title .*/title Install Cameo to disk/' \
      -e 's/^sort-key .*/sort-key 00-install/' \
      -e 's/^\(options .*\)$/\1 cameo.install/' \
      "$e" >"$new"
  grep -q '^sort-key ' "$new" || printf 'sort-key 00-install\n' >>"$new"
  installer_entries=$((installer_entries + 1))
  break
done
LOADER="$BUILD/efiboot/loader/loader.conf"
if [ -f "$LOADER" ]; then
  if grep -q '^default' "$LOADER"; then
    sed -i 's/^default.*/default 00-cameo-install*/' "$LOADER"
  else
    printf 'default 00-cameo-install*\n' >>"$LOADER"
  fi
  log "systemd-boot default → Install Cameo to disk"
fi

# syslinux (BIOS path): append a LABEL built from the working entry's own
# LINUX/INITRD/APPEND lines with the marker, then append DEFAULT/ONTIMEOUT (plus
# MENU DEFAULT) pointing at it. The directives are appended AFTER the label in the
# same included config, so the later value wins over releng's earlier default —
# self-contained and valid without touching the loadconfig chain in syslinux.cfg.
for f in "$BUILD"/syslinux/*.cfg; do
  [ -e "$f" ] || continue
  grep -qE '^[[:space:]]*APPEND ' "$f" || continue
  grep -q '^LABEL cameo_install$' "$f" && break
  linux_line="$(grep -m1 -E '^[[:space:]]*(LINUX|KERNEL) ' "$f" || true)"
  initrd_line="$(grep -m1 -E '^[[:space:]]*INITRD ' "$f" || true)"
  append_line="$(grep -m1 -E '^[[:space:]]*APPEND ' "$f" || true)"
  sysappend_line="$(grep -m1 -E '^[[:space:]]*SYSAPPEND ' "$f" || true)"
  [ -n "$append_line" ] || continue
  {
    printf '\n'
    printf 'LABEL cameo_install\n'
    printf 'MENU LABEL Install Cameo to disk\n'
    printf 'MENU DEFAULT\n'
    [ -n "$linux_line" ] && printf '%s\n' "$linux_line"
    [ -n "$initrd_line" ] && printf '%s\n' "$initrd_line"
    printf '%s cameo.install\n' "$append_line"
    [ -n "$sysappend_line" ] && printf '%s\n' "$sysappend_line"
    printf '\n'
    printf 'DEFAULT cameo_install\n'
    printf 'ONTIMEOUT cameo_install\n'
  } >>"$f"
  installer_entries=$((installer_entries + 1))
  log "syslinux default → Install Cameo to disk"
  break
done

# GRUB (secondary UEFI/loopback path): append a menuentry built from the working
# entry's own `linux`/`initrd` lines, then `set default` to it. GRUB reads the
# whole config before drawing the menu, so a trailing `set default` wins.
for g in "$BUILD"/grub/grub.cfg "$BUILD"/grub/loopback.cfg; do
  [ -f "$g" ] || continue
  grep -q 'cameo.install' "$g" && continue
  linux_line="$(grep -m1 -E '^[[:space:]]*linux ' "$g" || true)"
  initrd_line="$(grep -m1 -E '^[[:space:]]*initrd ' "$g" || true)"
  [ -n "$linux_line" ] || continue
  {
    printf '\n'
    printf 'menuentry "Install Cameo to disk" {\n'
    printf '    set gfxpayload=keep\n'
    printf '%s cameo.install\n' "$linux_line"
    [ -n "$initrd_line" ] && printf '%s\n' "$initrd_line"
    printf '}\n'
    printf 'set default="Install Cameo to disk"\n'
  } >>"$g"
  installer_entries=$((installer_entries + 1))
done

log "Added $installer_entries install boot entr(ies), set as default (systemd-boot/syslinux/grub)"

# 1f. Render the Cameo mark into the syslinux menu background. releng's
# archiso_head.cfg points MENU BACKGROUND at splash.png, so overwriting that
# file is all the wiring needed. A missing rasteriser is not fatal: the stock
# splash still boots, and a cosmetic asset must never fail a build.
SPLASH_SVG="$REPO/docs/brand/cameo-splash.svg"
if [ -f "$SPLASH_SVG" ] && [ -d "$BUILD/syslinux" ]; then
  if command -v rsvg-convert >/dev/null 2>&1; then
    rsvg-convert -w 640 -h 480 -o "$BUILD/syslinux/splash.png" "$SPLASH_SVG"
    log "Rendered the Cameo boot splash (rsvg-convert)"
  elif command -v magick >/dev/null 2>&1; then
    magick -background none "$SPLASH_SVG" -resize 640x480! "$BUILD/syslinux/splash.png"
    log "Rendered the Cameo boot splash (ImageMagick)"
  else
    log "No SVG rasteriser found - keeping the stock splash"
  fi
fi

# 1g. Theme the syslinux menu chrome so the boot screen reads as Cameo, not the
# stock Arch blue-on-grey. Appended after releng's own MENU COLOR block in the
# file that drives the menu (UI vesamenu), so the later values win. Colours are
# syslinux #AARRGGBB — ember selection, warm off-white text, graphite ground —
# the same palette as the splash rendered above.
for f in "$BUILD"/syslinux/*.cfg; do
  [ -e "$f" ] || continue
  grep -qE '^[[:space:]]*(UI[[:space:]]+vesamenu|MENU[[:space:]]+TITLE)' "$f" || continue
  {
    printf '\n'
    printf 'MENU TITLE Cameo Linux\n'
    printf 'MENU COLOR title       1;38;5;209 #ffff7a1a #00000000 std\n'
    printf 'MENU COLOR sel         7;37;40    #ff171512 #ffff7a1a all\n'
    printf 'MENU COLOR unsel       0          #ffe8e1d5 #00000000 std\n'
    printf 'MENU COLOR hotkey      1;38;5;209 #ffffd08a #00000000 std\n'
    printf 'MENU COLOR hotsel      7;37;40    #ff171512 #ffff7a1a all\n'
    printf 'MENU COLOR border      0          #22e8e1d5 #00000000 std\n'
    printf 'MENU COLOR timeout     0          #ff8a8090 #00000000 std\n'
    printf 'MENU COLOR timeout_msg 0          #ff8a8090 #00000000 std\n'
    printf 'MENU COLOR help        0          #ff9a958c #00000000 std\n'
  } >>"$f"
  log "Themed the syslinux boot menu (Cameo palette)"
  break
done

# 2. Build the cameo CLI (native, for the ISO's arch) and stage it into the image.
#
# Not as root. `cargo build` runs every dependency's build script and every
# proc-macro in the tree as the building user, so building the CLI here as root
# executes ~35 crates' worth of arbitrary compile-time code with full privileges
# — and leaves a root-owned target/ in the user's checkout afterwards. Under
# sudo we drop back to the invoking user; a genuine root login cannot, so it
# says so and at least keeps its artefacts out of the source tree.
# Both front ends ship: `cameo` (the CLI) and `cameod` (the control-plane daemon
# that serves the browser console). One `cargo build` produces both.
CARGO_TARGET="${CAMEO_CARGO_TARGET_DIR:-$WORK/cargo-target}"
if [ "${CAMEO_SKIP_CARGO:-}" = "1" ]; then
  log "CAMEO_SKIP_CARGO=1 — using prebuilt bins in $CARGO_TARGET/release"
else
  log "Building the cameo CLI + cameod daemon (release)..."
  mkdir -p "$CARGO_TARGET"
  if [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ] && command -v runuser >/dev/null 2>&1; then
    log "Dropping privileges to $SUDO_USER for the build"
    chown -R "$SUDO_USER" "$CARGO_TARGET"
    runuser -u "$SUDO_USER" -- env CARGO_TARGET_DIR="$CARGO_TARGET" \
      cargo build --release -p cameo-cli -p cameo-daemon --manifest-path "$REPO/Cargo.toml"
  else
    log "No SUDO_USER — compiling as root. Build scripts and proc-macros will run"
    log "with full privileges; prefer 'sudo $0' from a normal account."
    CARGO_TARGET_DIR="$CARGO_TARGET" \
      cargo build --release -p cameo-cli -p cameo-daemon --manifest-path "$REPO/Cargo.toml"
  fi
fi
if [ ! -x "$CARGO_TARGET/release/cameo" ] || [ ! -x "$CARGO_TARGET/release/cameod" ]; then
  die "missing $CARGO_TARGET/release/cameo or cameod (build them, or unset CAMEO_SKIP_CARGO)"
fi
install -Dm755 "$CARGO_TARGET/release/cameo" "$BUILD/airootfs/usr/local/bin/cameo"
install -Dm755 "$CARGO_TARGET/release/cameod" "$BUILD/airootfs/usr/local/bin/cameod"

# 3. Lite edition: drop the heavy ROCm / PyTorch packages (Vulkan-only, much smaller).
#
# Compilers stay off both editions (packages.x86_64). The image is an appliance:
# llama.cpp is the packaged binary, not something built on the box.
if [ "$EDITION" = "lite" ]; then
  # ggml-hip matches neither ^rocm nor ^python-pytorch, so it needs its own
  # alternative or the "Vulkan-only" edition pulls the whole ROCm stack in as
  # a dependency of the compute backend.
  grep -viE '^(rocm|hip-|python-pytorch|ggml-hip)' "$PROFILE/packages.x86_64" > "$BUILD/packages.x86_64"
  log "Lite edition: ROCm/PyTorch excluded — Vulkan baseline only."
fi

# 3b. Starter model: bake a small GGUF so first boot can chat with no internet.
#
# ~380 MiB, not in git. Cached under $WORK/models across rebuilds. Skip with
# CAMEO_SKIP_STARTER=1 (CI / air-gapped rebuild of an already-cached tree).
if [ "${CAMEO_SKIP_STARTER:-}" = "1" ]; then
  log "CAMEO_SKIP_STARTER=1 — ISO will not include a starter GGUF."
else
  log "Fetching starter model (qwen2.5-0.5b, ~380 MiB, cached after the first build)…"
  chmod +x "$REPO/scripts/fetch-starter-model.sh"
  "$REPO/scripts/fetch-starter-model.sh" \
    --cache "$WORK/models" \
    --dest "$BUILD/airootfs/usr/share/cameo/models"
fi

# 4. Build the ISO.
export CAMEO_ZSTD_LEVEL="${CAMEO_ZSTD_LEVEL:-19}"
log "Running mkarchiso (this takes a while and downloads packages; zstd level $CAMEO_ZSTD_LEVEL)..."
MKARCHISO_ARGS=(-v -w "$WORK/tmp" -o "$OUT")
if [ -n "${CAMEO_PACMAN_CACHE:-}" ]; then
  mkdir -p "$CAMEO_PACMAN_CACHE"
  MKARCHISO_ARGS+=(-c "$CAMEO_PACMAN_CACHE")
  log "pacman cache: $CAMEO_PACMAN_CACHE"
fi
mkarchiso "${MKARCHISO_ARGS[@]}" "$BUILD"

log "Done. ISO(s) in: $OUT"
ls -lh "$OUT" || true
log "Write it to USB with:  sudo dd if=$OUT/cameo-*.iso of=/dev/sdX bs=4M status=progress oflag=sync"
