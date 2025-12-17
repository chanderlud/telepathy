import 'dart:collection';

import 'package:flutter/foundation.dart';
import 'package:telepathy/core/utils/format_utils.dart';
import 'package:telepathy/src/rust/flutter.dart';

/// A controller responsible for managing the statistics of the call.
class StatisticsController extends ChangeNotifier {
  static const int lossWindowSize = 100;

  Statistics? _statistics;
  final ListQueue<int> _lossWindow = ListQueue<int>(lossWindowSize)
    ..addAll(List<int>.filled(lossWindowSize, 0));

  List<int> get lossWindow => List.unmodifiable(_lossWindow);

  int get latency => _statistics?.latency.toInt() ?? 0;

  double get inputLevel => _statistics?.inputLevel ?? 0;

  double get outputLevel => _statistics?.outputLevel ?? 0;

  String get upload => formatBandwidth(_statistics?.uploadBandwidth.toInt());

  String get download =>
      formatBandwidth(_statistics?.downloadBandwidth.toInt());

  /// called when the backend has updated statistics
  void setStatistics(Statistics statistics) {
    _statistics = statistics;

    _lossWindow.add(statistics.loss.toInt());
    if (_lossWindow.length > lossWindowSize) {
      _lossWindow.removeFirst();
    }

    notifyListeners();
  }
}
