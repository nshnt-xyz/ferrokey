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
import androidx.compose.ui.input.pointer.PointerInputChange
import androidx.compose.ui.input.pointer.PointerInputScope
import androidx.compose.ui.input.pointer.changedToUp
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
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

/**
 * Touchpad gestures. Moving a finger past the touch slop calls [onMove];
 * lifting it without moving calls [onTap]. Touching again within the
 * double-tap timeout of a tap (tap and a half) calls [onHoldStart] right
 * away and [onHoldEnd] when that finger lifts, so moving in between drags.
 * A quick tap in hold mode presses and releases, so tapping twice still
 * double-clicks, and three times triple-clicks.
 */
private suspend fun PointerInputScope.touchpadGestures(
    onTap: () -> Unit,
    onHoldStart: () -> Unit,
    onHoldEnd: () -> Unit,
    onMove: (Offset) -> Unit,
    onGestureEnd: () -> Unit,
) {
    // When the last gesture that didn't move ended, or null if the last one moved.
    var lastTapUp: Long? = null

    awaitEachGesture {
        val down = awaitFirstDown(requireUnconsumed = false)
        val tapUp = lastTapUp
        val held = tapUp != null && down.uptimeMillis - tapUp <= viewConfiguration.doubleTapTimeoutMillis
        lastTapUp = null

        var pointer = down.id
        var travelled = Offset.Zero
        var moving = false
        var upTime: Long? = null

        if (held) onHoldStart()
        try {
            while (true) {
                val changes = awaitPointerEvent().changes
                val tracked = changes.firstOrNull { it.id == pointer } ?: break
                val change = if (tracked.changedToUp()) {
                    // Follow another finger if one is still down.
                    val next = changes.firstOrNull { it.pressed }
                    if (next == null) {
                        upTime = tracked.uptimeMillis
                        break
                    }
                    pointer = next.id
                    next
                } else {
                    tracked
                }

                val delta = change.positionChange()
                if (moving) {
                    change.consume()
                    if (delta != Offset.Zero) onMove(delta)
                } else {
                    travelled += delta
                    if (travelled.getDistance() > viewConfiguration.touchSlop) {
                        moving = true
                        change.consume()
                        onMove(travelled)
                    }
                }
            }
        } finally {
            // Also runs if the gesture is cancelled, so a held button never sticks.
            if (held) onHoldEnd()
            onGestureEnd()
        }

        val up = upTime
        if (!moving && up != null) {
            if (!held) onTap()
            lastTapUp = up
        }
    }
}
