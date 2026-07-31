package com.messagingapp.connect.ui

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.ActivityCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import com.messagingapp.connect.MainActivity

private const val CHANNEL_ID = "messages"

/**
 * Tracks whether the app currently has a visible screen, via
 * [ProcessLifecycleOwner] -- which fires for the whole process rather than
 * a single Activity, so it survives the rotation-triggered recreation this
 * app disables anyway (`android:configChanges` in the manifest) and would
 * still be correct if that ever changes.
 */
object AppForegroundTracker : DefaultLifecycleObserver {
    @Volatile
    var isForeground: Boolean = false
        private set

    fun install() {
        ProcessLifecycleOwner.get().lifecycle.addObserver(this)
    }

    override fun onStart(owner: LifecycleOwner) {
        isForeground = true
    }

    override fun onStop(owner: LifecycleOwner) {
        isForeground = false
    }
}

/**
 * Local notifications only -- there is no push infrastructure (no FCM, no
 * server-side offline delivery), so these only fire while this process is
 * still alive in the background. If Android has killed the app outright,
 * nothing will notify you until you reopen it. A true always-on background
 * story would need a foreground service (to survive the OS reclaiming the
 * process) and is deliberately out of scope for this pass.
 */
object Notifications {
    fun ensureChannel(context: Context) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "Messages",
                NotificationManager.IMPORTANCE_HIGH
            )
            context.getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
        }
    }

    /** [conversationKey] groups notifications per-conversation so a burst of
     * messages updates one notification instead of piling up separate ones. */
    fun notify(context: Context, conversationKey: String, title: String, text: String) {
        val openIntent = Intent(context, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
        }
        val pendingIntent = PendingIntent.getActivity(
            context, 0, openIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )
        val notification = NotificationCompat.Builder(context, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.ic_dialog_email)
            .setContentTitle(title)
            .setContentText(text)
            .setAutoCancel(true)
            .setContentIntent(pendingIntent)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .build()

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ActivityCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS)
                != PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        NotificationManagerCompat.from(context).notify(conversationKey.hashCode(), notification)
    }
}
