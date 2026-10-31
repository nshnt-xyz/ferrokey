package xyz.nshnt.ferrokey

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
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
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.PointerInputChange
import androidx.compose.ui.input.pointer.PointerInputScope
import androidx.compose.ui.input.pointer.changedToUp
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.unit.dp
import xyz.nshnt.ferrokey.core.InputEvent
import xyz.nshnt.ferrokey.core.MouseButton
import kotlin.math.roundToInt

/** A touchpad surface plus left/right buttons, connected to [address]. */
@Composable
fun TouchpadScreen(address: String, send: (InputEvent) -> Unit, onDisconnect: () -> Unit) {
    val click = { button: MouseButton ->
        send(InputEvent.MouseButton(button, pressed = true))
        send(InputEvent.MouseButton(button, pressed = false))
    }
    val haptics = LocalHapticFeedback.current
    var holding by remember { mutableStateOf(false) }

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
                        // Carry sub-pixel remainders over so slow drags still move.
                        var remainder = Offset.Zero
                        touchpadGestures(
                            onTap = { click(MouseButton.LEFT) },
                            onHoldStart = {
                                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                holding = true
                                send(InputEvent.MouseButton(MouseButton.LEFT, pressed = true))
                            },
                            onHoldEnd = {
                                holding = false
                                send(InputEvent.MouseButton(MouseButton.LEFT, pressed = false))
                            },
                            onMove = { delta ->
                                val total = remainder + delta
                                val dx = total.x.roundToInt()
                                val dy = total.y.roundToInt()
                                remainder = total - Offset(dx.toFloat(), dy.toFloat())
                                if (dx != 0 || dy != 0) send(InputEvent.MouseMove(dx, dy))
                            },
                            onGestureEnd = { remainder = Offset.Zero },
                        )
                    },
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    text = if (holding) "Holding" else "Touchpad",
                    color = if (holding) {
                        MaterialTheme.colorScheme.primary
                    } else {
                        MaterialTheme.colorScheme.onSurfaceVariant
                    },
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                FilledTonalButton(onClick = { click(MouseButton.LEFT) }, modifier = Modifier.weight(1f)) {
                    Text("Left")
                }
                FilledTonalButton(onClick = { click(MouseButton.RIGHT) }, modifier = Modifier.weight(1f)) {
                    Text("Right")
                }
            }
        }
    }
}

private enum class Start { Tap, Drag, Hold }

/**
 * Touchpad gestures: a quick tap calls [onTap]; moving a finger calls [onMove];
 * touching and holding still for the long-press timeout calls [onHoldStart],
 * after which moving drags and lifting the finger calls [onHoldEnd].
 */
private suspend fun PointerInputScope.touchpadGestures(
    onTap: () -> Unit,
    onHoldStart: () -> Unit,
    onHoldEnd: () -> Unit,
    onMove: (Offset) -> Unit,
    onGestureEnd: () -> Unit,
) = awaitEachGesture {
    var pointer = awaitFirstDown(requireUnconsumed = false).id
    var travelled = Offset.Zero

    // Returns the tracked pointer's change, switching to another finger if it
    // lifts while others are still down, or null once every finger is up.
    fun track(changes: List<PointerInputChange>): PointerInputChange? {
        val change = changes.firstOrNull { it.id == pointer } ?: return null
        if (!change.changedToUp()) return change
        val next = changes.firstOrNull { it.pressed } ?: return null
        pointer = next.id
        return next
    }

    // Decide what this gesture is: lifted before the timeout (tap), moved past
    // the touch slop (drag), or neither (hold).
    val start = withTimeoutOrNull(viewConfiguration.longPressTimeoutMillis) {
        var decided: Start? = null
        while (decided == null) {
            val change = track(awaitPointerEvent().changes)
            if (change == null) {
                decided = Start.Tap
            } else {
                travelled += change.positionChange()
                if (travelled.getDistance() > viewConfiguration.touchSlop) {
                    change.consume()
                    decided = Start.Drag
                }
            }
        }
        decided
    } ?: Start.Hold

    if (start == Start.Tap) {
        onTap()
        return@awaitEachGesture
    }

    if (start == Start.Hold) onHoldStart() else onMove(travelled)
    try {
        while (true) {
            val change = track(awaitPointerEvent().changes) ?: break
            val delta = change.positionChange()
            change.consume()
            if (delta != Offset.Zero) onMove(delta)
        }
    } finally {
        // Also runs if the gesture is cancelled, so a held button never sticks.
        if (start == Start.Hold) onHoldEnd()
        onGestureEnd()
    }
}
