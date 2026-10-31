package xyz.nshnt.ferrokey

import android.app.Application
import android.graphics.BitmapFactory
import android.os.Build
import android.widget.Toast
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import xyz.nshnt.ferrokey.core.Client
import xyz.nshnt.ferrokey.core.ClientException
import xyz.nshnt.ferrokey.core.InputEvent
import xyz.nshnt.ferrokey.core.Modifiers
import xyz.nshnt.ferrokey.core.PairedServer
import xyz.nshnt.ferrokey.core.ScreenFrame
import xyz.nshnt.ferrokey.core.ScreenListener
import xyz.nshnt.ferrokey.core.SpecialKey
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

    /** Whether the connected server can share its screen. */
    var screenAvailable by mutableStateOf(false)
        private set

    /** Whether the user asked to see the screen. */
    var watching by mutableStateOf(false)
        private set

    /** The latest frame of the computer's screen, while [watching]. */
    var screen by mutableStateOf<ImageBitmap?>(null)
        private set

    /** Size frames were last asked for, to ask again after a pause. */
    private var screenSize = 0 to 0

    /** Whether frames are paused while the app is out of sight. */
    private var screenPaused = false

    /**
     * Counts screen starts and stops, so a frame decoded for an earlier
     * start isn't shown after a stop.
     */
    private var screenGeneration = 0

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
        screenAvailable = false
        stopWatching()
    }

    /** Starts showing the screen in frames of at most [width] by [height]. */
    fun startScreen(width: Int, height: Int) {
        if (!screenAvailable || width <= 0 || height <= 0) return
        screenSize = width to height
        watching = true
        screenPaused = false
        requestFrames()
    }

    fun stopScreen() {
        if (!watching) return
        stopWatching()
        sending { client.stopScreen() }
    }

    /** Pauses frames while the app is out of sight, and resumes them after. */
    fun setVisible(visible: Boolean) {
        if (!watching) return
        if (!visible && !screenPaused) {
            screenPaused = true
            screenGeneration++
            sending { client.stopScreen() }
        } else if (visible && screenPaused) {
            screenPaused = false
            requestFrames()
        }
    }

    /** Moves the pointer to a point on the screen, as fractions of its size. */
    fun pointAt(x: Float, y: Float) = sending { client.pointAt(x, y) }

    private fun stopWatching() {
        watching = false
        screenPaused = false
        screen = null
        screenGeneration++
    }

    private fun requestFrames() {
        val generation = ++screenGeneration
        val listener = object : ScreenListener {
            // Runs on the core's network thread. Decoding here, not on the
            // main thread, is what the server's flow control waits for.
            override fun onFrame(frame: ScreenFrame) {
                val bitmap = try {
                    BitmapFactory.decodeByteArray(frame.jpeg, 0, frame.jpeg.size)
                } catch (e: Exception) {
                    null
                } ?: return
                val image = bitmap.asImageBitmap()
                viewModelScope.launch(Dispatchers.Main) {
                    if (generation == screenGeneration) screen = image
                }
            }

            override fun onStopped() {
                viewModelScope.launch(Dispatchers.Main) {
                    if (generation == screenGeneration) {
                        stopWatching()
                        Toast.makeText(getApplication(), "The computer stopped sharing its screen", Toast.LENGTH_SHORT).show()
                    }
                }
            }
        }
        val (width, height) = screenSize
        sending { client.startScreen(width.toUInt(), height.toUInt(), listener) }
    }

    fun send(event: InputEvent) = sending { client.send(event) }

    /** Types [text], skipping characters a US keyboard layout can't type. */
    fun typeText(text: String, modifiers: Modifiers) = sending {
        if (client.typeText(text, modifiers) > 0u) {
            Toast.makeText(getApplication(), "Skipped characters a US keyboard can't type", Toast.LENGTH_SHORT).show()
        }
    }

    fun tapKey(key: SpecialKey, modifiers: Modifiers) = sending { client.tapKey(key, modifiers) }

    private inline fun sending(block: () -> Unit) {
        // Late events, such as a button release from a gesture cancelled by
        // disconnecting, are dropped; the server releases held buttons itself.
        if (connectedTo == null) return
        try {
            block()
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
                screenAvailable = client.screenAvailable()
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
