import Foundation
import Combine

enum AppTheme: String, CaseIterable, Identifiable {
    case solarizedLight = "Solarized Light"
    case solarizedDark = "Solarized Dark"

    var id: String { rawValue }
}

/// App-wide preferences, persisted to `UserDefaults`. A singleton (not a
/// per-view `@StateObject`) so any view -- including ones that don't need
/// live reactivity, like a one-off sheet -- can read the current values
/// without threading them through initializers.
final class AppSettings: ObservableObject {
    static let shared = AppSettings()

    @Published var theme: AppTheme {
        didSet { UserDefaults.standard.set(theme.rawValue, forKey: "theme") }
    }
    @Published var autoCorrectEnabled: Bool {
        didSet { UserDefaults.standard.set(autoCorrectEnabled, forKey: "autoCorrectEnabled") }
    }

    private init() {
        if let raw = UserDefaults.standard.string(forKey: "theme"), let saved = AppTheme(rawValue: raw) {
            theme = saved
        } else {
            theme = .solarizedLight
        }
        if UserDefaults.standard.object(forKey: "autoCorrectEnabled") != nil {
            autoCorrectEnabled = UserDefaults.standard.bool(forKey: "autoCorrectEnabled")
        } else {
            autoCorrectEnabled = true
        }
    }
}
