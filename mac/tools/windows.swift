// Lists one process's windows, to see whether its menu is open (menus sit on layer 101) or a window
// was left behind. Usage: windows PID [SECONDS]
// With no SECONDS it prints every window once, on screen or not. With SECONDS it prints the on-screen
// ones every half second for that long, each line led by the time since the start.
import CoreGraphics
import Foundation

let arguments = CommandLine.arguments
guard arguments.count > 1, let pid = Int(arguments[1]) else {
    print("usage: windows PID [SECONDS]")
    exit(1)
}

/// Describes each window of the process, on screen only or all of them.
func describe(onScreenOnly: Bool) -> [String] {
    let option: CGWindowListOption = onScreenOnly ? .optionOnScreenOnly : .optionAll
    let list = CGWindowListCopyWindowInfo(option, kCGNullWindowID) as? [[String: Any]] ?? []
    return list.filter { ($0[kCGWindowOwnerPID as String] as? Int) == pid }.map { window in
        let bounds = window[kCGWindowBounds as String] as? [String: Any] ?? [:]
        let layer = window[kCGWindowLayer as String] ?? "?"
        let isOnScreen = window[kCGWindowIsOnscreen as String] as? Bool ?? false
        let place = "\(bounds["X"] ?? "?"),\(bounds["Y"] ?? "?") \(bounds["Width"] ?? "?")x\(bounds["Height"] ?? "?")"
        return "id \(window[kCGWindowNumber as String] ?? "?") layer \(layer) on screen \(isOnScreen) \(place)"
    }
}

if arguments.count > 2, let seconds = Double(arguments[2]) {
    let start = Date()
    while Date().timeIntervalSince(start) < seconds {
        print(String(format: "%5.1f", Date().timeIntervalSince(start)), describe(onScreenOnly: true).joined(separator: " | "))
        fflush(stdout)
        Thread.sleep(forTimeInterval: 0.5)
    }
} else {
    describe(onScreenOnly: false).forEach { print($0) }
}
