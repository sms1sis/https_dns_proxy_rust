package io.github.SafeDNS

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.VpnService
import android.os.Build
import android.os.ParcelFileDescriptor
import android.util.Log
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.*
import java.io.FileInputStream
import java.io.FileOutputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.nio.ByteBuffer

class ProxyService : VpnService() {

    private var vpnInterface: ParcelFileDescriptor? = null
    
    private val serviceScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var heartbeatJob: Job? = null
    private var forwardJob: Job? = null

    // Single persistent UDP socket shared by all DNS queries.
    // Demultiplexed by transaction ID via pendingQueries map.
    // Eliminates per-query socket creation and the fixed 8-thread executor.
    private var dnsSocket: DatagramSocket? = null
    private val pendingQueries = java.util.concurrent.ConcurrentHashMap<Short, kotlinx.coroutines.CompletableDeferred<ByteArray>>()
    private var dnsReaderJob: Job? = null
    
    private var currentHeartbeatDomain: String? = null
    
    // Active configuration tracking
    private var runningPort: Int = 0
    private var runningUrl: String = ""
    private var runningBootstrap: String = ""
    private var runningCacheTtl: Long = 0
    private var runningTcpLimit: Int = 0
    private var runningPollInterval: Long = 0
    private var runningHttp3: Boolean = false
    private var runningAllowIpv6: Boolean = false
    private var runningHeartbeatDomain: String = ""
    private var runningExcludedApps: Set<String> = emptySet()

    companion object {
        const val CHANNEL_ID = "ProxyServiceChannel"
        const val NOTIFICATION_ID = 1
        private const val TAG = "SafeDNS"

        // Tracks whether the VPN layer (VpnService / packet forwarding) is active.
        // Distinct from isProxyRunning() which reflects the Rust backend state.
        @Volatile
        var isVpnActive = false
            private set

        @JvmStatic
        external fun getLatency(): Int
        @JvmStatic
        external fun getLogs(): Array<String>
        @JvmStatic
        external fun getStats(): IntArray
        @JvmStatic
        external fun clearStats()
        @JvmStatic
        external fun clearCache()
        @JvmStatic
        external fun clearLogs()
        /// Returns true while run_proxy is executing on the Rust side.
        /// Use this to wait for a clean shutdown before calling startProxy() again.
        @JvmStatic
        external fun isProxyRunning(): Boolean

        @JvmStatic
        fun nativeLog(level: String, tag: String, message: String) {
            when (level) {
                "ERROR" -> Log.e(tag, message)
                "WARN" -> Log.w(tag, message)
                "INFO" -> Log.i(tag, message)
                "DEBUG" -> Log.d(tag, message)
                else -> Log.v(tag, message)
            }
        }

        init {
            System.loadLibrary("https_dns_proxy_rust")
        }
    }

    private external fun initLogger(context: Context)
    private external fun startProxy(
        listenAddr: String,
        listenPort: Int,
        resolverUrl: String,
        bootstrapDns: String,
        allowIpv6: Boolean,
        cacheTtl: Long,
        tcpLimit: Int,
        pollInterval: Long,
        useHttp3: Boolean,
        excludeSuffixes: String  // comma-separated suffixes, e.g. "every1dns.net"
    ): Int
    private external fun stopProxy()

