import 'package:flutter/material.dart';

/// Custom Switch widget.
class CustomSwitch extends StatelessWidget {
  final bool value;
  final bool? disabled;
  final void Function(bool)? onChanged;

  const CustomSwitch(
      {super.key, required this.value, required this.onChanged, this.disabled});

  @override
  Widget build(BuildContext context) {
    return Transform.scale(
      scale: 0.85,
      child: Switch(
        value: value,
        onChanged: disabled == true ? null : onChanged,
        inactiveTrackColor: const Color(0xFF80848e),
        activeTrackColor: disabled == true
            ? const Color(0xFF80848e)
            : Theme.of(context).colorScheme.secondary,
      ),
    );
  }
}
