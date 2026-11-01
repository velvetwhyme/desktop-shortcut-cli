#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

pkgver=$(sed -n 's/^pkgver=//p' PKGBUILD | head -n 1)
pkgname=$(sed -n 's/^pkgname=//p' PKGBUILD | head -n 1)
archive="${pkgname}-${pkgver}.tar.gz"
root=$(cd .. && pwd)

tar \
  --sort=name \
  --mtime="@${SOURCE_DATE_EPOCH:-$(date +%s)}" \
  --owner=0 \
  --group=0 \
  --numeric-owner \
  --transform "s,^,${pkgname}-${pkgver}/," \
  -czf "$archive" \
  -C "$root" \
  Cargo.toml Cargo.lock src tests README.md LICENSE

checksum=$(sha256sum "$archive" | awk '{print $1}')
sed -i "s/^sha256sums=.*/sha256sums=('$checksum')/" PKGBUILD
makepkg --printsrcinfo > .SRCINFO

printf 'Обновлён %s\nsha256: %s\n' "$archive" "$checksum"
