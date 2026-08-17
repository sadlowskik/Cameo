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
bootmodes=(
  'bios.syslinux.mbr'
  'bios.syslinux.eltorito'
  'uefi-x64.systemd-boot.esp'
  'uefi-x64.systemd-boot.eltorito'
)
arch="x86_64"
pacman_conf="pacman.conf"
airootfs_image_type="squashfs"
airootfs_image_tool_options=('-comp' 'zstd' '-Xcompression-level' '15' '-b' '1M')
file_permissions=(
  ["/etc/shadow"]="0:0:400"
  ["/usr/local/bin/cameo-firstboot"]="0:0:755"
  ["/usr/local/bin/cameo"]="0:0:755"
)
