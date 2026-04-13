import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';

/// Overlay widget shown when a CTAP/WebAuthn authentication request
/// is waiting for the user to tap their physical security key.
class CtapPromptOverlay extends StatelessWidget {
  final String message;
  final VoidCallback onCancel;

  const CtapPromptOverlay({
    Key? key,
    required this.message,
    required this.onCancel,
  }) : super(key: key);

  @override
  Widget build(BuildContext context) {
    return Positioned(
      bottom: 80,
      left: 0,
      right: 0,
      child: Center(
        child: Card(
          elevation: 8,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(12),
          ),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 16),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                const _PulsingIcon(),
                const SizedBox(height: 12),
                Text(
                  translate(message),
                  style: Theme.of(context).textTheme.titleMedium,
                ),
                const SizedBox(height: 16),
                TextButton(
                  onPressed: onCancel,
                  child: Text(translate('Cancel')),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _PulsingIcon extends StatefulWidget {
  const _PulsingIcon();

  @override
  State<_PulsingIcon> createState() => _PulsingIconState();
}

class _PulsingIconState extends State<_PulsingIcon>
    with SingleTickerProviderStateMixin {
  late AnimationController _controller;

  @override
  void initState() {
    super.initState();
    _controller = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1000),
    )..repeat(reverse: true);
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return FadeTransition(
      opacity: Tween(begin: 0.5, end: 1.0).animate(_controller),
      child: Icon(
        Icons.vpn_key,
        size: 48,
        color: Theme.of(context).colorScheme.primary,
      ),
    );
  }
}
