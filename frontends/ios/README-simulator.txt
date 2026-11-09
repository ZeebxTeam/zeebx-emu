Zeebx for the Simulator

The app is simulator/Zeebx.app. It only opens in the Mac's Simulator.
The game goes in Documents/roms, inside the app. A loose .mod works,
and so does a .zip or a .7z. Open Zeebx once before copying the game,
so the roms folder exists, and tap Refresh afterwards.

Drag Zeebx.app onto the Simulator window, or:

    xcrun simctl boot "iPhone 16"
    xcrun simctl install booted simulator/Zeebx.app
    xcrun simctl launch booted io.github.zeebxteam.zeebx

The game goes in through the app's folder on disk. Pasting through Finder
writes a zero-byte file; if you delete it, the sharing creates the empty
file again.

    ditto "Game.zip" "$(xcrun simctl get_app_container booted io.github.zeebxteam.zeebx data)/Documents/roms/"

With more than one simulator running, booted doesn't pick which one. In its
place, use the identifier that `xcrun simctl list devices` shows.
