package io.github.https_dns_proxy_rust

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Intent
import android.net.VpnService
import android.os.Build
import android.os.ParcelFileDescriptor
import android.util.Log
import androidx.core.app.NotificationCompat
import kotlin.concurrent.thread

class ProxyService : VpnService() {

    private var vpnInterface: ParcelFileDescriptor? = null

    companion object {
        const val CHANNEL_ID = "ProxyServiceChannel"
        const val NOTIFICATION_ID = 1
        private const val TAG = "ProxyService"

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
        val listenAddr = "0.0.0.0"
        val listenPort = intent?.getIntExtra("listenPort", 5053) ?: 5053
        val resolverUrl = intent?.getStringExtra("resolverUrl") ?: "https://dns.google/dns-query"
        val bootstrapDns = intent?.getStringExtra("bootstrapDns") ?: "8.8.8.8,1.1.1.1"

        // 1. Start Foreground Notification IMMEDIATELY
        val mainActivityIntent = Intent(this, MainActivity::class.java)
        val pendingIntent = PendingIntent.getActivity(
            this, 0, mainActivityIntent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )

        val notification = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("SafeDNS Active")
            .setContentText("DNS queries are being encrypted via $resolverUrl")
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setContentIntent(pendingIntent)
            .setOngoing(true)
            .build()

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                android.content.pm.ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }

        // 2. Establish VPN Interface to capture DNS traffic
        try {
            vpnInterface = Builder()
                .setSession("SafeDNS")
                .addAddress("10.0.0.2", 24)
                .addDnsServer("10.0.0.2") // Point DNS to the VPN interface where our proxy will listen
                .addRoute("0.0.0.0", 0)    // Route all traffic through VPN to ensure DNS is captured
                .establish()
            
            Log.d(TAG, "VPN Interface established")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to establish VPN", e)
            stopSelf()
            return START_NOT_STICKY
        }

        // 3. Start the Rust proxy in a separate thread
        thread {
            Log.d(TAG, "Starting Rust proxy on $listenAddr:$listenPort")
            startProxy(listenAddr, listenPort, resolverUrl, bootstrapDns)
        }

        return START_STICKY
    }

    override fun onDestroy() {
        stopProxy()
        try {
            vpnInterface?.close()
            vpnInterface = null
        } catch (e: Exception) {
            Log.e(TAG, "Error closing VPN interface", e)
        }
        super.onDestroy()
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val serviceChannel = NotificationChannel(
                CHANNEL_ID,
                "SafeDNS Service Channel",
                NotificationManager.IMPORTANCE_LOW
            )
            val manager = getSystemService(NotificationManager::class.java)
            manager?.createNotificationChannel(serviceChannel)
        }
    }
}
