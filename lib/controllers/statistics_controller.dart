import 'dart:collection';
import 'dart:math';

import 'package:flutter/foundation.dart';
import 'package:telepathy/core/utils/format_utils.dart';
import 'package:telepathy/src/rust/flutter.dart';

/// A controller responsible for managing the statistics of the call.
class StatisticsController extends ChangeNotifier {
  static const int lossWindowSize = 100;

  Statistics? _statistics;
  final ListQueue<int> _lossWindow = ListQueue<int>(lossWindowSize)
    ..addAll(List<int>.filled(lossWindowSize, 0));

  int _lossWindowVersion = 0;
  int _cachedLossWindowMax = 1;

  List<int> get lossWindow => _lossWindow.toList();

  int get lossWindowVersion => _lossWindowVersion;

  int get lossWindowMax => _cachedLossWindowMax;

  int get latency => _statistics?.latency.toInt() ?? 0;

  double get inputLevel => _statistics?.inputLevel ?? 0;

  double get outputLevel => _statistics?.outputLevel ?? 0;

  String get upload => formatBandwidth(_statistics?.uploadBandwidth.toInt());

  String get download =>
      formatBandwidth(_statistics?.downloadBandwidth.toInt());

  /// called when the backend has updated statistics
  void setStatistics(Statistics statistics) {
    int loss = statistics.loss.toInt();
    int? removed;

    if (_lossWindow.length > lossWindowSize) {
      removed = _lossWindow.removeFirst();
    }

    if (_cachedLossWindowMax < loss) {
      _cachedLossWindowMax = loss;
    } else if (removed == _cachedLossWindowMax && removed != null) {
      _cachedLossWindowMax = _lossWindow.reduce(max);
    }

    _lossWindow.add(loss);
    _lossWindowVersion++;
    _statistics = statistics;
    notifyListeners();
  }
}
