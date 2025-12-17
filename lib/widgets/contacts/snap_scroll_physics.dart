import 'package:flutter/widgets.dart';

class SnapScrollPhysics extends ScrollPhysics {
  final double itemExtent;

  const SnapScrollPhysics({
    required this.itemExtent,
    super.parent,
  });

  @override
  SnapScrollPhysics applyTo(ScrollPhysics? ancestor) {
    return SnapScrollPhysics(
      itemExtent: itemExtent,
      parent: buildParent(ancestor),
    );
  }

  double _getTargetPixels(
    ScrollMetrics position,
    Tolerance tolerance,
    double velocity,
  ) {
    double page = position.pixels / itemExtent;

    // Decide direction based on velocity
    if (velocity < -tolerance.velocity) {
      page -= 0.5;
    } else if (velocity > tolerance.velocity) {
      page += 0.5;
    }

    return (page.roundToDouble()) * itemExtent;
  }

  @override
  Simulation? createBallisticSimulation(
    ScrollMetrics position,
    double velocity,
  ) {
    // Let parent handle overscroll at edges
    if ((velocity <= 0.0 && position.pixels <= position.minScrollExtent) ||
        (velocity >= 0.0 && position.pixels >= position.maxScrollExtent)) {
      return super.createBallisticSimulation(position, velocity);
    }

    final target = _getTargetPixels(position, toleranceFor(position), velocity);

    if (target == position.pixels) {
      return null;
    }

    // Instant jump: simulation that is already at the target & done
    return _JumpToSimulation(target);
  }
}

class _JumpToSimulation extends Simulation {
  final double target;

  _JumpToSimulation(this.target);

  @override
  double x(double time) => target;

  @override
  double dx(double time) => 0.0;

  @override
  bool isDone(double time) => true;
}

