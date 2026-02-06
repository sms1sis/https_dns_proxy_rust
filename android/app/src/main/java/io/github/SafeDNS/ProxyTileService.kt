package io.github.SafeDNS

import android.content.Intent
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService

class ProxyTileService : TileService() {

    override fun onStartListening() {
        super.onStartListening()
        updateTile()
    }

    override fun onClick() {
        super.onClick()
        if (ProxyService.isProxyRunning) {
            val intent = Intent(this, ProxyService::class.java).apply {
                action = "STOP"
            }
            startService(intent)
        } else {
            val intent = Intent(this, ProxyService::class.java)
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) {
                startForegroundService(intent)
            } else {
                startService(intent)
            }
        }
        android.os.Handler(mainLooper).postDelayed({
            updateTile()
        }, 500)
    }

    private fun updateTile() {
        val tile = qsTile ?: return
        val isRunning = ProxyService.isProxyRunning
        
        tile.state = if (isRunning) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
        tile.label = getString(R.string.tile_label)
        
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.Q) {
            tile.subtitle = if (isRunning) "Active" else "Inactive"
        }
        
        tile.updateTile()
    }
}
