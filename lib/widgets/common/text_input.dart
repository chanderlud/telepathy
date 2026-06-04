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
  final int? maxLines;
  final void Function(String)? onChanged;
  final void Function(String)? onSubmitted;

  /// When non-null, renders a built-in error message under the field and
  /// tints the outline/label so the error state is visible both at rest
  /// and on hover. Use this instead of the raw `error` slot so the border
  /// color tracks the theme's error color rather than a hard-coded red.
  final String? errorText;

  /// Escape hatch for callers that need to render an arbitrary widget in
  /// the error slot (e.g. a rich `Text` with inline links). When supplied,
  /// no error-state border/label tinting is applied -- callers are
  /// responsible for styling the field themselves.
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
      this.maxLines,
      this.onChanged,
      this.onSubmitted,
      this.errorText,
      this.error})
      : assert(errorText == null || error == null,
            'errorText and error are mutually exclusive');

  @override
  Widget build(BuildContext context) {
    final errorColor = Theme.of(context).colorScheme.error;
    // Lerp the error color toward black so the outline darkens slightly on
    // hover while still clearly signalling the error state.
    final errorHoverColor = Color.lerp(errorColor, Colors.black, 0.16)!;
    
    final InputBorder border = errorText == null
        ? const OutlineInputBorder(
            borderRadius: BorderRadius.all(Radius.circular(10.0)),
          )
        : WidgetStateInputBorder.resolveWith((Set<WidgetState> states) {
            final hovered = states.contains(WidgetState.hovered);
            final focused = states.contains(WidgetState.focused);
            return OutlineInputBorder(
              borderRadius:
                  const BorderRadius.all(Radius.circular(10.0)),
              borderSide: BorderSide(
                color: hovered ? errorHoverColor : errorColor,
                width: focused ? 2 : 1,
              ),
            );
          });

    return TextField(
      controller: controller,
      obscureText: obscureText ?? false,
      enabled: enabled,
      focusNode: focusNode,
      autofocus: autofocus ?? false,
      maxLines: obscureText == true ? 1 : (maxLines ?? 1),
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
        errorText: errorText,
        error: error,
        labelStyle: errorText == null
            ? null
            : WidgetStateTextStyle.resolveWith((Set<WidgetState> states) {
                return TextStyle(
                  color: states.contains(WidgetState.hovered)
                      ? errorHoverColor
                      : errorColor,
                );
              }),
        floatingLabelStyle: errorText == null
            ? null
            : WidgetStateTextStyle.resolveWith((Set<WidgetState> states) {
                return TextStyle(
                  color: states.contains(WidgetState.hovered)
                      ? errorHoverColor
                      : errorColor,
                );
              }),
        border: border,
        contentPadding: const EdgeInsets.all(10.0),
      ),
    );
  }
}
