import ApplicationServices
import CoreGraphics
import Foundation

let ownerName = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "DiskDeck"

guard CGPreflightPostEventAccess() else {
    fputs("Accessibility is required to focus the signed window. Enable the terminal or Codex in System Settings → Privacy & Security → Accessibility.\n", stderr)
    exit(77)
}

func appWindow() -> CGRect? {
    guard let raw = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID)
        as? [[String: Any]] else {
        return nil
    }
    for info in raw {
        guard info[kCGWindowOwnerName as String] as? String == ownerName,
              info[kCGWindowName as String] as? String == ownerName,
              info[kCGWindowLayer as String] as? Int == 0,
              let boundsValue = info[kCGWindowBounds as String] else {
            continue
        }
        let bounds = CGRect(dictionaryRepresentation: boundsValue as! CFDictionary)
        if let bounds, bounds.width >= 200, bounds.height >= 100 {
            return bounds
        }
    }
    return nil
}

var bounds: CGRect?
for _ in 0..<50 {
    bounds = appWindow()
    if bounds != nil { break }
    Thread.sleep(forTimeInterval: 0.1)
}

guard let bounds else {
    fputs("could not find the onscreen DiskDeck window\n", stderr)
    exit(1)
}

let point = CGPoint(x: bounds.minX + min(120, bounds.width / 2), y: bounds.minY + 18)
for eventType in [CGEventType.mouseMoved, .leftMouseDown, .leftMouseUp] {
    guard let event = CGEvent(
        mouseEventSource: nil,
        mouseType: eventType,
        mouseCursorPosition: point,
        mouseButton: .left
    ) else {
        fputs("could not create window-focus events\n", stderr)
        exit(1)
    }
    event.post(tap: .cghidEventTap)
    Thread.sleep(forTimeInterval: 0.1)
}
