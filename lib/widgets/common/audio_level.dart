import 'package:flutter/material.dart';

const Color grey = Color(0xFF80848e);
const Color quietColor = Colors.green;
const Color mediumColor = Colors.yellow;
const Color loudColor = Colors.red;

class AudioLevel extends StatelessWidget {
  final double level;
  final int numRectangles;
  const AudioLevel(
      {super.key, required this.level, required this.numRectangles});

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: numRectangles * 13.0, // 8 (rect width) + 5 (margin)
      height: 25,
      child: CustomPaint(
        painter: _AudioLevelPainter(
          level: level,
          numRectangles: numRectangles,
        ),
      ),
    );
  }
}

class _AudioLevelPainter extends CustomPainter {
  final double level;
  final int numRectangles;

  _AudioLevelPainter({required this.level, required this.numRectangles});

  @override
  void paint(Canvas canvas, Size size) {
    final double threshold = level * numRectangles;
    final int maxIndex = numRectangles - 1;

    const double rectWidth = 8;
    const double rectHeight = 25;
    const double margin = 5;
    const Radius radius = Radius.circular(5);

    for (int index = 0; index < numRectangles; index++) {
      final double fraction = index / maxIndex;
      final Color color = index >= threshold ? grey : getColor(fraction);

      final double x = index * (rectWidth + margin);
      final rrect = RRect.fromLTRBR(x, 0, x + rectWidth, rectHeight, radius);

      canvas.drawRRect(rrect, Paint()..color = color);
    }
  }

  @override
  bool shouldRepaint(covariant _AudioLevelPainter oldDelegate) {
    return oldDelegate.level != level;
  }
}

/// Calculates a color for the given index
Color getColor(double fraction) {
  // determine the color based on the fraction
  if (fraction <= 0.5) {
    // scale fraction to [0, 1] for the first half
    double scaledFraction = fraction * 2;
    return Color.lerp(quietColor, mediumColor, scaledFraction)!;
  } else {
    // scale fraction to [0, 1] for the second half
    double scaledFraction = (fraction - 0.5) * 2;
    return Color.lerp(mediumColor, loudColor, scaledFraction)!;
  }
}
