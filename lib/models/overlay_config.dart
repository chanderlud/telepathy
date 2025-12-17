import 'package:flutter/material.dart';

class OverlayConfig {
  bool enabled;
  double x;
  double y;
  double width;
  double height;
  String fontFamily;
  Color fontColor;
  int fontHeight;
  Color backgroundColor;

  OverlayConfig({
    required this.enabled,
    required this.x,
    required this.y,
    required this.width,
    required this.height,
    required this.fontFamily,
    required this.fontColor,
    required this.fontHeight,
    required this.backgroundColor,
  });
}

