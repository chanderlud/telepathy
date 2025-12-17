import 'package:flutter/material.dart';

/// Custom Button Widget.
class Button extends StatelessWidget {
  final String text;
  final VoidCallback onPressed;
  final double? width;
  final double? height;
  final bool disabled;
  final Color? disabledColor;
  final bool noSplash;

  const Button(
      {super.key,
      required this.text,
      required this.onPressed,
      this.width,
      this.height,
      this.disabled = false,
      this.disabledColor,
      this.noSplash = false});

  @override
  Widget build(BuildContext context) {
    Widget child;

    if (width == null || height == null) {
      child = Text(text);
    } else {
      child = SizedBox(
        width: width!,
        height: height,
        child: Center(child: Text(text)),
      );
    }

    return ElevatedButton(
      onPressed: () {
        if (!disabled) {
          onPressed();
        }
      },
      style: ButtonStyle(
        splashFactory: noSplash ? NoSplash.splashFactory : null,
        backgroundColor: disabled
            ? WidgetStateProperty.all(disabledColor ?? Colors.grey)
            : WidgetStateProperty.all(Theme.of(context).colorScheme.primary),
        foregroundColor: WidgetStateProperty.all(Colors.white),
        overlayColor: disabled
            ? WidgetStateProperty.all(disabledColor ?? Colors.grey)
            : WidgetStateProperty.all(Theme.of(context).colorScheme.secondary),
        surfaceTintColor: WidgetStateProperty.all(Colors.transparent),
        mouseCursor: disabled
            ? WidgetStateProperty.all(SystemMouseCursors.basic)
            : WidgetStateProperty.all(SystemMouseCursors.click),
      ),
      child: child,
    );
  }
}
