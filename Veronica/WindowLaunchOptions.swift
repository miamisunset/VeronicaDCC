import AppKit
import CoreGraphics
import Foundation

/// Test-only window geometry pin (`--vrn-window-size=WxH`, UI tests).
///
/// Pixel oracles couple to window geometry (issue #94): the suite never
/// pinned the window, so macOS restoration reopened whatever size the last
/// session left — a 3200-wide window reads a 2x painted fraction versus the
/// calibration geometry with zero code changes. The pin restores
/// determinism: call `clearRestoredFrames()` from the app entrypoint before
/// any window exists (restoration otherwise wins), then `applyPinnedSize()`
/// from the content view's first appear.
///
/// Both entry points no-op without the flag, so production behavior —
/// restoration included — is untouched.
nonisolated enum WindowLaunchOptions {
    /// Launch-argument prefix; the remainder parses as `<width>x<height>`.
    static let windowSizeArgumentPrefix = "--vrn-window-size="

    /// True when the pin flag is present, even when malformed (callers fail
    /// loudly on `pinnedSize == nil` instead of measuring at drifted
    /// geometry).
    static var hasFlag: Bool {
        CommandLine.arguments.contains { $0.hasPrefix(windowSizeArgumentPrefix) }
    }

    /// Pinned content size in points, or `nil` when the flag is absent or
    /// malformed.
    static var pinnedSize: CGSize? {
        guard
            let raw = CommandLine.arguments.first(where: {
                $0.hasPrefix(windowSizeArgumentPrefix)
            })
        else { return nil }
        let dims = raw
            .dropFirst(windowSizeArgumentPrefix.count)
            .split(separator: "x")
            .compactMap { Double($0) }
        guard dims.count == 2, dims[0] > 0, dims[1] > 0 else { return nil }
        return CGSize(width: dims[0], height: dims[1])
    }

    /// Deletes persisted window and split frames so restoration cannot leak
    /// a previous session's geometry into the pinned run. Must run before
    /// the first window exists (app entrypoint `init`); afterwards the
    /// autosave keys are already consumed. Prefix-matched rather than
    /// hardcoded so view-hierarchy renames cannot silently orphan a key.
    static func clearRestoredFrames() {
        let defaults = UserDefaults.standard
        for key in defaults.dictionaryRepresentation().keys
        where key.hasPrefix("NSWindow Frame") || key.hasPrefix("NSSplitView Subview Frames") {
            defaults.removeObject(forKey: key)
        }
    }

    /// Resizes the key window to `pinnedSize`, preserving its origin (which
    /// keeps the window on-screen regardless of display size). No-op without
    /// a well-formed flag. Async hop lets the freshly restored window finish
    /// appearing first; callers measure long after (engine warm-up), so the
    /// size has always landed before the first oracle.
    static func applyPinnedSize() {
        guard let size = pinnedSize else { return }
        Task { @MainActor in
            guard let window = NSApp.keyWindow ?? NSApp.windows.first(where: \.isVisible) else {
                return
            }
            var frame = window.frame
            frame.size = NSSize(width: size.width, height: size.height)
            window.setFrame(frame, display: true)
        }
    }
}
