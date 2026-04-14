import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

// Conditional import: use dart:html WebSocket on web, stub on native.
import 'ctap_websocket_stub.dart'
    if (dart.library.html) 'ctap_websocket_web.dart' as platform;

/// Client for the CTAP Companion App's WebSocket server.
///
/// Used by the RustDesk web client to relay CTAP commands to a physical
/// FIDO2 security key connected to the user's local machine via the
/// standalone ctap-companion binary.
class CtapCompanionClient {
  platform.WebSocketAdapter? _ws;
  bool _fidoAvailable = false;
  String _deviceName = '';
  final Map<String, Completer<Map<String, dynamic>>> _pending = {};
  int _nextId = 0;

  bool get isConnected => _ws != null && _ws!.isOpen;
  bool get fidoAvailable => _fidoAvailable;
  String get deviceName => _deviceName;

  /// Attempt to connect to the local companion app.
  /// Tries port 21118, then 21119.
  Future<bool> connect() async {
    for (final port in [21118, 21119]) {
      try {
        final ws = platform.WebSocketAdapter('ws://127.0.0.1:$port');
        await ws.ready.timeout(const Duration(seconds: 2));
        ws.onMessage(_onMessage);
        ws.onClose((_) => _onDisconnect());
        _ws = ws;

        // Send hello
        _send({
          'type': 'hello',
          'version': 1,
          'origin': Uri.base.origin,
        });

        // Wait for hello_ack
        final ackCompleter = Completer<bool>();
        Timer? timeout;
        void onAck(String data) {
          final msg = jsonDecode(data) as Map<String, dynamic>;
          if (msg['type'] == 'hello_ack') {
            _fidoAvailable = msg['fido_available'] as bool? ?? false;
            _deviceName = msg['device_name'] as String? ?? '';
            if (!ackCompleter.isCompleted) ackCompleter.complete(true);
          }
        }

        // Temporarily listen for hello_ack
        ws.onMessage((String data) {
          onAck(data);
          _onMessage(data);
        });

        timeout = Timer(const Duration(seconds: 2), () {
          if (!ackCompleter.isCompleted) ackCompleter.complete(false);
        });

        final success = await ackCompleter.future;
        timeout.cancel();

        if (success) {
          // Restore normal message handler
          ws.onMessage(_onMessage);
          return true;
        } else {
          ws.close();
          _ws = null;
        }
      } catch (e) {
        _ws = null;
        continue;
      }
    }
    return false;
  }

  /// Relay a CTAP2 command to the physical key via the companion.
  /// Returns the response payload and error code.
  Future<CtapCompanionResponse> relay(int command, Uint8List payload) async {
    if (!isConnected) {
      return CtapCompanionResponse(
        command: command,
        payload: Uint8List(0),
        errorCode: 0x01,
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

    try {
      final resp = await completer.future.timeout(
        const Duration(seconds: 30),
      );
      return CtapCompanionResponse.fromJson(resp);
    } on TimeoutException {
      _pending.remove(id);
      return CtapCompanionResponse(
        command: command,
        payload: Uint8List(0),
        errorCode: 0x2D, // CTAP2_ERR_KEEPALIVE_CANCEL
      );
    }
  }

  /// Cancel an in-progress relay.
  void cancel(String id) {
    _send({'type': 'ctap_cancel', 'id': id});
  }

  void disconnect() {
    _ws?.close();
    _ws = null;
    _pending.clear();
  }

  void _send(Map<String, dynamic> msg) {
    _ws?.send(jsonEncode(msg));
  }

  void _onMessage(String data) {
    try {
      final msg = jsonDecode(data) as Map<String, dynamic>;
      switch (msg['type']) {
        case 'hello_ack':
          _fidoAvailable = msg['fido_available'] as bool? ?? false;
          _deviceName = msg['device_name'] as String? ?? '';
          break;
        case 'ctap_response':
          final id = msg['id'] as String?;
          if (id != null) {
            _pending.remove(id)?.complete(msg);
          }
          break;
        case 'status':
          _fidoAvailable = msg['fido_available'] as bool? ?? false;
          _deviceName = msg['device_name'] as String? ?? '';
          break;
      }
    } catch (e) {
      // Ignore malformed messages
    }
  }

  void _onDisconnect() {
    _ws = null;
    // Fail all pending requests
    for (final completer in _pending.values) {
      if (!completer.isCompleted) {
        completer.complete({
          'type': 'ctap_response',
          'id': '',
          'command': 0,
          'payload': '',
          'error_code': 0x01,
        });
      }
    }
    _pending.clear();
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
