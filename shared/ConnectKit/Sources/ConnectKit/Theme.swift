import SwiftUI

/// Solarized Light palette (https://ethanschoonover.com/solarized/).
enum Solarized {
    static let base3 = Color(red: 0xfd / 255, green: 0xf6 / 255, blue: 0xe3 / 255)   // background
    static let base2 = Color(red: 0xee / 255, green: 0xe8 / 255, blue: 0xd5 / 255)   // background highlights
    static let base1 = Color(red: 0x93 / 255, green: 0xa1 / 255, blue: 0xa1 / 255)   // secondary content
    static let base00 = Color(red: 0x65 / 255, green: 0x7b / 255, blue: 0x83 / 255)  // body text
    static let base01 = Color(red: 0x58 / 255, green: 0x6e / 255, blue: 0x75 / 255)  // emphasized text
    static let blue = Color(red: 0x26 / 255, green: 0x8b / 255, blue: 0xd2 / 255)
    static let red = Color(red: 0xdc / 255, green: 0x32 / 255, blue: 0x2f / 255)
    static let yellow = Color(red: 0xb5 / 255, green: 0x89 / 255, blue: 0x00 / 255)
}

public extension View {
    /// Applies the app's default theme (Solarized Light).
    func appTheme() -> some View {
        self
            .preferredColorScheme(.light)
            .tint(Solarized.blue)
    }
}
