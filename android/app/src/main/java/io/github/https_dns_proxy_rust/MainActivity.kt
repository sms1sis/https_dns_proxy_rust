package io.github.https_dns_proxy_rust

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.https_dns_proxy_rust.ui.theme.HttpsDnsProxyTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            HttpsDnsProxyTheme {
                Surface(
                    modifier = Modifier.fillMaxSize(),
                    color = MaterialTheme.colorScheme.background
                ) {
                    ProxyScreen()
                }
            }
        }
    }

    @OptIn(ExperimentalMaterial3Api::class)
    @Composable
    fun ProxyScreen() {
        var isRunning by remember { mutableStateOf(false) }
        var resolverUrl by remember { mutableStateOf("https://dns.google/dns-query") }
        var listenPort by remember { mutableStateOf("5053") }
        var bootstrapDns by remember { mutableStateOf("8.8.8.8,1.1.1.1") }

        Scaffold(
            topBar = {
                LargeTopAppBar(
                    title = {
                        Text(
                            "DNS-over-HTTPS",
                            fontWeight = FontWeight.Bold
                        )
                    }
                )
            }
        ) { padding ->
            Column(
                modifier = Modifier
                    .padding(padding)
                    .padding(16.dp)
                    .verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(24.dp)
            ) {
                // Status Card
                StatusCard(isRunning)

                // Configuration Section
                Text(
                    "Configuration",
                    style = MaterialTheme.typography.titleMedium,
                    color = MaterialTheme.colorScheme.primary
                )

                OutlinedTextField(
                    value = resolverUrl,
                    onValueChange = { resolverUrl = it },
                    label = { Text("Resolver URL") },
                    modifier = Modifier.fillMaxWidth(),
                    shape = MaterialTheme.shapes.medium
                )

                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(16.dp)
                ) {
                    OutlinedTextField(
                        value = listenPort,
                        onValueChange = { listenPort = it },
                        label = { Text("Port") },
                        modifier = Modifier.weight(1f),
                        shape = MaterialTheme.shapes.medium
                    )
                    OutlinedTextField(
                        value = bootstrapDns,
                        onValueChange = { bootstrapDns = it },
                        label = { Text("Bootstrap DNS") },
                        modifier = Modifier.weight(2f),
                        shape = MaterialTheme.shapes.medium
                    )
                }

                Spacer(modifier = Modifier.weight(1f))

                // Control Button
                Button(
                    onClick = {
                        if (isRunning) {
                            stopService(Intent(this@MainActivity, ProxyService::class.java))
                        } else {
                            val intent = Intent(this@MainActivity, ProxyService::class.java).apply {
                                putExtra("resolverUrl", resolverUrl)
                                putExtra("listenPort", listenPort.toIntOrNull() ?: 5053)
                                putExtra("bootstrapDns", bootstrapDns)
                            }
                            startForegroundService(intent)
                        }
                        isRunning = !isRunning
                    },
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(64.dp),
                    colors = ButtonDefaults.buttonColors(
                        containerColor = if (isRunning) MaterialTheme.colorScheme.errorContainer else MaterialTheme.colorScheme.primary,
                        contentColor = if (isRunning) MaterialTheme.colorScheme.onErrorContainer else MaterialTheme.colorScheme.onPrimary
                    ),
                    shape = MaterialTheme.shapes.extraLarge
                ) {
                    Icon(
                        if (isRunning) Icons.Default.Stop else Icons.Default.PlayArrow,
                        contentDescription = null
                    )
                    Spacer(Modifier.width(8.dp))
                    Text(
                        if (isRunning) "Stop Proxy" else "Start Proxy",
                        fontSize = 18.sp,
                        fontWeight = FontWeight.SemiBold
                    )
                }
            }
        }
    }

    @Composable
    fun StatusCard(isRunning: Boolean) {
        Card(
            modifier = Modifier.fillMaxWidth(),
            colors = CardDefaults.cardColors(
                containerColor = if (isRunning) 
                    MaterialTheme.colorScheme.primaryContainer 
                else 
                    MaterialTheme.colorScheme.surfaceVariant
            ),
            shape = MaterialTheme.shapes.extraLarge
        ) {
            Row(
                modifier = Modifier
                    .padding(24.dp)
                    .fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically
            ) {
                Column(modifier = Modifier.weight(1f)) {
                    Text(
                        if (isRunning) "Service Active" else "Service Inactive",
                        style = MaterialTheme.typography.headlineSmall,
                        fontWeight = FontWeight.Bold
                    )
                    Text(
                        if (isRunning) "Your DNS traffic is protected" else "Tap start to enable protection",
                        style = MaterialTheme.typography.bodyMedium
                    )
                }
            }
        }
    }
}
