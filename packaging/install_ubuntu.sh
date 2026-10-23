#!/usr/bin/env bash
# Установщик text_workspace для Ubuntu 24.04 и новее (amd64).
#   ./packaging/install_ubuntu.sh           пакет .deb и установка текущему пользователю
#   ./packaging/install_ubuntu.sh --deb     только пакет в packaging/out/
#   ./packaging/install_ubuntu.sh --system  пакет и установка в систему через apt
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
project=$(cd "$here/.." && pwd)
cd "$project"

mode=user
case "${1:-}" in
  "") mode=user ;;
  --deb) mode=deb ;;
  --system) mode=system ;;
  *)
    echo "Использование: $0 [--deb | --system]" >&2
    exit 2
    ;;
esac

if [[ ! -r /etc/os-release ]]; then
  echo "Не найден /etc/os-release. Установщик рассчитан на Ubuntu 24.04 и новее." >&2
  exit 1
fi
# shellcheck disable=SC1091
. /etc/os-release
if [[ "${ID:-}" != "ubuntu" ]]; then
  echo "Сейчас система «${ID:-unknown}». Этот установщик собирает пакет для Ubuntu." >&2
  exit 1
fi
if ! printf '%s\n%s\n' "24.04" "${VERSION_ID:-0}" | sort -C -V; then
  echo "Нужна Ubuntu 24.04 или новее (GTK 4.14, GtkSourceView 5, WebKitGTK 6). Сейчас ${VERSION_ID:-unknown}." >&2
  exit 1
fi

if [[ -f "$project/env.sh" ]]; then
  # shellcheck disable=SC1091
  source "$project/env.sh"
fi

if ! pkg-config --exists gtk4 gtksourceview-5 webkitgtk-6.0; then
  echo "Нет заголовков для сборки. На Ubuntu 24.04:" >&2
  echo "  sudo apt install pkg-config libgtk-4-dev libgtksourceview-5-dev libwebkitgtk-6.0-dev" >&2
  exit 1
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "Нужен Rust и cargo: https://rustup.rs" >&2
  exit 1
fi

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
arch=$(dpkg --print-architecture)
if [[ -z "$version" || -z "$arch" ]]; then
  echo "Не удалось прочитать версию или архитектуру." >&2
  exit 1
fi

echo "Сборка release ${version} для ${arch}..."
cargo build --release
binary="$project/target/release/text_workspace"
if [[ ! -x "$binary" ]]; then
  echo "Сборка не создала $binary" >&2
  exit 1
fi

stage=$(mktemp -d)
cleanup() { rm -rf "$stage"; }
trap cleanup EXIT

mkdir -p "$stage/usr/bin" "$stage/usr/share/applications" \
  "$stage/usr/share/icons/hicolor/256x256/apps" \
  "$stage/etc/apparmor.d" "$stage/DEBIAN"
chmod 755 "$stage"
install -m 0755 "$binary" "$stage/usr/bin/verst"
install -m 0644 "$here/local.textworkspace.desktop" "$stage/usr/share/applications/local.textworkspace.desktop"
install -m 0644 "$here/verst.png" "$stage/usr/share/icons/hicolor/256x256/apps/verst.png"
install -m 0644 "$here/apparmor.profile" "$stage/etc/apparmor.d/verst"
find "$stage" -type d -exec chmod 755 {} +

cat >"$stage/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if [ "$1" = "configure" ] && [ -x /usr/sbin/apparmor_parser ]; then
  /usr/sbin/apparmor_parser -r -W -T /etc/apparmor.d/verst || true
fi
exit 0
EOF
cat >"$stage/DEBIAN/prerm" <<'EOF'
#!/bin/sh
set -e
if [ "$1" = "remove" ] && [ -x /usr/sbin/apparmor_parser ] && [ -f /etc/apparmor.d/verst ]; then
  /usr/sbin/apparmor_parser -R /etc/apparmor.d/verst || true
fi
exit 0
EOF
chmod 0755 "$stage/DEBIAN/postinst" "$stage/DEBIAN/prerm"

# Явные пакеты нужны для запуска, а не только для загрузки .so.
# shlibdeps дополняет их тем, что реально слинковано в бинарник.
declare -a depends_items=(
  "libgtk-4-1 (>= 4.14)"
  "libgtksourceview-5-0 (>= 5.12)"
  "libwebkitgtk-6.0-4 (>= 2.44)"
  "bubblewrap"
  "xdg-dbus-proxy"
  "apparmor"
)
if command -v dpkg-shlibdeps >/dev/null 2>&1; then
  shlib_dir=$(mktemp -d)
  mkdir -p "$shlib_dir/debian"
  printf 'Source: verst\n' >"$shlib_dir/debian/control"
  probed=$(
    cd "$shlib_dir" &&
      dpkg-shlibdeps -O "$stage/usr/bin/verst" 2>/dev/null |
      sed -n 's/^shlibs:Depends=//p' || true
  )
  rm -rf "$shlib_dir"
  if [[ -n "$probed" ]]; then
    IFS=',' read -ra probed_items <<<"$probed"
    for item in "${probed_items[@]}"; do
      item="${item#"${item%%[![:space:]]*}"}"
      item="${item%"${item##*[![:space:]]}"}"
      name="${item%% *}"
      known=0
      for have in "${depends_items[@]}"; do
        if [[ "${have%% *}" == "$name" ]]; then
          known=1
          break
        fi
      done
      if [[ "$known" -eq 0 && -n "$name" ]]; then
        depends_items+=("$item")
      fi
    done
  fi
fi
depends=$(IFS=', '; echo "${depends_items[*]}")

cat >"$stage/DEBIAN/control" <<EOF
Package: verst
Version: ${version}
Section: editors
Priority: optional
Architecture: ${arch}
Maintainer: Verst <verst@localhost>
Depends: ${depends}
Replaces: textworkspace
Conflicts: textworkspace
Description: Verst — локальный редактор текстовых файлов
 Редактор Markdown и других текстовых файлов для Ubuntu.
 Навигация по диску, запиненные каталоги, поиск и связи внутри пина.
EOF

mkdir -p "$project/packaging/out"
deb="$project/packaging/out/verst_${version}_${arch}.deb"
dpkg-deb --root-owner-group -Zxz -b "$stage" "$deb"
echo "Пакет: $deb"

install_user() {
  mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications" \
    "$HOME/.local/share/icons/hicolor/256x256/apps"
  install -m 0755 "$binary" "$HOME/.local/bin/verst"
  install -m 0644 "$here/verst.png" "$HOME/.local/share/icons/hicolor/256x256/apps/verst.png"
  desktop="$HOME/.local/share/applications/local.textworkspace.desktop"
  # Профиль AppArmor закрывает только /usr/bin/verst. Локальный бинарник
  # без этого переменного окружения падает в bwrap: Permission denied.
  sed "s|^Exec=.*|Exec=env WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 $HOME/.local/bin/verst %U|" \
    "$here/local.textworkspace.desktop" >"$desktop"
  chmod 0644 "$desktop"
  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$HOME/.local/share/applications" || true
  fi
  if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f "$HOME/.local/share/icons/hicolor" || true
  fi
  echo "Установлено для пользователя: $HOME/.local/bin/verst"
  echo "В меню приложений оно называется Verst. Каталог ~/.local/bin должен быть в PATH."
}

case "$mode" in
  deb) ;;
  user) install_user ;;
  system)
    if ! command -v sudo >/dev/null 2>&1; then
      echo "Для системной установки нужен sudo. Пакет уже собран: $deb" >&2
      exit 1
    fi
    sudo apt install -y "$deb"
    ;;
esac
