import 'dart:math';

import 'package:collection/collection.dart';
import 'package:flutter/material.dart';
import 'package:telepathy/widgets/common/audio_level.dart';

class GradientMiniLineChart extends StatelessWidget {
  final List<int> values;
  final double strokeWidth;

  const GradientMiniLineChart({
    super.key,
    required this.values,
    this.strokeWidth = 2,
  });

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: 250,
      height: 50,
      child: CustomPaint(
        painter: _GradientMiniLineChartPainter(
          values: values,
          strokeWidth: strokeWidth,
        ),
      ),
    );
  }
}

class _GradientMiniLineChartPainter extends CustomPainter {
  final List<int> values;
  final double strokeWidth;

  _GradientMiniLineChartPainter({
    required this.values,
    required this.strokeWidth,
  });

  @override
  void paint(Canvas canvas, Size size) {
    if (values.isEmpty) return;

    double maximum = max(values.max.toDouble(), 1);
    final clamped = values
        .map((v) => (1 - (v.toDouble() / maximum)) * size.height)
        .map((v) => v.clamp(1, size.height).toDouble())
        .toList();

    final count = clamped.length;

    // Build the full path once
    final path = Path();
    final dx = size.width / (count - 1);

    path.moveTo(0, clamped[0]);
    for (int i = 1; i < count; i++) {
      final x = dx * i;
      path.lineTo(x, clamped[i]);
    }

    final bounds = Offset.zero & size;

    // 1) Draw the line in a solid color into a layer
    canvas.saveLayer(bounds, Paint());

    final linePaint = Paint()
      ..color = Colors.white
      ..style = PaintingStyle.stroke
      ..strokeWidth = strokeWidth
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round
      ..isAntiAlias = true;

    canvas.drawPath(path, linePaint);

    // 2) Draw vertical gradient, masked to the line with srcIn
    final gradientPaint = Paint()
      ..shader = const LinearGradient(
        begin: Alignment.bottomCenter,
        end: Alignment.topCenter,
        colors: [
          quietColor,
          mediumColor,
          loudColor,
        ],
        stops: [0.0, 0.5, 1.0],
      ).createShader(bounds)
      ..blendMode = BlendMode.srcIn;

    canvas.drawRect(bounds, gradientPaint);

    canvas.restore();
  }

  @override
  bool shouldRepaint(covariant _GradientMiniLineChartPainter oldDelegate) {
    return oldDelegate.values != values ||
        oldDelegate.strokeWidth != strokeWidth;
  }
}
