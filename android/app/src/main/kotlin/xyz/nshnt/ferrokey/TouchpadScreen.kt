package xyz.nshnt.ferrokey

import android.os.SystemClock
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
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
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.input.pointer.AwaitPointerEventScope
import androidx.compose.ui.input.pointer.PointerId
import androidx.compose.ui.input.pointer.PointerInputScope
import androidx.compose.ui.input.pointer.changedToUp
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import xyz.nshnt.ferrokey.core.InputEvent
import xyz.nshnt.ferrokey.core.Modifiers
import xyz.nshnt.ferrokey.core.MouseButton
import xyz.nshnt.ferrokey.core.SpecialKey
import kotlin.math.min
import kotlin.math.roundToInt

/** The computer's screen, as far as [TouchpadScreen] is concerned. */
class ScreenState(
    /** Whether the computer shares its screen at all. */
    val available: Boolean,
    val watching: Boolean,
    /** The latest frame, once one has arrived. */
    val frame: ImageBitmap?,
    /** Starts showing the screen, in frames of at most this size. */
    val start: (width: Int, height: Int) -> Unit,
    val stop: () -> Unit,
    /** Moves the pointer to a point on the screen, as fractions of its size. */
    val pointAt: (x: Float, y: Float) -> Unit,
)

/**
 * A touchpad surface plus left/right buttons, connected to [address]. The
 * keyboard button brings up the soft keyboard, whose typing goes to the
 * computer, along with a row of keys it lacks.
 *
 * If the computer shares its screen, the touchpad can show it. Touches then
 * point directly at what they land on, or work as a touchpad over the
 * picture.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun TouchpadScreen(
    address: String,
    send: (InputEvent) -> Unit,
    typeText: (String, Modifiers) -> Unit,
    tapKey: (SpecialKey, Modifiers) -> Unit,
    onDisconnect: () -> Unit,
    screen: ScreenState,
) {
    val click = { button: MouseButton ->
        send(InputEvent.MouseButton(button, pressed = true))
        send(InputEvent.MouseButton(button, pressed = false))
    }
    var holding by remember { mutableStateOf(false) }
    var padSize by remember { mutableStateOf(IntSize.Zero) }
    // While the screen shows, whether touches point at it directly.
    var direct by remember { mutableStateOf(true) }
    val pointing = screen.watching && direct
    val frame by rememberUpdatedState(screen.frame)

    // Modifiers from the key bar apply to the next key only.
    var modifiers by remember { mutableStateOf(NoModifiers) }
    val tap = { key: SpecialKey ->
        tapKey(key, modifiers)
        modifiers = NoModifiers
    }
    val edit = { backspaces: Int, text: String ->
        val singleChar = text.isNotEmpty() && text.codePointCount(0, text.length) == 1
        when {
            // A single key press takes the armed modifiers.
            backspaces == 1 && text.isEmpty() -> tap(SpecialKey.BACKSPACE)
            backspaces == 0 && singleChar -> {
                typeText(text, modifiers)
                modifiers = NoModifiers
            }
            // A whole word, suggestion or correction is typed plainly, so
            // Ctrl doesn't turn into Ctrl+Backspace or Ctrl+H, Ctrl+E, ...
            else -> {
                repeat(backspaces) { tapKey(SpecialKey.BACKSPACE, NoModifiers) }
                if (text.isNotEmpty()) typeText(text, NoModifiers)
            }
        }
    }

    val focusRequester = remember { FocusRequester() }
    val focusManager = LocalFocusManager.current
    val keyboardController = LocalSoftwareKeyboardController.current
    val keyboardVisible = WindowInsets.isImeVisible
    // The key bar is the only sign a modifier is armed, so disarm them when
    // it goes away with the keyboard.
    LaunchedEffect(keyboardVisible) {
        if (!keyboardVisible) modifiers = NoModifiers
    }

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
                if (screen.available) {
                    TextButton(
                        onClick = {
                            if (screen.watching) screen.stop() else screen.start(padSize.width, padSize.height)
                        },
                    ) {
                        Text(if (screen.watching) "Hide screen" else "Screen")
                    }
                }
                TextButton(onClick = onDisconnect) { Text("Disconnect") }
                KeyboardCapture(focusRequester, onEdit = edit)
            }
            Box(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(24.dp))
                    .background(MaterialTheme.colorScheme.surfaceVariant)
                    .onSizeChanged { padSize = it }
                    .pointerInput(pointing) {
                        if (pointing) {
                            directGestures(
                                pointAt = { position ->
                                    frame?.let { frame ->
                                        val point = fractionOn(frame, size, position)
                                        screen.pointAt(point.x, point.y)
                                    }
                                },
                                onPress = { send(InputEvent.MouseButton(MouseButton.LEFT, pressed = true)) },
                                onRelease = { send(InputEvent.MouseButton(MouseButton.LEFT, pressed = false)) },
                                onTap = { click(MouseButton.LEFT) },
                            )
                            return@pointerInput
                        }
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
                val shown = screen.frame
                when {
                    screen.watching && shown != null -> Image(
                        bitmap = shown,
                        contentDescription = "Computer screen",
                        contentScale = ContentScale.Fit,
                        modifier = Modifier.fillMaxSize(),
                    )
                    screen.watching -> Text(
                        text = "Waiting for the screen…",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    else -> Text(
                        text = if (holding) "Holding" else "Touchpad",
                        color = if (holding) {
                            MaterialTheme.colorScheme.primary
                        } else {
                            MaterialTheme.colorScheme.onSurfaceVariant
                        },
                    )
                }
            }
            if (screen.watching) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    FilterChip(
                        selected = direct,
                        onClick = { direct = true },
                        label = { Text("Point") },
                    )
                    FilterChip(
                        selected = !direct,
                        onClick = { direct = false },
                        label = { Text("Touchpad") },
                    )
                }
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

/**
 * Touchscreen-style gestures on the shown screen. Touching moves the pointer
 * there through [pointAt]; lifting without moving past the touch slop is a
 * tap, and moving further drags with the left button held from where the
 * touch started.
 */
