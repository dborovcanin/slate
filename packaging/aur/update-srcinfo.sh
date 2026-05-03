#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

for pkg_dir in "${root_dir}"/*; do
  [[ -d "${pkg_dir}" ]] || continue
  [[ -f "${pkg_dir}/PKGBUILD" ]] || continue
  (
    cd "${pkg_dir}"
    makepkg --printsrcinfo > .SRCINFO
  )
done
