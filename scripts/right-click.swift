import ApplicationServices
import CoreGraphics
import Foundation

guard CommandLine.arguments.count == 3,
      let x = Double(CommandLine.arguments[1]),
      let y = Double(CommandLine.arguments[2]),
      x.isFinite,
      y.isFinite else {
    fputs("usage: right-click.swift <x> <y>\n", stderr)
    exit(64)
}

guard CGPreflightPostEventAccess() else {
    fputs("Accessibility is required to post a right-click. Enable the terminal or Codex in System Settings → Privacy & Security → Accessibility.\n", stderr)
    exit(77)
}

let point = CGPoint(x: x, y: y)
guard let down = CGEvent(
    mouseEventSource: nil,
    mouseType: .rightMouseDown,
    mouseCursorPosition: point,
    mouseButton: .right
), let up = CGEvent(
    mouseEventSource: nil,
    mouseType: .rightMouseUp,
    mouseCursorPosition: point,
    mouseButton: .right
) else {
    fputs("could not create right-click events\n", stderr)
    exit(1)
}

down.post(tap: .cghidEventTap)
Thread.sleep(forTimeInterval: 0.12)
up.post(tap: .cghidEventTap)
