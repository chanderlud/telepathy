import 'package:flutter/material.dart';

class AppTheme {
  static ThemeData dark(
    BuildContext context, {
    required int primaryColor,
    required int secondaryColor,
  }) {
    return ThemeData(
      dialogTheme: const DialogThemeData(
        surfaceTintColor: Color(0xFF27292A),
      ),
      sliderTheme: SliderThemeData(
        showValueIndicator: ShowValueIndicator.onDrag,
        overlayColor: Colors.transparent,
        trackShape: CustomTrackShape(),
        inactiveTrackColor: const Color(0xFF121212),
        activeTrackColor: Color(primaryColor),
      ),
      colorScheme: ColorScheme.dark(
        primary: Color(primaryColor),
        secondary: Color(secondaryColor),
        brightness: Brightness.dark,
        surface: const Color(0xFF222425),
        secondaryContainer: const Color(0xFF191919),
        tertiaryContainer: const Color(0xFF27292A),
        surfaceDim: const Color(0xFF121212),
      ),
      switchTheme: SwitchThemeData(
        trackOutlineWidth: WidgetStateProperty.all(0),
        trackOutlineColor: WidgetStateProperty.all(Colors.transparent),
        overlayColor: WidgetStateProperty.all(Colors.transparent),
        thumbColor: WidgetStateProperty.all(
            Theme.of(context).tabBarTheme.indicatorColor),
      ),
      dropdownMenuTheme: DropdownMenuThemeData(
        menuStyle: MenuStyle(
          backgroundColor: WidgetStateProperty.all(const Color(0xFF191919)),
          surfaceTintColor: WidgetStateProperty.all(const Color(0xFF191919)),
        ),
      ),
    );
  }
}

/// Removes the padding from a Slider.
class CustomTrackShape extends RoundedRectSliderTrackShape {
  @override
  Rect getPreferredRect({
    required RenderBox parentBox,
    Offset offset = Offset.zero,
    required SliderThemeData sliderTheme,
    bool isEnabled = false,
    bool isDiscrete = false,
  }) {
    final trackHeight = sliderTheme.trackHeight;
    final trackLeft = offset.dx;
    final trackTop = offset.dy + (parentBox.size.height - trackHeight!) / 2;
    final trackWidth = parentBox.size.width;
    return Rect.fromLTWH(trackLeft, trackTop, trackWidth, trackHeight);
  }
}

