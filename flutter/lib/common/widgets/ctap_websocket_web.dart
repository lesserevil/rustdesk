/// Web platform WebSocket adapter using dart:html.
import 'dart:async';
import 'dart:html' as html;

class WebSocketAdapter {
  final html.WebSocket _ws;
  final Completer<void> _ready = Completer<void>();
  void Function(String)? _onMessageHandler;
  void Function(dynamic)? _onCloseHandler;

  WebSocketAdapter(String url) : _ws = html.WebSocket(url) {
    _ws.onOpen.first.then((_) {
      if (!_ready.isCompleted) _ready.complete();
    });
    _ws.onError.first.then((e) {
      if (!_ready.isCompleted) _ready.completeError(e);
    });
    _ws.onMessage.listen((event) {
      final data = event.data;
      if (data is String && _onMessageHandler != null) {
        _onMessageHandler!(data);
      }
    });
    _ws.onClose.listen((event) {
      _onCloseHandler?.call(event);
    });
  }

  bool get isOpen => _ws.readyState == html.WebSocket.OPEN;

  Future<void> get ready => _ready.future;

  void onMessage(void Function(String) handler) {
    _onMessageHandler = handler;
  }

  void onClose(void Function(dynamic) handler) {
    _onCloseHandler = handler;
  }

  void send(String data) {
    if (isOpen) {
      _ws.send(data);
    }
  }

  void close() {
    _ws.close();
  }
}
