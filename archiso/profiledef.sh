#!/usr/bin/env bash
# Cameo ISO build profile (archiso).
#
# Build on a LINUX host (cannot build on Windows) with the `archiso` package:
#   mkarchiso -v -w work/ -o out/ archiso/
#
# Baseline boot files (pacman.conf, efiboot/, syslinux/, grub/) are taken from
# the stock `archiso` releng profile at /usr/share/archiso/configs/releng/ —
# copy those in, then Cameo overlays the packages and airootfs tuning here.
# shellcheck disable=SC2034

iso_name="cameo"
iso_label="CAMEO_$(date +%Y%m)"
iso_publisher="Cameo <https://github.com/korbin/cameo>"
iso_application="Cameo — LLMs on AMD GPUs"
iso_version="$(date +%Y.%m.%d)"
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
airootfs_image_tool_options=('-comp' 'zstd' '-Xcompression-level' '15' '-b' '1M')
# The airootfs is releng's with Cameo overlaid, so releng's own files land in
# the image and need their upstream modes declared here too -- otherwise they
# arrive with whatever mode the archiso package happened to install, and a
# world-readable /root/.gnupg makes pacman-key complain about unsafe
# permissions.
file_permissions=(
  ["/etc/shadow"]="0:0:400"
  ["/root"]="0:0:750"
  ["/root/.automated_script.sh"]="0:0:755"
  ["/root/.gnupg"]="0:0:700"
  ["/usr/local/bin/choose-mirror"]="0:0:755"
  ["/usr/local/bin/Installation_guide"]="0:0:755"
  ["/usr/local/bin/livecd-sound"]="0:0:755"
  ["/usr/local/bin/cameo-firstboot"]="0:0:755"
  ["/usr/local/bin/cameo"]="0:0:755"
)
