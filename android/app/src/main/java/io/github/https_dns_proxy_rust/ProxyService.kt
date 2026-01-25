package io.github.https_dns_proxy_rust

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingCorner
import android.app.Service
import android.content.Intent
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import kotlin.concurrent.thread

class ProxyService : Service() {

    companion object {
        const val CHANNEL_ID = "ProxyServiceChannel"
        const val NOTIFICATION_ID = 1

        init {
            System.loadLibrary("https_dns_proxy_rust")
        }
    }

    private external fun initLogger()
    private external fun startProxy(listenAddr: String, listenPort: Int, resolverUrl: String, bootstrapDns: String): Int
    private external fun stopProxy()

    override fun onCreate() {
        super.onCreate()
        initLogger()
        createNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val listenAddr = intent?.getStringExtra("listenAddr") ?: "127.0.0.1"
        val listenPort = intent?.getIntExtra("listenPort", 5053) ?: 5053
        val resolverUrl = intent?.getStringExtra("resolverUrl") ?: "https://dns.google/dns-query"
        val bootstrapDns = intent?.getStringExtra("bootstrapDns") ?: "8.8.8.8,1.1.1.1"

        val notification = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("DNS Proxy Running")
            .setContentText("Listening on $listenAddr:$listenPort")
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .build()

        startForeground(NOTIFICATION_ID, notification)

        thread {
            startProxy(listenAddr, listenPort, resolverUrl, bootstrapDns)
        }

        return START_STICKY
    }

    override fun onDestroy() {
        stopProxy()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val serviceChannel = NotificationChannel(
                CHANNEL_ID,
                "DNS Proxy Service Channel",
                NotificationManager.IMPORTANCE_LOW
            )
            val manager = getSystemService(NotificationManager::class.java)
            manager.createNotificationChannel(serviceChannel)
        }
    }
}
