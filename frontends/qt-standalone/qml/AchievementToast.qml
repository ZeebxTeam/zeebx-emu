import QtQuick
import QtQuick.Layouts

// The RetroAchievements notices over the game, one at a time. The `Image` downloads the icon from the URL.
Rectangle {
    id: toast

    property var queue: []

    function show(title, text, icon) {
        queue.push({ title: title, text: text, icon: icon })
        if (!visible)
            next()
    }

    function next() {
        const item = queue.shift()
        visible = item !== undefined
        if (!visible)
            return
        titleText.text = item.title
        noticeText.text = item.text
        icon.source = item.icon
        timer.restart()
    }

    visible: false
    width: row.implicitWidth + 24
    height: row.implicitHeight + 16
    radius: 6
    color: "#e6202020"
    border.color: "#505050"

    RowLayout {
        id: row

        anchors.centerIn: parent
        spacing: 10

        Image {
            id: icon

            Layout.preferredWidth: 48
            Layout.preferredHeight: 48
            visible: status === Image.Ready
            asynchronous: true
            fillMode: Image.PreserveAspectFit
        }

        ColumnLayout {
            spacing: 2

            Text {
                id: titleText

                color: "white"
                font.bold: true
            }
            Text {
                id: noticeText

                visible: text !== ""
                color: "#cccccc"
            }
        }
    }

    Timer {
        id: timer

        interval: 4000
        onTriggered: toast.next()
    }
}
