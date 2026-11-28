// Sends test input to the real screen, for checks that need a real click on a menu bar icon.
// Usage: input pos | move X Y | click X Y | rclick X Y | esc
// Coordinates are screen points from the top left of the main display. Show the hands-off pill first,
// put the pointer back after (read it with `pos` before), and never take a screenshot between a click
// and an `esc` that should close a menu: the capture takes the key focus and the Esc never reaches it.
import CoreGraphics
import Foundation

let arguments = CommandLine.arguments

/// Posts one mouse event at a point.
func post(_ type: CGEventType, _ point: CGPoint, _ button: CGMouseButton = .left) {
    CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: button)!.post(tap: .cghidEventTap)
}

/// Reads the X and Y arguments.
func point() -> CGPoint {
    CGPoint(x: Double(arguments[2])!, y: Double(arguments[3])!)
}

switch arguments.count > 1 ? arguments[1] : "" {
case "pos":
    let location = CGEvent(source: nil)!.location
    print("\(Int(location.x)) \(Int(location.y))")
case "move":
    post(.mouseMoved, point())
case "click":
    post(.mouseMoved, point()); usleep(80_000)
    post(.leftMouseDown, point()); usleep(70_000)
    post(.leftMouseUp, point())
case "rclick":
    post(.mouseMoved, point()); usleep(80_000)
    post(.rightMouseDown, point(), .right); usleep(70_000)
    post(.rightMouseUp, point(), .right)
case "esc":
    for isDown in [true, false] {
        CGEvent(keyboardEventSource: nil, virtualKey: 53, keyDown: isDown)!.post(tap: .cghidEventTap)
        usleep(40_000)
    }
default:
    print("usage: input pos | move X Y | click X Y | rclick X Y | esc")
    exit(1)
}
