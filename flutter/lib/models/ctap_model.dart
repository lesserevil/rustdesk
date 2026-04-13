import 'package:flutter/foundation.dart';

/// Model for CTAP/WebAuthn security key prompt state.
class CtapModel extends ChangeNotifier {
  bool _promptVisible = false;
  String _promptMessage = '';

  bool get promptVisible => _promptVisible;
  String get promptMessage => _promptMessage;

  void showPrompt(String message) {
    _promptMessage = message;
    _promptVisible = true;
    notifyListeners();
  }

  void hidePrompt() {
    _promptVisible = false;
    notifyListeners();
  }
}
