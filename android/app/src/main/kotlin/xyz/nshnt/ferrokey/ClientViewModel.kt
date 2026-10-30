package xyz.nshnt.ferrokey

import android.app.Application
import android.os.Build
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import xyz.nshnt.ferrokey.core.Client
import xyz.nshnt.ferrokey.core.ClientException
import xyz.nshnt.ferrokey.core.InputEvent
import xyz.nshnt.ferrokey.core.PairedServer
import java.io.File

/**
 * Owns the Rust [Client] so the connection survives configuration changes
 * such as rotation, and exposes its state to Compose.
 */
class ClientViewModel(application: Application) : AndroidViewModel(application) {
    private val client = Client(File(application.filesDir, "ferrokey").path, Build.MODEL)

    /** Address of the connected server, or null when disconnected. */
    var connectedTo by mutableStateOf<String?>(null)
        private set

    /** True while pairing or connecting. */
    var busy by mutableStateOf(false)
        private set

    var error by mutableStateOf<String?>(null)
        private set

    var pairedServers by mutableStateOf(client.pairedServers())
        private set

    init {
        // The client only notices a dead server when a write fails, which
        // its keepalives guarantee happens within a few seconds.
        viewModelScope.launch {
            while (true) {
                delay(1000)
                if (connectedTo != null && !client.isConnected()) disconnect("Connection lost")
            }
        }
    }

    fun pair(address: String, code: String) = open(address) { client.pair(address, code) }

    fun connect(address: String) = open(address) { client.connect(address) }

    fun forget(address: String) {
        try {
            client.forget(address)
        } catch (e: ClientException) {
            error = e.message
        }
        pairedServers = client.pairedServers()
    }

    fun disconnect(message: String? = null) {
        client.disconnect()
        connectedTo = null
        error = message
    }

    fun send(event: InputEvent) {
        try {
            client.send(event)
        } catch (e: ClientException) {
            disconnect("Connection lost")
        }
    }

    private fun open(address: String, block: () -> Unit) {
        if (busy) return
        busy = true
        error = null
        viewModelScope.launch {
            try {
                withContext(Dispatchers.IO) { block() }
                connectedTo = address.trim()
            } catch (e: ClientException) {
                error = e.message
            } finally {
                busy = false
                pairedServers = client.pairedServers()
            }
        }
    }

    override fun onCleared() {
        client.close()
    }
}
