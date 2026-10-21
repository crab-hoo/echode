import Foundation

struct TrackFolderLayout {
    let disclosure: CGRect
    let titleX: Double
    let titleY: Double
    let titleWidth: Double

    init(row: CGRect, depth: Int, folder: Bool, titleEnd: Double? = nil) {
        // Indentation is clipped to the available header width, never to a
        // maximum nesting depth. Deep trees still expose their disclosure.
        let indent = min(Double(depth) * 12, max(0, row.width - 76))
        let y = row.minY + (row.height >= 52 ? 0 : max(0, (row.height - 24) / 2))
        disclosure = CGRect(x: 4 + indent, y: y, width: folder ? 20 : 0, height: min(26, row.height))
        titleX = row.height < 52 && !folder ? 30 + indent : 8 + indent + (folder ? 20 : 0)
        titleY = row.minY + (row.height >= 52 ? 4 : max(0, (row.height - 24) / 2))
        titleWidth = max(0, min(row.width - (row.height >= 52 ? 37 : 35), titleEnd ?? row.width) - titleX)
    }
}
