import SwiftUI

private struct SolarizedPalette {
    let base3: Color
    let base2: Color
    let base1: Color
    let base01: Color
    let base00: Color
    let blue: Color
    let red: Color
    let yellow: Color

    // https://ethanschoonover.com/solarized/ -- light uses base3/base2 as
    // background tones and base01/base00 as foreground; dark swaps to
    // base03/base02 backgrounds and base1/base0 foreground. Accent colors
    // (blue/red/yellow) are constant across both.
    static let light = SolarizedPalette(
        base3: Color(red: 0xfd / 255, green: 0xf6 / 255, blue: 0xe3 / 255),
        base2: Color(red: 0xee / 255, green: 0xe8 / 255, blue: 0xd5 / 255),
        base1: Color(red: 0x93 / 255, green: 0xa1 / 255, blue: 0xa1 / 255),
        base01: Color(red: 0x58 / 255, green: 0x6e / 255, blue: 0x75 / 255),
        base00: Color(red: 0x65 / 255, green: 0x7b / 255, blue: 0x83 / 255),
        blue: Color(red: 0x26 / 255, green: 0x8b / 255, blue: 0xd2 / 255),
        red: Color(red: 0xdc / 255, green: 0x32 / 255, blue: 0x2f / 255),
        yellow: Color(red: 0xb5 / 255, green: 0x89 / 255, blue: 0x00 / 255)
    )

    static let dark = SolarizedPalette(
        base3: Color(red: 0x00 / 255, green: 0x2b / 255, blue: 0x36 / 255),  // base03
        base2: Color(red: 0x07 / 255, green: 0x36 / 255, blue: 0x42 / 255),  // base02
        base1: Color(red: 0x58 / 255, green: 0x6e / 255, blue: 0x75 / 255),  // base01
        base01: Color(red: 0x93 / 255, green: 0xa1 / 255, blue: 0xa1 / 255), // base1
        base00: Color(red: 0x83 / 255, green: 0x94 / 255, blue: 0x96 / 255), // base0
        blue: Color(red: 0x26 / 255, green: 0x8b / 255, blue: 0xd2 / 255),
        red: Color(red: 0xdc / 255, green: 0x32 / 255, blue: 0x2f / 255),
        yellow: Color(red: 0xb5 / 255, green: 0x89 / 255, blue: 0x00 / 255)
    )
}

private extension AppTheme {
    var palette: SolarizedPalette {
        switch self {
        case .solarizedLight: return .light
        case .solarizedDark: return .dark
        }
    }
}

/// Live colors for the currently-selected `AppSettings.shared.theme`. Kept
/// as a static namespace so the many existing `Solarized.xxx` call sites
/// don't need to change -- each read is fresh, so any view that's already
/// observing `AppSettings` (directly or via re-rendering for another
/// reason) picks up a theme change immediately.
enum Solarized {
    static var base3: Color { AppSettings.shared.theme.palette.base3 }
    static var base2: Color { AppSettings.shared.theme.palette.base2 }
    static var base1: Color { AppSettings.shared.theme.palette.base1 }
    static var base01: Color { AppSettings.shared.theme.palette.base01 }
    static var base00: Color { AppSettings.shared.theme.palette.base00 }
    static var blue: Color { AppSettings.shared.theme.palette.blue }
    static var red: Color { AppSettings.shared.theme.palette.red }
    static var yellow: Color { AppSettings.shared.theme.palette.yellow }
}

public extension View {
    /// Applies the app's accent color. Color scheme (light/dark) is set
    /// dynamically in `ContentView` instead, where the view already
    /// observes `AppSettings` and can react to theme changes live.
    func appTheme() -> some View {
        self.tint(Solarized.blue)
    }
}
