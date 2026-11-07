package com.hachiwii.oink.android

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.hardware.display.DisplayManager
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.widget.Toast

/**
 * Keeps the backend alive while the app is in the background: discovery,
 * pairing, input from other computers and clipboard delivery.
 */
class OinkService : Service() {
    private var multicast: WifiManager.MulticastLock? = null
    private val displays = object : DisplayManager.DisplayListener {
        override fun onDisplayAdded(displayId: Int) {}
        override fun onDisplayRemoved(displayId: Int) {}
        override fun onDisplayChanged(displayId: Int) = Backend.screenChanged()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, getString(R.string.channel_name), NotificationManager.IMPORTANCE_LOW),
        )
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(ID, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
        } else {
            startForeground(ID, notification())
        }
        // mDNS and the UDP beacon need multicast/broadcast reception on Wi-Fi.
        multicast = getSystemService(WifiManager::class.java)
            ?.createMulticastLock("oink-discovery")
            ?.apply {
                setReferenceCounted(false)
                acquire()
            }
        getSystemService(DisplayManager::class.java).registerDisplayListener(displays, Handler(Looper.getMainLooper()))
        Backend.start {
            Backend.startError?.let {
                Toast.makeText(this, getString(R.string.start_failed, it), Toast.LENGTH_LONG).show()
            }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            Backend.stop()
            stopSelf()
            return START_NOT_STICKY
        }
        return START_STICKY
    }

    override fun onDestroy() {
        getSystemService(DisplayManager::class.java).unregisterDisplayListener(displays)
        multicast?.release()
        super.onDestroy()
    }

    private fun notification(): Notification {
        val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), flags)
        val clipboard = PendingIntent.getActivity(
            this,
            1,
            Intent(this, ClipboardActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            flags,
        )
        val stop = PendingIntent.getService(this, 2, Intent(this, OinkService::class.java).setAction(ACTION_STOP), flags)
        return Notification.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(getString(R.string.notification_title))
            .setContentText(getString(R.string.notification_text))
            .setContentIntent(open)
            .setOngoing(true)
            .addAction(Notification.Action.Builder(null, getString(R.string.notification_send_clipboard), clipboard).build())
            .addAction(Notification.Action.Builder(null, getString(R.string.notification_stop), stop).build())
            .build()
    }

    companion object {
        private const val CHANNEL = "sharing"
        private const val ID = 1
        private const val ACTION_STOP = "com.hachiwii.oink.android.STOP"

        fun start(context: Context) {
            context.startForegroundService(Intent(context, OinkService::class.java))
        }
    }
}
