import 'dart:async';

import 'package:collection/collection.dart';
import 'package:flutter/foundation.dart';
import 'package:telepathy/core/utils/console.dart';
import 'package:telepathy/src/rust/telepathy.dart';

class AudioDevices extends ChangeNotifier {
  final Telepathy telepathy;
  Timer? periodicTimer;

  late List<String> _inputDevices = [];
  late List<String> _outputDevices = [];

  final ListEquality<String> _listEquality = const ListEquality<String>();
  bool _refreshing = false;

  List<String> get inputDevices => ['Default', ..._inputDevices];

  List<String> get outputDevices => ['Default', ..._outputDevices];

  AudioDevices({required this.telepathy}) {
    DebugConsole.debug('AudioDevices created');
    updateDevices();
  }

  @override
  void dispose() {
    periodicTimer?.cancel();
    periodicTimer = null;
    super.dispose();
  }

  Future<void> updateDevices() async {
    if (_refreshing) return;
    _refreshing = true;
    try {
      var (inputDevices, outputDevices) = await telepathy.listDevices();

      bool notify = false;

      if (!_listEquality.equals(_inputDevices, inputDevices)) {
        _inputDevices = inputDevices;
        notify = true;
      }

      if (!_listEquality.equals(_outputDevices, outputDevices)) {
        _outputDevices = outputDevices;
        notify = true;
      }

      if (notify) {
        notifyListeners();
      }
    } catch (e, st) {
      DebugConsole.debug('Failed to update audio devices: $e\n$st');
    } finally {
      _refreshing = false;
    }
  }

  void startUpdates() {
    periodicTimer?.cancel();
    periodicTimer = Timer.periodic(const Duration(milliseconds: 500), (timer) {
      updateDevices();
    });
  }

  void pauseUpdates() {
    periodicTimer?.cancel();
    periodicTimer = null;
  }
}
