package xyz.nshnt.ferrokey

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import xyz.nshnt.ferrokey.core.PairedServer

/** Lists paired computers to connect to, and pairs new ones. */
@Composable
fun ConnectScreen(model: ClientViewModel) {
    Surface(modifier = Modifier.fillMaxSize()) {
        Column(
            modifier = Modifier
                .safeDrawingPadding()
                .verticalScroll(rememberScrollState())
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Text("ferrokey", style = MaterialTheme.typography.headlineMedium)
            if (model.busy) LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
            model.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }

            if (model.pairedServers.isNotEmpty()) {
                Text("Paired computers", style = MaterialTheme.typography.titleMedium)
                model.pairedServers.forEach { server ->
                    PairedServerCard(
                        server,
                        enabled = !model.busy,
                        onConnect = { model.connect(server.address) },
                        onForget = { model.forget(server.address) },
                    )
                }
            }

            PairForm(enabled = !model.busy, onPair = model::pair)
        }
    }
}

@Composable
private fun PairedServerCard(
    server: PairedServer,
    enabled: Boolean,
    onConnect: () -> Unit,
    onForget: () -> Unit,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Row(
            modifier = Modifier.padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(modifier = Modifier.weight(1f)) {
                Text(server.address, style = MaterialTheme.typography.bodyLarge)
                Text(
                    server.fingerprint,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            TextButton(onClick = onForget, enabled = enabled) { Text("Forget") }
            Button(onClick = onConnect, enabled = enabled) { Text("Connect") }
        }
    }
}

@Composable
private fun PairForm(enabled: Boolean, onPair: (address: String, code: String) -> Unit) {
    var address by rememberSaveable { mutableStateOf("") }
    var code by rememberSaveable { mutableStateOf("") }
    val digits = code.filter(Char::isDigit)

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("Pair a new computer", style = MaterialTheme.typography.titleMedium)
        Text(
            "Run ferrokey-server on the computer and enter its address and the pairing code it shows.",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        OutlinedTextField(
            value = address,
            onValueChange = { address = it },
            label = { Text("Address, e.g. 192.168.1.20") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            value = code,
            onValueChange = { code = it },
            label = { Text("Pairing code") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
            modifier = Modifier.fillMaxWidth(),
        )
        Button(
            onClick = { onPair(address, digits) },
            enabled = enabled && address.isNotBlank() && digits.length == 6,
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("Pair")
        }
    }
}
