#!/usr/bin/env bash
# source ./env.sh
# Подключает локальный префикс с заголовками GTK, если системный pkg-config их не видит.
root="${TEXT_WORKSPACE_SYSROOT:-$HOME/.cache/text_workspace_sysroot}"
pc="$root/usr/lib/x86_64-linux-gnu/pkgconfig"
if [ -f "$pc/gtk4.pc" ]; then
  case ":${PKG_CONFIG_PATH:-}:" in
    *":$pc:"*) ;;
    *) export PKG_CONFIG_PATH="$pc${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}" ;;
  esac
fi
