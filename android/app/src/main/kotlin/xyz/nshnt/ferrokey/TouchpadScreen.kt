package xyz.nshnt.ferrokey

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.unit.dp
import xyz.nshnt.ferrokey.core.InputEvent
import xyz.nshnt.ferrokey.core.Modifiers
import xyz.nshnt.ferrokey.core.MouseButton
import xyz.nshnt.ferrokey.core.SpecialKey
import kotlin.math.roundToInt

/**
 * A touchpad surface plus left/right buttons, connected to [address]. The
 * keyboard button brings up the soft keyboard, whose typing goes to the
 * computer, along with a row of keys it lacks.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun TouchpadScreen(
    address: String,
    send: (InputEvent) -> Unit,
    typeText: (String, Modifiers) -> Unit,
    tapKey: (SpecialKey, Modifiers) -> Unit,
    onDisconnect: () -> Unit,
) {
    val click = { button: MouseButton ->
        send(InputEvent.MouseButton(button, pressed = true))
        send(InputEvent.MouseButton(button, pressed = false))
    }

    // Modifiers from the key bar apply to the next key only.
    var modifiers by remember { mutableStateOf(NoModifiers) }
    val type = { text: String ->
        typeText(text, modifiers)
        modifiers = NoModifiers
    }
    val tap = { key: SpecialKey ->
        tapKey(key, modifiers)
        modifiers = NoModifiers
    }

    val focusRequester = remember { FocusRequester() }
    val focusManager = LocalFocusManager.current
    val keyboardController = LocalSoftwareKeyboardController.current
    val keyboardVisible = WindowInsets.isImeVisible

    Surface(modifier = Modifier.fillMaxSize()) {
        Column(
            modifier = Modifier.safeDrawingPadding().padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    text = "Connected to $address",
                    style = MaterialTheme.typography.titleMedium,
                    modifier = Modifier.weight(1f),
                )
                TextButton(onClick = onDisconnect) { Text("Disconnect") }
                KeyboardCapture(
                    focusRequester,
                    onText = type,
                    onBackspace = { tap(SpecialKey.BACKSPACE) },
                )
            }
            Box(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .background(
                        MaterialTheme.colorScheme.surfaceVariant,
                        RoundedCornerShape(24.dp),
                    )
                    .pointerInput(Unit) {
                        detectTapGestures(onTap = { click(MouseButton.LEFT) })
                    }
                    .pointerInput(Unit) {
                        // Carry sub-pixel remainders over so slow drags still move.
                        var remainder = Offset.Zero
                        detectDragGestures(onDragEnd = { remainder = Offset.Zero }) { change, drag ->
                            change.consume()
                            val total = remainder + drag
                            val dx = total.x.roundToInt()
                            val dy = total.y.roundToInt()
                            remainder = total - Offset(dx.toFloat(), dy.toFloat())
                            if (dx != 0 || dy != 0) send(InputEvent.MouseMove(dx, dy))
                        }
                    },
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    text = "Touchpad",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                FilledTonalButton(onClick = { click(MouseButton.LEFT) }, modifier = Modifier.weight(1f)) {
                    Text("Left")
                }
                FilledTonalButton(onClick = { click(MouseButton.RIGHT) }, modifier = Modifier.weight(1f)) {
                    Text("Right")
                }
                FilledTonalButton(
                    onClick = {
                        if (keyboardVisible) {
                            keyboardController?.hide()
                            focusManager.clearFocus()
                        } else {
                            focusRequester.requestFocus()
                            keyboardController?.show()
                        }
                    },
                    modifier = Modifier.weight(1f),
                ) {
                    Text("Keyboard")
                }
            }
            if (keyboardVisible) {
                SpecialKeyBar(
                    modifiers,
                    onModifiersChange = { modifiers = it },
                    onKey = tap,
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        }
    }
}
