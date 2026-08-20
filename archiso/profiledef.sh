#!/usr/bin/env bash
# Cameo ISO build profile (archiso).
#
# Build on a LINUX host (cannot build on Windows) with the `archiso` package.
# Use scripts/build-iso.sh rather than calling mkarchiso directly: it stages the
# profile, merges releng's airootfs, and exports the identity variables below.
#
# Baseline boot files (pacman.conf, efiboot/, syslinux/, grub/) are taken from
# the stock `archiso` releng profile at /usr/share/archiso/configs/releng/ —
# copy those in, then Cameo overlays the packages and airootfs tuning here.
# shellcheck disable=SC2034

iso_name="${CAMEO_ISO_NAME:-cameo}"
# Identity comes from the build script, which derives it from the commit (or
# SOURCE_DATE_EPOCH) so the same source produces the same image name and volume
# label. The `date` fallbacks exist only for a direct mkarchiso invocation, and
# are the non-reproducible path.
iso_label="${CAMEO_ISO_LABEL:-CAMEO_$(date -u +%Y%m)}"
iso_publisher="Cameo <https://github.com/korbin/cameo>"
iso_application="Cameo — LLMs on AMD GPUs"
iso_version="${CAMEO_ISO_VERSION:-$(date -u +%Y.%m.%d)}"
install_dir="cameo"
buildmodes=('iso')
# The old four-name form ('bios.syslinux.mbr', 'uefi-x64.systemd-boot.esp',
# and their eltorito variants) is deprecated upstream; each pair collapses into
# a single mode. mkarchiso warns on the old names.
bootmodes=(
  'bios.syslinux'
  'uefi.systemd-boot'
)
arch="x86_64"
pacman_conf="pacman.conf"
airootfs_image_type="squashfs"
# Level 19 rather than 15: the image carries a full build toolchain and the ROCm
# stack by choice, so the compression setting is where size comes back without
# removing anything. Past 19 the extra minutes buy very little.
airootfs_image_tool_options=('-comp' 'zstd' '-Xcompression-level' '19' '-b' '1M')
# The airootfs is releng's with Cameo overlaid, so releng's own files land in
# the image and need their upstream modes declared here too -- otherwise they
# arrive with whatever mode the archiso package happened to install, and a
# world-readable /root/.gnupg makes pacman-key complain about unsafe
# permissions.
#
# Nothing here may name a path the build script deletes: mkarchiso fails on a
# file_permissions entry that does not exist. /root/.automated_script.sh and
# /usr/local/bin/Installation_guide are removed there (boot-time remote
# execution, and an installer Cameo does not have), so neither is listed.
file_permissions=(
  ["/etc/shadow"]="0:0:400"
  ["/root"]="0:0:750"
  ["/root/.gnupg"]="0:0:700"
  ["/usr/local/bin/choose-mirror"]="0:0:755"
  ["/usr/local/bin/livecd-sound"]="0:0:755"
  ["/usr/local/bin/cameo-firstboot"]="0:0:755"
  ["/usr/local/bin/cameo-console-init"]="0:0:755"
  ["/usr/local/bin/cameo-storage-init"]="0:0:755"
  ["/usr/local/bin/cameo-persist-cache"]="0:0:755"
  ["/usr/local/bin/cameo"]="0:0:755"
  ["/usr/local/bin/cameod"]="0:0:755"
  ["/usr/local/bin/cameo-install"]="0:0:755"
  ["/usr/local/bin/cameo-install-guided"]="0:0:755"
  ["/usr/local/bin/cameo-update"]="0:0:755"
)
