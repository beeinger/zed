#!/usr/bin/env sh
set -eu

# FORK:branding — Peekado-only. Must never remove official Zed paths.

check_remaining_installations() {
    platform="$(uname -s)"
    if [ "$platform" = "Darwin" ]; then
        remaining=$(ls -d /Applications/Peekado*.app 2>/dev/null | wc -l)
        [ "$remaining" -eq 0 ]
    else
        remaining=$(ls -d "$HOME/.local/peekado"*.app 2>/dev/null | wc -l)
        [ "$remaining" -eq 0 ]
    fi
}

prompt_remove_preferences() {
    printf "Do you want to keep your Peekado preferences? [Y/n] "
    read -r response
    case "$response" in
        [nN]|[nN][oO])
            rm -rf "$HOME/.config/peekado"
            echo "Preferences removed."
            ;;
        *)
            echo "Preferences kept."
            ;;
    esac
}

main() {
    platform="$(uname -s)"
    channel="${ZED_CHANNEL:-stable}"

    if [ "$platform" = "Darwin" ]; then
        platform="macos"
    elif [ "$platform" = "Linux" ]; then
        platform="linux"
    else
        echo "Unsupported platform $platform"
        exit 1
    fi

    "$platform"

    echo "Peekado has been uninstalled"
}

linux() {
    suffix=""
    if [ "$channel" != "stable" ]; then
        suffix="-$channel"
    fi

    appid=""
    db_suffix="stable"
    case "$channel" in
      stable)
        appid="dev.peekado.Peekado"
        db_suffix="stable"
        ;;
      nightly)
        appid="dev.peekado.Peekado-Nightly"
        db_suffix="nightly"
        ;;
      preview)
        appid="dev.peekado.Peekado-Preview"
        db_suffix="preview"
        ;;
      dev)
        appid="dev.peekado.Peekado-Dev"
        db_suffix="dev"
        ;;
      *)
        echo "Unknown release channel: ${channel}. Using stable app ID."
        appid="dev.peekado.Peekado"
        db_suffix="stable"
        ;;
    esac

    rm -rf "$HOME/.local/peekado$suffix.app"
    rm -f "$HOME/.local/bin/peekado"
    rm -f "$HOME/.local/share/applications/${appid}.desktop"
    rm -rf "$HOME/.local/share/peekado/db/0-$db_suffix"
    rm -f "$HOME/.local/share/peekado/peekado-$db_suffix.sock"

    if check_remaining_installations; then
        rm -rf "$HOME/.local/share/peekado"
        prompt_remove_preferences
    fi

    rm -rf "$HOME/.peekado_server"
}

macos() {
    app="Peekado.app"
    db_suffix="stable"
    app_id="dev.peekado.Peekado"
    case "$channel" in
      nightly)
        app="Peekado Nightly.app"
        db_suffix="nightly"
        app_id="dev.peekado.Peekado-Nightly"
        ;;
      preview)
        app="Peekado Preview.app"
        db_suffix="preview"
        app_id="dev.peekado.Peekado-Preview"
        ;;
      dev)
        app="Peekado Dev.app"
        db_suffix="dev"
        app_id="dev.peekado.Peekado-Dev"
        ;;
    esac

    if [ -d "/Applications/$app" ]; then
        rm -rf "/Applications/$app"
    fi

    rm -f "$HOME/.local/bin/peekado"
    rm -f /usr/local/bin/peekado
    rm -rf "$HOME/Library/Application Support/Peekado/db/0-$db_suffix"
    rm -rf "$HOME/Library/Application Support/com.apple.sharedfilelist/com.apple.LSSharedFileList.ApplicationRecentDocuments/$app_id.sfl"*
    rm -rf "$HOME/Library/Caches/$app_id"
    rm -rf "$HOME/Library/HTTPStorages/$app_id"
    rm -rf "$HOME/Library/Preferences/$app_id.plist"
    rm -rf "$HOME/Library/Saved Application State/$app_id.savedState"

    if check_remaining_installations; then
        rm -rf "$HOME/Library/Application Support/Peekado"
        rm -rf "$HOME/Library/Logs/Peekado"
        prompt_remove_preferences
    fi

    rm -rf "$HOME/.peekado_server"
}

main "$@"
