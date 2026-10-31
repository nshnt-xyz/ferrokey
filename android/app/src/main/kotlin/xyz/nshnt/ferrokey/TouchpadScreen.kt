package xyz.nshnt.ferrokey

import android.os.SystemClock
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
import androidx.compose.ui.input.pointer.AwaitPointerEventScope
import androidx.compose.ui.input.pointer.PointerId
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
 * Touchpad gestures, like a laptop touchpad. Moving a finger calls [onMove]
 * as it moves. Lifting it without moving past the touch slop is a tap, but
 * [onTap] waits out the double-tap timeout first: touching again within it
 * (tap and a half) calls [onHoldStart] instead, and [onHoldEnd] when that
 * finger lifts, so moving in between drags. If the second touch is quick and
 * doesn't move past the slop, it was a double tap, and [onTap] is called once
 * more to make it a double click; a longer one is a held click.
 *
 * Holding the first click back keeps the drag's press from arriving right
 * after a click, which the computer would take as a double click.
 */
private suspend fun PointerInputScope.touchpadGestures(
    onTap: () -> Unit,
    onHoldStart: () -> Unit,
    onHoldEnd: () -> Unit,
    onMove: (Offset) -> Unit,
    onGestureEnd: () -> Unit,
) = awaitEachGesture {
    val first = awaitFirstDown(requireUnconsumed = false)
    val moved = try {
        followTouch(first.id, onMove)
    } finally {
        onGestureEnd()
    }
    if (moved) return@awaitEachGesture

    val second = withTimeoutOrNull(viewConfiguration.doubleTapTimeoutMillis) {
        awaitFirstDown(requireUnconsumed = false)
    }
    if (second == null) {
        onTap()
        return@awaitEachGesture
    }

    onHoldStart()
    val dragged = try {
        followTouch(second.id, onMove)
    } finally {
        // Also runs if the gesture is cancelled, so a held button never sticks.
        onHoldEnd()
        onGestureEnd()
    }
    // Pointer event times share this clock, and the finger has just lifted.
    val heldFor = SystemClock.uptimeMillis() - second.uptimeMillis
    if (!dragged && heldFor < DOUBLE_TAP_MAX_MILLIS) onTap()
}

/** Longest second touch that still counts as a double tap, not a hold. */
private const val DOUBLE_TAP_MAX_MILLIS = 200L

/**
 * Follows a touch until every finger lifts, switching to another finger if
 * [pointer] lifts first, and passes all its movement to [onMove] as it
 * happens. Returns whether it moved past the touch slop, i.e. wasn't a tap.
 */
private suspend fun AwaitPointerEventScope.followTouch(
    pointer: PointerId,
    onMove: (Offset) -> Unit,
): Boolean {
    var tracking = pointer
    var travelled = Offset.Zero
    var moved = false
    while (true) {
        val changes = awaitPointerEvent().changes
        var change = changes.firstOrNull { it.id == tracking } ?: return moved
        if (change.changedToUp()) {
            change = changes.firstOrNull { it.pressed } ?: return moved
            tracking = change.id
        }

        // Move right away rather than after the slop, so drags don't lag;
        // the few pixels a tap wobbles don't matter.
        val delta = change.positionChange()
        change.consume()
        if (delta != Offset.Zero) onMove(delta)
        if (!moved) {
            travelled += delta
            moved = travelled.getDistance() > viewConfiguration.touchSlop
        }
    }
}
