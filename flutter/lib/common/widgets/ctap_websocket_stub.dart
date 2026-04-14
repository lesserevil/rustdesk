/// Stub WebSocket adapter for native platforms.
/// On native, the CTAP relay is handled in-process via hidapi (Rust side),
/// so the companion WebSocket is not used.

class WebSocketAdapter {
  WebSocketAdapter(String url) {
    throw UnsupportedError('WebSocket companion not available on native');
  }

  bool get isOpen => false;
  Future<void> get ready => Future.error('Not supported');
  void onMessage(void Function(String) handler) {}
  void onClose(void Function(dynamic) handler) {}
  void send(String data) {}
  void close() {}
}
