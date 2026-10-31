package xyz.nshnt.ferrokey

import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewmodel.compose.viewModel

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            FerrokeyTheme {
                FerrokeyApp(viewModel())
            }
        }
    }

    // Don't stream the screen to an app nobody can see.
    override fun onStart() {
        super.onStart()
        model().setVisible(true)
    }

    override fun onStop() {
        super.onStop()
        model().setVisible(false)
    }

    /** The same model [FerrokeyApp] gets from `viewModel()`. */
    private fun model() = ViewModelProvider(this)[ClientViewModel::class.java]
}

@Composable
private fun FerrokeyApp(model: ClientViewModel) {
    val address = model.connectedTo
    if (address == null) {
        ConnectScreen(model)
    } else {
        TouchpadScreen(
            address,
            send = model::send,
            typeText = model::typeText,
            tapKey = model::tapKey,
            onDisconnect = { model.disconnect() },
            screen = ScreenState(
                available = model.screenAvailable,
                watching = model.watching,
                frame = model.screen,
                start = model::startScreen,
                stop = model::stopScreen,
                pointAt = model::pointAt,
            ),
        )
    }
}

@Composable
private fun FerrokeyTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val colorScheme = when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S -> {
            val context = LocalContext.current
            if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        }
        dark -> darkColorScheme()
        else -> lightColorScheme()
    }
    MaterialTheme(colorScheme = colorScheme, content = content)
}
