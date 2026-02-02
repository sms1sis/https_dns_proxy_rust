package io.github.https_dns_proxy_rust

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.VpnService
import android.os.Build
import android.os.Bundle
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
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CenterAlignedTopAppBar
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.FloatingActionButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.surfaceColorAtElevation
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
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
        val context = LocalContext.current
        var isRunning by remember { mutableStateOf(false) }
        var resolverUrl by remember { mutableStateOf("https://dns.google/dns-query") }
        var listenPort by remember { mutableStateOf("5053") }
        var bootstrapDns by remember { mutableStateOf("8.8.8.8,1.1.1.1") }

        val vpnPermissionLauncher = rememberLauncherForActivityResult(
            ActivityResultContracts.StartActivityForResult()
        ) { result ->
            if (result.resultCode == RESULT_OK) {
                startProxyService(resolverUrl, listenPort, bootstrapDns)
                isRunning = true
            }
        }

        val permissionLauncher = rememberLauncherForActivityResult(
            ActivityResultContracts.RequestPermission()
        ) { _ -> }

        LaunchedEffect(Unit) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                if (ContextCompat.checkSelfPermission(
                        context,
                        Manifest.permission.POST_NOTIFICATIONS
                    ) != PackageManager.PERMISSION_GRANTED
                ) {
                    permissionLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
                }
            }
        }

        Scaffold(
            topBar = {
                CenterAlignedTopAppBar(
                    title = {
                        Text(
                            "SafeDNS",
                            fontWeight = FontWeight.ExtraBold,
                            letterSpacing = 1.sp
                        )
                    },
                    colors = TopAppBarDefaults.centerAlignedTopAppBarColors(
                        containerColor = MaterialTheme.colorScheme.background
                    )
                )
            }
        ) { padding ->
            Column(
                modifier = Modifier
                    .padding(padding)
                    .padding(horizontal = 20.dp)
                    .fillMaxSize()
                    .verticalScroll(rememberScrollState()),
                horizontalAlignment = Alignment.CenterHorizontally
            ) {
                Spacer(modifier = Modifier.height(24.dp))

                StatusHero(isRunning)

                Spacer(modifier = Modifier.height(40.dp))

                Card(
                    modifier = Modifier.fillMaxWidth(),
                    shape = MaterialTheme.shapes.extraLarge,
                    colors = CardDefaults.cardColors(
                        containerColor = MaterialTheme.colorScheme.surfaceColorAtElevation(2.dp)
                    )
                ) {
                    Column(
                        modifier = Modifier.padding(24.dp),
                        verticalArrangement = Arrangement.spacedBy(20.dp)
                    ) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Icon(Icons.Filled.Settings, null, tint = MaterialTheme.colorScheme.primary)
                            Spacer(Modifier.width(12.dp))
                            Text(
                                "Configuration",
                                style = MaterialTheme.typography.titleLarge,
                                fontWeight = FontWeight.Bold
                            )
                        }

                        OutlinedTextField(
                            value = resolverUrl,
                            onValueChange = { resolverUrl = it },
                            label = { Text("DoH Resolver URL") },
                            modifier = Modifier.fillMaxWidth(),
                            shape = MaterialTheme.shapes.large
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
                                shape = MaterialTheme.shapes.large
                            )
                            OutlinedTextField(
                                value = bootstrapDns,
                                onValueChange = { bootstrapDns = it },
                                label = { Text("Bootstrap") },
                                modifier = Modifier.weight(2f),
                                shape = MaterialTheme.shapes.large
                            )
                        }
                    }
                }

                Spacer(modifier = Modifier.height(48.dp))

                FloatingActionButton(
                    onClick = {
                        if (isRunning) {
                            val stopIntent = Intent(this@MainActivity, ProxyService::class.java).apply {
                                action = "STOP"
                            }
                            startService(stopIntent)
                            isRunning = false
                        } else {
                            val vpnIntent = VpnService.prepare(this@MainActivity)
                            if (vpnIntent != null) {
                                vpnPermissionLauncher.launch(vpnIntent)
                            } else {
                                startProxyService(resolverUrl, listenPort, bootstrapDns)
                                isRunning = true
                            }
                        }
                    },
                    modifier = Modifier.size(80.dp),
                    shape = CircleShape,
                    containerColor = if (isRunning) MaterialTheme.colorScheme.errorContainer else MaterialTheme.colorScheme.primaryContainer,
                    contentColor = if (isRunning) MaterialTheme.colorScheme.onErrorContainer else MaterialTheme.colorScheme.onPrimaryContainer,
                    elevation = FloatingActionButtonDefaults.elevation(defaultElevation = 0.dp, pressedElevation = 8.dp)
                ) {
                    AnimatedContent(
                        targetState = isRunning,
                        transitionSpec = {
                            scaleIn() + fadeIn() togetherWith scaleOut() + fadeOut()
                        }
                    ) { running ->
                        Icon(
                            imageVector = if (running) Icons.Filled.Stop else Icons.Filled.PlayArrow,
                            contentDescription = null,
                            modifier = Modifier.size(36.dp)
                        )
                    }
                }
                
                Spacer(modifier = Modifier.height(16.dp))
                
                Text(
                    text = if (isRunning) "STOP" else "START",
                    style = MaterialTheme.typography.labelLarge,
                    fontWeight = FontWeight.Black,
                    color = if (isRunning) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary
                )

                Spacer(modifier = Modifier.height(32.dp))
            }
        }
    }

    private fun startProxyService(resolverUrl: String, listenPort: String, bootstrapDns: String) {
        val intent = Intent(this, ProxyService::class.java).apply {
            putExtra("resolverUrl", resolverUrl)
            putExtra("listenPort", listenPort.toIntOrNull() ?: 5053)
            putExtra("bootstrapDns", bootstrapDns)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            startForegroundService(intent)
        } else {
            startService(intent)
        }
    }

    @Composable
    fun StatusHero(isRunning: Boolean) {
        val color = if (isRunning) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant
        val alpha by animateFloatAsState(if (isRunning) 1f else 0.3f)

        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Box(
                modifier = Modifier
                    .size(160.dp)
                    .clip(CircleShape)
                    .background(color.copy(alpha = 0.1f)),
                contentAlignment = Alignment.Center
            ) {
                Icon(
                    imageVector = if (isRunning) Icons.Filled.CheckCircle else Icons.Filled.Warning,
                    contentDescription = null,
                    modifier = Modifier.size(80.dp),
                    tint = color.copy(alpha = alpha)
                )
            }
            
            Spacer(modifier = Modifier.height(24.dp))
            
            Text(
                text = if (isRunning) "Protected" else "Unprotected",
                style = MaterialTheme.typography.headlineMedium,
                fontWeight = FontWeight.ExtraBold,
                color = if (isRunning) MaterialTheme.colorScheme.onBackground else MaterialTheme.colorScheme.outline
            )
            
            Text(
                text = if (isRunning) "DNS queries are encrypted" else "Tap start to encrypt your DNS",
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant
            )
        }
    }
}
