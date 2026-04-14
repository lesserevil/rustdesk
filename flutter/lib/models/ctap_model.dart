import 'dart:convert';
import 'dart:typed_data';

import 'package:flutter/foundation.dart';
import 'package:flutter_hbb/common/widgets/ctap_websocket.dart';
import 'package:flutter_hbb/models/model.dart';
import 'package:flutter_hbb/models/platform_model.dart';

/// Model for CTAP/WebAuthn security key passthrough.
///
/// On native clients, the Rust side handles the relay directly via hidapi.
/// On web clients, this model manages the relay through the companion app.
class CtapModel extends ChangeNotifier {
  final WeakReference<FFI> parent;
  bool _promptVisible = false;
  String _promptMessage = '';
  CtapCompanionClient? _companion;
  bool _companionConnected = false;

  CtapModel(this.parent);

  bool get promptVisible => _promptVisible;
  String get promptMessage => _promptMessage;
  bool get companionConnected => _companionConnected;
  bool get isWeb => kIsWeb;

  void showPrompt(String message) {
    _promptMessage = message;
    _promptVisible = true;
    notifyListeners();
  }

  void hidePrompt() {
    _promptVisible = false;
    notifyListeners();
  }

  /// Handle a CTAP request event from the Rust side (web client path).
  ///
  /// On web, the Rust/WASM side cannot access USB directly, so it pushes
  /// a `ctap_request` event to Dart. We relay to the companion app.
  Future<void> handleCtapRequest(Map<String, dynamic> evt) async {
    if (!kIsWeb) return; // Native handles this in Rust

    final command = int.tryParse(evt['command'] ?? '') ?? 0;
    final payloadB64 = evt['payload'] ?? '';
    final payload =
        payloadB64.isEmpty ? Uint8List(0) : base64Decode(payloadB64);

    showPrompt('Tap your security key');

    // Connect to companion if not already connected
    if (_companion == null || !_companion!.isConnected) {
      _companion = CtapCompanionClient();
      _companionConnected = await _companion!.connect();
      notifyListeners();

      if (!_companionConnected) {
        // Companion not running — send error back to remote
        _sendCtapResponse(command, Uint8List(0), 0x2E); // NO_CREDENTIALS
        hidePrompt();
        return;
      }
    }

    // Relay to companion
    final response = await _companion!.relay(command, payload);

    // Send response back to remote via FFI
    _sendCtapResponse(response.command, response.payload, response.errorCode);
    hidePrompt();
  }

  /// Handle cancellation from the user (Cancel button on prompt).
  void cancelFromUser() {
    final sessionId = parent.target?.sessionId;
    if (sessionId == null) return;

    // On web, cancel the companion relay
    _companion?.cancel('current');

    // Send cancel/error response to remote
    _sendCtapResponse(0x10, Uint8List(0), 0x2D); // KEEPALIVE_CANCEL
    hidePrompt();
  }

  /// Send a CTAP response back to the remote server.
  void _sendCtapResponse(int command, Uint8List payload, int errorCode) {
    final sessionId = parent.target?.sessionId;
    if (sessionId == null) return;

    // Encode as JSON and send via the web bridge
    final responseData = jsonEncode({
      'command': command,
      'payload': base64Encode(payload),
      'is_response': true,
      'error_code': errorCode,
    });

    try {
      ffiSetByName('send_ctap_response', responseData);
    } catch (e) {
      debugPrint('Failed to send CTAP response: $e');
    }
  }

  void dispose() {
    _companion?.disconnect();
    super.dispose();
  }
}
