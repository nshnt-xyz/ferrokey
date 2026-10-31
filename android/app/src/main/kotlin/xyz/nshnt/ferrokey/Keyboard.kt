package xyz.nshnt.ferrokey

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AssistChip
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import xyz.nshnt.ferrokey.core.Modifiers
import xyz.nshnt.ferrokey.core.SpecialKey
import xyz.nshnt.ferrokey.core.typableLen

val NoModifiers = Modifiers(ctrl = false, alt = false, shift = false, meta = false)

/**
 * The field always holds this, so the keyboard has something to delete
 * when the user presses backspace (some keyboards send nothing otherwise).
 */
private const val SENTINEL = " "

private fun sentinel() = TextFieldValue(SENTINEL, TextRange(SENTINEL.length))

/**
 * An invisible text field that receives the soft keyboard's input once
 * [focusRequester] focuses it.
 *
 * Keyboards edit text rather than sending key presses, so each edit is
 * diffed against the previous text and reported to [onEdit] as a number of
 * backspaces followed by the text to type. Removed characters the computer
 * never received (ones a US layout can't type) don't count as backspaces.
 * Between words the field is reset to [SENTINEL] so it never grows; while
 * the keyboard is composing a word it's left alone, as resetting it would
 * break the keyboard's word suggestions.
 */
@Composable
fun KeyboardCapture(
    focusRequester: FocusRequester,
    onEdit: (backspaces: Int, text: String) -> Unit,
) {
    var value by remember { mutableStateOf(sentinel()) }
    BasicTextField(
        value = value,
        onValueChange = { new ->
            val old = value.text
            val common = old.commonPrefixWith(new.text).length
            val backspaces = typableLen(old.substring(common)).toInt()
            val added = new.text.substring(common)
            if (backspaces > 0 || added.isNotEmpty()) onEdit(backspaces, added)
            value = if (new.composition == null) sentinel() else new
        },
        keyboardOptions = KeyboardOptions(
            capitalization = KeyboardCapitalization.None,
            autoCorrectEnabled = false,
            // The computer is assumed to use a US layout, so ask for a
            // keyboard that sticks to ASCII.
            keyboardType = KeyboardType.Ascii,
        ),
        modifier = Modifier.size(1.dp).alpha(0f).focusRequester(focusRequester),
    )
}

private val namedKeys = listOf(
    "Esc" to SpecialKey.ESCAPE,
    "Tab" to SpecialKey.TAB,
    "←" to SpecialKey.LEFT,
    "↑" to SpecialKey.UP,
    "↓" to SpecialKey.DOWN,
    "→" to SpecialKey.RIGHT,
    "Home" to SpecialKey.HOME,
    "End" to SpecialKey.END,
    "PgUp" to SpecialKey.PAGE_UP,
    "PgDn" to SpecialKey.PAGE_DOWN,
    "Del" to SpecialKey.DELETE,
    "Ins" to SpecialKey.INSERT,
    "F1" to SpecialKey.F1,
    "F2" to SpecialKey.F2,
    "F3" to SpecialKey.F3,
    "F4" to SpecialKey.F4,
    "F5" to SpecialKey.F5,
    "F6" to SpecialKey.F6,
    "F7" to SpecialKey.F7,
    "F8" to SpecialKey.F8,
    "F9" to SpecialKey.F9,
    "F10" to SpecialKey.F10,
    "F11" to SpecialKey.F11,
    "F12" to SpecialKey.F12,
)

/**
 * Modifier toggles and keys the soft keyboard lacks, in a scrolling row.
 * Modifiers stay on until the next key is sent, so Ctrl then "c" sends Ctrl+C.
 */
@Composable
fun SpecialKeyBar(
    modifiers: Modifiers,
    onModifiersChange: (Modifiers) -> Unit,
    onKey: (SpecialKey) -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier = modifier.horizontalScroll(rememberScrollState()),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        FilterChip(
            selected = modifiers.ctrl,
            onClick = { onModifiersChange(modifiers.copy(ctrl = !modifiers.ctrl)) },
            label = { Text("Ctrl") },
        )
        FilterChip(
            selected = modifiers.alt,
            onClick = { onModifiersChange(modifiers.copy(alt = !modifiers.alt)) },
            label = { Text("Alt") },
        )
        FilterChip(
            selected = modifiers.shift,
            onClick = { onModifiersChange(modifiers.copy(shift = !modifiers.shift)) },
            label = { Text("Shift") },
        )
        FilterChip(
            selected = modifiers.meta,
            onClick = { onModifiersChange(modifiers.copy(meta = !modifiers.meta)) },
            label = { Text("Super") },
        )
        namedKeys.forEach { (label, key) ->
            AssistChip(onClick = { onKey(key) }, label = { Text(label) })
        }
    }
}
