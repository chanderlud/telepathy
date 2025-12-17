import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';

class InterfaceController with ChangeNotifier {
  final SharedPreferences options;

  InterfaceController({required this.options});

  late int primaryColor;

  int get secondaryColor => darkenColor(primaryColor, 0.1);

  Future<void> init() async {
    primaryColor = options.getInt("primaryColor") ?? 0xFF5538e5;
    notifyListeners();
  }

  Future<void> setPrimaryColor(int color) async {
    primaryColor = color;
    await options.setInt('primaryColor', color);
    notifyListeners();
  }
}

int darkenColor(int colorInt, double amount) {
  assert(amount >= 0 && amount <= 1, '"amount" should be between 0.0 and 1.0');

  // Separate out ARGB channels:
  final a = (colorInt >> 24) & 0xFF;
  final r = (colorInt >> 16) & 0xFF;
  final g = (colorInt >> 8) & 0xFF;
  final b = colorInt & 0xFF;

  // Multiply each channel by (1 - amount) to darken; clamp to [0,255].
  int darkerR = (r * (1 - amount)).clamp(0, 255).toInt();
  int darkerG = (g * (1 - amount)).clamp(0, 255).toInt();
  int darkerB = (b * (1 - amount)).clamp(0, 255).toInt();

  // Reassemble back into a 32-bit ARGB int:
  return (a << 24) | (darkerR << 16) | (darkerG << 8) | darkerB;
}

