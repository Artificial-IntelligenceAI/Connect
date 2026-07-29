package com.messagingapp.connect.ui

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

enum class AppTheme(val label: String) {
    SOLARIZED_LIGHT("Solarized Light"),
    SOLARIZED_DARK("Solarized Dark")
}

/**
 * App-wide preferences, persisted to SharedPreferences. A process-wide
 * singleton (via [getInstance]) so any composable can read or write it
 * without threading it through every call site as a parameter.
 */
class AppSettings private constructor(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences("settings", Context.MODE_PRIVATE)

    var theme: AppTheme by mutableStateOf(
        AppTheme.entries.find { it.name == prefs.getString(KEY_THEME, null) } ?: AppTheme.SOLARIZED_LIGHT
    )
        private set

    var autoCorrectEnabled: Boolean by mutableStateOf(prefs.getBoolean(KEY_AUTOCORRECT, true))
        private set

    init {
        Solarized.current = theme
    }

    fun selectTheme(newTheme: AppTheme) {
        theme = newTheme
        Solarized.current = newTheme
        prefs.edit().putString(KEY_THEME, newTheme.name).apply()
    }

    fun setAutoCorrect(enabled: Boolean) {
        autoCorrectEnabled = enabled
        prefs.edit().putBoolean(KEY_AUTOCORRECT, enabled).apply()
    }

    companion object {
        private const val KEY_THEME = "theme"
        private const val KEY_AUTOCORRECT = "autoCorrectEnabled"

        @Volatile private var instance: AppSettings? = null

        fun getInstance(context: Context): AppSettings =
            instance ?: synchronized(this) {
                instance ?: AppSettings(context).also { instance = it }
            }
    }
}
