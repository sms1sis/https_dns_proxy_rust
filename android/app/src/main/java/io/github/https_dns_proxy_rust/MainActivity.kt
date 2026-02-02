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
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
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

        // DNS Profiles
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
                        title = { Text("SafeDNS", fontWeight = FontWeight.ExtraBold) },
                        navigationIcon = {
                            IconButton(onClick = { scope.launch { drawerState.open() } }) {
                                Icon(Icons.Filled.Menu, "Menu")
                            }
                        },
                        colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background)
                    )
                },
                bottomBar = {
                    NavigationBar {
                        NavigationBarItem(
                            icon = { Icon(Icons.Filled.Home, null) },
                            label = { Text("Dashboard") },
                            selected = currentTab == 0,
                            onClick = { currentTab = 0 }
                        )
                        NavigationBarItem(
                            icon = { Icon(Icons.Filled.History, null) },
                            label = { Text("Logs") },
                            selected = currentTab == 1,
                            onClick = { currentTab = 1 }
                        )
                    }
                }
            ) { padding ->
                Box(modifier = Modifier.padding(padding)) {
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
                            onToggle = {
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
                .padding(horizontal = 20.dp)
                .verticalScroll(rememberScrollState()),
            horizontalAlignment = Alignment.CenterHorizontally
        ) {
            Spacer(modifier = Modifier.height(24.dp))
            StatusHero(isRunning, latency)
            Spacer(modifier = Modifier.height(32.dp))

            Card(
                modifier = Modifier.fillMaxWidth(),
                shape = RoundedCornerShape(24.dp),
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.5f))
            ) {
                Column(modifier = Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    Text("DNS Profile", style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
                    
                    var expanded by remember { mutableStateOf(false) }
                    Box {
                        OutlinedCard(
                            onClick = { expanded = true },
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            Row(modifier = Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                                Text(profiles[selectedProfileIndex].name, modifier = Modifier.weight(1f))
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
                        label = { Text("DoH URL") },
                        modifier = Modifier.fillMaxWidth(),
                        enabled = selectedProfileIndex == profiles.size - 1 || profiles[selectedProfileIndex].url.isEmpty()
                    )
                    
                    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        OutlinedTextField(
                            value = bootstrapDns,
                            onValueChange = onBootstrapChange,
                            label = { Text("Bootstrap") },
                            modifier = Modifier.weight(1f),
                            enabled = selectedProfileIndex == profiles.size - 1
                        )
                        OutlinedTextField(
                            value = listenPort,
                            onValueChange = onPortChange,
                            label = { Text("Port") },
                            modifier = Modifier.weight(0.6f)
                        )
                    }
                }
            }

            Spacer(modifier = Modifier.height(40.dp))
            
            FloatingActionButton(
                onClick = onToggle,
                modifier = Modifier.size(80.dp),
                shape = CircleShape,
                containerColor = if (isRunning) MaterialTheme.colorScheme.errorContainer else MaterialTheme.colorScheme.primaryContainer
            ) {
                Icon(
                    imageVector = if (isRunning) Icons.Filled.Stop else Icons.Filled.PlayArrow,
                    contentDescription = null,
                    modifier = Modifier.size(36.dp)
                )
            }
            Spacer(modifier = Modifier.height(16.dp))
            Text(if (isRunning) "STOP" else "START", fontWeight = FontWeight.Black, color = if (isRunning) Color.Red else MaterialTheme.colorScheme.primary)
            Spacer(modifier = Modifier.height(24.dp))
        }
    }

    @Composable
    fun LogScreen(logs: Array<String>) {
        val context = LocalContext.current
        val listState = androidx.compose.foundation.lazy.rememberLazyListState()
        
        // Auto-scroll to bottom when logs change
        LaunchedEffect(logs.size) {
            if (logs.isNotEmpty()) {
                listState.animateScrollToItem(logs.size - 1)
            }
        }

        Column(modifier = Modifier.fillMaxSize().padding(16.dp)) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Text("Query Logs", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
                IconButton(onClick = { saveLogsToFile(context, logs) }) {
                    Icon(Icons.Default.Save, contentDescription = "Save Logs")
                }
            }
            Spacer(Modifier.height(16.dp))
            Card(modifier = Modifier.fillMaxSize().weight(1f), shape = RoundedCornerShape(16.dp)) {
                if (logs.isEmpty()) {
                    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                        Text("No logs yet. Start the service to see queries.")
                    }
                } else {
                    androidx.compose.foundation.lazy.LazyColumn(
                        state = listState,
                        modifier = Modifier.fillMaxSize().padding(12.dp),
                        contentPadding = PaddingValues(bottom = 16.dp)
                    ) {
                        items(logs.size) { index ->
                            Text(
                                text = logs[index],
                                style = MaterialTheme.typography.bodySmall,
                                modifier = Modifier.padding(vertical = 6.dp)
                            )
                            if (index < logs.size - 1) {
                                HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.3f))
                            }
                        }
                    }
                }
            }
        }
    }

    private fun saveLogsToFile(context: android.content.Context, logs: Array<String>) {
        if (logs.isEmpty()) {
            android.widget.Toast.makeText(context, "No logs to save", android.widget.Toast.LENGTH_SHORT).show()
            return
        }

        try {
            val fileName = "SafeDNS_Logs_${System.currentTimeMillis()}.txt"
            val downloadsDir = android.os.Environment.getExternalStoragePublicDirectory(android.os.Environment.DIRECTORY_DOWNLOADS)
            val file = java.io.File(downloadsDir, fileName)
            
            java.io.FileOutputStream(file).use { out ->
                logs.forEach { line ->
                    out.write((line + "\n").toByteArray())
                }
            }

            // Tell the OS to index the new file so it appears in file managers
            android.media.MediaScannerConnection.scanFile(context, arrayOf(file.absolutePath), null, null)
            
            android.widget.Toast.makeText(context, "Logs saved to Downloads/$fileName", android.widget.Toast.LENGTH_LONG).show()
        } catch (e: Exception) {
            Log.e("SafeDNS", "Failed to save logs", e)
            android.widget.Toast.makeText(context, "Failed to save logs: ${e.message}", android.widget.Toast.LENGTH_SHORT).show()
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
        Column(modifier = Modifier.padding(16.dp).verticalScroll(rememberScrollState())) {
            Text("Settings", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
            Spacer(Modifier.height(24.dp))
            
            Text("Theme", style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
            listOf("Light", "Dark", "AMOLED", "System").forEach { mode ->
                NavigationDrawerItem(
                    label = { Text(mode) },
                    selected = themeMode == mode,
                    onClick = { onThemeChange(mode) }
                )
            }
            
            HorizontalDivider(modifier = Modifier.padding(vertical = 16.dp))
            
            Text("Advanced", style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
            Spacer(Modifier.height(8.dp))
            
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(modifier = Modifier.weight(1f)) {
                    Text("Auto-start on Boot", fontWeight = FontWeight.Medium)
                    Text("Start service when device reboots", style = MaterialTheme.typography.bodySmall)
                }
                Switch(checked = autoStart, onCheckedChange = onAutoStartChange)
            }
            
            Spacer(Modifier.height(16.dp))
            
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(modifier = Modifier.weight(1f)) {
                    Text("Latency Heartbeat", fontWeight = FontWeight.Medium)
                    Text("Periodic DNS ping for real-time stats", style = MaterialTheme.typography.bodySmall)
                }
                Switch(checked = heartbeatEnabled, onCheckedChange = onHeartbeatChange)
            }
            
            if (heartbeatEnabled) {
                Spacer(Modifier.height(8.dp))
                OutlinedTextField(
                    value = heartbeatDomain,
                    onValueChange = onHeartbeatDomainChange,
                    label = { Text("Heartbeat Domain") },
                    modifier = Modifier.fillMaxWidth(),
                    shape = RoundedCornerShape(12.dp),
                    singleLine = true
                )
                Spacer(Modifier.height(8.dp))
                OutlinedTextField(
                    value = heartbeatInterval,
                    onValueChange = onHeartbeatIntervalChange,
                    label = { Text("Interval (seconds)") },
                    modifier = Modifier.fillMaxWidth(),
                    shape = RoundedCornerShape(12.dp),
                    singleLine = true,
                    keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(keyboardType = androidx.compose.ui.text.input.KeyboardType.Number)
                )
            }
            
            Spacer(Modifier.height(32.dp))
            NavigationDrawerItem(
                label = { Text("About") },
                selected = false,
                icon = { Icon(Icons.Default.Info, null) },
                onClick = { onAboutClick(); onClose() }
            )
        }
    }

    @Composable
    fun StatusHero(isRunning: Boolean, latency: Int) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Box(contentAlignment = Alignment.Center) {
                val color = if (isRunning) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant
                Box(Modifier.size(160.dp).clip(CircleShape).background(color.copy(alpha = 0.1f)))
                Icon(
                    imageVector = if (isRunning) Icons.Default.CheckCircle else Icons.Default.Warning,
                    contentDescription = null,
                    modifier = Modifier.size(80.dp),
                    tint = color
                )
            }
            Spacer(modifier = Modifier.height(16.dp))
            Text(if (isRunning) "Protected" else "Unprotected", style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.ExtraBold)
            if (isRunning) {
                Text("Latency: ${latency}ms", color = if (latency < 100) Color.Green else if (latency < 250) Color.Yellow else Color.Red)
            }
        }
    }

    @Composable
    fun AboutDialog(onDismiss: () -> Unit, uriHandler: androidx.compose.ui.platform.UriHandler) {
        AlertDialog(
            onDismissRequest = onDismiss,
            title = { Text("About SafeDNS") },
            text = {
                Column {
                    Text("Version: v0.2.0", fontWeight = FontWeight.Bold)
                    Text("Developer: sms1sis")
                    Spacer(Modifier.height(16.dp))
                    TextButton(onClick = { uriHandler.openUri("https://github.com/sms1sis/https_dns_proxy_rust/tree/android-app") }) {
                        Text("View on GitHub")
                    }
                }
            },
            confirmButton = { TextButton(onClick = onDismiss) { Text("Close") } }
        )
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
