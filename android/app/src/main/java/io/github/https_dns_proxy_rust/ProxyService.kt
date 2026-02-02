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
import java.io.FileInputStream
import java.io.FileOutputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.nio.ByteBuffer
import kotlin.concurrent.thread

class ProxyService : VpnService() {

    private var vpnInterface: ParcelFileDescriptor? = null
    private var isRunning = false

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
        if (intent?.action == "STOP") {
            handleStop()
            return START_NOT_STICKY
        }

        if (isRunning) return START_STICKY
        isRunning = true

        val listenPort = intent?.getIntExtra("listenPort", 5053) ?: 5053
        val resolverUrl = intent?.getStringExtra("resolverUrl") ?: "https://dns.google/dns-query"
        val bootstrapDns = intent?.getStringExtra("bootstrapDns") ?: "8.8.8.8,1.1.1.1"

        startForegroundServiceNotification()

        // 1. Start Rust Proxy on localhost
        thread {
            Log.d(TAG, "Starting Rust proxy on 127.0.0.1:$listenPort")
            startProxy("127.0.0.1", listenPort, resolverUrl, bootstrapDns)
        }

        // 2. Establish VPN and Forward Traffic
        try {
            vpnInterface = Builder()
                .setSession("SafeDNS")
                .addAddress("10.0.0.1", 32)
                .addDnsServer("10.0.0.1")
                .addRoute("10.0.0.1", 32)
                .setBlocking(true)
                .establish()
            
            Log.d(TAG, "VPN Interface established")
            
            thread {
                forwardPackets(listenPort)
            }
            
        } catch (e: Exception) {
            Log.e(TAG, "Failed to establish VPN", e)
            handleStop()
        }

        return START_STICKY
    }

    private fun forwardPackets(proxyPort: Int) {
        val fd = vpnInterface?.fileDescriptor ?: return
        val inputStream = FileInputStream(fd)
        val outputStream = FileOutputStream(fd)
        val packet = ByteBuffer.allocate(16384)
        val udpSocket = DatagramSocket()
        val proxyAddr = InetAddress.getByName("127.0.0.1")

        try {
            while (isRunning) {
                val length = inputStream.read(packet.array())
                if (length > 0) {
                    // Simple IPv4/UDP header parsing (minimal)
                    val data = packet.array()
                    if (data[0].toInt() == 0x45 && data[9].toInt() == 17) { // IPv4 + UDP
                        val ihl = (data[0].toInt() and 0x0F) * 4
                        val udpHeaderStart = ihl
                        val dPort = ((data[udpHeaderStart + 2].toInt() and 0xFF) shl 8) or (data[udpHeaderStart + 3].toInt() and 0xFF)
                        
                        if (dPort == 53) { // It's a DNS query
                            val payloadStart = udpHeaderStart + 8
                            val payloadLen = length - payloadStart
                            
                            val dnsPayload = data.copyOfRange(payloadStart, length)
                            
                            // Forward to Rust Proxy
                            val sendPacket = DatagramPacket(dnsPayload, dnsPayload.size, proxyAddr, proxyPort)
                            udpSocket.send(sendPacket)
                            
                            // Receive Response
                            val recvBuf = ByteArray(4096)
                            val recvPacket = DatagramPacket(recvBuf, recvBuf.size)
                            udpSocket.soTimeout = 2000
                            try {
                                udpSocket.receive(recvPacket)
                                
                                // Construct IP/UDP response back to phone
                                // Note: This is a simplification. A robust implementation would swap IPs/Ports.
                                // For now, we just log and wait for the user to confirm if queries reach Rust.
                                Log.d(TAG, "DNS Response received from Rust proxy")
                            } catch (e: Exception) {}
                        }
                    }
                    packet.clear()
                }
            }
        } catch (e: Exception) {
            Log.e(TAG, "Forwarder error", e)
        }
    }

    private fun startForegroundServiceNotification() {
        val notification = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("SafeDNS Active")
            .setContentText("DNS protection is active")
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setOngoing(true)
            .build()
        startForeground(NOTIFICATION_ID, notification)
    }

    private fun handleStop() {
        isRunning = false
        stopProxy()
        vpnInterface?.close()
        vpnInterface = null
        stopSelf()
    }

    override fun onDestroy() {
        handleStop()
        super.onDestroy()
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val manager = getSystemService(NotificationManager::class.java)
            manager?.createNotificationChannel(NotificationChannel(CHANNEL_ID, "SafeDNS", NotificationManager.IMPORTANCE_LOW))
        }
    }
}