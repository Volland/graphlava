import Foundation
#if canImport(UIKit) && !os(watchOS)
import UIKit
#endif

/// Runs work inside an OS background-execution window (iOS
/// `beginBackgroundTask`), so suspend completes before the app is suspended.
public protocol BackgroundTaskRunner: Sendable {
    func run(_ name: String, _ work: @escaping @Sendable () -> Void)
}

/// Runs work at once (macOS, tests, extensions).
public struct ImmediateRunner: BackgroundTaskRunner {
    public init() {}
    public func run(_ name: String, _ work: @escaping @Sendable () -> Void) { work() }
}

#if canImport(UIKit) && !os(watchOS)
/// `UIApplication.beginBackgroundTask` around the work, on a utility queue.
public struct UIKitBackgroundTaskRunner: BackgroundTaskRunner {
    public init() {}
    public func run(_ name: String, _ work: @escaping @Sendable () -> Void) {
        DispatchQueue.main.async {
            let app = UIApplication.shared
            var task: UIBackgroundTaskIdentifier = .invalid
            task = app.beginBackgroundTask(withName: name) {
                if task != .invalid { app.endBackgroundTask(task) }
                task = .invalid
            }
            DispatchQueue.global(qos: .userInitiated).async {
                work()
                DispatchQueue.main.async {
                    if task != .invalid { app.endBackgroundTask(task) }
                    task = .invalid
                }
            }
        }
    }
}
#endif

/// Suspends the database when the app leaves the foreground and resumes it
/// when it returns (rule `api-ios-suspend`, [[07-platforms#iOS]]): on
/// `willResignActive` or `didEnterBackground` it calls `suspend` inside a
/// background-task window; on `willEnterForeground` or `didBecomeActive`,
/// `resume`. Suspend releases the file lock only for an app-group file; a
/// private-container file keeps it (the `ios-private-container` exception).
// @tg: implements:: [[openspec:mobile-bindings#iOS suspend and resume]]
public final class LifecycleObserver: @unchecked Sendable {
    public enum Event: Sendable { case background, foreground }

    private weak var db: Database?
    private let runner: BackgroundTaskRunner
    private let center: NotificationCenter
    private var tokens: [NSObjectProtocol] = []
    private let lock = NSLock()
    private var inBackground = false

    /// Observes `background` and `foreground` notification names on `center`.
    public init(database: Database, center: NotificationCenter = .default,
                background: [Notification.Name], foreground: [Notification.Name],
                runner: BackgroundTaskRunner) {
        db = database
        self.runner = runner
        self.center = center
        for n in background {
            tokens.append(center.addObserver(forName: n, object: nil, queue: nil) { [weak self] _ in
                self?.handle(.background)
            })
        }
        for n in foreground {
            tokens.append(center.addObserver(forName: n, object: nil, queue: nil) { [weak self] _ in
                self?.handle(.foreground)
            })
        }
    }

    deinit { tokens.forEach { center.removeObserver($0) } }

    /// The platform's observer: UIKit notifications on iOS; none elsewhere.
    static func standard(for db: Database) -> LifecycleObserver? {
        #if canImport(UIKit) && !os(watchOS)
        guard Bundle.main.bundleURL.pathExtension == "app" else { return nil }
        return LifecycleObserver(
            database: db,
            background: [UIApplication.willResignActiveNotification, UIApplication.didEnterBackgroundNotification],
            foreground: [UIApplication.willEnterForegroundNotification, UIApplication.didBecomeActiveNotification],
            runner: UIKitBackgroundTaskRunner())
        #else
        return nil
        #endif
    }

    /// Handles one lifecycle event; repeated events of one kind are ignored.
    public func handle(_ event: Event) {
        lock.lock()
        let changed = inBackground != (event == .background)
        inBackground = event == .background
        lock.unlock()
        guard changed, let db else { return }
        switch event {
        case .background:
            runner.run("graphlava.suspend") { try? db.suspend() }
        case .foreground:
            try? db.resume()
        }
    }
}
