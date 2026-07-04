import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:telepathy/core/rust/lib.dart';

class AudioSettingsController with ChangeNotifier {
  final SharedPreferencesAsync options;

  /// the output volume for calls (applies to output device)
  late double outputVolume;

  /// the input volume for calls (applies to input device)
  late double inputVolume;

  /// the output volume for sound effects
  late double soundVolume;

  /// the input sensitivity for calls
  late double inputSensitivity;

  /// whether to use rnnoise
  late bool useDenoise;

  /// the name of a denoise model
  late String? denoiseModel;

  /// the output device for calls
  late String? outputDeviceId;

  /// the input device for calls
  late String? inputDeviceId;

  AudioSettingsController({required this.options});

  Future<void> init() async {
    outputVolume = await options.getDouble('outputVolume') ?? 0;
    inputVolume = await options.getDouble('inputVolume') ?? 0;
    soundVolume = await options.getDouble('soundVolume') ?? -10;
    inputSensitivity = await options.getDouble('inputSensitivity') ?? -16;
    useDenoise = await options.getBool('useDenoise') ?? true;
    denoiseModel = await options.getString('denoiseModel') ?? 'Vanilla';
    outputDeviceId = await options.getString('outputDeviceId');
    inputDeviceId = await options.getString('inputDeviceId');

    notifyListeners();
  }

  Future<void> updateOutputVolume(double volume) async {
    outputVolume = volume;
    await options.setDouble('outputVolume', volume);
    notifyListeners();
  }

  Future<void> updateInputVolume(double volume) async {
    inputVolume = volume;
    await options.setDouble('inputVolume', volume);
    notifyListeners();
  }

  Future<void> updateSoundVolume(double volume) async {
    soundVolume = volume;
    await options.setDouble('soundVolume', volume);
    notifyListeners();
  }

  Future<void> updateInputSensitivity(double sensitivity) async {
    inputSensitivity = sensitivity;
    await options.setDouble('inputSensitivity', sensitivity);
    notifyListeners();
  }

  Future<void> updateUseDenoise(bool use) async {
    useDenoise = use;
    await options.setBool('useDenoise', use);
    notifyListeners();
  }

  Future<void> setDenoiseModel(String? model) async {
    denoiseModel = model;

    if (model != null) {
      await options.setString('denoiseModel', model);
    } else {
      await options.remove('denoiseModel');
    }

    notifyListeners();
  }

  Future<void> updateOutputDevice(String? deviceId) async {
    outputDeviceId = deviceId;
    await _setOptionalString('outputDeviceId', deviceId);

    notifyListeners();
  }

  Future<void> updateInputDevice(String? deviceId) async {
    inputDeviceId = deviceId;
    await _setOptionalString('inputDeviceId', deviceId);

    notifyListeners();
  }

  Future<void> pruneMissingDevices({
    required List<AudioDevice> inputDevices,
    required List<AudioDevice> outputDevices,
  }) async {
    var changed = false;

    final savedInputId = inputDeviceId;
    if (savedInputId != null &&
        !inputDevices.any((device) => device.id == savedInputId)) {
      inputDeviceId = null;
      await _setOptionalString('inputDeviceId', null);
      changed = true;
    }

    final savedOutputId = outputDeviceId;
    if (savedOutputId != null &&
        !outputDevices.any((device) => device.id == savedOutputId)) {
      outputDeviceId = null;
      await _setOptionalString('outputDeviceId', null);
      changed = true;
    }

    if (changed) {
      notifyListeners();
    }
  }

  Future<void> _setOptionalString(String key, String? value) {
    return value == null ? options.remove(key) : options.setString(key, value);
  }
}