    private var connectivityManager: android.net.ConnectivityManager? = null
    private val networkCallback = object : android.net.ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: android.net.Network) {
            super.onAvailable(network)
            if (BuildConfig.DEBUG) Log.d(TAG, "Network available — triggering bootstrap refresh")
            // When the network changes (WiFi → mobile, reconnect after drop),
            // restart the Rust backend so it re-resolves the DoH resolver's IPs.
            // Only do this if the VPN is already running to avoid a startup race.
            if (isVpnActive) {
                serviceScope.launch {
                    stopProxy()
                    var waited = 0
                    while (isProxyRunning() && waited < 3000) {
                        delay(100); waited += 100
                    }
                    startProxy(
                        "127.0.0.1", runningPort, runningUrl, runningBootstrap,
                        runningAllowIpv6, runningCacheTtl, runningTcpLimit,
                        runningPollInterval, runningHttp3, "every1dns.net"
                    )
                    if (BuildConfig.DEBUG) Log.d(TAG, "Backend restarted after network change")
                }
            }
        }
    }

    override fun onCreate() {
        super.onCreate()
        initLogger(this)
        createNotificationChannel()
        connectivityManager = getSystemService(Context.CONNECTIVITY_SERVICE) as android.net.ConnectivityManager
        connectivityManager?.registerDefaultNetworkCallback(networkCallback)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == "STOP") {
            handleStop()
            return START_NOT_STICKY
        }

        // Start foreground immediately to prevent ANR/Crash
        isVpnActive = true
        startForegroundServiceNotification()

        val prefs = getSharedPreferences("settings", Context.MODE_PRIVATE)
        val listenPort = intent?.getIntExtra("listenPort", -1).takeIf { it != null && it != -1 }
            ?: prefs.getString("listen_port", "5053")?.toIntOrNull() ?: 5053
        
        val resolverUrl = intent?.getStringExtra("resolverUrl") 
            ?: prefs.getString("resolver_url", "https://cloudflare-dns.com/dns-query") ?: "https://cloudflare-dns.com/dns-query"

        val bootstrapDns = intent?.getStringExtra("bootstrapDns")
            ?: prefs.getString("bootstrap_dns", "1.1.1.1") ?: "1.1.1.1"

        val allowIpv6 = intent?.getBooleanExtra("allowIpv6", prefs.getBoolean("allow_ipv6", false))
            ?: prefs.getBoolean("allow_ipv6", false)
        
        val cacheTtl = intent?.getLongExtra("cacheTtl", -1L).takeIf { it != null && it != -1L }
            ?: prefs.getString("cache_ttl", "300")?.toLongOrNull() ?: 300L

        val tcpLimit = intent?.getIntExtra("tcpLimit", 20) ?: 20
        val pollInterval = intent?.getLongExtra("pollInterval", 120L) ?: 120L
        val useHttp3 = intent?.getBooleanExtra("useHttp3", false) ?: false
        
        val heartbeatEnabled = intent?.getBooleanExtra("heartbeatEnabled", prefs.getBoolean("heartbeat_enabled", true)) 
            ?: prefs.getBoolean("heartbeat_enabled", true)
        
        val heartbeatDomain = intent?.getStringExtra("heartbeatDomain")
            ?: prefs.getString("heartbeat_domain", "google.com") ?: "google.com"
            
        val heartbeatInterval = intent?.getLongExtra("heartbeatInterval", -1L).takeIf { it != null && it != -1L }
            ?: prefs.getString("heartbeat_interval", "10")?.toLongOrNull() ?: 10L

        // Comma-separated cache exclusion suffixes — always exclude every1dns.net heartbeat domains
        val excludeSuffixes = intent?.getStringExtra("excludeSuffixes") ?: "every1dns.net"

        val excludedApps = prefs.getStringSet("excluded_apps", emptySet()) ?: emptySet()

        if (BuildConfig.DEBUG) Log.d(TAG, "onStartCommand: vpnReady=${vpnInterface != null}, url=$resolverUrl")

        if (vpnInterface != null) {
            val configChanged = runningPort != listenPort || runningUrl != resolverUrl || 
                               runningBootstrap != bootstrapDns || runningCacheTtl != cacheTtl ||
                               runningTcpLimit != tcpLimit || runningPollInterval != pollInterval ||
                               runningHttp3 != useHttp3 || runningHeartbeatDomain != heartbeatDomain ||
                               runningExcludedApps != excludedApps ||
                               // heartbeatEnabled toggle must also trigger a backend restart
                               // so the heartbeat loop starts/stops correctly
                               (heartbeatEnabled != (heartbeatJob != null))
            
            if (configChanged) {
                if (BuildConfig.DEBUG) Log.d(TAG, "Dynamic config change detected. Restarting backend...")
                stopProxy()
                
                runningPort = listenPort
                runningUrl = resolverUrl
                runningBootstrap = bootstrapDns
                runningCacheTtl = cacheTtl
                runningTcpLimit = tcpLimit
                runningPollInterval = pollInterval
                runningHttp3 = useHttp3
                runningAllowIpv6 = allowIpv6
                runningHeartbeatDomain = heartbeatDomain
                runningExcludedApps = excludedApps
                
                serviceScope.launch {
                    // Poll until the old run_proxy has fully released the port
                    // (or give up after 3 s). A fixed 1 s delay was not reliable
                    // on slower devices and caused port-already-in-use crashes.
                    var waited = 0
                    while (isProxyRunning() && waited < 3000) {
                        delay(100)
                        waited += 100
                    }
                    if (BuildConfig.DEBUG) Log.d(TAG, "Initializing Rust proxy on 127.0.0.1:$listenPort (waited ${waited}ms for shutdown)")
                    val res = startProxy("127.0.0.1", listenPort, resolverUrl, bootstrapDns, allowIpv6, cacheTtl, tcpLimit, pollInterval, useHttp3, "every1dns.net")
                    if (BuildConfig.DEBUG) Log.d(TAG, "Backend proxy initialized (result: $res)")
                    
                    if (heartbeatEnabled) {
                        if (BuildConfig.DEBUG) Log.d(TAG, "Triggering post-restart heartbeat")
                        startHeartbeat(heartbeatDomain, listenPort, heartbeatInterval)
                    }
                }
            } else {
                if (BuildConfig.DEBUG) Log.d(TAG, "No config change, refreshing heartbeat only")
                if (heartbeatEnabled) {
                    startHeartbeat(heartbeatDomain, listenPort, heartbeatInterval)
                } else {
                    stopHeartbeat()
                }
            }
            return START_STICKY
        }
        
        // Initial start
        runningPort = listenPort
        runningUrl = resolverUrl
        runningBootstrap = bootstrapDns
        runningCacheTtl = cacheTtl
        runningTcpLimit = tcpLimit
        runningPollInterval = pollInterval
        runningHttp3 = useHttp3
        runningAllowIpv6 = allowIpv6
        runningHeartbeatDomain = heartbeatDomain
        runningExcludedApps = excludedApps

        serviceScope.launch {
            if (BuildConfig.DEBUG) Log.d(TAG, "Starting Rust proxy on 127.0.0.1:$listenPort")
            startProxy("127.0.0.1", listenPort, resolverUrl, bootstrapDns, allowIpv6, cacheTtl, tcpLimit, pollInterval, useHttp3, "every1dns.net")        }

        try {
            val builder = Builder()
                .setSession(getString(R.string.app_name))
                .addAddress("10.0.0.1", 32)
                .addDnsServer("10.0.0.2") // Virtual DNS IP
                .addRoute("10.0.0.2", 32) // Route only the virtual DNS IP
                .setMtu(1500)
                .setBlocking(true)

            if (allowIpv6) {
                builder.addAddress("fd00::1", 128)
                       .addDnsServer("fd00::2")
                       .addRoute("fd00::2", 128)
            }

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP) {
                builder.allowBypass()
                builder.addDisallowedApplication(packageName) // Always exclude ourselves
                
                // Exclude user-selected apps
                val excludedApps = prefs.getStringSet("excluded_apps", emptySet()) ?: emptySet()
                excludedApps.forEach { pkg ->
                    try {
                        builder.addDisallowedApplication(pkg)
                    } catch (e: Exception) {
                        Log.w(TAG, "Failed to exclude app: $pkg")
                    }
                }
            }

            vpnInterface = builder.establish()
            
            if (BuildConfig.DEBUG) Log.d(TAG, "VPN Interface established (IPv6: $allowIpv6)")
            // Start shared DNS socket before the packet forwarding loop
            startDnsSocket(listenPort)
            forwardJob = serviceScope.launch { 
                delay(1000)
                if (BuildConfig.DEBUG) Log.d(TAG, "Starting packet forwarding loop on port $listenPort")
                forwardPackets(listenPort) 
            }
            
            if (heartbeatEnabled) {
                startHeartbeat(heartbeatDomain, listenPort, heartbeatInterval)
            }
            
        } catch (e: Exception) {
            Log.e(TAG, "Failed to establish VPN", e)
            handleStop()
        }

        return START_STICKY
    }

    private fun startHeartbeat(domain: String, port: Int, interval: Long) {
        stopHeartbeat()
        currentHeartbeatDomain = domain
        heartbeatJob = serviceScope.launch {
            if (BuildConfig.DEBUG) Log.d(TAG, "Starting heartbeat loop (interval ${interval}s)")
            try {
                while (isActive && isVpnActive && currentHeartbeatDomain == domain) {
                    delay(interval * 1000)
                    // Use a direct HTTPS HEAD request to the DoH resolver instead of
                    // a fake DNS wire packet — this avoids polluting the proxy's query
                    // log and stats with synthetic heartbeat entries, and gives a more
                    // accurate latency measurement since it exercises the actual DoH path.
                    try {
                        val url = java.net.URL(runningUrl)
                        val conn = url.openConnection() as java.net.HttpURLConnection
                        conn.requestMethod = "HEAD"
                        conn.connectTimeout = 4000
                        conn.readTimeout = 4000
                        conn.connect()
                        conn.disconnect()
                        if (BuildConfig.DEBUG) Log.d(TAG, "Heartbeat ping OK (${conn.responseCode})")
                    } catch (e: Exception) {
                        if (e !is CancellationException) {
                            Log.w(TAG, "Heartbeat ping failed: ${e.message}")
                        }
                    }
                }
            } catch (e: Exception) {
                if (e !is CancellationException) Log.e(TAG, "Heartbeat error", e)
            } finally {
                if (BuildConfig.DEBUG) Log.d(TAG, "Heartbeat loop stopped")
            }
        }
    }

    private fun stopHeartbeat() {
        currentHeartbeatDomain = null
        heartbeatJob?.cancel()
        heartbeatJob = null
    }

    private suspend fun forwardPackets(proxyPort: Int) {
        val fd = vpnInterface?.fileDescriptor ?: return
        val inputStream = FileInputStream(fd)
        val outputStream = FileOutputStream(fd)
        val packet = ByteBuffer.allocate(16384)
        val proxyAddr = InetAddress.getByName("127.0.0.1")
        try {
            withContext(Dispatchers.IO) {
                while (isActive && isVpnActive) {
                    val length = inputStream.read(packet.array())
                    if (length > 0) {
                        val data = packet.array().copyOf(length)
                        val version = (data[0].toInt() and 0xF0)
                        
                        if (version == 0x40 && (data[9].toInt() and 0xFF) == 17) { // IPv4 UDP
                            val ihl = (data[0].toInt() and 0x0F) * 4
                            val dPort = ((data[ihl + 2].toInt() and 0xFF) shl 8) or (data[ihl + 3].toInt() and 0xFF)
                            
                            if (dPort == 53 || InetAddress.getByAddress(data.copyOfRange(16, 20)).hostAddress == "10.0.0.2") {
                                val capturedData = data
                                serviceScope.launch {
                                    try {
                                        val dnsPayload = capturedData.copyOfRange(ihl + 8, length)
                                        val response = handleDnsQuery(dnsPayload, proxyAddr, proxyPort)
                                        if (response != null) {
                                            synchronized(outputStream) {
                                                outputStream.write(constructIpv4Udp(capturedData, response.data, response.length))
                                            }
                                        }
                                    } catch (e: Exception) {
                                        if (e !is CancellationException) Log.e(TAG, "IPv4 DNS error", e)
                                    }
                                }
                            }
                        } else if (version == 0x60) { // IPv6
                            val nextHeader = data[6].toInt() and 0xFF
                            if (nextHeader == 17) { // UDP
                                val dPort = ((data[42].toInt() and 0xFF) shl 8) or (data[43].toInt() and 0xFF)
                                if (dPort == 53) {
                                    val capturedData = data
                                    serviceScope.launch {
                                        try {
                                            val dnsPayload = capturedData.copyOfRange(48, length)
                                            val response = handleDnsQuery(dnsPayload, proxyAddr, proxyPort)
                                            if (response != null) {
                                                synchronized(outputStream) {
                                                    outputStream.write(constructIpv6Udp(capturedData, response.data, response.length))
                                                }
                                            }
                                        } catch (e: Exception) {
                                            if (e !is CancellationException) Log.e(TAG, "IPv6 DNS error", e)
                                        }
                                    }
                                }
                            }
                        }
                        packet.clear()
                    }
                    yield() 
                }
            }
        } catch (e: Exception) {
            if (e !is CancellationException) {
                Log.e(TAG, "forwardPackets critical error", e)
            }
        }
    }

    /** Start the shared DNS socket and its background reader loop. */
    private fun startDnsSocket(proxyPort: Int) {
        stopDnsSocket()
        val socket = DatagramSocket()
        protect(socket)
        dnsSocket = socket
        dnsReaderJob = serviceScope.launch(Dispatchers.IO) {
            val buf = ByteArray(4096)
            val packet = DatagramPacket(buf, buf.size)
            while (isActive) {
                try {
                    socket.receive(packet)
                    // Extract transaction ID (first 2 bytes) to match the waiter
                    if (packet.length >= 2) {
                        val txId = (((buf[0].toInt() and 0xFF) shl 8) or (buf[1].toInt() and 0xFF)).toShort()
                        val data = buf.copyOf(packet.length)
                        pendingQueries.remove(txId)?.complete(data)
                    }
                } catch (e: Exception) {
                    if (isActive) Log.w(TAG, "DNS socket read error: ${e.message}")
                    break
                }
            }
        }
        if (BuildConfig.DEBUG) Log.d(TAG, "Persistent DNS socket started")
    }

    private fun stopDnsSocket() {
        dnsReaderJob?.cancel()
        dnsReaderJob = null
        dnsSocket?.close()
        dnsSocket = null
        pendingQueries.values.forEach { it.cancel() }
        pendingQueries.clear()
    }

    /** Send a DNS query over the shared socket and await the response. */
    private suspend fun handleDnsQuery(payload: ByteArray, proxyAddr: InetAddress, proxyPort: Int): DatagramPacket? {
        val socket = dnsSocket ?: return null
        if (payload.size < 2) return null

        val txId = (((payload[0].toInt() and 0xFF) shl 8) or (payload[1].toInt() and 0xFF)).toShort()
        val deferred = kotlinx.coroutines.CompletableDeferred<ByteArray>()
        pendingQueries[txId] = deferred

        return try {
            if (BuildConfig.DEBUG) Log.d(TAG, "Outbound DNS query: ${payload.size} bytes to $proxyAddr:$proxyPort")
            socket.send(DatagramPacket(payload, payload.size, proxyAddr, proxyPort))
            val responseData = withTimeout(4000) { deferred.await() }
            if (BuildConfig.DEBUG) Log.d(TAG, "Inbound DNS response: ${responseData.size} bytes from $proxyAddr:$proxyPort")
            DatagramPacket(responseData, responseData.size)
        } catch (e: Exception) {
            pendingQueries.remove(txId)
            if (e !is CancellationException) Log.e(TAG, "DNS query failed: ${e.message}")
            null
        }
    }

    private fun constructIpv6Udp(request: ByteArray, payload: ByteArray, payloadLen: Int): ByteArray {
        val response = ByteArray(40 + 8 + payloadLen)
        // Copy IPv6 header base
        System.arraycopy(request, 0, response, 0, 40)
        // Swap Source and Destination IPs (indices 8-23 and 24-39)
        System.arraycopy(request, 24, response, 8, 16)
        System.arraycopy(request, 8, response, 24, 16)
        // Payload length in IPv6 header (UDP header + DNS payload)
        val ipv6PayloadLen = 8 + payloadLen
        response[4] = (ipv6PayloadLen shr 8).toByte()
        response[5] = (ipv6PayloadLen and 0xFF).toByte()
        // Swap UDP ports
        response[40] = request[42]; response[41] = request[43]
        response[42] = request[40]; response[43] = request[41]
        // UDP length
        response[44] = (ipv6PayloadLen shr 8).toByte()
        response[45] = (ipv6PayloadLen and 0xFF).toByte()
        
        // Copy DNS payload
        System.arraycopy(payload, 0, response, 48, payloadLen)

        // UDP checksum (REQUIRED for IPv6)
        val checksum = calculateIpv6UdpChecksum(response)
        response[46] = (checksum shr 8).toByte()
        response[47] = (checksum and 0xFF).toByte()

        return response
    }

    private fun calculateIpv6UdpChecksum(packet: ByteArray): Int {
        var sum: Long = 0
        
        // Pseudo-header: Source Address
        for (i in 8..23 step 2) {
            sum += (((packet[i].toInt() and 0xFF) shl 8) or (packet[i + 1].toInt() and 0xFF)).toLong()
        }
        // Pseudo-header: Destination Address
        for (i in 24..39 step 2) {
            sum += (((packet[i].toInt() and 0xFF) shl 8) or (packet[i + 1].toInt() and 0xFF)).toLong()
        }
        // Pseudo-header: UDP Length
        sum += (((packet[4].toInt() and 0xFF) shl 8) or (packet[5].toInt() and 0xFF)).toLong()
        // Pseudo-header: Next Header (17 for UDP)
        sum += 17

        // UDP Header + Payload
        for (i in 40 until packet.size step 2) {
            if (i == 46) continue // Skip checksum field itself
            if (i + 1 < packet.size) {
                sum += (((packet[i].toInt() and 0xFF) shl 8) or (packet[i + 1].toInt() and 0xFF)).toLong()
            } else {
                sum += ((packet[i].toInt() and 0xFF) shl 8).toLong()
            }
        }

        while ((sum shr 16) > 0) {
            sum = (sum and 0xFFFF) + (sum shr 16)
        }
        
        var res = (sum.inv() and 0xFFFF).toInt()
        if (res == 0) res = 0xFFFF
        return res
    }

    private fun constructIpv4Udp(request: ByteArray, payload: ByteArray, payloadLen: Int): ByteArray {
        val ihl = (request[0].toInt() and 0x0F) * 4
        val totalLen = ihl + 8 + payloadLen
        val response = ByteArray(totalLen)
        System.arraycopy(request, 0, response, 0, ihl)
        // Swap Source and Destination IPs
        System.arraycopy(request, 16, response, 12, 4)
        System.arraycopy(request, 12, response, 16, 4)
        
        // Swap UDP ports: response source = request dest, response dest = request source
        response[ihl] = request[ihl + 2]; response[ihl + 1] = request[ihl + 3]
        response[ihl + 2] = request[ihl]; response[ihl + 3] = request[ihl + 1]
        val udpLen = 8 + payloadLen
        response[ihl + 4] = (udpLen shr 8).toByte(); response[ihl + 5] = (udpLen and 0xFF).toByte()
        System.arraycopy(payload, 0, response, ihl + 8, payloadLen)
        response[2] = (totalLen shr 8).toByte(); response[3] = (totalLen and 0xFF).toByte()
        response[10] = 0; response[11] = 0
        var checksum: Long = 0
        for (i in 0 until ihl step 2) {
            checksum += (((response[i].toInt() and 0xFF) shl 8) or (response[i + 1].toInt() and 0xFF)).toLong()
        }
        while ((checksum shr 16) > 0) {
            checksum = (checksum and 0xFFFF) + (checksum shr 16)
        }
        val finalChecksum = (checksum.inv() and 0xFFFF).toInt()
        response[10] = (finalChecksum shr 8).toByte(); response[11] = (finalChecksum and 0xFF).toByte()
        return response
    }

    private fun startForegroundServiceNotification() {
        val intent = Intent(this, MainActivity::class.java)
        val pendingIntent = PendingIntent.getActivity(
            this, 0, intent,
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) PendingIntent.FLAG_IMMUTABLE else 0
        )

        val notification = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle(getString(R.string.notification_title))
            .setContentText(getString(R.string.notification_content))
            .setSmallIcon(R.drawable.ic_stat_shield)
            .setLargeIcon(android.graphics.BitmapFactory.decodeResource(resources, R.mipmap.ic_launcher))
            .setContentIntent(pendingIntent)
            .setOngoing(true)
            .build()
        startForeground(NOTIFICATION_ID, notification)
    }

    private fun handleStop() {
        isVpnActive = false
        stopHeartbeat()
        stopProxy()
        stopDnsSocket()
        serviceScope.cancel()
        try { vpnInterface?.close(); vpnInterface = null } catch (e: Exception) {}
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) stopForeground(STOP_FOREGROUND_REMOVE)
        else { @Suppress("DEPRECATION") stopForeground(true) }
        stopSelf()
    }

    override fun onRevoke() { handleStop(); super.onRevoke() }
    override fun onDestroy() {
        connectivityManager?.unregisterNetworkCallback(networkCallback)
        handleStop()
        super.onDestroy()
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val manager = getSystemService(NotificationManager::class.java)
            manager?.createNotificationChannel(NotificationChannel(CHANNEL_ID, getString(R.string.app_name), NotificationManager.IMPORTANCE_LOW))
        }
    }
}
