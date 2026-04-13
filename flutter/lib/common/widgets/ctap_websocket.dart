import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';
import 'package:flutter/foundation.dart' show kIsWeb;

/// Client for the CTAP Companion App's WebSocket server.
///
/// Used by the RustDesk web client to relay CTAP commands to a physical
/// FIDO2 security key connected to the user's local machine.
///
/// Only functional on web platform; on native, use the built-in local driver.
class CtapCompanionClient {
  dynamic _ws; // WebSocket on web, null on native
  bool _fidoAvailable = false;
  String _deviceName = '';
  final Map<String, Completer<Map<String, dynamic>>> _pending = {};
  int _nextId = 0;

  bool get isConnected => _ws != null;
  bool get fidoAvailable => _fidoAvailable;
  String get deviceName => _deviceName;

  /// Attempt to connect to the local companion app.
  /// Tries port 21118, then 21119.
  /// Returns true if connected successfully.
  Future<bool> connect() async {
    if (!kIsWeb) return false;

    for (final port in [21118, 21119]) {
      try {
        // Web-only WebSocket API would be used here.
        // This is a placeholder — actual web implementation requires
        // dart:html WebSocket which is only available on web.
        // For now, return false to indicate companion not available.
        return false;
      } catch (_) {
        continue;
      }
    }
    return false;
  }

  /// Relay a CTAP2 command to the physical key via the companion.
  Future<CtapCompanionResponse> relay(int command, Uint8List payload) async {
    if (!isConnected) {
      return CtapCompanionResponse(
        command: command,
        payload: Uint8List(0),
        errorCode: 0x01, // CTAP1_ERR_OTHER
      );
    }

    final id = 'req-${_nextId++}';
    final completer = Completer<Map<String, dynamic>>();
    _pending[id] = completer;

    _send({
      'type': 'ctap_relay',
      'id': id,
      'command': command,
      'payload': base64Encode(payload),
    });

    final resp = await completer.future.timeout(
      const Duration(seconds: 30),
      onTimeout: () => {
        'type': 'ctap_response',
        'id': id,
        'command': command,
        'payload': '',
        'error_code': 0x2D, // CTAP2_ERR_KEEPALIVE_CANCEL
      },
    );

    return CtapCompanionResponse.fromJson(resp);
  }

  /// Cancel an in-progress relay.
  void cancel(String id) {
    _send({'type': 'ctap_cancel', 'id': id});
  }

  void disconnect() {
    // Close WebSocket if connected
    _ws = null;
    _pending.clear();
  }

  void _send(Map<String, dynamic> msg) {
    // Placeholder — would use _ws.send(jsonEncode(msg)) on web
  }

  void _onMessage(String data) {
    final msg = jsonDecode(data) as Map<String, dynamic>;
    switch (msg['type']) {
      case 'hello_ack':
        _fidoAvailable = msg['fido_available'] as bool;
        _deviceName = msg['device_name'] as String? ?? '';
        break;
      case 'ctap_response':
        final id = msg['id'] as String;
        _pending.remove(id)?.complete(msg);
        break;
      case 'status':
        _fidoAvailable = msg['fido_available'] as bool;
        _deviceName = msg['device_name'] as String? ?? '';
        break;
    }
  }
}

class CtapCompanionResponse {
  final int command;
  final Uint8List payload;
  final int errorCode;

  CtapCompanionResponse({
    required this.command,
    required this.payload,
    required this.errorCode,
  });

  factory CtapCompanionResponse.fromJson(Map<String, dynamic> json) {
    final payloadStr = json['payload'] as String? ?? '';
    return CtapCompanionResponse(
      command: json['command'] as int? ?? 0,
      payload: payloadStr.isEmpty ? Uint8List(0) : base64Decode(payloadStr),
      errorCode: json['error_code'] as int? ?? 0,
    );
  }
}
