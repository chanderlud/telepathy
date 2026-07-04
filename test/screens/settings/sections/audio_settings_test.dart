import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shared_preferences_platform_interface/in_memory_shared_preferences_async.dart';
import 'package:shared_preferences_platform_interface/shared_preferences_async_platform_interface.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/core/rust/lib.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/screens/settings/sections/audio_settings.dart';
import 'package:telepathy/widgets/common/index.dart';

void main() {
  setUp(() {
    SharedPreferencesAsyncPlatform.instance =
        InMemorySharedPreferencesAsync.empty();
  });

  tearDown(() {
    SharedPreferencesAsyncPlatform.instance = null;
  });

  testWidgets(
      'Sound Test shows the audio error dialog and clears inAudioTest after a DartError',
      (WidgetTester tester) async {
    final telepathy = _FakeTelepathy();
    final stateController = StateController();
    final audioSettingsController = _FakeAudioSettingsController();

    final audioDevices = _FakeAudioDevices(telepathy: telepathy);
    final preferencesController = _FakePreferencesController();
    final networkSettingsController = _FakeNetworkSettingsController();

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<StateController>.value(value: stateController),
          ChangeNotifierProvider<AudioSettingsController>.value(
            value: audioSettingsController,
          ),
          ChangeNotifierProvider<PreferencesController>.value(
            value: preferencesController,
          ),
          ChangeNotifierProvider<NetworkSettingsController>.value(
            value: networkSettingsController,
          ),
          ChangeNotifierProvider<AudioDevices>.value(value: audioDevices),
          ChangeNotifierProvider<StatisticsController>.value(
            value: StatisticsController(),
          ),
          Provider<Telepathy>.value(value: telepathy),
          Provider<SoundPlayer>.value(value: _FakeSoundPlayer()),
        ],
        child: const MaterialApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: AudioSettings(
                constraints: BoxConstraints(maxWidth: 800, maxHeight: 2000),
              ),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.widgetWithText(Button, 'Sound Test'));
    await tester.pump();

    expect(find.widgetWithText(Button, 'End Test'), findsOneWidget);
    expect(stateController.inAudioTest, isTrue);

    telepathy.completeAudioTestError();
    await tester.pumpAndSettle();

    expect(find.text('Error in Audio Test'), findsOneWidget);
    expect(find.text('microphone unavailable'), findsOneWidget);
    expect(find.widgetWithText(Button, 'Sound Test'), findsOneWidget);
    expect(find.widgetWithText(Button, 'End Test'), findsNothing);
    expect(stateController.inAudioTest, isFalse);
  });
}

class _FakeAudioDevices extends AudioDevices {
  _FakeAudioDevices({required super.telepathy});

  @override
  void pauseUpdates() {}

  @override
  void startUpdates() {}
}

class _FakeAudioSettingsController extends AudioSettingsController {
  _FakeAudioSettingsController() : super(options: SharedPreferencesAsync()) {
    outputVolume = 0;
    inputVolume = 0;
    soundVolume = -10;
    inputSensitivity = -16;
    useDenoise = true;
    denoiseModel = 'Vanilla';
    outputDeviceId = null;
    inputDeviceId = null;
  }
}

class _FakePreferencesController extends PreferencesController {
  _FakePreferencesController() : super(options: SharedPreferencesAsync()) {
    playCustomRingtones = true;
    customRingtoneFile = null;
    efficiencyMode = false;
  }
}

class _FakeNetworkSettingsController extends NetworkSettingsController {
  _FakeNetworkSettingsController() : super(options: SharedPreferencesAsync()) {
    codecConfig = _FakeCodecConfig();
  }
}

class _FakeCodecConfig implements CodecConfig {
  bool _enabled = true;
  bool _vbr = true;
  double _residualBits = 5.0;

  @override
  void dispose() {}

  @override
  bool get isDisposed => false;

  @override
  void setEnabled({required bool enabled}) {
    _enabled = enabled;
  }

  @override
  void setResidualBits({required double residualBits}) {
    _residualBits = residualBits;
  }

  @override
  void setVbr({required bool vbr}) {
    _vbr = vbr;
  }

  @override
  (bool, bool, double) toValues() => (_enabled, _vbr, _residualBits);
}

class _FakeSoundPlayer implements SoundPlayer {
  @override
  bool get isDisposed => false;

  @override
  void dispose() {}

  @override
  ArcHost host() {
    throw UnimplementedError();
  }

  @override
  Future<FlutterSoundHandle> play({required List<int> bytes}) {
    throw UnimplementedError();
  }

  @override
  Future<void> updateOutputDevice({String? deviceId}) async {}

  @override
  void updateOutputVolume({required double volume}) {}
}

class _FakeTelepathy implements Telepathy {
  final Completer<void> _audioTestCompleter = Completer<void>();

  @override
  bool get isDisposed => false;

  @override
  void dispose() {}

  @override
  Future<void> audioTest() => _audioTestCompleter.future;

  void completeAudioTestError() {
    _audioTestCompleter.completeError(
      const DartError(message: 'microphone unavailable'),
    );
  }

  @override
  ChatMessage buildChat({
    required Contact contact,
    required String text,
    required List<(String, Uint8List)> attachments,
  }) {
    throw UnimplementedError();
  }

  @override
  Future<void> endCall() async {}

  @override
  Future<void> joinRoom({required List<String> memberStrings}) async {}

  @override
  Future<(List<AudioDevice>, List<AudioDevice>)> listDevices() async =>
      (<AudioDevice>[], <AudioDevice>[]);

  @override
  void pauseStatistics() {}

  @override
  Future<void> restartManager() async {}

  @override
  void resumeStatistics() {}

  @override
  Future<void> sendChat({required ChatMessage message}) async {}

  @override
  void setContactOutputVolume({required Contact contact}) {}

  @override
  void setDeafened({required bool deafened}) {}

  @override
  void setDenoise({required bool denoise}) {}

  @override
  void setEfficiencyMode({required bool enabled}) {}

  @override
  Future<void> setIdentity({required List<int> key}) async {}

  @override
  Future<void> setInputDevice({String? deviceId}) async {}

  @override
  void setInputVolume({required double decibel}) {}

  @override
  Future<void> setModel({Uint8List? model}) async {}

  @override
  void setMuted({required bool muted}) {}

  @override
  Future<void> setOutputDevice({String? deviceId}) async {}

  @override
  void setOutputVolume({required double decibel}) {}

  @override
  void setPlayCustomRingtones({required bool play}) {}

  @override
  void setRmsThreshold({required double decimal}) {}

  @override
  void setSendCustomRingtone({required bool send}) {}

  @override
  Future<void> shutdown() async {}

  @override
  Future<void> startCall({required Contact contact}) async {}

  @override
  Future<void> startManager() async {}

  @override
  Future<void> startScreenshare({required Contact contact}) async {}

  @override
  Future<void> startSession({required Contact contact}) async {}

  @override
  Future<void> stopSession({required Contact contact}) async {}
}
