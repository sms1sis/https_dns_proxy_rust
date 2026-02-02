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

        thread {
            Log.d(TAG, "Starting Rust proxy on 127.0.0.1:$listenPort")
            startProxy("127.0.0.1", listenPort, resolverUrl, bootstrapDns)
        }

        try {
            val builder = Builder()
                .setSession("SafeDNS")
                .addAddress("10.0.0.1", 32)
                .addDnsServer("1.1.1.1") // Changed to Cloudflare to avoid potential Google IP routing conflicts
                .addRoute("1.1.1.1", 32) // Route ONLY the target DNS IP
                .setBlocking(true)

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP) {
                builder.allowBypass()
                builder.addDisallowedApplication(packageName)
            }

            vpnInterface = builder.establish()
            Log.d(TAG, "VPN Interface established (Targeted Routing: 1.1.1.1)")
            
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
                    val data = packet.array()
                    if ((data[0].toInt() and 0xF0) == 0x40 && data[9].toInt() == 17) {
                        val ihl = (data[0].toInt() and 0x0F) * 4
                        val dPort = ((data[ihl + 2].toInt() and 0xFF) shl 8) or (data[ihl + 3].toInt() and 0xFF)
                        
                        if (dPort == 53) {
                            // Log.d(TAG, "Intercepted DNS query to 1.1.1.1") 
                            val dnsPayload = data.copyOfRange(ihl + 8, length)
                            val sendPacket = DatagramPacket(dnsPayload, dnsPayload.size, proxyAddr, proxyPort)
                            udpSocket.send(sendPacket)
                            
                            val recvBuf = ByteArray(4096)
                            val recvPacket = DatagramPacket(recvBuf, recvBuf.size)
                            // Increased timeout to 4s to handle mobile network latency
                            udpSocket.soTimeout = 4000 
                            try {
                                udpSocket.receive(recvPacket)
                                val response = constructIpv4Udp(data, recvPacket.data, recvPacket.length)
                                outputStream.write(response)
                                // Log.d(TAG, "Sent DNS response back to system")
                            } catch (e: Exception) {
                                Log.w(TAG, "Timeout waiting for Rust proxy response")
                            }
                        }
                    }
                    packet.clear()
                }
            }
        } catch (e: Exception) {
            Log.e(TAG, "Forwarder error", e)
        }
    }

    private fun constructIpv4Udp(request: ByteArray, payload: ByteArray, payloadLen: Int): ByteArray {
        val ihl = (request[0].toInt() and 0x0F) * 4
        val totalLen = ihl + 8 + payloadLen
        val response = ByteArray(totalLen)
        System.arraycopy(request, 0, response, 0, ihl)
        System.arraycopy(request, 16, response, 12, 4)
        System.arraycopy(request, 12, response, 16, 4)
        response[ihl] = request[ihl + 2]
        response[ihl + 1] = request[ihl + 3]
        response[ihl + 2] = request[ihl]
        response[ihl + 3] = request[ihl + 1]
        val udpLen = 8 + payloadLen
        response[ihl + 4] = (udpLen shr 8).toByte()
        response[ihl + 5] = (udpLen and 0xFF).toByte()
        response[ihl + 6] = 0
        response[ihl + 7] = 0
        System.arraycopy(payload, 0, response, ihl + 8, payloadLen)
        response[2] = (totalLen shr 8).toByte()
        response[3] = (totalLen and 0xFF).toByte()
        response[10] = 0
        response[11] = 0
        var checksum = 0
        for (i in 0 until ihl step 2) {
            checksum += ((response[i].toInt() and 0xFF) shl 8) or (response[i + 1].toInt() and 0xFF)
        }
        while ((checksum shr 16) > 0) checksum = (checksum and 0xFFFF) + (checksum shr 16)
        checksum = checksum.inv() and 0xFFFF
        response[10] = (checksum shr 8).toByte()
        response[11] = (checksum and 0xFF).toByte()
        return response
    }

    private fun startForegroundServiceNotification() {
        val notification = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("SafeDNS Active")
            .setContentText("Protecting DNS queries")
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
        stopForeground(true)
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