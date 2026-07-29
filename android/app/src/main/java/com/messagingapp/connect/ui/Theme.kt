package com.messagingapp.connect.ui

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color

private data class SolarizedPalette(
    val base3: Color,
    val base2: Color,
    val base1: Color,
    val base01: Color,
    val base00: Color,
    val blue: Color,
    val red: Color,
    val yellow: Color
)

// https://ethanschoonover.com/solarized/ -- light uses base3/base2 as
// background tones and base01/base00 as foreground; dark swaps to
// base03/base02 backgrounds and base1/base0 foreground. Accent colors
// (blue/red/yellow) are constant across both.
private val lightPalette = SolarizedPalette(
    base3 = Color(0xFFFDF6E3),
    base2 = Color(0xFFEEE8D5),
    base1 = Color(0xFF93A1A1),
    base01 = Color(0xFF586E75),
    base00 = Color(0xFF657B83),
    blue = Color(0xFF268BD2),
    red = Color(0xFFDC322F),
    yellow = Color(0xFFB58900)
)

private val darkPalette = SolarizedPalette(
    base3 = Color(0xFF002B36), // base03
    base2 = Color(0xFF073642), // base02
    base1 = Color(0xFF586E75), // base01
    base01 = Color(0xFF93A1A1), // base1
    base00 = Color(0xFF839496), // base0
    blue = Color(0xFF268BD2),
    red = Color(0xFFDC322F),
    yellow = Color(0xFFB58900)
)

/**
 * Live colors for the currently-selected [AppTheme]. Reading any property
 * inside a composable subscribes it to [current] (a Compose `State`), so
 * changing the theme recomposes every screen using these colors
 * automatically -- no need to thread a theme parameter everywhere.
 */
object Solarized {
    var current: AppTheme by mutableStateOf(AppTheme.SOLARIZED_LIGHT)

    private val palette: SolarizedPalette
        get() = if (current == AppTheme.SOLARIZED_LIGHT) lightPalette else darkPalette

    val base3 get() = palette.base3
    val base2 get() = palette.base2
    val base1 get() = palette.base1
    val base01 get() = palette.base01
    val base00 get() = palette.base00
    val blue get() = palette.blue
    val red get() = palette.red
    val yellow get() = palette.yellow
}
