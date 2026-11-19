macOS may refuse Zeebx the first time it opens, with the warning
"Zeebx Qt.app is damaged and can't be opened. You should move it to the Trash."

The app is not damaged. The system flags whatever came from the internet
and, since this build is not signed by Apple yet, Gatekeeper uses that sentence.

Drag Zeebx to the Applications folder and run in Terminal:

    xattr -dr com.apple.quarantine "/Applications/Zeebx Qt.app"

Then:

    open "/Applications/Zeebx Qt.app"
