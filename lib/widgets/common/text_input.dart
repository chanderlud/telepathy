import 'package:flutter/material.dart';

/// Custom TextInput Widget.
class TextInput extends StatelessWidget {
  final String labelText;
  final String? hintText;
  final TextEditingController controller;
  final bool? obscureText;
  final bool? enabled;
  final FocusNode? focusNode;
  final bool? autofocus;
  final void Function(String)? onChanged;
  final void Function(String)? onSubmitted;
  final Widget? error;

  const TextInput(
      {super.key,
      required this.labelText,
      this.hintText,
      required this.controller,
      this.obscureText,
      this.enabled,
      this.focusNode,
      this.autofocus,
      this.onChanged,
      this.onSubmitted,
      this.error});

  @override
  Widget build(BuildContext context) {
    return TextField(
      controller: controller,
      obscureText: obscureText ?? false,
      enabled: enabled,
      focusNode: focusNode,
      autofocus: autofocus ?? false,
      onChanged: onChanged,
      onSubmitted: onSubmitted,
      decoration: InputDecoration(
        labelText: labelText,
        hintText: hintText,
        hintStyle: const TextStyle(
            fontSize: 13,
            fontStyle: FontStyle.normal,
            color: Color(0xFFa9a9aa),
            fontWeight: FontWeight.w600),
        fillColor: Theme.of(context).colorScheme.tertiaryContainer,
        filled: true,
        error: error,
        border: const OutlineInputBorder(
          borderRadius: BorderRadius.all(Radius.circular(10.0)),
        ),
        contentPadding: const EdgeInsets.all(10.0),
      ),
    );
  }
}
