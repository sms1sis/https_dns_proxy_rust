package io.github.https_dns_proxy_rust

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.VpnService
import android.os.Build
import android.os.Bundle
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.*
import androidx.compose.animation.core.*
import androidx.compose.foundation.background
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ListAlt
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import io.github.https_dns_proxy_rust.ui.theme.HttpsDnsProxyTheme
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContent {
            val context = LocalContext.current
            val prefs = remember { context.getSharedPreferences("settings", MODE_PRIVATE) }
            var themeMode by remember { mutableStateOf(prefs.getString("theme", "System") ?: "System") }
            
            val darkTheme = when (themeMode) {
                "Light" -> false
                "Dark" -> true
                "AMOLED" -> true
                else -> isSystemInDarkTheme()
            }
            
            val amoled = themeMode == "AMOLED"

            HttpsDnsProxyTheme(darkTheme = darkTheme, amoled = amoled) {
                Surface(
                    modifier = Modifier.fillMaxSize(),
                    color = MaterialTheme.colorScheme.background
                ) {
                    ProxyScreen(
                        themeMode = themeMode,
                        onThemeChange = { 
                            themeMode = it
                            prefs.edit().putString("theme", it).apply()
                        }
                    )
                }
            }
        }
    }

    @OptIn(ExperimentalMaterial3Api::class)
    @Composable
    fun ProxyScreen(themeMode: String, onThemeChange: (String) -> Unit) {
        val context = LocalContext.current
        val prefs = remember { context.getSharedPreferences("settings", MODE_PRIVATE) }
        
        var isRunning by remember { mutableStateOf(ProxyService.isProxyRunning) }
        var currentTab by remember { mutableStateOf(0) }
        var latency by remember { mutableStateOf(0) }
        var logs by remember { mutableStateOf(emptyArray<String>()) }
        var autoStart by remember { mutableStateOf(prefs.getBoolean("auto_start", false)) }
        var heartbeatEnabled by remember { mutableStateOf(prefs.getBoolean("heartbeat_enabled", true)) }
        var heartbeatDomain by remember { mutableStateOf(prefs.getString("heartbeat_domain", "google.com") ?: "google.com") }
        var heartbeatInterval by remember { mutableStateOf(prefs.getString("heartbeat_interval", "10") ?: "10") }

        val profiles = listOf(
            DnsProfile("Cloudflare", "https://cloudflare-dns.com/dns-query", "1.1.1.1"),
            DnsProfile("Google", "https://dns.google/dns-query", "8.8.8.8"),
            DnsProfile("AdGuard", "https://dns.adguard-dns.com/dns-query", "94.140.14.14"),
            DnsProfile("Quad9", "https://dns.quad9.net/dns-query", "9.9.9.9"),
            DnsProfile("Custom", "", "")
        )
        
        var selectedProfileIndex by remember { mutableStateOf(0) }
        var resolverUrl by remember { mutableStateOf(profiles[0].url) }
        var bootstrapDns by remember { mutableStateOf(profiles[0].bootstrap) }
        var listenPort by remember { mutableStateOf("5053") }

        val drawerState = rememberDrawerState(initialValue = DrawerValue.Closed)
        val scope = rememberCoroutineScope()
        var showAboutDialog by remember { mutableStateOf(false) }
        val uriHandler = LocalUriHandler.current

        LaunchedEffect(Unit) {
            while (true) {
                isRunning = ProxyService.isProxyRunning
                if (isRunning) {
                    latency = ProxyService.getLatency()
                    logs = ProxyService.getLogs()
                }
                delay(1000)
            }
        }

        val vpnPermissionLauncher = rememberLauncherForActivityResult(
            ActivityResultContracts.StartActivityForResult()
        ) { result ->
            if (result.resultCode == RESULT_OK) {
                startProxyService(resolverUrl, listenPort, bootstrapDns, heartbeatEnabled, heartbeatDomain, heartbeatInterval)
                isRunning = true
            }
        }

        if (showAboutDialog) {
            AboutDialog(onDismiss = { showAboutDialog = false }, uriHandler = uriHandler)
        }

        val onToggle = {
            if (isRunning) {
                val stopIntent = Intent(context, ProxyService::class.java).apply { action = "STOP" }
                context.startService(stopIntent)
                isRunning = false
            } else {
                val vpnIntent = VpnService.prepare(context)
                if (vpnIntent != null) vpnPermissionLauncher.launch(vpnIntent)
                else {
                    startProxyService(resolverUrl, listenPort, bootstrapDns, heartbeatEnabled, heartbeatDomain, heartbeatInterval)
                    isRunning = true
                }
            }
        }

        ModalNavigationDrawer(
            drawerState = drawerState,
            drawerContent = {
                ModalDrawerSheet {
                    DrawerContent(
                        themeMode = themeMode,
                        autoStart = autoStart,
                        heartbeatEnabled = heartbeatEnabled,
                        heartbeatDomain = heartbeatDomain,
                        heartbeatInterval = heartbeatInterval,
                        onThemeChange = onThemeChange,
                        onAutoStartChange = { 
                            autoStart = it
                            prefs.edit().putBoolean("auto_start", it).apply()
                        },
                        onHeartbeatChange = { enabled ->
                            heartbeatEnabled = enabled
                            prefs.edit().putBoolean("heartbeat_enabled", enabled).apply()
                            if (isRunning) {
                                startProxyService(resolverUrl, listenPort, bootstrapDns, enabled, heartbeatDomain, heartbeatInterval)
                            }
                        },
                        onHeartbeatDomainChange = { domain ->
                            heartbeatDomain = domain
                            prefs.edit().putString("heartbeat_domain", domain).apply()
                            if (isRunning) {
                                startProxyService(resolverUrl, listenPort, bootstrapDns, heartbeatEnabled, domain, heartbeatInterval)
                            }
                        },
                        onHeartbeatIntervalChange = { interval ->
                            heartbeatInterval = interval
                            prefs.edit().putString("heartbeat_interval", interval).apply()
                            if (isRunning) {
                                startProxyService(resolverUrl, listenPort, bootstrapDns, heartbeatEnabled, heartbeatDomain, interval)
                            }
                        },
                        onAboutClick = { showAboutDialog = true },
                        onClose = { scope.launch { drawerState.close() } }
                    )
                }
            }
        ) {
            Scaffold(
                topBar = {
                    CenterAlignedTopAppBar(
                        title = { 
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Icon(Icons.Default.Shield, null, modifier = Modifier.size(24.dp), tint = MaterialTheme.colorScheme.primary)
                                Spacer(Modifier.width(8.dp))
                                Text("SafeDNS", fontWeight = FontWeight.Black, letterSpacing = (-0.5).sp)
                            }
                        },
                        navigationIcon = {
                            IconButton(onClick = { scope.launch { drawerState.open() } }) {
                                Icon(Icons.Filled.Menu, "Menu")
                            }
                        },
                        colors = TopAppBarDefaults.topAppBarColors(containerColor = Color.Transparent)
                    )
                },
                bottomBar = {
                    NavigationBar(
                        containerColor = MaterialTheme.colorScheme.surface.copy(alpha = 0.8f),
                        tonalElevation = 0.dp
                    ) {
                        NavigationBarItem(
                            icon = { Icon(Icons.Filled.Dashboard, null) },
                            label = { Text("App") },
                            selected = currentTab == 0,
                            onClick = { currentTab = 0 }
                        )
                        NavigationBarItem(
                            icon = { Icon(Icons.AutoMirrored.Filled.ListAlt, null) },
                            label = { Text("Activity") },
                            selected = currentTab == 1,
                            onClick = { currentTab = 1 }
                        )
                    }
                }
            ) { contentPadding ->
                Box(modifier = Modifier.padding(contentPadding).fillMaxSize()) {
                    if (isRunning) {
                        Box(Modifier.fillMaxSize().background(
                            Brush.radialGradient(
                                colors = listOf(MaterialTheme.colorScheme.primary.copy(alpha = 0.05f), Color.Transparent),
                                center = androidx.compose.ui.geometry.Offset(500f, 500f),
                                radius = 1000f
                            )
                        ))
                    }

                    if (currentTab == 0) {
                        DashboardScreen(
                            isRunning = isRunning,
                            latency = latency,
                            resolverUrl = resolverUrl,
                            bootstrapDns = bootstrapDns,
                            listenPort = listenPort,
                            profiles = profiles,
                            selectedProfileIndex = selectedProfileIndex,
                            onProfileSelect = { index ->
                                selectedProfileIndex = index
                                if (index < profiles.size - 1) {
                                    resolverUrl = profiles[index].url
                                    bootstrapDns = profiles[index].bootstrap
                                }
                            },
                            onUrlChange = { resolverUrl = it },
                            onBootstrapChange = { bootstrapDns = it },
                            onPortChange = { listenPort = it },
                            onToggle = onToggle
                        )
                    } else {
                        LogScreen(logs)
                    }
                }
            }
        }
    }

    @OptIn(ExperimentalMaterial3Api::class)
    @Composable
    fun DashboardScreen(
        isRunning: Boolean,
        latency: Int,
        resolverUrl: String,
        bootstrapDns: String,
        listenPort: String,
        profiles: List<DnsProfile>,
        selectedProfileIndex: Int,
        onProfileSelect: (Int) -> Unit,
        onUrlChange: (String) -> Unit,
        onBootstrapChange: (String) -> Unit,
        onPortChange: (String) -> Unit,
        onToggle: () -> Unit
    ) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(horizontal = 24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.SpaceEvenly
        ) {
            StatusHero(isRunning, latency, onToggle)

            // Main Settings Card
            ElevatedCard(
                modifier = Modifier.fillMaxWidth(),
                shape = RoundedCornerShape(32.dp),
                colors = CardDefaults.elevatedCardColors(
                    containerColor = MaterialTheme.colorScheme.surface
                ),
                elevation = CardDefaults.elevatedCardElevation(defaultElevation = 2.dp)
            ) {
                Column(modifier = Modifier.padding(24.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Surface(
                            color = MaterialTheme.colorScheme.primaryContainer,
                            shape = CircleShape,
                            modifier = Modifier.size(32.dp)
                        ) {
                            Box(contentAlignment = Alignment.Center) {
                                Icon(Icons.Default.Tune, null, modifier = Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onPrimaryContainer)
                            }
                        }
                        Spacer(Modifier.width(12.dp))
                        Text("Configuration", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
                    }
                    
                    var expanded by remember { mutableStateOf(false) }
                    Column {
                        Text("Service Provider", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary)
                        Spacer(Modifier.height(4.dp))
                        OutlinedCard(
                            onClick = { expanded = true },
                            modifier = Modifier.fillMaxWidth(),
                            shape = RoundedCornerShape(16.dp)
                        ) {
                            Row(modifier = Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                                Text(profiles[selectedProfileIndex].name, modifier = Modifier.weight(1f), fontWeight = FontWeight.Medium)
                                Icon(Icons.Default.KeyboardArrowDown, null)
                            }
                        }
                        DropdownMenu(expanded = expanded, onDismissRequest = { expanded = false }) {
                            profiles.forEachIndexed { index, profile ->
                                DropdownMenuItem(
                                    text = { Text(profile.name) },
                                    onClick = { onProfileSelect(index); expanded = false }
                                )
                            }
                        }
                    }

                    OutlinedTextField(
                        value = resolverUrl,
                        onValueChange = onUrlChange,
                        label = { Text("Resolver Endpoint") },
                        modifier = Modifier.fillMaxWidth(),
                        shape = RoundedCornerShape(16.dp),
                        singleLine = true,
                        enabled = selectedProfileIndex == profiles.size - 1 || profiles[selectedProfileIndex].url.isEmpty()
                    )
                    
                    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        OutlinedTextField(
                            value = bootstrapDns,
                            onValueChange = onBootstrapChange,
                            label = { Text("Bootstrap") },
                            modifier = Modifier.weight(1f),
                            shape = RoundedCornerShape(16.dp),
                            singleLine = true,
                            enabled = selectedProfileIndex == profiles.size - 1
                        )
                        OutlinedTextField(
                            value = listenPort,
                            onValueChange = onPortChange,
                            label = { Text("Port") },
                            modifier = Modifier.weight(0.7f),
                            shape = RoundedCornerShape(16.dp),
                            singleLine = true
                        )
                    }
                }
            }
            
            // Helpful hint instead of button
            Text(
                if (isRunning) "TAP SHIELD TO DISCONNECT" else "TAP SHIELD TO CONNECT", 
                style = MaterialTheme.typography.labelSmall,
                fontWeight = FontWeight.Bold, 
                color = if (isRunning) Color(0xFFE91E63).copy(alpha = 0.7f) else MaterialTheme.colorScheme.primary.copy(alpha = 0.7f),
                letterSpacing = 1.5.sp
            )
        }
    }

    @Composable
    fun LogScreen(logs: Array<String>) {
        val context = LocalContext.current
        val listState = androidx.compose.foundation.lazy.rememberLazyListState()
        
        LaunchedEffect(logs.size) {
            if (logs.isNotEmpty()) {
                listState.animateScrollToItem(logs.size - 1)
            }
        }

        Column(modifier = Modifier.fillMaxSize().padding(24.dp)) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Text("Network Activity", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold)
                Surface(
                    onClick = { saveLogsToFile(context, logs) },
                    color = MaterialTheme.colorScheme.secondaryContainer,
                    shape = CircleShape,
                    modifier = Modifier.size(40.dp)
                ) {
                    Box(contentAlignment = Alignment.Center) {
                        Icon(Icons.Default.FileDownload, null, modifier = Modifier.size(20.dp))
                    }
                }
            }
            Spacer(Modifier.height(20.dp))
            Card(
                modifier = Modifier.fillMaxSize().weight(1f), 
                shape = RoundedCornerShape(24.dp),
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.3f))
            ) {
                if (logs.isEmpty()) {
                    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                        Column(horizontalAlignment = Alignment.CenterHorizontally) {
                            Icon(Icons.Default.CloudQueue, null, modifier = Modifier.size(48.dp), tint = MaterialTheme.colorScheme.outline)
                            Spacer(Modifier.height(12.dp))
                            Text("No activity detected", color = MaterialTheme.colorScheme.outline)
                        }
                    }
                } else {
                    androidx.compose.foundation.lazy.LazyColumn(
                        state = listState,
                        modifier = Modifier.fillMaxSize().padding(16.dp),
                        contentPadding = PaddingValues(bottom = 16.dp)
                    ) {
                        items(logs.size) { index ->
                            Text(
                                text = logs[index],
                                style = androidx.compose.ui.text.TextStyle(
                                    fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace,
                                    fontSize = 12.sp
                                ),
                                modifier = Modifier.padding(vertical = 8.dp)
                            )
                            if (index < logs.size - 1) {
                                HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.5f))
                            }
                        }
                    }
                }
            }
        }
    }

    private fun saveLogsToFile(context: android.content.Context, logs: Array<String>) {
        if (logs.isEmpty()) return
        try {
            val fileName = "SafeDNS_Logs_${System.currentTimeMillis()}.txt"
            val downloadsDir = android.os.Environment.getExternalStoragePublicDirectory(android.os.Environment.DIRECTORY_DOWNLOADS)
            val file = java.io.File(downloadsDir, fileName)
            java.io.FileOutputStream(file).use { out ->
                logs.forEach { line -> out.write((line + "\n").toByteArray()) }
            }
            android.media.MediaScannerConnection.scanFile(context, arrayOf(file.absolutePath), null, null)
            android.widget.Toast.makeText(context, "Saved to Downloads", android.widget.Toast.LENGTH_SHORT).show()
        } catch (e: Exception) {
            Log.e("SafeDNS", "Failed to save logs", e)
        }
    }

    @Composable
    fun DrawerContent(
        themeMode: String,
        autoStart: Boolean,
        heartbeatEnabled: Boolean,
        heartbeatDomain: String,
        heartbeatInterval: String,
        onThemeChange: (String) -> Unit,
        onAutoStartChange: (Boolean) -> Unit,
        onHeartbeatChange: (Boolean) -> Unit,
        onHeartbeatDomainChange: (String) -> Unit,
        onHeartbeatIntervalChange: (String) -> Unit,
        onAboutClick: () -> Unit,
        onClose: () -> Unit
    ) {
        Column(modifier = Modifier.padding(24.dp).verticalScroll(rememberScrollState())) {
            Text("Preferences", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold)
            Spacer(modifier = Modifier.height(32.dp))
            
            Text("Appearance", style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
            Spacer(Modifier.height(12.dp))
            listOf("Light", "Dark", "AMOLED", "System").forEach { mode ->
                NavigationDrawerItem(
                    label = { Text(mode) },
                    selected = themeMode == mode,
                    onClick = { onThemeChange(mode) },
                    shape = RoundedCornerShape(16.dp)
                )
            }
            
            HorizontalDivider(modifier = Modifier.padding(vertical = 24.dp))
            
            Text("Service", style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
            Spacer(Modifier.height(16.dp))
            
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(modifier = Modifier.weight(1f)) {
                    Text("Auto-start", fontWeight = FontWeight.Bold)
                    Text("Enable protection on boot", style = MaterialTheme.typography.bodySmall)
                }
                Switch(checked = autoStart, onCheckedChange = onAutoStartChange)
            }
            
            Spacer(Modifier.height(24.dp))
            
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(modifier = Modifier.weight(1f)) {
                    Text("Latency Pulse", fontWeight = FontWeight.Bold)
                    Text("Keep stats fresh in background", style = MaterialTheme.typography.bodySmall)
                }
                Switch(checked = heartbeatEnabled, onCheckedChange = onHeartbeatChange)
            }
            
            if (heartbeatEnabled) {
                Spacer(Modifier.height(16.dp))
                OutlinedTextField(
                    value = heartbeatDomain,
                    onValueChange = onHeartbeatDomainChange,
                    label = { Text("Ping Domain") },
                    modifier = Modifier.fillMaxWidth(),
                    shape = RoundedCornerShape(16.dp),
                    singleLine = true
                )
                Spacer(Modifier.height(12.dp))
                OutlinedTextField(
                    value = heartbeatInterval,
                    onValueChange = onHeartbeatIntervalChange,
                    label = { Text("Interval (sec)") },
                    modifier = Modifier.fillMaxWidth(),
                    shape = RoundedCornerShape(16.dp),
                    singleLine = true,
                    keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(keyboardType = androidx.compose.ui.text.input.KeyboardType.Number)
                )
            }
            
            Spacer(Modifier.height(40.dp))
            NavigationDrawerItem(
                label = { Text("About Application") },
                selected = false,
                icon = { Icon(Icons.Default.Info, null) },
                onClick = { onAboutClick(); onClose() },
                shape = RoundedCornerShape(16.dp)
            )
        }
    }

    @Composable
    fun StatusHero(isRunning: Boolean, latency: Int, onToggle: () -> Unit) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Box(contentAlignment = Alignment.Center) {
                val color = if (isRunning) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant
                
                // Outer static ring
                Box(Modifier.size(220.dp).clip(CircleShape).background(color.copy(alpha = 0.05f)))
                
                // Animated pulse
                if (isRunning) {
                    val infiniteTransition = rememberInfiniteTransition()
                    val pulseAlpha by infiniteTransition.animateFloat(
                        initialValue = 0.1f,
                        targetValue = 0.3f,
                        animationSpec = infiniteRepeatable(
                            animation = tween(2000, easing = LinearEasing),
                            repeatMode = RepeatMode.Reverse
                        )
                    )
                    val pulseScale by infiniteTransition.animateFloat(
                        initialValue = 1f,
                        targetValue = 1.2f,
                        animationSpec = infiniteRepeatable(
                            animation = tween(1500, easing = FastOutSlowInEasing),
                            repeatMode = RepeatMode.Reverse
                        )
                    )
                    Box(
                        Modifier
                            .size(160.dp)
                            .scale(pulseScale)
                            .clip(CircleShape)
                            .background(color.copy(alpha = pulseAlpha))
                    )
                }

                Surface(
                    onClick = onToggle,
                    shape = CircleShape,
                    color = if (isRunning) MaterialTheme.colorScheme.primaryContainer else color.copy(alpha = 0.1f),
                    modifier = Modifier.size(140.dp).shadow(if (isRunning) 8.dp else 0.dp, CircleShape)
                ) {
                    Box(contentAlignment = Alignment.Center) {
                        Icon(
                            imageVector = Icons.Default.Shield,
                            contentDescription = null,
                            modifier = Modifier.size(72.dp),
                            tint = if (isRunning) MaterialTheme.colorScheme.onPrimaryContainer else color
                        )
                    }
                }
            }
            
            Spacer(modifier = Modifier.height(32.dp))
            Text(
                if (isRunning) "SYSTEM PROTECTED" else "UNPROTECTED", 
                style = MaterialTheme.typography.titleMedium, 
                fontWeight = FontWeight.ExtraBold,
                letterSpacing = 2.sp,
                color = if (isRunning) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline
            )
            
            if (isRunning) {
                Spacer(Modifier.height(12.dp))
                Surface(
                    color = (if (latency < 150) Color(0xFF4CAF50) else Color(0xFFFFC107)).copy(alpha = 0.1f),
                    shape = RoundedCornerShape(8.dp)
                ) {
                    Row(modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                        Box(Modifier.size(8.dp).background(if (latency < 150) Color(0xFF4CAF50) else Color(0xFFFFC107), CircleShape))
                        Spacer(Modifier.width(8.dp))
                        Text("${latency}ms", style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.Bold)
                    }
                }
            }
        }
    }

    @Composable
    fun AboutDialog(onDismiss: () -> Unit, uriHandler: androidx.compose.ui.platform.UriHandler) {
        val context = LocalContext.current
        val scope = rememberCoroutineScope()
        var checkingUpdate by remember { mutableStateOf(false) }

        AlertDialog(
            onDismissRequest = onDismiss,
            title = { Text("About SafeDNS") },
            text = {
                Column {
                    Text("Version: v0.2.0", fontWeight = FontWeight.Bold)
                    Text("Developer: sms1sis")
                    Spacer(Modifier.height(16.dp))
                    
                    Button(
                        onClick = {
                            scope.launch {
                                checkingUpdate = true
                                checkForUpdates(context, uriHandler)
                                checkingUpdate = false
                            }
                        },
                        modifier = Modifier.fillMaxWidth(),
                        enabled = !checkingUpdate,
                        shape = RoundedCornerShape(12.dp)
                    ) {
                        if (checkingUpdate) {
                            CircularProgressIndicator(modifier = Modifier.size(18.dp), strokeWidth = 2.dp, color = MaterialTheme.colorScheme.onPrimary)
                            Spacer(Modifier.width(8.dp))
                            Text("Checking...")
                        } else {
                            Icon(Icons.Default.Update, null, modifier = Modifier.size(18.dp))
                            Spacer(Modifier.width(8.dp))
                            Text("Check for Updates")
                        }
                    }

                    Spacer(Modifier.height(8.dp))
                    TextButton(onClick = { uriHandler.openUri("https://github.com/sms1sis/https_dns_proxy_rust/tree/android-app") }) {
                        Text("View on GitHub")
                    }
                }
            },
            confirmButton = { TextButton(onClick = onDismiss) { Text("Close") } }
        )
    }

    private suspend fun checkForUpdates(context: android.content.Context, uriHandler: androidx.compose.ui.platform.UriHandler) {
        val repoUrl = "https://api.github.com/repos/sms1sis/https_dns_proxy_rust/releases/latest"
        val currentVersion = "v0.2.0"

        kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) {
            try {
                val connection = java.net.URL(repoUrl).openConnection() as java.net.HttpURLConnection
                connection.requestMethod = "GET"
                connection.setRequestProperty("Accept", "application/vnd.github.v3+json")
                connection.connectTimeout = 5000
                connection.readTimeout = 5000

                if (connection.responseCode == 200) {
                    val response = connection.inputStream.bufferedReader().use { it.readText() }
                    // Simple JSON parsing for tag_name
                    val tagName = response.substringAfter("\"tag_name\":\"").substringBefore("\"")
                    
                    kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.Main) {
                        if (tagName != currentVersion && tagName.startsWith("v")) {
                            android.widget.Toast.makeText(context, "New version available: $tagName", android.widget.Toast.LENGTH_LONG).show()
                            uriHandler.openUri("https://github.com/sms1sis/https_dns_proxy_rust/releases/latest")
                        } else {
                            android.widget.Toast.makeText(context, "You are on the latest version", android.widget.Toast.LENGTH_SHORT).show()
                        }
                    }
                }
            } catch (e: Exception) {
                kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.Main) {
                    android.widget.Toast.makeText(context, "Failed to check for updates", android.widget.Toast.LENGTH_SHORT).show()
                }
            }
        }
    }

    private fun startProxyService(resolverUrl: String, listenPort: String, bootstrapDns: String, heartbeatEnabled: Boolean, heartbeatDomain: String, heartbeatInterval: String) {
        val intent = Intent(this, ProxyService::class.java).apply {
            putExtra("resolverUrl", resolverUrl)
            putExtra("listenPort", listenPort.toIntOrNull() ?: 5053)
            putExtra("bootstrapDns", bootstrapDns)
            putExtra("heartbeatEnabled", heartbeatEnabled)
            putExtra("heartbeatDomain", heartbeatDomain)
            putExtra("heartbeatInterval", heartbeatInterval.toLongOrNull() ?: 10L)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) startForegroundService(intent)
        else startService(intent)
    }
}

data class DnsProfile(val name: String, val url: String, val bootstrap: String)