private suspend fun PointerInputScope.directGestures(
    pointAt: (Offset) -> Unit,
    onPress: () -> Unit,
    onRelease: () -> Unit,
    onTap: () -> Unit,
) = awaitEachGesture {
    val down = awaitFirstDown(requireUnconsumed = false)
    down.consume()
    pointAt(down.position)
    var dragging = false
    var lifted = false
    try {
        var travelled = Offset.Zero
        while (true) {
            val change = awaitPointerEvent().changes.firstOrNull { it.id == down.id } ?: break
            if (change.changedToUp()) {
                change.consume()
                lifted = true
                break
            }
            travelled += change.positionChange()
            change.consume()
            if (!dragging && travelled.getDistance() > viewConfiguration.touchSlop) {
                dragging = true
                onPress()
            }
            if (dragging) pointAt(change.position)
        }
    } finally {
        // Also runs if the gesture is cancelled, so a held button never sticks.
        if (dragging) onRelease()
    }
    if (!dragging && lifted) onTap()
}

/**
 * Where [position] in a [pad]-sized box falls on [frame] drawn in it with
 * [ContentScale.Fit], as fractions of the frame's size. Points outside the
 * picture snap to its edge.
 */
private fun fractionOn(frame: ImageBitmap, pad: IntSize, position: Offset): Offset {
    val scale = min(pad.width / frame.width.toFloat(), pad.height / frame.height.toFloat())
    val width = frame.width * scale
    val height = frame.height * scale
    val left = (pad.width - width) / 2
    val top = (pad.height - height) / 2
    return Offset(
        ((position.x - left) / width).coerceIn(0f, 1f),
        ((position.y - top) / height).coerceIn(0f, 1f),
    )
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
