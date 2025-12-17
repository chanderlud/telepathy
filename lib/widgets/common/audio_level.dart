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
    return LayoutBuilder(
      builder: (context, constraints) {
        double threshold = level * numRectangles;
        int maxIndex = numRectangles - 1;

        // generate the rectangles
        List<Widget> rectangles = List.generate(numRectangles, (index) {
          // calculate the fraction of the index in relation to the max index
          double fraction = index / maxIndex;

          return Container(
            width: 8,
            height: 25,
            margin: const EdgeInsets.only(right: 5),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(5),
              color: index >= threshold ? grey : getColor(fraction),
            ),
          );
        });

        return Row(
          children: rectangles,
        );
      },
    );
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
